# PORTING.md — pi (TypeScript → Rust) 移植映射规范

本文件是 `earendil-works/pi` monorepo（TypeScript）移植为 Rust workspace 的**跨
crate 共用映射规范**。后续每一次"这段 TS 该怎么翻译成 Rust"的判断，都应先查本
文件的映射表与陷阱表，而不是每个模块重新拍板——否则同一种 TS 模式在不同文件里
会被翻成不一致的 Rust 写法。

模式映射来自现有 Rust 代码实际采用的约定；
高危陷阱表来自 `PORTING_MISTAKES.md` 里登记过的回归 bug（每条标注对应编号，便于回溯）。

---

## 1. Crate 映射与依赖方向

| Crate             | 对应源码目录（TS）             | 职责一句话                                                |
| ----------------- | ------------------------------ | --------------------------------------------------------- |
| `pi-ai`           | `packages/ai`                  | 统一多 Provider LLM API（OpenAI/Anthropic/Google 等）     |
| `pi-agent-core`   | `packages/agent`               | Agent 运行时：状态机、工具调用循环、事件流                |
| `pi-coding-agent` | `packages/coding-agent`        | 内置工具集（read/write/edit/bash/grep/find/ls）+ 扩展系统 |
| `pi-cli`          | （coding-agent 的 CLI 入口）   | CLI 二进制入口、包管理 CLI                                |
| `pi-extension-api`| （coding-agent 的扩展 API 抽出）| 扩展侧公开 API 类型（RegisteredCommand/Tool/Shortcut…）  |
| `pi-extensions`   | （内置扩展实现）               | 随仓库分发的内置扩展                                      |
| `pi-tui`          | `packages/tui`                 | 终端 UI 渲染层——最小可用形态（Elm 架构 + ratatui 0.29 + vendored grok-build markdown 管线；组件层不复刻原版，见 DEVIATIONS.md） |

**依赖方向固定为单向，禁止反向依赖：**

```
pi-ai  ←  pi-agent-core  ←  pi-coding-agent  ←  pi-cli
                                  ↑
                          pi-extension-api
                          pi-extensions
```

`pi-ai` 不应该知道 `pi-agent-core` 的存在，以此类推。扩展 API 类型独立成
`pi-extension-api`，避免 `pi-coding-agent` 的内部实现泄露给扩展作者。

---

## 2. 模式映射表（TS → Rust）

