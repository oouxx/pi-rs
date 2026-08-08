# 运行时扩展加载 — 可行性评估

> 阶段一（架构分析）产出。本文档只做方案评估，不改代码。
> 对应 TS 源：`packages/coding-agent/src/core/extensions/{loader,runner,types,index}.ts`
> 对应 Rust 现状：`crates/pi-extension-api/src/{lib,hook}.rs`、
> `crates/pi-coding-agent/src/core/extensions/{api,dispatcher,mod}.rs`、
> `crates/pi-extensions/src/{lib,goal}.rs`、`crates/pi-cli/src/{run,args,package_manager_cli}.rs`

## 1. TS 侧运行时加载契约（要复刻的是什么）

`loader.ts` 的完整行为链：

1. **模块动态导入**：用 `jiti.import(extensionPath, { default: true })` 在运行时
   加载任意 `.ts`/`.js` 文件，取 default export 作为 `ExtensionFactory`
   （一个 `(api: ExtensionAPI) => Promise<void>` 的函数）。Bun 二进制模式下
   用 `virtualModules` 注入打包好的 SDK 包；Node/dev 模式下用 `alias` 解析。
2. **ExtensionAPI 构造**：`createExtensionAPI(extension, runtime, cwd, eventBus)`
   产出对象，暴露：
   - 注册类：`on(event, handler)`、`registerTool`、`registerCommand`、
     `registerShortcut`、`registerFlag`、`registerMessageRenderer`、
     `registerEntryRenderer`、`registerProvider`、`unregisterProvider`
   - 动作类（委托给共享 `runtime`）：`sendMessage`、`sendUserMessage`、
     `appendEntry`、`setSessionName`、`setLabel`、`exec`、`getActiveTools`、
     `setActiveTools`、`getCommands`、`setModel`、`setThinkingLevel`、
     `getFlag`、`getSessionName` ...
   - `events`：`EventBus`（`on/emit/off`）
3. **两阶段生命周期**：`createExtensionRuntime()` 产出一个"动作方法全是
   抛异常占位"的 runtime，允许加载阶段只做注册（`registerProvider` 进
   `pendingProviderRegistrations` 队列）；`runner.bindCore()` 之后才把
   占位替换成真实实现并 flush 队列。还带 `assertActive/invalidate`
   防止扩展拿到 stale ctx（`newSession/fork/switchSession/reload` 后）。
4. **清单发现**：`readPiManifest` 读 `package.json` 的 `pkg.pi.extensions`
   数组；`isExtensionFile` 判断 `.ts/.js`；`loadExtensionsFromDirs` 扫描
   `agent_dir/extensions` 等目录。
5. **缓存**：`extensionCache` 以 path 为键，cwd 变化或
   `clearExtensionCache()` 时失效（generation 计数）。
6. **sourceInfo 传播**：`createSyntheticSourceInfo(extensionPath, {source, baseDir})`
   贯穿到 `RegisteredCommand/RegisteredTool` 的 `sourceInfo` 字段。
7. **虚拟模块**：typebox、pi-agent-core、pi-tui、pi-ai(/compat)、
   pi-coding-agent —— 让扩展能 `import` 宿主 SDK。

## 2. Rust 侧现状（差距定位）

| 维度 | TS | Rust | 差距性质 |
|------|----|------|---------|
| 扩展单元 | 运行时 `.ts/.js` 文件 + factory | 编译期 `HookHandler` trait impl | **范式差** |
| 注册时机 | factory 调 `api.on/registerX` | `reg.register(Box::new(...))` 硬编码 | **范式差** |
| `extension_paths`/`--extensions` | live，传给 `loadExtensions` | 死参数（`run.rs` 解析后未用） | 回归 gap |
| `pi install` → 加载 | 装 + symlink + 扫描加载 | 只装 + symlink，不加载 | 回归 gap |
| `ExtensionRuntime` 两阶段 | 占位 → bindCore | 无对应（编译期已绑定） | 范式差 |
| `EventBus` / `api.events` | 完整 | `EventPublisher` 较弱 | 见对比分析 |
| `sourceInfo` | command/tool/shortcut 都带 | `RegisteredCommand/RegisteredTool` 缺 | 见对比分析 |
| virtual modules | 打包 SDK 注入 | 无 | 范式差 |
| `registerProvider` 运行时注册 | 是 | `ModelRegistry::register_provider` 在 host 侧 | 接口未暴露给扩展 |

