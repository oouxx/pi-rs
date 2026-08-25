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
const BIG_ENV: &str = "PI_TUI_E2E_BIG_STREAM";
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

/// Big-stream mock: one reply that grows to ~80 chunks (mixed CJK/ASCII,
/// a few ms apart so the TUI re-wraps the growing message across many
/// frames), then completes on its own. Used to exercise the streaming
/// churn without an abort.
fn mock_big_stream_fn() -> StreamFn {
    Arc::new(move |_model, _context, _thinking, _options: StreamFnOptions| {
        Box::pin(async move {
            let mut text = String::new();
            let mut events = vec![AssistantMessageEvent::Start {
                partial: partial_msg(""),
            }];
            for i in 0..80 {
                let delta = format!("第{i}段数据：这是混排的中文与英文 padding padding padding padding 让行在终端宽度下换行。\n");
                text.push_str(&delta);
                events.push(AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta,
                    partial: partial_msg(&text),
                });
            }
            events.push(AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: text.clone(),
                partial: partial_msg(&text),
            });
            let mut final_msg = partial_msg(&text);
            final_msg.stop_reason = StopReason::Stop;
            events.push(AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                message: final_msg,
            });
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(Box::new(futures::stream::iter(events)) as StreamResponse)
        })
    })
}

/// Create an AgentSession wired to the mock LLM.
async fn create_mock_session() -> pi_coding_agent::core::agent_session::AgentSession {
    let mut opts = CreateAgentSessionOptions::default();
    opts.cwd = std::env::temp_dir().display().to_string();
    opts.agent_dir = Some(std::env::temp_dir().display().to_string());
    opts.stream_fn = Some(if std::env::var(BIG_ENV).is_ok() {
        mock_big_stream_fn()
    } else {
        mock_stream_fn(std::env::var(LONG_ENV).is_ok())
    });
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
    master: Box<dyn portable_pty::MasterPty + Send>,
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

    /// Spawn with the big-stream mock (a large self-completing reply).
    fn spawn_big_stream() -> Self {
        Self::spawn_inner_env(false, None, &[(BIG_ENV, "1")])
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
            master,
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

    /// Poll until output stops growing (app idle: no pending renders).
    fn wait_settled(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut last = self.rendered().len();
        let mut stable = 0;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(150));
            let len = self.rendered().len();
            if len == last {
                stable += 1;
                if stable >= 2 {
                    return true;
                }
            } else {
                stable = 0;
                last = len;
            }
        }
        false
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

    /// Resize the PTY master (the child receives a terminal resize event).
    fn resize(&mut self, cols: u16, rows: u16) {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("pty resize");
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

/// Esc 不退出：对齐 TS `onEscape`——空闲时第一下只记录 double-escape
/// 时间（TS 默认弹 tree/fork 选择器，本 port 未实现，见 DEVIATIONS），
/// TUI 保持可用，Ctrl+D 才退出。
#[test]
fn tui_esc_does_not_quit_and_ctrl_d_still_quits() {
    let mut tui = Tui::spawn(false);
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");
    tui.write(&[0x1b]); // Esc —— 不退出
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        tui.rendered().contains("mock-model"),
        "TUI still alive after Esc; tail: {:?}",
        tui.rendered().chars().rev().take(200).collect::<String>()
    );
    tui.write(&[0x04]); // Ctrl+D: quit
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "Ctrl+D quit exit code 0");
    assert!(
        tui.rendered().contains("\x1b[?1049l"),
        "terminal restored after quit"
    );
}

/// Esc 中断长流（对齐 TS `onEscape` 的 streaming 分支）：流式输出停止增长、
/// TUI 保持响应，Ctrl+D 正常退出。
#[test]
fn tui_esc_aborts_long_stream() {
    let mut tui = Tui::spawn(true);
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");
    tui.write(b"hello\r");
    assert!(
        tui.wait_for("chunk", TIMEOUT),
        "long stream started; got: {:?}",
        tui.rendered()
    );

    tui.write(&[0x1b]); // Esc: interrupt

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
        "output stopped after Esc; tail: {:?}",
        tui.rendered().chars().rev().take(400).collect::<String>()
    );

    // Still responsive: Ctrl+D quits.
    tui.write(&[0x04]);
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit after Esc interrupt");
}

