//! End-to-end tests for the extension system.
//!
//! Covers: event dispatch (fire-and-forget + result-returning), error handling,
//! hot reload, tool management API, and lifecycle events.

use pi_agent_core::types::{AgentContext, AgentMessage, AgentToolCall, BeforeToolCallContext};
use pi_coding_agent::core::extensions::dispatcher;
use pi_coding_agent::core::extensions::runtime::ExtensionRuntime;

/// A comprehensive test extension that exercises all major features.
const FULL_EXTENSION_SRC: &str = r#"
export default function (pi) {
  // Register tools
  pi.registerTool({
    name: "greet",
    description: "Greet someone",
    parameters: { type: "object", properties: { name: { type: "string" } } },
    async execute(callId, args, signal, onUpdate, ctx) {
      return { greeting: "Hello, " + (args.name ?? "world") + "!" };
    },
  });

  pi.registerTool({
    name: "count",
    description: "Count to a number",
    parameters: { type: "object", properties: { n: { type: "integer" } } },
    async execute(callId, args, signal, onUpdate, ctx) {
      const n = args.n ?? 0;
      let result = "";
      for (let i = 1; i <= n; i++) {
        result += i + (i < n ? ", " : "");
      }
      return { count: result };
    },
  });

  // Register event handlers
  pi.on("session_start", (event, ctx) => {
    pi.ui.notify("session started: " + (event.reason ?? "unknown"));
  });

  pi.on("agent_start", (event, ctx) => {
    pi.ui.notify("agent started");
  });

  pi.on("tool_call", (event, ctx) => {
    if (event.toolName === "blocked_tool") {
      return { block: true, reason: "blocked by test" };
    }
    return { block: false };
  });

  pi.on("tool_result", (event, ctx) => {
    if (event.toolName === "greet" && event.content && event.content.greeting) {
      return { content: { greeting: event.content.greeting + " (modified)" } };
    }
    return null;
  });

  pi.on("message_end", (event, ctx) => {
    // Just acknowledge receipt
    pi.ui.notify("message ended");
  });

  // Register commands
  pi.registerCommand("test-command", { description: "A test command" });

  // Register flags
  pi.registerFlag("test-flag", { description: "A test flag", type: "boolean", default: false });

  // Test getFlag
  const flag = pi.getFlag("test-flag");
  if (flag !== false) {
    throw new Error("Expected test-flag to be false, got " + flag);
  }
}
"#;

/// A minimal extension that tests error handling.
const ERROR_EXTENSION_SRC: &str = r#"
export default function (pi) {
  pi.registerTool({
    name: "error_tool",
    description: "A tool that throws an error",
    parameters: { type: "object", properties: {} },
    async execute(callId, args, signal, onUpdate, ctx) {
      throw new Error("Intentional error for testing");
    },
  });

  pi.on("session_start", (event, ctx) => {
    throw new Error("Handler error for testing");
  });
}
"#;

/// A minimal extension for hot-reload testing.
const RELOAD_EXTENSION_SRC: &str = r#"
export default function (pi) {
  pi.registerTool({
    name: "reload_tool",
    description: "A tool for reload testing",
    parameters: { type: "object", properties: {} },
    async execute(callId, args, signal, onUpdate, ctx) {
      return { reloaded: true };
    },
  });
}
"#;

fn write_extension(temp: &tempfile::TempDir, name: &str, src: &str) -> std::path::PathBuf {
    let ext_path = temp.path().join(name);
    std::fs::write(&ext_path, src).unwrap();
    ext_path
}

fn before_ctx(tool_name: &str, args: serde_json::Value) -> BeforeToolCallContext {
    BeforeToolCallContext {
        assistant_message: AgentMessage::User {
            content: vec![],
            timestamp: 0,
        },
        tool_call: AgentToolCall {
            id: "call_1".into(),
            name: tool_name.into(),
            arguments: args.clone(),
        },
        args,
        context: AgentContext {
            system_prompt: String::new(),
            messages: vec![],
            tools: None,
        },
    }
}

// ============================================================================
// Basic extension lifecycle
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_extension_load_and_tool_execution() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext_path = write_extension(&temp, "full_ext.ts", FULL_EXTENSION_SRC);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // Verify tools are registered
    let names: std::collections::HashSet<&str> =
        load.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("greet"), "greet tool should be registered");
    assert!(names.contains("count"), "count tool should be registered");
    assert!(load.errors.is_empty(), "load produced errors: {:?}", load.errors);

    // Execute greet tool
    let resp = rt
        .call_tool("greet", serde_json::json!({ "name": "World" }), &cwd)
        .await
        .expect("greet call failed");
    assert_eq!(resp.result["greeting"], "Hello, World!");

    // Execute count tool
    let resp = rt
        .call_tool("count", serde_json::json!({ "n": 5 }), &cwd)
        .await
        .expect("count call failed");
    assert_eq!(resp.result["count"], "1, 2, 3, 4, 5");

    let _ = rt.stop().await;
}