| TS 模式                            | Rust 对应写法                                                       | 备注 / 项目实际采用位置                                    |
| ---------------------------------- | ------------------------------------------------------------------- | --------------------------------------------------------- |
| `field?: T`                        | `Option<T>`，配合 `#[serde(skip_serializing_if = "Option::is_none")]` | 见 `pi-ai/src/types.rs` 各 `Option` 字段                  |
| `type A = X \| Y`                  | `enum A { X(..), Y(..) }`                                           | 带数据的 union 用 tag                                     |
| `interface Provider { ... }`       | `trait Provider { ... }`                                            | trait-first，见 `pi-ai` Provider 抽象                     |
| `interface Foo { 字段... }`（数据）| `struct Foo { ... }` + `#[derive(Serialize, Deserialize)]`          | 数据型 interface → struct                                 |
| `?? defaultValue`                  | 显式 `match` / `unwrap_or_else(\|\| ...)`，**非** `unwrap_or(...)`  | 见陷阱表 #短路求值；`unwrap_or` 立即求值会执行不该执行的副作用 |
| `a ?? b` 无副作用                  | `unwrap_or(b)` 可用                                                 | 仅当 `b` 是纯值、无副作用时                                |
| `Promise<T>`                       | `async fn -> T` 或 `impl Future<Output = T>`                        |                                                           |
| `Promise.all([...])`               | `futures::join_all` / `tokio::join!`                                | 涉及事件顺序时见陷阱表 #async 并发顺序                    |
| `EventEmitter` / `.on(type, cb)`   | `tokio::sync::mpsc` 或 `broadcast` + `Stream`                       | 禁止用回调闭包硬翻译事件流                                |
| `agent.subscribe(handler)`         | `tokio::sync::mpsc` channel，事件枚举 `AgentSessionEvent`           | 见 `pi-agent-core` 事件流                                 |
| `throw new Error(msg)`             | `Result<T, E>` + `thiserror` 错误类型                               | 每个 crate 用 `thiserror` 定义自己的错误类型（见 §4）     |
| `try { ... } catch (e) { emitError }` | `match result { Ok(_) => ..., Err(e) => rpc_error(...) }`        | 禁止 `.ok()` 静默吞错（见陷阱表 #错误静默吞掉）           |
| `Map<K, V>`                        | `HashMap<K, V>` 或 `BTreeMap`（需有序时）                           |                                                           |
| `Set<T>`                           | `HashSet<T>`                                                        |                                                           |
| `readonly`                         | 无直接对应；用不提供 `&mut` 的 API 约束                             |                                                           |
| `as const` / 字面量联合            | `enum` + `#[serde(rename_all = "...")]`                             | 见 §3 serde 命名约定                                      |
| `Record<string, T>`                | `HashMap<String, T>` 或 `serde_json::Map<String, Value>`            | 动态键时用后者                                            |
| `any` / 动态类型                   | `serde_json::Value`，**单独标注为高风险点**                         | 见陷阱表 #动态类型                                        |
| 数值（number）                     | 视语义选 `i64`/`u64`/`f64`/`usize`，逐处确认截断意图                 | 见陷阱表 #数值截断                                        |
| `string | (TextContent \| ImageContent)[]`（多形态内容）| enum + 显式覆盖**两种**形态，不能只处理 string          | 见陷阱表 #多形态内容（PORTING_MISTAKES #68）              |

---

## 3. Serde 序列化命名约定（最高频真实陷阱）

TS 侧 JSON wire format 一律是 **camelCase**（`sourceInfo`、`firstKeptEntryId`、
`tokensBefore`、`argumentHint` …）。Rust struct 默认用 snake_case 字段名。这是
本项目**出现次数最多**的一类回归（见 PORTING_MISTAKES 多条 serde 命名不匹配）。

**强制约定：**

- 凡是要序列化给 TS 客户端 / 扩展 / RPC 消费的 struct/enum，**必须**加
  `#[serde(rename_all = "camelCase")]`（struct 字段、enum 变体）。
- TS 用 `{ type: "xxx", ... }` 的 tagged union → Rust 用
  `#[serde(tag = "type", rename_all = "camelCase")]`（注意：`rename_all` 在 enum
  上**只影响变体名/`type` 标签的值**，不影响变体内部字段名；变体内部字段仍需在
  struct 上单独加 `rename_all`，否则会以 snake_case 序列化——这是真实踩过的坑，
  见 PORTING_MISTAKES "Serde field naming mismatch" 系列）。
- TS 用 `"user" | "project" | "temporary"` 这类小写字符串联合 → Rust enum 加
  `#[serde(rename_all = "lowercase")]`。
- optional 字段加 `#[serde(skip_serializing_if = "Option::is_none")]`，匹配 TS
  `field?: T` 在 `undefined` 时不输出键的行为。
- 单字段重命名用 `#[serde(rename = "argumentHint")]`（见 `BuiltinSlashCommand`）。

**项目实际范例：** `pi-ai/src/types.rs`、`pi-coding-agent/.../slash_commands.rs`
（`SlashCommandInfo` / `BuiltinSlashCommand`）、`rpc_types.rs`。

---

## 4. 错误处理约定

- 每个 crate 用 `thiserror` 定义自己的错误类型（项目已采用：`SessionError`、
  `CompactionError`、`BranchSummaryError`、`HarnessError`、`ExecutionError`、
  `EditError` 等）。
- **禁止 `.unwrap()` / `.expect()`**（测试代码除外）；用 `Result<T, E>` 显式
  传播。workspace 已用 `[workspace.lints.clippy]` 把 `unwrap_used`/`expect_used`
  设为 `warn`，`cargo clippy -- -D warnings` 下会变 error。