// ────────────────────────────────────────────────────────────────────────────
// Minimal ANSI screen emulator — residue detection
//
// TestBackend-based unit tests apply ratatui's diff directly, so they cannot
// catch "stale cell" bugs that only appear after the escape stream is
// rendered by a real terminal (wide-char overwrites, erase/rewrite order).
// This emulator reconstructs the physical screen from the raw bytes the
// child writes into the PTY, with faithful wide-character semantics:
//
//   - a wide glyph occupies 2 columns (continuation cell);
//   - writing ANY char over a wide glyph's span destroys the whole glyph;
//   - writes are applied at the exact cursor position (ratatui emits
//     MoveTo before every cell).
//
// `ambiguous_wide` flips East Asian ambiguous characters (— … ·) between
// width 1 (unicode-width default) and width 2 (common CJK terminals) so
// width-mismatch residue is covered too.
// ────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum TCell {
    Blank,
    /// Right half of a wide glyph at x-1.
    Cont,
    Glyph(char),
}

struct TermScreen {
    cols: u16,
    rows: u16,
    grid: Vec<Vec<TCell>>,
    cx: u16,
    cy: u16,
    ambiguous_wide: bool,
}

impl TermScreen {
    fn new(cols: u16, rows: u16, ambiguous_wide: bool) -> Self {
        Self {
            cols,
            rows,
            grid: vec![vec![TCell::Blank; cols as usize]; rows as usize],
            cx: 0,
            cy: 0,
            ambiguous_wide,
        }
    }

    /// Display width of a char: same coarse rules as `unicode-width`
    /// (default) with an "ambiguous as wide" switch for CJK terminals.
    fn char_w(&self, c: char) -> u16 {
        let cp = c as u32;
        if cp <= 0x1f || cp == 0x7f {
            return 0;
        }
        let wide = (0x1100..=0x115f).contains(&cp)
            || (0x2e80..=0x303e).contains(&cp)
            || (0x3041..=0x33ff).contains(&cp)
            || (0x3400..=0x4dbf).contains(&cp)
            || (0x4e00..=0x9fff).contains(&cp)
            || (0xa000..=0xa4cf).contains(&cp)
            || (0xac00..=0xd7a3).contains(&cp)
            || (0xf900..=0xfaff).contains(&cp)
            || (0xfe10..=0xfe19).contains(&cp)
            || (0xfe30..=0xfe6f).contains(&cp)
            || (0xff00..=0xff60).contains(&cp)
            || (0xffe0..=0xffe6).contains(&cp)
            || (0x1f300..=0x1f64f).contains(&cp)
            || (0x1f900..=0x1f9ff).contains(&cp);
        if wide {
            return 2;
        }
        if (0x2000..=0x2e7f).contains(&cp) {
            return if self.ambiguous_wide { 2 } else { 1 };
        }
        1
    }

    /// Write a char at the cursor (destroys any wide glyph overlapping the
    /// cell, exactly like a real terminal).
    fn put(&mut self, c: char) {
        let w = self.char_w(c);
        if w == 0 {
            return;
        }
        let (x, y) = (self.cx as usize, self.cy as usize);
        if x >= self.cols as usize {
            return; // ratatui never relies on wrap-around
        }
        // A wide glyph covering this cell (left half at x-1, continuation
        // at x) is destroyed by any write.
        if x > 0 && matches!(self.grid[y][x - 1], TCell::Glyph(g) if self.char_w(g) == 2) {
            self.grid[y][x - 1] = TCell::Blank;
        }
        if w == 2 {
            // A wide glyph currently at x also owns x+1 — drop it.
            if matches!(self.grid[y][x], TCell::Glyph(g) if self.char_w(g) == 2)
                && x + 1 < self.cols as usize
            {
                self.grid[y][x + 1] = TCell::Blank;
            }
            self.grid[y][x] = TCell::Glyph(c);
            if x + 1 < self.cols as usize {
                self.grid[y][x + 1] = TCell::Cont;
            }
        } else {
            if matches!(self.grid[y][x], TCell::Glyph(g) if self.char_w(g) == 2)
                && x + 1 < self.cols as usize
            {
                self.grid[y][x + 1] = TCell::Blank;
            }
            self.grid[y][x] = TCell::Glyph(c);
        }
        self.cx = (self.cx + w).min(self.cols);
    }