**根本 gap：没有 `jiti.import()` 等价物** —— 无法在运行时把任意源码文件
变成一个能回调进 Rust 注册表的对象。这不是补一个 `load_extensions()`
函数能解决的，必须先选定**宿主运行时模型**。

## 3. 候选方案

### 方案 A：嵌入 JS/TS 运行时（deno_core / rquickjs / boa）

在 Rust 进程内嵌一个 JS 引擎，**原样跑现有的 TS 扩展**。

- **引擎选型**：
  - `deno_core`（V8 isolate）：原生 TS 支持（无需 jiti）、异步桥成熟
    （`v8::Promise` ↔ tokio Future）、性能好。代价：V8 ~30MB 体积、
    unsafe FFI、构建依赖重。
  - `rquickjs`（QuickJS binding）：体积小、可嵌入、支持 ES2020。
    代价：无原生 TS，需要先 transpile（`swc`/`esbuild` 快照，或在
    `pi install` 时预编译成 JS 落盘）；异步桥要自己写。
  - `boa_engine`（纯 Rust）：无 unsafe、无 C 依赖、最易打包。
    代价：性能最差、TS 不支持、ES 覆盖不全，跑真实扩展大概率踩兼容坑。
- **桥要实现的东西**：把 `ExtensionAPI` 的每个方法暴露成 JS 可调用
  的 host function（`register_tool/register_command/on/.../sendMessage/
  exec/...`），数据用 `serde_json` 双向 marshal；`RuntimeHandle` 的
  50+ 个 `Arc<dyn Fn>` 闭包变成 JS→Rust 的回调宿主函数；`EventBus`
  的 `emit` 触发 JS 侧注册的 handler（要维护 JS handler 表）。
- **virtual modules**：把 `@earendil-works/pi-*`/`typebox` 实现成
  JS shim，内部全部转成对 Rust host function 的调用。
- **两阶段生命周期**：Rust 侧维护一个 `PendingRegistrations` 队列，
  加载阶段只入队，`bind_core` 后 flush（对应 TS `pendingProviderRegistrations`）。
- **优点**：**唯一能原样跑现有 TS 扩展生态**的方案；`loader.ts` 契约
  可逐条对齐；`extension_paths`/`--extensions`/`pi install` 端到端打通。
- **缺点**：依赖重；FFI 桥 + 异步桥工作量大（实质是重写
  `ExtensionRunner.bindCore()` + `createExtensionAPI` 的 Rust↔JS 版）；
  安全沙箱 = 与 TS 版同等（扩展跑任意代码，需信任模型）。
- **工作量**：高。最现实的是 `deno_core`（省掉 TS transpile）。

### 方案 B：WASM 扩展（wasmtime + Component Model）

扩展编译成 WASM Component，Rust 用 `wasmtime` 加载。

- 用 WIT 定义扩展接口（`register-tool`、`register-command`、各 hook、
  host calls）；TS 扩展经 `jco componentize` 或 `wit-bindgen` 生成绑定。
- **优点**：可移植、沙箱化、语言无关（Rust/Python 扩展也能写）。
- **缺点**：TS→WASM Component 工具链不成熟；现有扩展的
  `import "@earendil-works/pi-*"` 必须改写成 WIT host import，**不能
  原样跑**；async/streaming 在 WASM 里别扭。
- **工作量**：非常高；生态未就绪。**当前不建议**。

### 方案 C：原生动态库（libloading + abi_stable/stabby）

扩展是编译好的 Rust `cdylib`，导出
`#[no_mangle] extern "C" fn pi_extension_create(api) -> Box<dyn HookHandler>`。

- 用 `abi_stable` 或 `stabby` 保持 trait ABI 跨版本稳定；host 用
  `libloading::Library::new(path)` 加载。
- **优点**：无脚本运行时开销、全 Rust 类型安全、性能最高、**和现有
  `HookHandler` trait 天然契合**；`extension_paths` 直接变成"每个 path
  当 cdylib 加载"。
- **缺点**：**扩展必须用 Rust 写**（TS 生态跑不了）；每个扩展必须对
  同一 `pi-extension-api` ABI 版本编译（跨 pi 版本脆弱，需要
  `abi_stable` 严格纪律）；平台特定二进制（无跨架构）；无沙箱。
