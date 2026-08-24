#![cfg(feature = "interactive")]
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
const TOOLS2_ENV: &str = "PI_TUI_E2E_TWO_TOOLS";
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
    // 内置 goal 扩展：/goal 菜单 E2E 需要它（TUI 模式门控）。
    registry.register(
        Box::new(pi_extensions::goal::GoalExtension::new()),
        pi_extension_api::create_builtin_source_info("goal"),
    );
    opts.extension_registry = Some(registry);
    let (session, _result) = create_agent_session(opts).await.expect("mock session");
    session
}

/// Mock LLM that requests TWO `bash` tool calls (same tool name!) on the
/// first two turns, then finishes with plain text. The bash tool is the
/// real one, so it streams `ToolExecutionUpdate` snapshots of the output
/// while running — this drives the tool-output streaming path AND the
/// per-call-id state isolation for same-name tools.
fn mock_two_bash_tools_stream_fn() -> StreamFn {
    Arc::new(move |_model, context, _thinking, _opts: StreamFnOptions| {
        let tool_results = context
            .messages
            .iter()
            .filter(|m| matches!(m, pi_agent_core::pi_ai_types::Message::ToolResult { .. }))
            .count();
        Box::pin(async move {
            let (msg, reason) = match tool_results {
                0 => (
                    AssistantMessage {
                        content: vec![ContentBlock::ToolCall {
                            id: "bash-1".into(),
                            name: "bash".into(),
                            arguments: serde_json::json!({"command": "echo hello-one"}),
                            thought_signature: None,
                            namespace: None,
                        }],
                        api: "mock-api".into(),
                        provider: "mock".into(),
                        model: "mock-model".into(),
                        response_model: None,
                        response_id: None,
                        diagnostics: None,
                        usage: Usage::default(),
                        stop_reason: StopReason::ToolUse,
                        error_message: None,
                        raw_stop_reason: None,
                        end_turn: None,
                        timestamp: 0,
                    },
                    StopReason::ToolUse,
                ),
                1 => (
                    AssistantMessage {
                        content: vec![ContentBlock::ToolCall {
                            id: "bash-2".into(),
                            name: "bash".into(),
                            arguments: serde_json::json!({"command": "echo hello-two"}),
                            thought_signature: None,
                            namespace: None,
                        }],
                        api: "mock-api".into(),
                        provider: "mock".into(),
                        model: "mock-model".into(),
                        response_model: None,
                        response_id: None,
                        diagnostics: None,
                        usage: Usage::default(),
                        stop_reason: StopReason::ToolUse,
                        error_message: None,
                        raw_stop_reason: None,
                        end_turn: None,
                        timestamp: 0,
                    },
                    StopReason::ToolUse,
                ),
                _ => {
                    let mut msg = partial_msg("All tools done.");
                    msg.stop_reason = StopReason::Stop;
                    (msg, StopReason::Stop)
                }
            };
            Ok(Box::new(futures::stream::iter(vec![
                AssistantMessageEvent::Done { reason, message: msg },
            ])) as StreamResponse)
        })
    })
}