- **禁止静默 fallback / `.ok()` 吞错**：TS 里 `?? defaultValue` 或 `.catch()` 掩
  盖了"本不该发生"的情况时，Rust 侧要显式判断并返回 `Err`（或视原意图 panic），
  不能默默吞掉（见陷阱表 #错误静默吞掉、PORTING_MISTAKES #26）。
- 函数参数最多 3 个，超过用 struct 传参（对应 TS 的 options object）。

---

## 5. 高危陷阱表（"长得像但行为不同"的模式）

这类问题最危险：代码能编译、不立刻报错，只在特定输入下才表现出差异。下表前 4
条是 CLAUDE.md 阶段一预设的标准陷阱；**后续条目是本项目实际踩过并修复的，已回
填进本表**，每条标注真实出现过的 `PORTING_MISTAKES.md` 编号，复核者遇到同类代
码时可提前认出来。新发现的同类模式应同时补进本表（CLAUDE.md 4.3 要求）。

| # | 模式 | TS 行为 | Rust 天真翻译的坑 | 正确做法 | 实例 |
| - | ---- | ------- | ----------------- | -------- | ---- |
| 1 | 短路默认值 | `a ?? (sideEffect())`，`a` 非 null 时 `sideEffect()` 不执行 | `a.unwrap_or(side_effect())` 的参数**立即求值**，副作用永远执行 | `unwrap_or_else(\|\| side_effect())` | — |
| 2 | 数值截断 | `Math.trunc()` 向零取整；负数/小数要确认意图是"向零"还是"向下" | `as i64` 也向零截断，但若原意图是 `floor`，负数上结果不同 | 逐个数值转换点确认原意图 | #41/#42 硬编码常量（`128_000` 代替 `contextWindow ?? 0`）属同类数值取值偏差 |
| 3 | 数组/字符串越界 | JS 越界下标返回 `undefined`，不 panic | `slice[i]` 越界 panic；TS 可能依赖越界返回 undefined 的隐式逻辑 | 显式 `.get(i)` 返回 `Option`，逐处确认是否依赖了隐式行为 | — |
| 4 | async 并发顺序 | `Promise.all([...])` 调度细节与 `tokio::join!`/`join_all` 不完全一致 | 状态机对时序敏感处（事件流顺序）直译可能改变事件到达顺序 | 涉及并发顺序的模块做"事件序列对齐"，不能只看单测通过 | #20（双写 stdout 交错破坏 JSONL）、#21（signal handler 与主循环双重持有 session）、#62（`event.message` 指向旧引用导致持久化用错对象） |
| 4b | **跨通道事件顺序**（async 并发顺序的变体） | TS 单一事件流（回调/订阅按序到达） | Rust 把相关事件拆到多个 channel，`tokio::select!` 同时就绪时**随机**挑一个——后续状态变更（如 badge 写入）可能在目标行创建之前到达而被静默丢弃，行为在两种到达顺序下不同（flaky） | 不依赖到达顺序：状态变更先缓存（`HashMap`），目标行创建事件到达时重放/合并；或把相关事件合到同一通道保证顺序 | #109（工具审批 badge 与 tool 行分处 UI/agent 两通道，先到先丢）、#106（输出永不静止暴露的主循环重绘时机） |
| 5 | **serde camelCase vs snake_case** | TS wire 一律 camelCase | Rust struct 默认 snake_case，忘加 `rename_all = "camelCase"` → 字段名全错 | 任何对外序列化的 struct/enum-内部字段都加 `rename_all`（见 §3） | PORTING_MISTAKES serde naming mismatch 系列（多条） |
| 6 | **响应格式/字段遗漏** | TS 返回完整对象，含 optional 字段、`null` vs 对象的区别 | Rust 返回简化占位（`{id, type:"entry"}` stub、总是非 null、漏 optional 字段） | 逐字段对照 TS 返回类型，optional → `Option`+`skip_serializing_if`，`T \| null` → `Option<T>` | #9, #10, #11, #12, #14, #15, #16, #17, #18, #22, #23, #24 |
| 7 | **no-op / 缺失实现** | TS 方法有真实副作用 | Rust 返回 success 但没调用对应方法（占位忘了补） | 确认每个 handler 真正调用了对应 session 方法 | #6, #7, #8 |
| 8 | **缺失校验** | TS `name.trim()` 非空等校验 | Rust 直接接受任意输入 | 补齐 TS 的输入校验，返回对应错误 | #13 |
| 9 | **缺失 enum 变体** | TS union 有 N 个成员 | Rust enum 漏掉某个 variant | 逐个核对 TS union/switch 的所有分支 | #19（`ExportHtml`）、#11/#12（`null` 分支） |
| 10 | **可选链的"未定义"与"键缺失"语义不同** | `obj?.field?.[key]`：`obj` 为 undefined 与 `obj` 存在但无该 key 是两种结果（如 `getSupportedThinkingLevels` 中 xhigh/max 只在 map 显式声明时支持） | `if let Some(map) = obj.field { ... }` 把"外层为 None"和"键缺失"合并成同一条路径，None 时误放行 | 用 `obj.field.as_ref().and_then(|m| m.get(key))` 得到 `Option<&Option<V>>`，match 三种情况（外层 None / 键→null / 键→值） | pi-ai `get_supported_thinking_levels`（无 thinkingLevelMap 时 xhigh/max 被误判为支持） |
| 10 | **错误静默吞掉** | TS `.catch()` → `emitError(id, type, msg)` | Rust `.ok()` 丢弃错误，返回 `()` 或假成功 | `match`/`?` 显式传播，失败发 `rpc_error` | #26, #（compact 失败返回 `compacted:false`） |
| 11 | **多形态内容（string \| array）** | TS `content` 既可是 `string` 也可是 `(Text\|Image)[]` | Rust 只 `as_str()` 处理 string，数组形态返回 `None` | 写 helper 覆盖两种形态，数组形态过滤 `type:"text"` 块 | #68 |
| 12 | **计数/编号边界** | TS occurrence 从 1 开始、`counts` 只统计特定集合 | Rust 把不该计入的项（如 builtins）也算进计数，编号起点偏移 | 严格复刻 TS 的计数集合与编号起点，必要时 `taken` 集合去重 | #70 |
| 13 | **缺失事件发送 / 状态变更** | TS agent loop 在某分支发事件或 mutate state | Rust 该分支只更新本地变量、漏发事件/漏改 state | 逐分支核对 TS 的 emit 与 state mutation | #39（漏发 `auto_retry_end`）、#40（漏删 retry 前的 assistant 消息）、#62 |
| 14 | **硬编码常量代替运行时值** | TS `this.model?.contextWindow ?? 0` | Rust 写死 `128_000` | 取运行时值，`?? 0` → `unwrap_or(0)` | #41, #42 |
| 15 | **路径解析/规范化缺失** | TS 解析相对路径、校验 cwd | Rust 直接用原始字符串 | 复刻 path 解析与 cwd 校验 | #（session cwd 系列 #65/#67）、相对路径条目 |
| 16 | **动态类型 `any`** | TS `any`/`Record<string, any>` | Rust 一律当 `Value` 处理可能丢失结构化信息 | 标注为高风险点，能建具体类型就建，否则 `serde_json::Value` + 显式注释 | 全项目 `serde_json::Value` 用点 |
| 17 | **数据结构 in-place 语义** | TS 对象引用被原地替换，后续读取已是新值 | Rust `Vec` 替换元素后，旧引用仍指向旧对象 | 替换后**重新读取**，不要复用旧引用 | #62 |
| 18 | **缺失前置检查 / 逻辑分支** | TS 在发送前/出错时做前置检查（context overflow 检测、pre-prompt compaction、retry 前清理） | Rust 漏掉该分支，错误直达或被忽略 | 逐个对照 TS 的 if/前置 guard，补齐对应检查与处理 | #44, #45 |
| 19 | **快照克隆被当作共享状态写入** | TS `this.session.model = x` 直接改共享对象 | 调用某个 `state()`/`getState()` 返回**克隆快照**（如 `Arc<RwLock<T>>` 的 `read().await.clone()`），在其上赋值/`push` 后丢弃——编译通过、行为静默丢失 | 写状态必须用真正的 setter 或 `update_state(&mut)` 写锁；任何 `let mut state = x.state().await` 后出现赋值/push 都要怀疑是克隆丢写 | #75（`set_model`/`set_thinking_level`/`set_active_tools_by_name`/`send_custom_message`/`_flush_pending_bash_messages`/`cycle_model`/`record_bash_result` 共 7 处） |
| 20 | **执行时上下文 vs 构造期快照** | TS `ExtensionRunner.createContext()` 每次 dispatch 新建 context，`ui`/`mode` 等字段是 getter，调用时实时读 runner 上可变的绑定（`bindExtensions({ uiContext })` 换绑后全部路径立即生效） | Rust 在构造期把整个上下文（含 ui/runtime）快照进长期存活的闭包 `Arc`，宿主事后换绑（如 TUI `set_extension_ui_context`）只改了另一个 context——编译通过，行为静默走旧默认值（如 `eprintln!("[pi] ...")` 打进 TUI 屏幕） | 需要"事后换绑"的字段用实时读取的委托/绑定（`RwLock` + delegating context，等价 TS getter），不要把可变绑定快照进长期存活闭包 | #93（bash session 环境变量）、#119（TUI 换绑 UI 后扩展工具 notify 仍走默认 eprintln） |
| 23 | **clone 语义不一致（部分共享、部分深拷贝）** | TS 单实例对象，`this.runtime.registerProvider(...)` 与后续 `session.model` 读的是同一份状态 | Rust 结构体手写 `Clone` 时一半字段 `Arc::clone`（共享）、一半 `RwLock::new(read().clone())`（深拷贝）：写入落到副本上，编译通过、测试若只用同实例也通过，但生产路径（扩展钩子拿到 clone、session 持有另一 clone）**静默不生效** | `Clone` 要么整体共享（可变状态一律 `Arc<RwLock<..>>`/`Arc<..>`），要么整体深拷贝并确保写入者就是持有者；引入新 clone 点时对照 `runtime.registerProvider` → session 可见性写回归测试 | extension `registerProvider` 的 `models` 对 session 不可见（PORTING_MISTAKES `register_provider` 条） |
| 24 | **多参数 API 折叠为单 payload 后参数错位** | TS `pi.registerProvider(providerId, config)` 两参：第一个是 provider id，`config.name` 是显示名 | 移植成单 `Value` payload 后沿用旧的“从 `config.name` 取 provider id”写法：provider 注册到错误的名字下（或直接丢失），且两参语义再难还原 | 定义显式 payload 结构（如 `{ providerId, config }`）并在文档注释里写清字段来源；不要复用 config 内部的同名字段充当外部参数 | extension `registerProvider` 把 `config.name` 当 provider id（同上条同一处修复） |

