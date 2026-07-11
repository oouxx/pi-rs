//! End-to-end test for the embedded deno_core extension runtime.
//!
//! Spawns a real `ExtensionRuntime`, loads a TypeScript extension from disk,
//! calls a registered tool, checks notifications + cwd injection, and
//! exercises the before_tool_call block hook — i.e. the full V8 round-trip
//! that no other test in this crate covers.

#![cfg(test)]

use pi_agent_core::types::{AgentContext, AgentMessage, AgentToolCall, BeforeToolCallContext};

use pi_coding_agent::core::extensions::dispatcher;
use pi_coding_agent::core::extensions::runtime::ExtensionRuntime;

/// A minimal extension that exercises the live paths the review found bugs in:
///   - tool registration + execute round-trip
///   - `pi.ui.notify` reaching the JS `pendingNotifications` buffer
///   - `pi.exec` defaulting to the session cwd
///   - a `tool_call` handler that blocks a specific tool
const EXTENSION_SRC: &str = r#"
export default function (pi) {
  pi.registerTool({
    name: "echo",
    description: "echo back the input message",
    parameters: { type: "object", properties: { msg: { type: "string" } } },
    async execute(callId, args, signal, onUpdate, ctx) {
      pi.ui.notify("echoing: " + (args.msg ?? ""));
      return { echoed: args.msg ?? "" };
    },
  });

  pi.registerTool({
    name: "whereami",
    description: "print the working directory the exec runs in",
    parameters: { type: "object", properties: {} },
    async execute(callId, args, signal, onUpdate, ctx) {
      // No explicit cwd — should default to the session cwd (the temp dir).
      const r = await pi.exec("pwd", []);
      return { stdout: r.stdout, stderr: r.stderr, exitCode: r.exitCode };
    },
  });

  pi.on("tool_call", (event, ctx) => {
    if (event.toolName === "blocked_tool") {
      return { block: true, reason: "forbidden by test extension" };
    }
    return { block: false };
  });
}
"#;

fn write_extension(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let ext_path = temp.path().join("test_ext.ts");
    std::fs::write(&ext_path, EXTENSION_SRC).unwrap();
    ext_path
}

/// Build a `BeforeToolCallContext` for a synthetic tool call. Only the fields
/// the dispatcher's `tool_call_payload` reads (`tool_call.id`, `tool_call.name`,
/// `args`) matter; `assistant_message`/`context` are unused by the dispatcher.
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

#[tokio::test(flavor = "multi_thread")]
async fn extension_loads_and_tool_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let ext_path = write_extension(&temp);
    let paths = vec![ext_path.to_string_lossy().to_string()];

    let rt = ExtensionRuntime::new().expect("extension runtime should spawn");
    let load = rt.load(&cwd, None, &paths).await.expect("load failed");

    // The extension registers two tools.
    let names: std::collections::HashSet<&str> =
        load.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("echo"), "echo tool should be registered: {load:?}");
    assert!(names.contains("whereami"), "whereami tool should be registered: {load:?}");
    assert!(load.errors.is_empty(), "load produced errors: {load:?}");

    // echo round-trip + notification bridge.
    let resp = rt
        .call_tool("echo", serde_json::json!({ "msg": "hi" }), &cwd)
        .await
        .expect("echo call failed");
    assert_eq!(resp.result["echoed"], "hi", "tool result should round-trip: {resp:?}");
    assert!(
        resp.notifications.iter().any(|n| n.contains("echoing: hi")),
        "notification should reach the caller via the JS buffer: {resp:?}"
    );

    // cwd injection: pi.exec("pwd", []) with no explicit cwd should run in the
    // session cwd (the temp dir), not the process cwd.
    let resp = rt
        .call_tool("whereami", serde_json::json!({}), &cwd)
        .await
        .expect("whereami call failed");
    let stdout = resp.result["stdout"].as_str().unwrap_or("").trim().to_string();
    let stderr = resp.result["stderr"].as_str().unwrap_or("");
    let exit_code = resp.result["exitCode"].as_i64().unwrap_or(-999);
    let canon_cwd = temp
        .path()
        .canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.clone());
    assert!(
        stdout == cwd || stdout == canon_cwd,
        "pi.exec should default to session cwd {cwd} (or {canon_cwd}), got stdout={stdout:?} stderr={stderr:?} exitCode={exit_code}"
    );

    // before_tool_call block hook via the dispatcher.
    let ctx = before_ctx("blocked_tool", serde_json::json!({}));
    let blocked = dispatcher::dispatch_tool_call(&rt, &ctx).await;
    assert!(
        blocked.is_some(),
        "blocked_tool should be blocked by the tool_call hook"
    );
    let blocked = blocked.unwrap();
    assert!(blocked.block, "block flag should be true");
    assert!(
        blocked.reason.as_deref().unwrap_or("").contains("forbidden"),
        "block reason should come from the extension: {blocked:?}"
    );

    // A non-blocked tool should NOT be blocked.
    let ctx = before_ctx("echo", serde_json::json!({}));
    let not_blocked = dispatcher::dispatch_tool_call(&rt, &ctx).await;
    assert!(not_blocked.is_none(), "echo should not be blocked: {not_blocked:?}");

    let _ = rt.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_extensions_loads_clean_and_yields_no_tools() {
    // An empty temp dir with no extension files: load should succeed with
    // zero tools and zero errors (the runtime is spawned but finds nothing).
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let rt = ExtensionRuntime::new().expect("runtime should spawn");
    let load = rt.load(&cwd, None, &[]).await.expect("load failed");
    assert!(load.tools.is_empty(), "no tools expected: {load:?}");
    assert!(load.errors.is_empty(), "no errors expected: {load:?}");
    let _ = rt.stop().await;
}