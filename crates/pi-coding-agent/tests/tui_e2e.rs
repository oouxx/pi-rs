#![allow(clippy::expect_used, clippy::unwrap_used, clippy::field_reassign_with_default)]

//! TUI end-to-end tests over a pseudo-terminal (no real TTY required).
//!
//! Architecture (fork mode, same pattern as grok-build's pty harness):
//!
//! - The test binary re-executes itself with `PI_TUI_E2E_CHILD=1` and
//!   `--exact child_process_runner`; the child builds an `AgentSession` with
//!   a **mock stream_fn** (no network, no API key) and runs the interactive
//!   TUI against the PTY slave.
//! - The parent opens a `portable-pty` pair, drives the TUI by writing keys
//!   to the master (Enter/Ctrl+C/Ctrl+D/Esc) and asserts on the rendered
//!   output streamed back from the master reader.
//!
//! Coverage: launch + prompt render, chat flow with streamed mock reply,
//! Ctrl+C abort of a long stream, clean quit paths (Esc / Ctrl+D) and
//! exit code 0 with terminal restored.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pi_agent_core::pi_ai_types::{
    AssistantMessage, AssistantMessageEvent, ContentBlock, StopReason, StreamResponse, Usage,
};
use pi_agent_core::types::{StreamFn, StreamFnOptions};
use pi_coding_agent::core::model_registry::ModelRegistry;
use pi_coding_agent::core::sdk::{create_agent_session, CreateAgentSessionOptions};
use pi_extension_api::hook::{CommandRegistration, CommandRegistry, HookHandler};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const CHILD_ENV: &str = "PI_TUI_E2E_CHILD";
const LONG_ENV: &str = "PI_TUI_E2E_LONG_STREAM";
const REAL_ENV: &str = "PI_E2E_REAL_OLLAMA";
const OLLAMA_MODEL: &str = "deepseek-v4-flash:0731";
const MOCK_REPLY: &str = "Hello from the mock LLM!";
const WIDTH: u16 = 100;
const HEIGHT: u16 = 30;
const TIMEOUT: Duration = Duration::from_secs(20);
/// Budget for a real LLM round-trip (network + generation).
const REAL_TIMEOUT: Duration = Duration::from_secs(90);

// ────────────────────────────────────────────────────────────────────────────
// Mock LLM
// ────────────────────────────────────────────────────────────────────────────

fn mock_model() -> pi_agent_core::pi_ai_types::Model {
    pi_agent_core::pi_ai_types::Model {
        id: "mock-model".into(),
        name: "Mock Model".into(),
        api: "mock-api".into(),
        provider: "mock".into(),
        base_url: "http://mock.local".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: pi_agent_core::pi_ai_types::ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: vec![],
        },
        context_window: 128_000,
        max_tokens: 4096,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn partial_msg(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            text_signature: None,
        }],
        api: "mock-api".into(),
        provider: "mock".into(),
        model: "mock-model".into(),
        response_model: None,
        response_id: Some("mock-response-1".into()),
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Pending,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