| 21 | **应用级快捷键遮蔽编辑器键位** | TS 应用级 handler 只拦截明确注册在 app 层的键（如 Ctrl+X/Ctrl+C），其余 ctrl/alt 字母落给编辑器（`tui.editor.*` 绑定：ctrl+b=left、ctrl+f=right、ctrl+a/e=行首尾…） | Rust 在 interactive 层把某个 ctrl 字母无条件映射成应用动作（如 `Ctrl+B → AbortBash`），该键永远到不了编辑器——两个方向表现不一致（Ctrl+F 能用、Ctrl+B 被吞），且原版根本没有这个应用级绑定 | 逐一核对应用级 handler 的键位是否在原版 app 层注册过；编辑器能处理的 ctrl/alt 字母不要在上层拦截 | `Ctrl+B → AbortBash`（应为编辑器 `cursorLeft`；bash 中止 = Esc），见 PORTING_MISTAKES（interactive.rs） |

| 22 | **前缀键未命中未回填** | TS 无 vim 前缀键；编辑器里普通字符一律插入 | Rust 自行加多键序列（`g`→`gg`），第一个键只置"待定"状态、不落字，后续键不匹配时直接处理后续键——序列未完成的那个字符被永久吞掉（`go` → `o`），且原版根本没有这个绑定 | 多键前缀必须缓存未完成的按键，下一键不匹配时先把它作为字面输入回填，再处理下一键；更优先的是先核对原版是否真有该键位 | `gg`/`G` 转录滚动（应为 Home/End，plain g/G 插入），见 PORTING_MISTAKES（pi-tui app.rs） |

