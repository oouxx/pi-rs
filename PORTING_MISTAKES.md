# PORTING_MISTAKES.md — 移植错误归档

对齐检查中修复的"回归 bug"（不是有意保留的偏差）记录于此。根因模式
尽量归到 `PORTING.md` 阶段一"高危陷阱"表已有的分类；新出现的模式
同时补回 `PORTING.md` 的陷阱表。

| 位置 | 现象 | 根因模式 | 修复方式 |
| ---- | ---- | -------- | -------- |
| `pi-coding-agent/src/core/extensions/loader.rs` `discover_extensions_in_dir` | `pi install` 装完的扩展（symlink 到 `agent_dir/extensions/`）在会话启动时不被发现，扩展工具不可用 | **隐式行为依赖**：TS 用 `entry.isDirectory() \|\| entry.isSymbolicLink()` 把 symlink 按目标类型分类；Rust 的 `entry.file_type()` 对 symlink 返回 link 自身（`is_dir=false, is_file=false`），原代码的 `.or_else(metadata)` 兜底只在 `file_type()` 失败时触发，symlink 场景永远不触发，导致 symlink 目录/文件被静默跳过 | 当 `file_type()` 报告 `is_symlink()` 时显式用 `std::fs::metadata()` 跟随目标分类；补 3 个测试（symlink 目录、symlink 文件、悬空 symlink） |
| `pi-coding-agent/src/core/extensions/js_runtime.rs` `TypescriptModuleLoader::resolve` | 真实 npm 扩展（如 `@narumitw/pi-goal`）用 `.js` 后缀 import（`import ... from "./goal.js"`，TS ESM 标准写法），文件在磁盘上是 `.ts`，加载报 "No such file or directory" | **隐式行为依赖**：TS 原版用 jiti 加载，jiti 自动做 `.js`→`.ts` 扩展名替换；Rust 的 `resolve_import` 只做字面解析，不尝试替换 | 在 `resolve` 步骤加 jiti 式扩展名回退：`.js`→`.ts`/`.tsx`/`.jsx`、`.mjs`→`.mts`、`.cjs`→`.cts`、目录→`index.*`；补测试 |
| `pi-coding-agent/src/core/package_manager.rs` `install` | `pi install foo`（裸名）被直接传给 `npm install`，而 TS 原版把裸名当本地路径（不存在则报 "Path does not exist"） | **缺失前置检查**：TS `install()` 先 `parseSource` 再按类型分发（npm/git/local），local 分支做 `existsSync` 校验；Rust 原实现不解析直接跑 npm | `install` 改为 `parse_source` 分发：npm→npm install（spec 去掉 `npm:` 前缀）、git→git clone、local→resolve+exists 校验；CLI 改调 `install_and_persist`（TS `installAndPersist`），默认 scope 从 project 改为 user（TS `local=false`） |