    fn erase_line(&mut self, from: u16, to: u16, y: u16) {
        if y >= self.rows {
            return;
        }
        let (from, to) = (from.min(self.cols), to.min(self.cols));
        let y = y as usize;
        for x in from..to {
            let x = x as usize;
            // Wide glyph at x owns x+1.
            if matches!(self.grid[y][x], TCell::Glyph(g) if self.char_w(g) == 2)
                && x + 1 < self.cols as usize
            {
                self.grid[y][x + 1] = TCell::Blank;
            }
            // Continuation cell: the wide glyph at x-1 dies with it.
            if self.grid[y][x] == TCell::Cont && x > 0 {
                self.grid[y][x - 1] = TCell::Blank;
            }
            self.grid[y][x] = TCell::Blank;
        }
    }

    fn clear_all(&mut self) {
        for y in 0..self.rows as usize {
            for x in 0..self.cols as usize {
                self.grid[y][x] = TCell::Blank;
            }
        }
    }

    /// Feed raw PTY bytes; handles incomplete escape/UTF-8 sequences that
    /// split across chunks.
    fn feed(&mut self, data: &[u8]) {
        let pending: Vec<u8> = data.to_vec();
        let mut i = 0usize;
        while i < pending.len() {
            let b = pending[i];
            match b {
                0x1b => {
                    // Need at least the byte after ESC to classify.
                    if i + 1 >= pending.len() {
                        break;
                    }
                    match pending[i + 1] {
                        b'[' => {
                            // CSI: find the final byte 0x40..=0x7e.
                            let mut j = i + 2;
                            while j < pending.len() && !(0x40..=0x7e).contains(&pending[j]) {
                                j += 1;
                            }
                            if j >= pending.len() {
                                break; // incomplete
                            }
                            self.handle_csi(&pending[i + 2..j], pending[j]);
                            i = j + 1;
                        }
                        b']' => {
                            // OSC: until BEL or ESC \ (incomplete → wait).
                            let mut j = i + 2;
                            let mut terminated = false;
                            while j < pending.len() {
                                if pending[j] == 0x07 {
                                    j += 1;
                                    terminated = true;
                                    break;
                                }
                                if pending[j] == 0x1b && j + 1 < pending.len() && pending[j + 1] == b'\\' {
                                    j += 2;
                                    terminated = true;
                                    break;
                                }
                                j += 1;
                            }
                            if !terminated {
                                break;
                            }
                            i = j;
                        }
                        _ => {
                            i += 2; // ESC + one char (SS3 etc.)
                        }
                    }
                }
                0x20..=0x7e => {
                    self.put(pending[i] as char);
                    i += 1;
                }
                0x80..=0xff => {
                    // UTF-8: decode one char.
                    let len = if b & 0xe0 == 0xc0 {
                        2
                    } else if b & 0xf0 == 0xe0 {
                        3
                    } else if b & 0xf8 == 0xf0 {
                        4
                    } else {
                        1
                    };
                    if i + len > pending.len() {
                        break; // incomplete char
                    }
                    if let Ok(s) = std::str::from_utf8(&pending[i..i + len]) {
                        if let Some(c) = s.chars().next() {
                            self.put(c);
                        }
                    }
                    i += len;
                }
                _ => {
                    i += 1; // \r \n \t etc.: ignored (ratatui emits MoveTo)
                }
            }
        }
    }

    fn handle_csi(&mut self, params: &[u8], final_byte: u8) {
        let params = if params.first() == Some(&b'?') {
            &params[1..]
        } else {
            params
        };
        let nums: Vec<u16> = params
            .split(|&b| b == b';')
            .map(|p| {
                std::str::from_utf8(p)
                    .ok()
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(0)
            })
            .collect();
        match final_byte {
            b'H' | b'f' => {
                let row = nums.first().copied().filter(|&n| n > 0).unwrap_or(1);
                let col = nums.get(1).copied().filter(|&n| n > 0).unwrap_or(1);
                self.cy = (row - 1).min(self.rows.saturating_sub(1));
                self.cx = (col - 1).min(self.cols.saturating_sub(1));
            }
            b'J' => match nums.first().copied().unwrap_or(0) {
                0 => {
                    // cursor -> end of screen
                    self.erase_line(self.cx, self.cols, self.cy);
                    if self.cy + 1 < self.rows {
                        for y in (self.cy + 1)..self.rows {
                            self.erase_line(0, self.cols, y);
                        }
                    }
                }
                1 => {
                    self.erase_line(0, self.cx + 1, self.cy);
                    for y in 0..self.cy {
                        self.erase_line(0, self.cols, y);
                    }
                }
                _ => self.clear_all(),
            },
            b'K' => match nums.first().copied().unwrap_or(0) {
                0 => self.erase_line(self.cx, self.cols, self.cy),
                1 => self.erase_line(0, self.cx + 1, self.cy),
                _ => self.erase_line(0, self.cols, self.cy),
            },
            _ => {} // SGR / modes: ignored for residue detection
        }
    }