> 复核重点（CLAUDE.md）：是否引入了上表模式、生命周期/所有权是否合理、错误路径
> 是否正确传播、状态机事件顺序是否与原版一致。

---

## 6. 不逐行复刻的部分（与 CLAUDE.md / DEVIATIONS.md 一致）

- **TUI 渲染层**（`pi-tui` / `packages/tui`）：组件层不逐行复刻原版
  （Ratatui + vendored `xai-grok-markdown` 管线替代原版自研组件库，见
  DEVIATIONS.md 与 THIRD-PARTY-NOTICES.md）；**状态机语义**（有哪些状态、
  转移条件）与原版一致；原版各 selector 组件（约 17k 行 TS）待后续任务。
- **扩展系统内部实现**允许偏离原版（DEVIATIONS.md 顶部已确认偏差），但**对外
  interface 与函数行为**必须与原 TS 版本一致。
- **Node.js 特定运行时行为**（process/fs API 细节）按 Rust 生态惯用法重写，不
  追求 API 名一致，但在代码注释里注明"有意偏离原版，原因 XXX"。

阶段四对齐检查遇到这三类差异时按**已确认偏差**处理，不要在"对齐"名义下悄悄改
回去（CLAUDE.md 4.2：先查 DEVIATIONS.md，状态"已确认保留"的禁止修改）。