/// Build the mock LLM stream.
///
/// Short mode: a fixed set of deltas then completion (used by the chat-flow
/// test). Long mode: keeps emitting deltas every 80 ms until the abort signal
/// fires, then finishes with `Stop` (used by the Ctrl+C test).
fn mock_stream_fn(long: bool) -> StreamFn {
    Arc::new(move |_model, _context, _thinking, options: StreamFnOptions| {
        Box::pin(async move {
            let signal = options.signal;
            let stream: StreamResponse = if long {
                // Round 1 emits TextStart (agent_loop only forwards deltas
                // once a Start was seen); subsequent rounds emit a delta
                // every 200 ms until the abort signal fires, then Done.
                let started = false;
                Box::new(futures::stream::unfold(
                    (started, String::new(), signal),
                    |(mut started, mut text, signal)| {
                        Box::pin(async move {
                            if let Some(rx) = signal.as_ref() {
                                if *rx.borrow() {
                                    let mut final_msg = partial_msg(&text);
                                    final_msg.stop_reason = StopReason::Aborted;
                                    return Some((
                                        AssistantMessageEvent::Done {
                                            reason: StopReason::Aborted,
                                            message: final_msg,
                                        },
                                        (started, text, signal),
                                    ));
                                }
                            }
                            let ev = if !started {
                                started = true;
                                AssistantMessageEvent::Start {
                                    partial: partial_msg(""),
                                }
                            } else {
                                text.push_str("chunk ");
                                AssistantMessageEvent::TextDelta {
                                    content_index: 0,
                                    delta: "chunk ".to_string(),
                                    partial: partial_msg(&text),
                                }
                            };
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            Some((ev, (started, text, signal)))
                        })
                    },
                ))
            } else {
                let mut text = String::new();
                let mut events = vec![AssistantMessageEvent::Start {
                    partial: partial_msg(""),
                }];
                for word in MOCK_REPLY.split(' ') {
                    text.push_str(word);
                    text.push(' ');
                    events.push(AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: format!("{word} "),
                        partial: partial_msg(text.trim_end()),
                    });
                }
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: 0,
                    content: MOCK_REPLY.to_string(),
                    partial: partial_msg(MOCK_REPLY),
                });
                let mut final_msg = partial_msg(MOCK_REPLY);
                final_msg.stop_reason = StopReason::Stop;
                events.push(AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message: final_msg,
                });
                Box::new(futures::stream::iter(events))
            };
            Ok(stream as StreamResponse)
        })
    })
}

/// Create an AgentSession wired to the mock LLM.
async fn create_mock_session() -> pi_coding_agent::core::agent_session::AgentSession {
    let mut opts = CreateAgentSessionOptions::default();
    opts.cwd = std::env::temp_dir().display().to_string();
    opts.agent_dir = Some(std::env::temp_dir().display().to_string());
    opts.stream_fn = Some(mock_stream_fn(std::env::var(LONG_ENV).is_ok()));
    opts.cli_provider = Some("mock".into());
    opts.cli_model = Some("mock-model".into());
    opts.enable_extensions = false;
    opts.model_registry = Some(ModelRegistry::new(vec![mock_model()]));
    let mut registry = pi_extension_api::ExtensionRegistry::new();
    registry.register(
        Box::new(TestCmdHandler),
        pi_extension_api::create_builtin_source_info("test-ext"),
    );
    opts.extension_registry = Some(registry);
    let (session, _result) = create_agent_session(opts).await.expect("mock session");
    session
}

/// Mock extension registering slash commands. The handlers write to
/// well-known files so the E2E test can observe execution.
struct TestCmdHandler;

#[async_trait::async_trait]
impl HookHandler for TestCmdHandler {
    fn name(&self) -> &str {
        "test-ext"
    }

    fn register_commands(&self, reg: &mut CommandRegistry) {
        reg.register(
            "testcmd",
            CommandRegistration {
                description: "test extension command".into(),
                execute: std::sync::Arc::new(|args, _ctx| {
                    Box::pin(async move {
                        std::fs::write("/tmp/tui_ext_cmd.log", format!("ran: {args}"))
                            .expect("write ext log");
                    })
                }),
                get_argument_completions: None,
            },
        );
        reg.register(
            "uitest",
            CommandRegistration {
                description: "exercise the extension UI bridge".into(),
                execute: std::sync::Arc::new(|_args, ctx| {
                    let ctx = ctx.cloned();
                    Box::pin(async move {
                        // Confirm dialog → user answer recorded.
                        if let Some(ctx) = ctx.as_ref() {
                            let ok = (ctx.ui.confirm)("Confirm?", &serde_json::json!({"k": "v"}));
                            std::fs::write("/tmp/tui_ui_confirm.log", format!("confirm={ok}"))
                                .expect("write confirm log");
                        }
                        // Select dialog → user choice recorded.
                        if let Some(ctx) = ctx.as_ref() {
                            let choice = (ctx.ui.select)(
                                "Pick one",
                                &["alpha".to_string(), "beta".to_string()],
                                None,
                            )
                            .await;
                            std::fs::write("/tmp/tui_ui_select.log", format!("select={choice:?}"))
                                .expect("write select log");
                        }
                    })
                }),
                get_argument_completions: None,
            },
        );
    }
}