// ============================================================================
// Event dispatch tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_extension_fire_and_forget_events() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext_path = write_extension(&temp, "event_ext.ts", FULL_EXTENSION_SRC);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let _load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // Dispatch session_start (fire-and-forget)
    let result = rt
        .dispatch_fire_and_forget(
            "session_start",
            serde_json::json!({ "type": "session_start", "reason": "test" }),
        )
        .await;
    assert!(result.is_ok(), "session_start dispatch should succeed");

    // Dispatch agent_start (fire-and-forget)
    let result = rt
        .dispatch_fire_and_forget("agent_start", serde_json::json!({}))
        .await;
    assert!(result.is_ok(), "agent_start dispatch should succeed");

    let _ = rt.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_extension_result_returning_events() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext_path = write_extension(&temp, "result_ext.ts", FULL_EXTENSION_SRC);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let _load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // Dispatch tool_call (result-returning) - should NOT block greet
    let result = rt
        .dispatch_result(
            "tool_call",
            serde_json::json!({
                "type": "tool_call",
                "toolCallId": "call-1",
                "toolName": "greet",
                "input": { "name": "test" },
            }),
        )
        .await;
    assert!(result.is_ok());
    let val = result.unwrap();
    assert_eq!(val.get("block").and_then(|v| v.as_bool()), Some(false));

    // Dispatch tool_call (result-returning) - should block blocked_tool
    let result = rt
        .dispatch_result(
            "tool_call",
            serde_json::json!({
                "type": "tool_call",
                "toolCallId": "call-2",
                "toolName": "blocked_tool",
                "input": {},
            }),
        )
        .await;
    assert!(result.is_ok());
    let val = result.unwrap();
    assert_eq!(val.get("block").and_then(|v| v.as_bool()), Some(true));
    assert!(val
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains("blocked"));

    // Dispatch tool_result (result-returning) - should modify greet result
    let result = rt
        .dispatch_result(
            "tool_result",
            serde_json::json!({
                "type": "tool_result",
                "toolCallId": "call-3",
                "toolName": "greet",
                "content": { "greeting": "Hello, World!" },
                "details": null,
                "isError": false,
            }),
        )
        .await;
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(!val.is_null(), "tool_result should return modified content");
    assert!(val
        .get("content")
        .and_then(|v| v.get("greeting"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains("modified"));

    // Dispatch message_end (result-returning)
    let result = rt
        .dispatch_result(
            "message_end",
            serde_json::json!({
                "type": "message_end",
                "message": { "role": "assistant", "content": "test" },
            }),
        )
        .await;
    assert!(result.is_ok());

    let _ = rt.stop().await;
}

// ============================================================================
// Error handling tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_extension_error_handling() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext_path = write_extension(&temp, "error_ext.ts", ERROR_EXTENSION_SRC);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // The error_tool should be registered despite the error handler
    assert!(load.tools.iter().any(|t| t.name == "error_tool"));

    // Calling error_tool should return an error
    let resp = rt
        .call_tool("error_tool", serde_json::json!({}), &cwd)
        .await;
    assert!(resp.is_err(), "error_tool should fail");

    // Dispatch session_start - the error handler should not crash the runtime
    let result = rt
        .dispatch_fire_and_forget(
            "session_start",
            serde_json::json!({ "type": "session_start", "reason": "test" }),
        )
        .await;
    assert!(result.is_ok(), "dispatch should not fail despite handler error");

    let _ = rt.stop().await;
}

// ============================================================================
// Hot reload tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_extension_hot_reload() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();

    // Place extension in a project-local path so it's reloadable
    let ext_dir = temp.path().join(".pi-rs").join("extensions").join("reload_ext");
    std::fs::create_dir_all(&ext_dir).unwrap();
    let ext_path = ext_dir.join("index.ts");
    std::fs::write(&ext_path, RELOAD_EXTENSION_SRC).unwrap();

    // Use the parent dir as cwd so the extension is discovered as project-local
    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let load = rt.load(&cwd, None, &[]).await.expect("load failed");

    // Verify reload_tool is registered
    assert!(
        load.tools.iter().any(|t| t.name == "reload_tool"),
        "reload_tool should be registered after initial load"
    );

    // Execute reload_tool
    let resp = rt
        .call_tool("reload_tool", serde_json::json!({}), &cwd)
        .await
        .expect("reload_tool call failed");
    assert_eq!(resp.result["reloaded"], true);

    // Reload extensions
    let reload_result = rt
        .reload(&cwd, None, &[])
        .await
        .expect("reload failed");
    assert!(
        reload_result.tools.iter().any(|t| t.name == "reload_tool"),
        "reload_tool should still be registered after reload"
    );

    // Execute reload_tool again after reload
    let resp = rt
        .call_tool("reload_tool", serde_json::json!({}), &cwd)
        .await
        .expect("reload_tool call after reload failed");
    assert_eq!(resp.result["reloaded"], true);

    let _ = rt.stop().await;
}