- **工作量**：loader 部分中低；但这是**全新生态**，不是 TS 扩展的移植。

### 方案 D：声明式清单 + 轻量脚本（rhai）

扩展 = TOML/JSON manifest（声明 tool/command/flag）+ 可选 rhai 脚本
（hook 逻辑）。复杂 hook 也可 shell-out 到外部进程。

- **优点**：轻量、`rhai` 纯 Rust 可沙箱、serde 集成好、无 FFI；
  manifest 驱动发现天然对应 `readPiManifest`。
- **缺点**：现有 TS 扩展跑不了（要用 rhai/manifest 重写）；rhai 表达力
  不足以覆盖完整 TS API（EventBus、renderer 等）；只适合简单
  tool/command/flag 类扩展。
- **工作量**：中。适合作为"简单扩展子集"的渐进方案。

### 方案 E：维持编译期注册，把"无运行时加载"记为永久偏差

保留 `HookHandler` 编译期注册；把 `extension_paths`/`--extensions`/
`enable_extensions` 标注为永久无效（或移除）；内置扩展编译进二进制。

- **优点**：零新增风险、贴合现有架构、无运行时/沙箱问题。
- **缺点**：用户无法运行时装第三方扩展；`pi install`→load 链路断
  （装能装，加载没人接）；生态割裂。
- **工作量**：零（只需在 `DEVIATIONS.md` 补登记）。

## 4. 对比矩阵

| 方案 | 跑现有 TS 扩展 | 契约对齐度 | 依赖/体积 | 沙箱 | 工作量 | 生态连续性 |
|------|:-:|:-:|:-:|:-:|:-:|:-:|
| A deno_core | ✅ 原样 | 高（逐条对齐 loader.ts） | 重（V8 ~30MB） | 同 TS | 高 | 强 |
| A rquickjs | ✅ 需 transpile | 高 | 中 | 同 TS | 高 | 强 |
| A boa | ⚠️ 兼容差 | 高 | 轻 | 同 TS | 高 | 弱 |
| B wasm | ❌ 需重写 | 中（新 ABI） | 中 | 强 | 非常高 | 中（新生态） |
| C cdylib | ❌ 必须 Rust | 高（贴合 HookHandler） | 轻 | 无 | 中低 | 弱（新生态） |
| D rhai | ❌ 需重写 | 低（子集） | 轻 | 强 | 中 | 弱（子集） |
| E 维持现状 | ❌ | — | 无 | — | 零 | 无 |

## 5. 推荐路径

决策取决于**项目目标**，分三种：

1. **目标 = 兼容现有 TS 扩展生态**（`pi install` 端到端可用、社区扩展
   能直接跑）→ **方案 A，引擎选 `deno_core`**。这是唯一能逐条对齐
   `loader.ts` 契约、且省掉 TS transpile 管线的选项。`rquickjs` 作为
   体积敏感时的备选，但要补 transpile 步骤（建议在 `pi install` 时
   用 `esbuild` 预编译落盘，运行时只跑 JS）。

2. **目标 = Rust 原生扩展生态**（不在乎跑 TS 扩展）→ **方案 C**
   （cdylib + `abi_stable`）最贴合现有 `HookHandler` trait，架构改动
   最小；若要沙箱/轻量，**方案 D**（rhai manifest）覆盖简单扩展子集。

3. **目标 = 现在就收口、不扩范围**→ **方案 E**：在 `DEVIATIONS.md`
   补一条"运行时加载器永久不实现，`extension_paths`/`--extensions`
   为永久死参数"，后续按需再启动方案 A。

**我的建议**：分阶段走 **E（现在）→ A（后续按需）**。
理由：(a) 当前阶段四的任务是收尾对齐，不应引入 V8 这类大依赖；
(b) 方案 A 是唯一保留 `loader.ts` 文档化契约和 `pi install` 工作流
端到端的方案，生态价值最高，值得作为"未来真要做运行时加载"时的
首选；(c) 方案 B 在 WASM Component/TS 工具链成熟前不现实；
(d) 方案 C/D 是新生态，与"复刻 pi"的目标偏离。

## 6. 若后续启动方案 A，必须闭合的子项清单

（即无论选哪个运行时模型，下面这些 gap 都要补，源自 TS 侧契约）