---

## 7. 与其他文档的关系

- `DEVIATIONS.md`（根 + 各 crate）：登记有意偏差，"已确认保留"的不可改回。
- `PORTING_MISTAKES.md`（各 crate）：每修一个真回归 bug 补一条；根因模式归到本
  文件 §5 陷阱表，新模式同步回填本表。
- `CONTRACT_ALIGNMENT.md`（各 crate）：公开 API 行为对照表，"是否一致=否"必须引
  用 `DEVIATIONS.md` 编号。
- 三个阶段工作流、模块合并检查清单见 `CLAUDE.md`，本文件不重复。

---

## 8. app-owned 文本选择子系统移植映射（TuiAltScreen 选择 → pi-tui）

原版位置：`packages/tui/src/tui-alt-screen.ts`（选择相关代码约 400 行 +
`test/tui-alt-screen.test.ts` 20 余条用例）。pi-rs 侧落在
`crates/pi-tui/src/selection.rs`（纯逻辑）+ `app.rs`（鼠标路由/高亮/命令）。

### 8.1 模块与职责

| 原版符号 | Rust 位置 | 职责 |
| -------- | --------- | ---- |
| `SelectionPoint` / `SelectionRange` | `selection::{SelPoint, SelSpace}` | 选择端点；`SelSpace` 区分转录内容行（Content）与屏幕行（Screen） |
| `SelectionGranularity` | `selection::Granularity` | `character` / `word` / `line` |
| `TERMINAL_WORD_SELECTION_JOINERS` | `selection::JOINERS` | `/`、`-` 连词 |
| `getWordSelection` / `getLineSelection` | `selection::{word_range, line_range}` | 双击/三击范围（UAX #29 word bounds + joiner 合并） |
| `getSelectionColumns` / `getActiveSelectionText` | `selection::{columns_for_row, extract_text}` | grapheme 列吸附 + 逐行裁剪、`trimEnd`、`\n` 连接 |
| `applySelectionHighlight` / `applySelection` | `app::apply_selection_highlight` | 选中单元格加 `REVERSED`（原版 `\x1b[7m`，每个 `m` 序列后重发） |
| `handleSelectionMouseEvent` | `app::handle_selection_mouse` | press/drag/release 状态机 |
| `updateSelectionAutoScroll` / `autoScrollSelection` | `app::selection_auto_scroll` | 拖到视口边缘后按 tick 滚动并延伸焦点 |
| `copyOnSelect` / `copySelection` | `Model::copy_on_select` + `Cmd::CopySelection` | 松手复制；宿主执行 system clipboard + 提示 |
| `hasActiveSelection` / `copyActiveSelectionToClipboard` | `Model::selection_text` / `Cmd::CopySelection` | `Ctrl+X` preferSelection 路径 |

