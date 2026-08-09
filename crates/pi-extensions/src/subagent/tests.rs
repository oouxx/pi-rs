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
    // 后台 run 目录用临时目录（noop 的 get_agent_dir 返回空串）。
    // Box::leak 保持目录存活到测试结束（进程退出时系统清理）。
    let agent_dir = tempfile::tempdir().unwrap();
    let agent_dir_owned = agent_dir.path().to_string_lossy().to_string();
    Box::leak(Box::new(agent_dir));
    handle.get_agent_dir = Arc::new(move || agent_dir_owned.clone());
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

/// tools 白名单：假脚本把收到的参数输出到 message_end，验证 --tools 传递。
#[tokio::test]
async fn test_subagent_passes_tools_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(
        dir.path(),
        r#"#!/bin/sh
echo "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ARGS: $@\"}]}}"
echo '{"type":"end"}'
"#,
    );
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call(
            "subagent",
            json!({ "task": "t", "tools": ["read", "bash"] }),
            &ctx,
        )
        .await
        .expect("handled");
    assert!(!out.is_error, "got: {out:?}");
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("--tools read,bash"), "got: {text}");
}

/// tools 白名单（字符串形式）：逗号分隔。
#[tokio::test]
async fn test_subagent_passes_tools_string() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(
        dir.path(),
        r#"#!/bin/sh
echo "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ARGS: $@\"}]}}"
echo '{"type":"end"}'
"#,
    );
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call(
            "subagent",
            json!({ "task": "t", "tools": "read, bash" }),
            &ctx,
        )
        .await
        .expect("handled");
    assert!(!out.is_error, "got: {out:?}");
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("--tools read,bash"), "got: {text}");
}

/// 无 tools 参数：不传 --tools。
#[tokio::test]
async fn test_subagent_no_tools_flag_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(
        dir.path(),
        r#"#!/bin/sh
echo "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ARGS: $@\"}]}}"
echo '{"type":"end"}'
"#,
    );
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call("subagent", json!({ "task": "t" }), &ctx)
        .await
        .expect("handled");
    assert!(!out.is_error, "got: {out:?}");
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(!text.contains("--tools"), "should not pass --tools, got: {text}");
}

/// 后台运行：async:true 返回 run_id，子进程完成后 status 查询返回 done + 输出。
#[tokio::test]
async fn test_subagent_async_background() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_pi(
        dir.path(),
        r#"#!/bin/sh
sleep 1
echo '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"background done"}]}}'
echo '{"type":"end"}'
"#,
    );
    let ext = SubagentExtension::new().with_pi_binary(&fake);
    let ctx = test_ctx("/tmp", None);

    // 启动后台 run
    let out = ext
        .handle_tool_call("subagent", json!({ "task": "t", "async": true }), &ctx)
        .await
        .expect("handled");
    assert!(!out.is_error, "got: {out:?}");
    let run_id = out.details
        .as_ref()
        .and_then(|d| d.get("runId"))
        .and_then(|v| v.as_str())
        .expect("runId in details")
        .to_string();
    assert!(!run_id.is_empty(), "runId should not be empty");

    // 轮询等待后台 task 完成（全量测试并发时 CPU 竞争，固定 sleep 不可靠）
    let mut done = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let out = ext
            .handle_tool_call(
                "subagent",
                json!({ "action": "status", "runId": run_id }),
                &ctx,
            )
            .await
            .expect("handled");
        let text = out.content[0]["text"].as_str().unwrap_or("");
        if text.contains("\"status\": \"done\"") {
            assert!(text.contains("background done"), "got: {text}");
            done = true;
            break;
        }
    }
    assert!(done, "background run did not complete within 10s");
}

/// 查询不存在的 run：报错。
#[tokio::test]
async fn test_subagent_async_status_not_found() {
    let ext = SubagentExtension::new();
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call(
            "subagent",
            json!({ "action": "status", "runId": "nonexistent" }),
            &ctx,
        )
        .await
        .expect("handled");
    assert!(out.is_error, "should error, got: {out:?}");
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("not found"), "got: {text}");
}

/// 后台 spawn 失败：status.json 记录 error。
#[tokio::test]
async fn test_subagent_async_spawn_failure() {
    let ext = SubagentExtension::new().with_pi_binary("/nonexistent/pi-binary");
    let ctx = test_ctx("/tmp", None);
    let out = ext
        .handle_tool_call("subagent", json!({ "task": "t", "async": true }), &ctx)
        .await
        .expect("handled");
    assert!(out.is_error, "should error, got: {out:?}");
    let text = out.content[0]["text"].as_str().unwrap_or("");
    assert!(text.contains("failed to spawn"), "got: {text}");
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