> 状态标记（2026-08-08 更新）：[x] 已实现；[~] 部分实现/有已知限制；[ ] 未实现

- [x] `ExtensionFactory` default-export 调用入口
- [x] `createExtensionAPI` 桥：`on` + 7 个 register + N 个 action + `events`
      （action 已通过 `ExtensionActionBus` 接通：读类读共享快照，写类在 turn 边界
      drain；`exec` 未接，见下文）
- [x] 事件 `ctx` 参数：`(event, ctx)` 对齐 TS —— `__pi.createCtx()` 提供完整
      `ExtensionContext` 表面（ui/mode/hasUI/cwd/isIdle/...），事件 handler 与
      tool execute 均传入
- [x] HookHandler 全事件面：adapter 补齐 resources_discover/project_trust/
      user_bash/context(+mut)/tool_result_mut/before_agent_start/input/
      before_provider_request/headers/after_provider_response/
      before_session_switch/fork/compact/tree/session_info_changed/
      thinking_level_select/session_tree 等全部钩子（result-bearing 钩子解析
      JS 返回值并应用）
- [x] `ExtensionRuntime` 两阶段（占位 → bind_core，含
      `pendingProviderRegistrations` 队列与 flush）
- [x] `assertActive/invalidate` 防止 stale ctx（newSession/fork/switch/reload 后）
      —— 机制已实现（`RuntimePhase::Invalidated` + `JsExtensionManager::invalidate()`），
      session 切换接线点预留为 `AgentSessionRuntime::set_before_session_invalidate`
      （当前真实模式不用 AgentSessionRuntime，接线留作后续）
- [x] 清单发现：`package.json` 的 `pi.extensions`、`.ts/.js` 判定、
      `agent_dir/extensions` 扫描
- [x] cwd 失效缓存 + `clearExtensionCache()`（generation 计数）
- [x] `sourceInfo` 传播到 `RegisteredCommand/RegisteredTool/RegisteredShortcut`
- [x] virtual modules：typebox + pi-agent-core + pi-tui + pi-ai(/compat) +
      pi-coding-agent 的 JS shim
- [x] `registerProvider`/`unregisterProvider` 在扩展 API 表面暴露并
      接到 `ModelRegistry::register_provider`
- [x] `EventBus`（`on/emit/off`）补齐，对接现有 `EventPublisher`
- [x] `extension_paths`/`--extensions`/`enable_extensions` 从死参数转 live
- [~] `pi install` 装完后触发加载（闭环 package_manager → loader）——下次会话
      启动时自动发现；无 TS 的运行时 /reload 命令

已知剩余限制（js-runtime 侧）：
- `exec` 已实现：独立 exec worker 线程（自带 tokio runtime，不依赖 session
  owner / V8 线程）+ 同步 op 阻塞等回复，turn 内调用不死锁；支持 timeout/cwd
  选项，返回 TS ExecResult 形状
- 写类 action 在 turn 边界生效（非立即）；`setModel` 乐观返回，模型在下次
  drain 应用
- CLI 扩展 flag → 扩展 `getFlag` 未接线（CLI 只收集 unknown_flags）；扩展自身
  注册的默认值经 JS map 可读
- `registerMarkdownTransformer` 已占位（no-op，TUI 未移植）
- session 切换 → `invalidate()` 的接线待做（机制已就绪，见上）

## 7. 与已有偏差记录的关系

- 当前 `DEVIATIONS.md` #2/#4（无 rebind）、#5（get_commands 不查扩展
  command）、#6（RPC 模式扩展禁用）均为**已确认保留**。若采纳方案 E，
  需新增一条"运行时加载器永久不实现"的已确认保留偏差，并把
  `extension_paths`/`--extensions` 明确标注为其后果。
- 若未来采纳方案 A，#5/#6 中"扩展禁用/不可查"的部分将随之闭合，
  届时需把对应行从"已确认保留"改为"已实现"并走阶段三契约对齐。

## 8. SDK shim 实测评估（基于真实 import 扫描）

> 数据源：`packages/coding-agent/examples/extensions/` 下全部 68 个示例扩展
> 的真实 `import` 语句。把第 5 节"估"的 SDK shim 行数换成"测"。

### 8.1 import 面按运行时成本三分类

