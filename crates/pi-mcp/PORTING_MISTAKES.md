# pi-mcp PORTING_MISTAKES

| 位置 | 现象 | 根因模式 | 修复方式 |
| ---- | ---- | -------- | -------- |
| `transports/stdio.rs` | 宿主进程退出时未关闭的 stdio MCP 服务器（含 npx/uvx 孙进程）会继续存活；v1.5.0 用 `liveProcessGroups` + `process.once("exit")` 清理 | 缺少进程生命周期退出钩子 | unix 下用 `OnceLock<Mutex<HashSet<u32>>>` 记录进程组，`libc::atexit` 注册 SIGTERM 清理；spawn 时注册、`close()` 时注销 |