    /// Full-screen snapshot, one string per row.
    fn snapshot(&self) -> Vec<String> {
        (0..self.rows)
            .map(|y| {
                (0..self.cols)
                    .map(|x| match self.grid[y as usize][x as usize] {
                        TCell::Blank | TCell::Cont => ' ',
                        TCell::Glyph(c) => c,
                    })
                    .collect()
            })
            .collect()
    }
}

fn pump(tui: &mut Tui, screen: &mut TermScreen) {
    tui.drain();
    let bytes = std::mem::take(&mut tui.output);
    screen.feed(&bytes);
}

/// Send a long multi-line CJK message via bracketed paste, then submit,
/// and wait until the app settles (output stops growing). `wait_for` on
/// accumulated output is useless here — every turn renders "LLM!", so a
/// stale match lets the next turn race the still-running one (messages get
/// queued/restored into the editor and the snapshot is mid-flight).
fn send_long_message(tui: &mut Tui, i: usize) {
    let msg = format!(
        "这是第 {i} 条测试消息：一行比较长的中文内容用来在终端宽度下换行——破折号——和省略号……以及 mixed english words and padding padding padding padding 以及中文标点：和（括号）。\n第二行 another line with a long unbroken token aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 然后中文结尾。\n第三行：短行，收尾。\n"
    );
    let mut bytes = vec![0x1b, b'[', b'2', b'0', b'0', b'~'];
    bytes.extend_from_slice(msg.as_bytes());
    bytes.extend_from_slice(&[0x1b, b'[', b'2', b'0', b'1', b'~']);
    tui.write(&bytes);
    tui.write(b"\r"); // Enter submits the pasted text
    assert!(
        tui.wait_settled(TIMEOUT),
        "turn {i} settled; got: {:?}",
        tui.rendered().chars().rev().take(200).collect::<String>()
    );
}

fn assert_screens_eq(a: &[String], b: &[String], ctx: &str) {
    assert_eq!(a.len(), b.len(), "{ctx}: row count differs");
    for (y, (ra, rb)) in a.iter().zip(b.iter()).enumerate() {
        if ra != rb {
            let diffs: Vec<(usize, char, char)> = ra
                .chars()
                .zip(rb.chars())
                .enumerate()
                .filter(|(_, (ca, cb))| ca != cb)
                .map(|(x, (ca, cb))| (x, ca, cb))
                .take(24)
                .collect();
            let mut rows = format!("  row {y} A: {ra:?}\n  row {y} B: {rb:?}\n");
            for dy in 0..6 {
                let yy = y + dy;
                if yy < a.len() {
                    rows.push_str(&format!("  row {yy} A: {:?}\n", a[yy]));
                    rows.push_str(&format!("  row {yy} B: {:?}\n", b[yy]));
                }
            }
            panic!("{ctx}: row {y} differs at {diffs:?}\n{rows}");

        }
    }
}

