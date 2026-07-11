# pi (TypeScript) → Rust 复刻工作流规范

## 0. 范围澄清（原文档存在的问题）

原始 prompt 写"上面四个拆分为四个 crate"，但只给出了两个仓库链接
(`packages/agent`、`packages/ai`)，且只列出两个 crate 名
(`pi-agent-core`、`pi-ai`)。这是自相矛盾的，Claude 在执行时会不知道
第三、四个 crate 对应哪个源码目录。

**按你现有项目的四层架构，明确范围如下：**

| Crate             | 对应源码目录                   | 职责一句话                                                |
| ----------------- | ------------------------------ | --------------------------------------------------------- |
| `pi-ai`           | `packages/ai`                  | 统一多 Provider LLM API（OpenAI/Anthropic/Google 等）     |
| `pi-agent-core`   | `packages/agent`               | Agent 运行时：状态机、工具调用循环、事件流                |
| `pi-coding-agent` | `packages/coding-agent`        | 内置工具集（read/write/edit/bash/grep/find/ls）+ 扩展系统 |
| `pi-tui`          | （coding-agent 内的 TUI 组件） | 终端 UI 渲染层，你计划用 Ratatui 重写而非逐行复刻         |

若本次任务实际只做前两个 crate，请在下达任务时明确写"本次仅做
`pi-ai` + `pi-agent-core`，`pi-coding-agent`/`pi-tui` 见后续任务"，
避免 Claude 在分析阶段擅自扩大或缩小范围。

依赖方向固定为单向：`pi-ai` ← `pi-agent-core` ← `pi-coding-agent` ←
`pi-tui`。禁止反向依赖（例如 `pi-ai` 不应该知道 `pi-agent-core` 的存在）。

---

## 第一步：架构分析（只产出文档，不写代码）

对**每一个源码目录**分别输出以下四类内容，禁止用"大致"、"类似"这种
模糊词，每一项都要能在原仓库源码中定位到具体文件/行号。

### 1.1 模块列表和职责

用表格，逐文件（不是逐目录）列出：

| 文件路径 | 导出的主要符号 | 职责（一句话） | 是否为公开 API（会被其他 crate 引用） |
| -------- | -------------- | -------------- | ------------------------------------- |

### 1.2 核心数据结构

对每个核心 type/interface：

- 原始 TS 定义（贴代码块）
- 字段逐个说明：类型、是否 optional、默认值、不变量（invariant）
- 标注这个类型在 Rust 里大概率要如何映射：
  - `interface` 用 `struct` 还是 `trait`？
  - TS 的可选字段 (`field?: T`) → `Option<T>`
  - TS 的联合类型 (`type A = X | Y`) → `enum`，并列出每个 variant
  - 是否涉及 `any`/动态类型（这类是复刻的高风险点，需要单独标注）

### 1.3 模块间依赖关系

- 用文字描述 import 关系（谁 import 谁），不要求画图工具，但要覆盖
  所有跨文件依赖
- 明确指出是否存在循环依赖，若有，说明 TS 里是怎么解决的（比如用
  interface 解耦），Rust 里对应用 trait 还是拆子 crate 解决

### 1.4 对外接口

- 每个公开函数/方法的完整签名（参数名、类型、返回类型、是否 async、
  是否可能 throw/reject 及在什么条件下）
- 事件流/回调类接口（比如 `agent.subscribe(...)`）要单独列出所有
  可能的事件 variant 和触发时机

**验收标准：** 第一步产出物必须让人不看原仓库代码，就能推导出 Rust
侧的类型定义骨架。如果做不到，说明分析深度不够，需要补充。

---

## 第二步：按模块重写（Rust 实现）

### 2.1 顺序

1. 先写 `pi-ai`（无对内部依赖），再写 `pi-agent-core`（依赖 `pi-ai`）
2. 同一 crate 内部，先写被依赖最多的底层模块（比如 message 类型定义），
   再写上层的 agent loop / provider 适配

### 2.2 每个模块的写作顺序（强制）

1. **类型定义**（struct/enum/trait，先不写方法体，`todo!()` 占位）
2. **测试**：针对第一步分析出的公开接口，写行为测试。测试用例来源
   优先级：
   - 原仓库如果有对应 `.test.ts`，直接翻译测试用例（输入/输出/边界
     条件保持一致）
   - 原仓库没有测试的公开行为，自己根据第一步的"不变量"补测试
3. **实现**，直到测试通过

### 2.3 代码规范（沿用你的 CLAUDE.md 约定）

- 禁止 `.unwrap()` / `.expect()`（测试代码除外），错误必须通过
  `Result<T, E>` 显式传播，用 `thiserror` 定义每个 crate 自己的错误类型
- 函数参数最多 3 个，超过用 struct 传参（对应 TS 里常见的 options
  object 参数）
- trait-first：TS 里的 interface（尤其是 Provider 抽象）优先映射为
  Rust trait，而不是直接用某个 provider 的具体结构体
- 禁止静默 fallback：TS 里如果某处用了 `?? defaultValue` 掩盖了
  "本不该发生"的情况，Rust 侧要显式判断并返回 Err 或 panic（视原意图
  而定，不能默默吞掉）
- 异步：TS 的 `async/await` + Promise → Rust 用 `async fn` +
  `tokio`；流式事件（`agent.subscribe`）→ 用 `tokio::sync::mpsc` 或
  自定义 `Stream` trait，不要用回调闭包硬翻译

### 2.4 明确不逐行复刻的部分

- TUI 渲染层不追求逐行复刻（你已决定用 Ratatui + TEA 架构重写），
  但**状态机语义**（有哪些状态、状态转移条件）必须与原版一致
- Node.js 特定的运行时行为（比如某些 process/fs API 细节）按 Rust
  生态惯用法重写，不追求 API 名字一致，但要在文档里注明"此处有意
  偏离原版，原因是 XXX"

---

## 第三步：验证（每个模块写完立刻做，不要攒到最后）

因为 TS 和 Rust 是两个运行时，不可能真的"跑同一份代码"，验证要拆成
三个可执行的层次：

### 3.1 单元级：黄金用例对齐

- 对第一步翻译过来的每个测试用例，人工核对：Rust 测试里的输入和
  期望输出，与原 TS 测试文件里的是否**字面一致**（不是"意思差不多"）
- 数值/字符串类的边界条件（空输入、超长输入、Unicode、null vs
  undefined 对应的 `None`）逐条列出核对表

### 3.2 契约级：接口行为对齐

针对每个公开 API，写一份对照表：

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |

对于 agent loop 这种有状态机语义的模块，额外做**事件序列对齐**：
给定同一个输入 prompt + mock 的 LLM 响应，原版和 Rust 版产出的事件
序列（event type 顺序）必须相同。

### 3.3 集成级：跨模块回归

- 每个 crate 完成后，跑一遍该 crate 所有测试 + `cargo clippy`
  （零警告）+ `cargo test --workspace`
- 涉及网络调用（真实 LLM Provider）的部分，用录制的 fixture
  （请求/响应 JSON）做离线回放测试，不依赖真实 API Key，保证测试
  可重复

### 3.4 验收清单（每个模块合并前必须打勾）

- [ ] 类型定义与第一步分析文档一致，无遗漏字段
- [ ] 翻译测试用例 100% 通过
- [ ] 补充的边界条件测试通过
- [ ] `cargo clippy --all-targets -- -D warnings` 无警告
- [ ] 无 `.unwrap()`/`.expect()`（测试代码除外）
- [ ] 公开 API 文档注释（`///`）覆盖所有 pub 项