| 类别 | 符号数 | shim 成本 | 说明 |
|------|:--:|:--:|------|
| **type-only**（`import { type X }`） | 12 | **0** | JS 运行时蒸发，编译期消失，无需 shim |
| **typebox 提供**（`StringEnum`/`Type`） | 2 | **0** | typebox 是纯 JS npm 包，作为 asset 打包（`rust-embed`），13 处 typebox import 全部免费 |
| **非-TUI 运行时值**（可做） | 23 | ~750–1150 行 | 见 8.2 |
| **TUI 组件/渲染**（被 TUI 偏差卡住） | 13 | stub ~55 / 真实=TUI 移植 | TUI 已是"已确认保留"偏差，本回合不复刻 |

type-only 清单（运行时不存在）：`Component`/`Focusable`/`SelectItem`/`SettingItem`/`TUI`（pi-tui）、`BashOperations`/`ExtensionAPI`（pi-coding-agent）、`Api`/`Context`/`Message`/`Model`/`UserMessage`（pi-ai）。

### 8.2 非-TUI 可行 shim 明细（23 个运行时符号）

**pi-tui 纯工具（7）** —— 纯逻辑，JS 重写即可，无需 TUI 移植：
`Key`/`parseKey`/`matchesKey`/`isKeyRelease`/`CURSOR_MARKER`/`visibleWidth`/`truncateToWidth` → ~100 行 JS

**pi-coding-agent 非-TUI（12）**：
- 配置类（`CONFIG_DIR_NAME`/`VERSION`）2 行
- host 桥：`getAgentDir` ~8、`parseFrontmatter` ~25（纯 JS 移植）、`defineTool` ~20
- 内置工具工厂 `createBashTool`/`createEditTool`/`createReadTool`/`createWriteTool`：4 × ~40 = ~160（每个：构造 `ToolDefinition` + execute 回调调 host op 跑现有内置工具）
- 数据变换 `convertToLlm`/`serializeConversation` ~80、`withFileMutationQueue` ~30
→ ~365 行 JS

**pi-ai 非-typebox（4）**：`complete` ~35、`getModel` ~15、`streamSimple` ~50、`registerApiProvider` ~30 → ~130 行 JS

**对应 Rust host op 胶水**：内置工具工厂调现有工具执行器 ~80、pi-ai 桥接到 provider 层 ~150–250、杂项 getter ~40 → **~250–400 行 Rust**

**非-TUI shim 合计：~500–750 JS + ~250–400 Rust ≈ 750–1150 行**

### 8.3 TUI-blocked 明细（13 个运行时符号）

- pi-tui 组件（8）：`Box`/`Container`/`Input`/`Markdown`/`SelectList`/`SettingsList`/`Spacer`/`Text`
- pi-coding-agent TUI 渲染助手（5）：`BorderedLoader`/`CustomEditor`/`DynamicBorder`/`getMarkdownTheme`/`getSettingsListTheme`

这些渲染到终端，Rust TUI 是**已确认保留的偏差**（本回合不复刻）。两条路：
- **stub**（非-TUI 模式抛 "TUI unavailable"）：~55 行，让这些扩展在 RPC/CLI 模式下显式不可用
- **真实实现** = 整个 TUI 移植工程，不在本次范围

### 8.4 对方案 A 总量的修正

| 组件 | 第 5 节估 | 实测修正 |
|------|:--:|:--:|
| #4 SDK shim（非-TUI） | 600–1200 JS（气球 2000+） | **~500–750 JS + ~250–400 Rust** |
| TUI shim | 含在上面 | **stub ~55 / 真实=TUI 移植（独立工程）** |

**方案 A 非-TUI 全量** ≈ 原 Rust 2700–4500 + shim Rust 250–400 + shim JS 500–750
≈ **~3500–5600 行**，比第 5 节的"~5k"略低且更确定。TUI 扩展要等 TUI 移植，
与方案 A 本身解耦。

### 8.5 结论

SDK shim 的真实成本被高估了：type-only（12）+ typebox-bundled（2）= 14 个
符号零成本；非-TUI 运行时 23 个符号约 750–1150 行、边界清晰；剩余 13 个
TUI 符号被已有的 TUI 偏差卡住，stub 即可，真实成本归到 TUI 移植工程而非
方案 A。气球风险 100% 集中在 TUI 符号上，而 TUI 本就是偏差。