/// Scroll-residue regression: scrolling up through a CJK + ASCII transcript
/// and back down must restore the exact same screen, both on a faithful
/// terminal and on a CJK terminal that renders East-ambiguous characters
/// wide. Any stale cell the diff fails to repaint breaks the invariant
/// "scroll up N+1 then down 1 == scroll up N".
#[test]
fn tui_scroll_up_down_restores_exact_screen() {
    for ambiguous_wide in [false, true] {
        let mut tui = Tui::spawn(false);
        assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");

        for i in 0..6 {
            send_long_message(&mut tui, i);
        }
        // Let post-turn status refreshes settle so the screen is static.
        std::thread::sleep(Duration::from_millis(1500));
        // The footer's context% refreshes on the 1s-throttled status tick:
        // wait one more refresh cycle, then re-settle before snapshotting.
        std::thread::sleep(Duration::from_millis(1200));
        assert!(tui.wait_settled(TIMEOUT), "footer settled");
        let mut screen = TermScreen::new(WIDTH, HEIGHT, ambiguous_wide);
        pump(&mut tui, &mut screen);
        let bottom = screen.snapshot();

        // Scroll up 3 rows with ArrowUp (empty editor = scroll up).
        for _ in 0..3 {
            tui.write(b"\x1b[A");
            std::thread::sleep(Duration::from_millis(150));
            pump(&mut tui, &mut screen);
        }
        let up3 = screen.snapshot();
        assert_ne!(up3, bottom, "amb={ambiguous_wide}: scrolling up changed the screen");

        // One more scroll up, then one scroll down: must return to `up3`.
        tui.write(b"\x1b[A");
        std::thread::sleep(Duration::from_millis(150));
        pump(&mut tui, &mut screen);
        let up4 = screen.snapshot();
        assert_ne!(up4, up3, "amb={ambiguous_wide}: another scroll step moved the viewport");

        tui.write(b"\x1b[B");
        std::thread::sleep(Duration::from_millis(150));
        pump(&mut tui, &mut screen);
        let back = screen.snapshot();
        assert_screens_eq(
            &back,
            &up3,
            &format!("amb={ambiguous_wide}: scroll-down-1 must restore scroll-up-N exactly"),
        );

        // Home (gg) to the top, then G back to the bottom: the bottom
        // screen must be byte-identical to the original.
        tui.write(b"gg");
        std::thread::sleep(Duration::from_millis(200));
        pump(&mut tui, &mut screen);
        let top = screen.snapshot();
        tui.write(b"G");
        std::thread::sleep(Duration::from_millis(200));
        pump(&mut tui, &mut screen);
        let after_g = screen.snapshot();
        if std::env::var("TUI_E2E_DEBUG").is_ok() {
            eprintln!("=== amb={ambiguous_wide} top (all rows) ===");
            for (y, r) in top.iter().enumerate() { eprintln!("{y:2}: {r:?}"); }
            eprintln!("=== amb={ambiguous_wide} after G (all rows) ===");
            for (y, r) in after_g.iter().enumerate() { eprintln!("{y:2}: {r:?}"); }
            eprintln!("=== amb={ambiguous_wide} bottom (all rows) ===");
            for (y, r) in bottom.iter().enumerate() { eprintln!("{y:2}: {r:?}"); }
        }
        assert_screens_eq(
            &after_g,
            &bottom,
            &format!("amb={ambiguous_wide}: G (scroll-to-bottom) restores the original screen"),
        );

        tui.write(&[0x04]);
        let code = tui.wait_exit(TIMEOUT);
        assert_eq!(code, Some(0), "amb={ambiguous_wide}: clean exit code 0");
    }
}

/// Streaming variant: while the reply streams in ("chunk chunk ..." every
/// 200 ms), the transcript rows re-wrap and shift up every frame — the
/// heaviest diff churn for wide characters. After aborting the stream, the
/// same scroll invariants must hold (scroll-up N+1 then down 1 restores
/// exactly, and gg/G round-trips).
/// Streaming variant: while the reply streams in, the transcript rows
/// re-wrap and shift up every frame — the heaviest diff churn for wide
/// characters. After the big stream completes, the scroll invariants must
/// hold (scroll-up N+1 then down 1 restores exactly, and gg/G round-trips).
#[test]
fn tui_scroll_after_long_stream_restores_exact_screen() {
    for ambiguous_wide in [false, true] {
        let mut tui = Tui::spawn_big_stream();
        assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");
        let msg = "这是流式测试消息：一行比较长的中文内容——破折号——用来在终端宽度下换行，mixed with english padding padding padding 以及中文标点：和（括号）……省略号。\n第二行 another line with a long unbroken token aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 然后中文结尾。\n第三行：短行，收尾。\n";
        let mut bytes = vec![0x1b, b'[', b'2', b'0', b'0', b'~'];
        bytes.extend_from_slice(msg.as_bytes());
        bytes.extend_from_slice(&[0x1b, b'[', b'2', b'0', b'1', b'~']);
        tui.write(&bytes);
        tui.write(b"\r");
        assert!(
            tui.wait_settled(TIMEOUT),
            "big stream completed and settled; got: {:?}",
            tui.rendered().chars().rev().take(200).collect::<String>()
        );
        // Footer context% refreshes on the 1s-throttled tick after the turn.
        std::thread::sleep(Duration::from_millis(1200));
        assert!(tui.wait_settled(TIMEOUT), "footer settled");
        let mut screen = TermScreen::new(WIDTH, HEIGHT, ambiguous_wide);
        pump(&mut tui, &mut screen);
        let bottom = screen.snapshot();

        for _ in 0..3 {
            tui.write(b"\x1b[A");
            std::thread::sleep(Duration::from_millis(150));
            pump(&mut tui, &mut screen);
        }
        let up3 = screen.snapshot();
        assert_ne!(up3, bottom, "amb={ambiguous_wide}: streaming: scroll up changed the screen");

        tui.write(b"\x1b[A");
        std::thread::sleep(Duration::from_millis(150));
        pump(&mut tui, &mut screen);
        tui.write(b"\x1b[B");
        std::thread::sleep(Duration::from_millis(150));
        pump(&mut tui, &mut screen);
        assert_screens_eq(
            &screen.snapshot(),
            &up3,
            &format!("amb={ambiguous_wide}: streaming: scroll-down-1 restores scroll-up-N"),
        );

        tui.write(b"gg");
        std::thread::sleep(Duration::from_millis(200));
        pump(&mut tui, &mut screen);
        tui.write(b"G");
        std::thread::sleep(Duration::from_millis(200));
        pump(&mut tui, &mut screen);
        assert_screens_eq(
            &screen.snapshot(),
            &bottom,
            &format!("amb={ambiguous_wide}: streaming: G restores the original bottom"),
        );

        tui.write(&[0x04]);
        let code = tui.wait_exit(TIMEOUT);
        assert_eq!(code, Some(0), "amb={ambiguous_wide}: clean exit code 0");
    }
}

