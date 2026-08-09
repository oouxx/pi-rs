// 手动调试工具：加载真实第三方扩展（pi-plan-mode）并触发 session_start，
// 验证 Bun 子进程扩展运行时端到端可用。依赖本机已安装该扩展，非 CI 测试。
#![allow(clippy::unwrap_used, clippy::expect_used)]
use pi_coding_agent::core::extensions::bun::load_bun_extensions;

#[tokio::test]
async fn dbg_plan() {
    let agent_dir = tempfile::tempdir().unwrap();
    let paths = vec!["/opt/homebrew/lib/node_modules/@narumitw/pi-plan-mode/src/index.ts".to_string()];
    let mut flags = std::collections::HashMap::new();
    flags.insert("plan".to_string(), "true".to_string());
    let loaded = load_bun_extensions(&paths, "/tmp", &agent_dir.path().to_string_lossy(), &flags)
        .await
        .expect("load")
        .expect("some");
    // 直接 fire_event 看 handler 是否完整跑通（无 bind_actions，快照为 null，
    // 扩展应优雅降级：getBranch 返回 []、getActiveTools 返回 []）。
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        loaded.runner.fire_event("session_start", serde_json::json!({"reason": "new"})),
    )
    .await;
    match r {
        Ok(Ok(v)) => eprintln!("fire_event result: {v}"),
        Ok(Err(e)) => eprintln!("fire_event error: {e}"),
        Err(_) => eprintln!("TIMEOUT"),
    }
}