// ============================================================================
// Tool management API tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_extension_tool_management() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext_path = write_extension(&temp, "tool_mgmt_ext.ts", FULL_EXTENSION_SRC);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // Verify tools are registered
    assert!(load.tools.iter().any(|t| t.name == "greet"));
    assert!(load.tools.iter().any(|t| t.name == "count"));

    // Verify commands are registered
    assert!(load.commands.iter().any(|c| c.name == "test-command"));

    let _ = rt.stop().await;
}

// ============================================================================
// Multiple extensions
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_extensions() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext1 = write_extension(&temp, "ext1.ts", FULL_EXTENSION_SRC);
    let ext2 = write_extension(&temp, "ext2.ts", RELOAD_EXTENSION_SRC);
    let paths = vec![
        ext1.to_string_lossy().to_string(),
        ext2.to_string_lossy().to_string(),
    ];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // Tools from both extensions should be registered
    let names: std::collections::HashSet<&str> =
        load.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("greet"), "greet from ext1");
    assert!(names.contains("count"), "count from ext1");
    assert!(names.contains("reload_tool"), "reload_tool from ext2");
    assert!(load.errors.is_empty(), "load produced errors: {:?}", load.errors);

    let _ = rt.stop().await;
}

// ============================================================================
// Before tool call dispatch via dispatcher
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatcher_before_tool_call() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext_path = write_extension(&temp, "dispatch_ext.ts", FULL_EXTENSION_SRC);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let _load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // blocked_tool should be blocked
    let ctx = before_ctx("blocked_tool", serde_json::json!({}));
    let blocked = dispatcher::dispatch_tool_call(&rt, &ctx).await;
    assert!(blocked.is_some(), "blocked_tool should be blocked");
    let blocked = blocked.unwrap();
    assert!(blocked.block);
    assert!(blocked.reason.as_deref().unwrap_or("").contains("blocked"));

    // greet should NOT be blocked
    let ctx = before_ctx("greet", serde_json::json!({}));
    let not_blocked = dispatcher::dispatch_tool_call(&rt, &ctx).await;
    assert!(not_blocked.is_none(), "greet should not be blocked");

    let _ = rt.stop().await;
}

// ============================================================================
// Session lifecycle events
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_session_lifecycle_events() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext_path = write_extension(&temp, "lifecycle_ext.ts", FULL_EXTENSION_SRC);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let _load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // Dispatch session lifecycle events (fire-and-forget)
    let events = vec![
        ("session_start", serde_json::json!({"type": "session_start", "reason": "test"})),
        ("session_shutdown", serde_json::json!({"type": "session_shutdown", "reason": "quit"})),
        ("session_before_switch", serde_json::json!({"type": "session_before_switch", "targetSession": "/tmp/test.jsonl"})),
        ("session_before_fork", serde_json::json!({"type": "session_before_fork", "entryId": "entry-1"})),
        ("session_before_compact", serde_json::json!({"type": "session_before_compact", "reason": "auto"})),
        ("session_compact", serde_json::json!({"type": "session_compact", "summary": "test", "tokensBefore": 1000})),
        ("session_before_tree", serde_json::json!({"type": "session_before_tree", "reason": "navigate"})),
        ("session_tree", serde_json::json!({"type": "session_tree", "tree": []})),
    ];

    for (event_type, payload) in events {
        let result = rt.dispatch_fire_and_forget(event_type, payload).await;
        assert!(result.is_ok(), "{} dispatch should succeed", event_type);
    }

    let _ = rt.stop().await;
}

// ============================================================================
// before_agent_start event
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_before_agent_start_event() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();

    // Extension that handles before_agent_start
    const BEFORE_AGENT_EXT: &str = r#"
export default function (pi) {
  pi.on("before_agent_start", (event, ctx) => {
    pi.ui.notify("before_agent_start fired");
    return { modified: true };
  });
}
"#;

    let ext_path = write_extension(&temp, "before_agent.ts", BEFORE_AGENT_EXT);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let _load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // Dispatch before_agent_start (result-returning)
    let result = rt
        .dispatch_result(
            "before_agent_start",
            serde_json::json!({
                "type": "before_agent_start",
                "systemPrompt": "test",
                "messages": [],
            }),
        )
        .await;
    assert!(result.is_ok(), "before_agent_start dispatch should succeed");

    let _ = rt.stop().await;
}