### 8.2 核心数据结构映射

- 原版 `scrollView` 字段（选择点属于哪个滚动视图）→ `SelSpace`：
  `Content`（转录，行号是**内容行**，需按 `body_scroll` 投影到屏幕）与
  `Screen`（docked 区域，行号即屏幕行）。`getSelectionBounds` 中
  `anchor.scrollView !== focus.scrollView` 的判定 → `space` 不同即返回 `None`
  （跨区选择不成立）。
- 原版每帧从 `previousScreen` / `scrollContentLines` 取源文本；Rust 在 press
  时把源文本快照进选择状态（`source_lines`），drag 期间复用——等价于原版
  用"上一帧"文本的语义。
- 原版反转视频作用于渲染后的 ANSI 行；Rust 作用于 ratatui `Buffer` 单元格，
  对宽字符起始单元格设置 modifier（延续单元格 symbol 为空，不受影响）。

### 8.3 高危陷阱标注

| 模式 | 原版行为 | Rust 天真翻译的坑 | 正确做法 |
| ---- | -------- | ----------------- | -------- |
| 列越界 | JS `sliceByColumn` 越界返回空串 | `line[i]` 越界 panic | 全部经 `columns_for_row` 裁剪到 `[min,max]` |
| 宽字符/组合字 | `getGraphemeCellRange` 把端点吸附到 grapheme 边界 | 按 `char` 而非 grapheme 切列，CJK/emoji/组合音标被切半 | 用 `unicode-segmentation` grapheme + `unicode-width` 累计单元格 |
| `trimEnd` | 每行复制前 `trimEnd`，行尾空格不进剪贴板 | 直接整行拼接带出 padding | `extract_text` 逐行 `trim_end` |
| 复制排序 | 松手时先 `stopSelectionAutoScroll` 再复制 | 先复制再停 autoscroll，多滚一帧、内容与高亮不一致 | 顺序：停 autoscroll → 更新 focus → 复制 |
| 焦点丢失 | `FOCUS_OUT` 只在 `selectionPressActive` 时清空 | 无脑清空，导致已完成的选择被误清 | 仅 `press_active` 时清 anchor/focus |
| 非滚动区选择 | 屏幕点以 `previousScreen` 为源 | 用转录内容行当屏幕行，行号错位 | `SelSpace` 区分两个坐标空间 |

### 8.4 对外接口（新增）

- `terminal::InputEvent::{Mouse(MouseEvent), FocusLost}`：press/drag/release/
  move 与焦点丢失（`EnableFocusChange` = `?1004h`，对齐原版
  `ENABLE_ALL_MOTION_MOUSE` 的 1004h）。
- `app::Msg::{Mouse(MouseEvent), FocusLost, SetCopyOnSelect(bool)}`。
- `app::Cmd::CopySelection(String)`：宿主写系统剪贴板后回显成功/失败（原版
  `flash("Copied!")`，Rust 无 flash 概念，沿用 system 消息，见 DEVIATIONS）。
- `Model::{selection_text, has_active_selection, set_copy_on_select,
  selection_auto_scroll_active, selection_auto_scroll}`。
- 设置项 `fullscreenCopyOnSelect`（默认 `true`）：
  `SettingsManager::{get,set}_fullscreen_copy_on_select` +
  `AgentSession::{get,set}_fullscreen_copy_on_select` +
  interactive 启动时写入模型。