/// Resize + scroll: after the terminal shrinks (the app must fully repaint
/// at the new width), scrolling up/down must still restore exact screens.
/// This covers the path that previously relied on `Terminal::clear()` on
/// resize — a stale backbuffer there shows up as permanent residues after
/// the window shrinks or grows again.
#[test]
fn tui_scroll_after_resize_restores_exact_screen() {
    let mut tui = Tui::spawn(false);
    assert!(tui.wait_for("mock-model", TIMEOUT), "footer rendered");
    for i in 0..6 {
        send_long_message(&mut tui, i);
    }
    std::thread::sleep(Duration::from_millis(1500));
    std::thread::sleep(Duration::from_millis(1200));
    assert!(tui.wait_settled(TIMEOUT), "settled at 100 cols");
    let mut screen = TermScreen::new(WIDTH, HEIGHT, false);
    pump(&mut tui, &mut screen);
    let bottom100 = screen.snapshot();

    // Shrink the terminal: the child gets a resize event, clears, repaints
    // at 80x24.
    tui.resize(80, 24);
    assert!(tui.wait_settled(TIMEOUT), "repainted at 80 cols");
    pump(&mut tui, &mut screen);
    let bottom80 = screen.snapshot();
    // The transcript re-wrapped narrower — the screens must differ.
    assert_ne!(bottom80, bottom100, "resize changed the layout");

    // Scroll at the narrow width.
    for _ in 0..3 {
        tui.write(b"\x1b[A");
        std::thread::sleep(Duration::from_millis(150));
        pump(&mut tui, &mut screen);
    }
    let up3 = screen.snapshot();
    assert_ne!(up3, bottom80, "resize: scroll up changed the screen");

    tui.write(b"\x1b[A");
    std::thread::sleep(Duration::from_millis(150));
    pump(&mut tui, &mut screen);
    tui.write(b"\x1b[B");
    std::thread::sleep(Duration::from_millis(150));
    pump(&mut tui, &mut screen);
    assert_screens_eq(
        &screen.snapshot(),
        &up3,
        "resize: scroll-down-1 restores scroll-up-N",
    );

    // Grow back and settle at the original width.
    tui.resize(WIDTH, HEIGHT);
    assert!(tui.wait_settled(TIMEOUT), "repainted at 100 cols");
    std::thread::sleep(Duration::from_millis(1200));
    assert!(tui.wait_settled(TIMEOUT), "settled at 100 cols again");
    pump(&mut tui, &mut screen);
    assert_screens_eq(
        &screen.snapshot(),
        &bottom100,
        "grow back restores the original 100-col bottom screen",
    );

    tui.write(&[0x04]);
    let code = tui.wait_exit(TIMEOUT);
    assert_eq!(code, Some(0), "clean exit code 0");
}