/// Model pointing at Ollama Cloud's OpenAI-compatible endpoint. The API key
/// is provided via `OPENAI_API_KEY` in the child env (mapped from
/// `OLLAMA_API_KEY` by the parent test — pi-ai has no `ollama` provider key).
fn ollama_model() -> pi_agent_core::pi_ai_types::Model {
    pi_agent_core::pi_ai_types::Model {
        id: OLLAMA_MODEL.into(),
        name: "Ollama Cloud (deepseek-v4-flash)".into(),
        api: "openai-completions".into(),
        provider: "openai".into(),
        base_url: "https://ollama.com/v1".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: pi_agent_core::pi_ai_types::ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: vec![],
        },
        context_window: 128_000,
        max_tokens: 4096,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

/// AgentSession wired to the *real* provider stack (default stream_fn +
/// Ollama Cloud model). Network + API key required.
async fn create_real_session() -> pi_coding_agent::core::agent_session::AgentSession {
    let mut opts = CreateAgentSessionOptions::default();
    opts.cwd = std::env::temp_dir().display().to_string();
    opts.agent_dir = Some(std::env::temp_dir().display().to_string());
    opts.stream_fn = Some(pi_coding_agent::core::sdk::create_default_stream_fn());
    opts.cli_provider = Some("openai".into());
    opts.cli_model = Some(OLLAMA_MODEL.into());
    opts.enable_extensions = false;
    opts.model_registry = Some(ModelRegistry::new(vec![ollama_model()]));
    let (session, _result) = create_agent_session(opts).await.expect("real session");
    session
}

// ────────────────────────────────────────────────────────────────────────────
// Child process runner (spawned by the parent test with the PTY attached)
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn child_process_runner() {
    if std::env::var(CHILD_ENV).is_err() {
        return; // runs only when spawned by the e2e tests
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let code = runtime.block_on(async {
        let session = if std::env::var(REAL_ENV).is_ok() {
            create_real_session().await
        } else {
            create_mock_session().await
        };
        pi_coding_agent::modes::interactive::run_interactive_mode(session).await
    });
    std::process::exit(code);
}

// ────────────────────────────────────────────────────────────────────────────
// PTY driver (parent side)
// ────────────────────────────────────────────────────────────────────────────

struct Tui {
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    output: Vec<u8>,
}

impl Tui {
    fn spawn(long_stream: bool) -> Self {
        Self::spawn_inner(long_stream, None)
    }

    /// Spawn with the real LLM path: `real_key` is mapped to
    /// `OPENAI_API_KEY` in the child env.
    fn spawn_real(long_stream: bool, real_key: &str) -> Self {
        Self::spawn_inner(long_stream, Some(real_key))
    }

    fn spawn_inner(long_stream: bool, real_key: Option<&str>) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: HEIGHT,
                cols: WIDTH,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new(std::env::current_exe().expect("current exe"));
        cmd.arg("--exact");
        cmd.arg("child_process_runner");
        cmd.arg("--nocapture");
        cmd.arg("--test-threads=1");
        cmd.env(CHILD_ENV, "1");
        if long_stream {
            cmd.env(LONG_ENV, "1");
        }
        if let Some(key) = real_key {
            cmd.env(REAL_ENV, "1");
            cmd.env("OPENAI_API_KEY", key);
        }
        let child = pty
            .slave
            .spawn_command(cmd)
            .expect("spawn child in pty");
        drop(pty.slave);

        let master = pty.master;
        let writer = master.take_writer().expect("pty writer");
        let mut reader = master.try_clone_reader().expect("master reader");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Self {
            writer,
            child,
            rx,
            output: Vec::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("pty write");
    }

    /// Drain any pending output into the accumulated buffer.
    fn drain(&mut self) {
        while let Ok(chunk) = self.rx.try_recv() {
            self.output.extend_from_slice(&chunk);
        }
    }

    fn rendered(&mut self) -> String {
        self.drain();
        String::from_utf8_lossy(&self.output).to_string()
    }

    /// Poll until `needle` appears in the rendered output or timeout.
    fn wait_for(&mut self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.rendered().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        false
    }

    /// Wait for process exit, returning the exit code (None if it didn't exit).
    fn wait_exit(&mut self, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status.exit_code() as i32);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// E2E tests
// ────────────────────────────────────────────────────────────────────────────

/// Full chat flow: launch → prompt visible → send message → mock reply
/// streams in → Ctrl+D quits → clean exit code 0.
#[test]
fn tui_chat_flow_renders_mock_reply_and_quits() {
    let mut tui = Tui::spawn(false);
    assert!(
        tui.wait_for("> ", TIMEOUT),
        "prompt rendered; got: {:?}",
        tui.rendered()
    );

    tui.write(b"hello\r");
    // The renderer emits words via cursor-positioned spans, so assert on
    // contiguous tokens rather than the full sentence.
    assert!(
        tui.wait_for("mock", TIMEOUT),
        "mock reply streamed; got: {:?}",
        tui.rendered()
    );
    assert!(
        tui.wait_for("LLM!", TIMEOUT),
        "reply finished; got: {:?}",
        tui.rendered()
    );

    tui.write(&[0x04]); // Ctrl+D: quit
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit code 0");
    // The TUI must have restored the terminal (left alternate screen).
    let out = tui.rendered();
    assert!(
        out.contains("\x1b[?1049l"),
        "alternate screen restored on quit: {:?}",
        out.chars().rev().take(200).collect::<String>()
    );
}

/// Ctrl+C mid-stream must abort the long mock stream: output stops growing
/// and the TUI remains usable (no hang), then quits cleanly.
#[test]
fn tui_ctrl_c_aborts_long_stream() {
    let mut tui = Tui::spawn(true);
    assert!(tui.wait_for("> ", TIMEOUT), "prompt rendered");
    tui.write(b"hello\r");
    // First chunk arrives.
    assert!(
        tui.wait_for("chunk", TIMEOUT),
        "long stream started; got: {:?}",
        tui.rendered()
    );

    tui.write(&[0x03]); // Ctrl+C: abort

    // Output must stop growing within ~2s (stream aborted, spinner stopped).
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_len = tui.rendered().len();
    let mut stable_rounds = 0;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        let len = tui.rendered().len();
        if len == last_len {
            stable_rounds += 1;
            if stable_rounds >= 3 {
                break;
            }
        } else {
            stable_rounds = 0;
            last_len = len;
        }
    }
    assert!(
        stable_rounds >= 3,
        "output stopped after Ctrl+C; tail: {:?}",
        tui.rendered().chars().rev().take(400).collect::<String>()
    );

    // Still responsive: Ctrl+D quits.
    tui.write(&[0x04]);
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit after abort");
}

/// End-to-end against a REAL LLM (Ollama Cloud, OpenAI-compatible endpoint).
///
/// Requires `OLLAMA_API_KEY`; skipped (with a note) when absent. Exercises
/// the full stack: TUI → interactive mode → AgentSession → default
/// stream_fn → provider HTTP → Ollama Cloud → streamed reply → render.
#[test]
fn tui_chat_flow_real_ollama() {
    let Some(key) = std::env::var("OLLAMA_API_KEY").ok().filter(|k| !k.is_empty()) else {
        eprintln!("SKIP: OLLAMA_API_KEY not set — real-LLM E2E skipped");
        return;
    };

    let mut tui = Tui::spawn_real(false, &key);
    assert!(
        tui.wait_for("> ", REAL_TIMEOUT),
        "prompt rendered; got: {:?}",
        tui.rendered()
    );

    tui.write(b"Reply with exactly: E2E-OK\r");
    assert!(
        tui.wait_for("assistant", REAL_TIMEOUT),
        "assistant message appeared; got: {:?}",
        tui.rendered()
    );
    assert!(
        tui.wait_for("E2E-OK", REAL_TIMEOUT),
        "real LLM replied; got: {:?}",
        tui.rendered().chars().rev().take(500).collect::<String>()
    );

    tui.write(&[0x04]); // Ctrl+D: quit
    let code = tui.wait_exit(REAL_TIMEOUT);
    assert_eq!(code, Some(0), "clean exit code 0");
}

/// Slash commands registered by extensions must be routed to the extension
/// handler (not sent to the model as a plain message).
#[test]
fn tui_extension_slash_command_executes() {
    let _ = std::fs::remove_file("/tmp/tui_ext_cmd.log");
    let mut tui = Tui::spawn(false);
    assert!(tui.wait_for("> ", TIMEOUT), "prompt rendered");

    tui.write(b"/testcmd hello\r");

    let deadline = Instant::now() + TIMEOUT;
    let mut executed = false;
    while Instant::now() < deadline {
        if let Ok(content) = std::fs::read_to_string("/tmp/tui_ext_cmd.log") {
            if content.contains("ran: hello") {
                executed = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    assert!(executed, "extension command handler ran with args");

    // Unknown commands fall through to the model as a plain message (the
    // user message appears on screen) rather than erroring.
    tui.write(b"/no-such-command\r");
    assert!(
        tui.wait_for("no-such-command", TIMEOUT),
        "unknown command sent as message; got: {:?}",
        tui.rendered()
    );

    tui.write(&[0x04]);
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit code 0");
}

/// Extension UI bridge: `ui.confirm` renders a dialog in the TUI; Tab
/// switches to Cancel, Enter resolves it; the extension observes the result.
/// `ui.select` renders a select list; Enter resolves the choice.
#[test]
fn tui_extension_ui_dialogs_work() {
    let _ = std::fs::remove_file("/tmp/tui_ui_confirm.log");
    let _ = std::fs::remove_file("/tmp/tui_ui_select.log");
    let mut tui = Tui::spawn(false);
    assert!(tui.wait_for("> ", TIMEOUT), "prompt rendered");

    // ── confirm: press Tab (→ Cancel) + Enter → false ──
    tui.write(b"/uitest\r");
    assert!(
        tui.wait_for("Confirm", TIMEOUT),
        "confirm dialog rendered; got: {:?}",
        tui.rendered()
    );
    tui.write(b"\t"); // Tab → Cancel
    tui.write(b"\r"); // Enter → resolve false
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(c) = std::fs::read_to_string("/tmp/tui_ui_confirm.log") {
            if c.contains("confirm=false") {
                break;
            }
        }
        assert!(Instant::now() < deadline, "confirm result never arrived");
        std::thread::sleep(Duration::from_millis(40));
    }

    // ── select: Enter picks the first option (alpha) ──
    tui.write(b"/uitest\r");
    assert!(
        tui.wait_for("alpha", TIMEOUT),
        "select dialog rendered; got: {:?}",
        tui.rendered()
    );
    tui.write(b"\r"); // Enter → alpha
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(s) = std::fs::read_to_string("/tmp/tui_ui_select.log") {
            if s.contains("Some(\"alpha\")") || s.contains("Some(\"alpha\")") {
                break;
            }
        }
        assert!(Instant::now() < deadline, "select result never arrived");
        std::thread::sleep(Duration::from_millis(40));
    }

    tui.write(&[0x04]);
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit code 0");
}

/// Esc quits immediately with a clean terminal restore.
#[test]
fn tui_esc_quits_cleanly() {
    let mut tui = Tui::spawn(false);
    assert!(tui.wait_for("> ", TIMEOUT), "prompt rendered");
    tui.write(&[0x1b]); // Esc
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "Esc quit exit code 0");
    assert!(
        tui.rendered().contains("\x1b[?1049l"),
        "terminal restored after Esc"
    );
}