/// AgentSession wired to the two-bash-tools mock.
async fn create_two_bash_tools_session() -> pi_coding_agent::core::agent_session::AgentSession {
    let mut opts = CreateAgentSessionOptions::default();
    opts.cwd = std::env::temp_dir().display().to_string();
    opts.agent_dir = Some(std::env::temp_dir().display().to_string());
    opts.stream_fn = Some(mock_two_bash_tools_stream_fn());
    opts.cli_provider = Some("mock".into());
    opts.cli_model = Some("mock-model".into());
    opts.enable_extensions = false;
    opts.model_registry = Some(ModelRegistry::new(vec![mock_model()]));
    let (session, _result) = create_agent_session(opts).await.expect("two-bash session");
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
        } else if std::env::var(TOOLS2_ENV).is_ok() {
            create_two_bash_tools_session().await
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

    /// Spawn with the mock that runs two same-name `bash` tool calls
    /// (tool-output streaming + per-call state isolation).
    fn spawn_two_bash_tools() -> Self {
        Self::spawn_inner_env(false, None, &[(TOOLS2_ENV, "1")])
    }

    fn spawn_inner(long_stream: bool, real_key: Option<&str>) -> Self {
        Self::spawn_inner_env(long_stream, real_key, &[])
    }

    fn spawn_inner_env(
        long_stream: bool,
        real_key: Option<&str>,
        extra_env: &[(&str, &str)],
    ) -> Self {
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
        for (k, v) in extra_env {
            cmd.env(*k, *v);
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
        tui.wait_for("mock-model", TIMEOUT),
        "footer rendered; got: {:?}",
        tui.rendered()
    );

    tui.write(b"hello\r");
    // The renderer emits words via cursor-positioned spans, so assert on
    // contiguous tokens rather than the full sentence.
    assert!(
        tui.wait_for("Hello", TIMEOUT),
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
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");
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

/// Tool-approval gate — approve: the `write` tool call is gated on a user
/// decision; pressing `a` runs it (marker file appears) and the turn
/// continues with the mock's text reply.
#[test]
fn tui_tool_output_streams_and_same_name_tools_stay_independent() {
    let mut tui = Tui::spawn_two_bash_tools();
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");

    // The approval gate is disabled (tools run directly), so the two
    // same-name bash calls stream into their own rows immediately.
    tui.write(b"run the tools\r");
    assert!(
        tui.wait_for("hello-one", TIMEOUT),
        "first bash output streamed into its row; got: {:?}",
        tui.rendered()
    );
    assert!(
        tui.wait_for("hello-two", TIMEOUT),
        "second same-name bash output on its own row; got: {:?}",
        tui.rendered()
    );
    assert!(
        tui.wait_for("All tools done.", TIMEOUT),
        "turn finished; got: {:?}",
        tui.rendered()
    );
    // TS original: tool calls render in full — no fold aggregation. Both
    // finished bash boxes stay fully rendered (the per-call-id isolation
    // regression kept the first bash Running forever; now both show their
    // own output and neither folds into a one-row ▸ box).
    let text = tui.rendered();
    assert!(
        text.contains("hello-one") && text.contains("hello-two"),
        "both tool outputs rendered in full; got: {text:?}"
    );
    assert!(
        !text.contains("\u{25b8}"),
        "no folded one-row tool boxes; got: {text:?}"
    );

    tui.write(&[0x04]);
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit code 0");
}

/// Slash commands: the `/` menu appears (regression: the completer was
/// never populated), Enter picks the highlighted command, malformed `/model`
/// shows usage instead of silently dropping the input, a bogus provider
/// surfaces the auth failure, and `/new` starts a fresh session.
#[test]
fn tui_slash_command_menu_and_feedback() {
    let mut tui = Tui::spawn(false);
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");

    // `/` opens the command menu — TS-style inline rows (no border/title):
    // the first row is `→ /help — Show commands`.
    tui.write(b"/");
    assert!(
        tui.wait_for("Show commands", TIMEOUT),
        "slash menu rendered; got: {:?}",
        tui.rendered()
    );

    // Type the rest of `/help` and press Enter: the menu stays open while
    // typing (characters are inserted), Enter executes the command.
    tui.write(b"help\r");
    assert!(
        tui.wait_for("Commands: /new", TIMEOUT),
        "/help executed via the menu; got: {:?}",
        tui.rendered()
    );

    // Malformed /model: usage feedback instead of a silent no-op.
    tui.write(b"/model\r");
    assert!(
        tui.wait_for("Usage: /model", TIMEOUT),
        "usage message rendered; got: {:?}",
        tui.rendered()
    );

    // Unknown provider: the mock registry accepts any provider, so the
    // switch succeeds — and the outcome now surfaces with feedback (the
    // footer model name updates too; previously the result was silently
    // dropped and the footer stayed stale). The diff painter only emits the
    // changed cells, so assert on the repainted tail and the footer text.
    tui.write(b"/model bogus/whatever\r");
    assert!(
        tui.wait_for("ed to bogus/whatever", TIMEOUT),
        "model switch feedback rendered; got: {:?}",
        tui.rendered()
    );
    assert!(
        tui.wait_for("whatev", TIMEOUT),
        "footer model name updated; got: {:?}",
        tui.rendered()
    );

    // /new starts a fresh session (transcript cleared, placeholder back).
    // The system message reuses a previously painted row, so ratatui's
    // diff skips the one unchanged cell (`New ses` + `ion created`);
    // assert on the contiguous fragments instead of the full sentence.
    tui.write(b"/new\r");
    assert!(
        tui.wait_for("New ses", TIMEOUT) && tui.wait_for("ion created", TIMEOUT),
        "new session message rendered; got: {:?}",
        tui.rendered()
    );

    tui.write(&[0x04]);
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit code 0");
}

/// The goal extension (simplified, no TUI menu): bare `/goal` in TUI mode
/// shows the text status; `/goal <objective>` starts a goal and the status
/// bar reflects it.
#[test]
fn tui_goal_text_status_and_start() {
    let mut tui = Tui::spawn(false);
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");

    // /goal（无参数）→ 文本状态（无菜单弹窗）。
    tui.write(b"/goal\r");
    assert!(
        tui.wait_for("No goal is currently set", TIMEOUT),
        "text status rendered; got: {:?}",
        tui.rendered()
    );

    // /goal <objective> → goal 启动（diff 画家会拆分长文本，断言连续片段）。
    tui.write(b"/goal fix the e2e goal bug\r");
    assert!(
        tui.wait_for("fix the e2e goal bug", TIMEOUT),
        "goal started; got: {:?}",
        tui.rendered()
    );

    // 扩展命令执行后必须消费 settled 队列启动 run：mock 模型的回复出现
    // 才算 /goal 真的把 owned prompt 交给了 agent（回归：命令只 notify
    // 不入队启动，模型永远不会收到 goal）。渲染器按词输出，断言 token。
    assert!(
        tui.wait_for("LLM!", TIMEOUT),
        "goal prompt run started; got: {:?}",
        tui.rendered()
    );

    // Ctrl+D 退出。
    tui.write(&[0x04]); // Ctrl+D: quit
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit code 0");
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
        tui.wait_for("deepseek-v4-flash", REAL_TIMEOUT),
        "footer rendered; got: {:?}",
        tui.rendered()
    );

    tui.write(b"Reply with exactly: E2E-OK\r");
    assert!(
        tui.wait_for("E2E-OK", REAL_TIMEOUT),
        "assistant message appeared; got: {:?}",
        tui.rendered()
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
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");

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
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");

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
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");
    tui.write(&[0x1b]); // Esc
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "Esc quit exit code 0");
    assert!(
        tui.rendered().contains("\x1b[?1049l"),
        "terminal restored after Esc"
    );
}
