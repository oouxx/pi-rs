//! subagent 扩展测试：假 pi 脚本模拟子进程 JSONL 输出，验证解析/超时/深度限制。

use super::*;
use pi_extension_api::{ExtensionContext, ExtensionUIContext, RuntimeHandle};
use std::sync::Arc;

/// 构造测试 ExtensionContext：可自定义 cwd 和深度环境变量。
fn test_ctx(cwd: &str, depth: Option<&str>) -> ExtensionContext {
    let mut handle = RuntimeHandle::noop();
    let cwd_owned = cwd.to_string();
    handle.get_cwd = Arc::new(move || cwd_owned.clone());
    let depth_owned = depth.map(|s| s.to_string());
    handle.get_env = Arc::new(move |name| {
        if name == SUBAGENT_DEPTH_ENV {
            depth_owned.clone()
        } else {
            None
        }
    });
    ExtensionContext::new(
        "test-session".into(),
        false,
        ExtensionUIContext {
            notify: Arc::new(|_, _| {}),
            set_status: Arc::new(|_, _| {}),
            confirm: Arc::new(|_, _| false),
        },
        handle,
    )
}

/// 写一个可执行的假 pi 脚本（输出固定 JSONL 事件流）。
fn write_fake_pi(dir: &std::path::Path, body: &str) -> String {
    let script = dir.join("fake-pi.sh");
    std::fs::write(&script, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script.to_string_lossy().to_string()
}

/// 正常输出：message_end 提取最终文本。
#[tokio::test]
async fn test_subagent_parses_child_output() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(
        dir.path(),
        r#"#!/bin/sh
echo '{"type":"start","message":{"role":"assistant","content":[]}}'
echo '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hello from child"}]}}'
echo '{"type":"end"}'
"#,
    );
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call("subagent", json!({ "task": "do something" }), &ctx)
        .await
        .expect("handled");
    assert!(!out.is_error, "should succeed, got: {out:?}");
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("hello from child"), "got: {text}");
}

/// 多段文本拼接 + image 块跳过。
#[tokio::test]
async fn test_subagent_concatenates_text_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(
        dir.path(),
        r#"#!/bin/sh
echo '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"part one"},{"type":"image","source":{"type":"base64","data":"x"}},{"type":"text","text":"part two"}]}}'
echo '{"type":"end"}'
"#,
    );
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call("subagent", json!({ "task": "t" }), &ctx)
        .await
        .expect("handled");
    assert!(!out.is_error);
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("part one") && text.contains("part two"), "got: {text}");
    assert!(!text.contains("base64"), "image block should be skipped: {text}");
}

/// 子进程无输出（无 message_end）：报错。
#[tokio::test]
async fn test_subagent_empty_output_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(
        dir.path(),
        r#"#!/bin/sh
echo '{"type":"start","message":{"role":"assistant","content":[]}}'
echo '{"type":"end"}'
"#,
    );
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call("subagent", json!({ "task": "t" }), &ctx)
        .await
        .expect("handled");
    assert!(out.is_error, "empty output should be an error, got: {out:?}");
}

/// 超时：脚本 sleep 超过 timeoutSeconds，报超时错误。
#[tokio::test]
async fn test_subagent_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(
        dir.path(),
        r#"#!/bin/sh
sleep 5
echo '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"late"}]}}'
echo '{"type":"end"}'
"#,
    );
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call("subagent", json!({ "task": "t", "timeoutSeconds": 1 }), &ctx)
        .await
        .expect("handled");
    assert!(out.is_error, "should time out, got: {out:?}");
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("timed out"), "got: {text}");
}

/// 深度限制：环境变量达到 MAX_DEPTH 时直接拒绝。
#[tokio::test]
async fn test_subagent_depth_limit() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(dir.path(), "#!/bin/sh\necho '{}'\n");
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", Some(&MAX_DEPTH.to_string()));
    let out = ext
        .handle_tool_call("subagent", json!({ "task": "t" }), &ctx)
        .await
        .expect("handled");
    assert!(out.is_error, "depth limit should reject, got: {out:?}");
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("depth limit"), "got: {text}");
}

/// 非 subagent 工具：返回 None（不处理）。
#[tokio::test]
async fn test_subagent_ignores_other_tools() {
    let ext = SubagentExtension::new();
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call("other_tool", json!({}), &ctx)
        .await;
    assert!(out.is_none(), "should not handle other tools");
}

/// 缺 task 参数：报错。
#[tokio::test]
async fn test_subagent_requires_task() {
    let ext = SubagentExtension::new();
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call("subagent", json!({}), &ctx)
        .await
        .expect("handled");
    assert!(out.is_error, "missing task should error, got: {out:?}");
}

/// extract_message_text 单元测试。
#[test]
fn test_extract_message_text() {
    // 纯文本
    let msg = json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi" }] });
    assert_eq!(extract_message_text(&msg).as_deref(), Some("hi"));
    // 空 content
    let msg = json!({ "role": "assistant", "content": [] });
    assert_eq!(extract_message_text(&msg), None);
    // 无 content 字段
    let msg = json!({ "role": "assistant" });
    assert_eq!(extract_message_text(&msg), None);
    // 非数组 content
    let msg = json!({ "role": "assistant", "content": "plain string" });
    assert_eq!(extract_message_text(&msg), None);
}
