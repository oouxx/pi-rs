# CONTRACT_ALIGNMENT.md — pi-coding-agent

## ModelRegistry

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| `new_with_models_path` | 不存在（测试专用构造函数） | `#[cfg(test)] pub fn new_with_models_path(builtin_models, models_path)` — 测试专用，避免依赖环境变量 | 是（有意偏差） | 见 DEVIATIONS.md #N/A — 测试辅助方法，非公开 API |
