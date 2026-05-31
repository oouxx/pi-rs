# claude
## 使用rust完整复刻https://github.com/earendil-works/pi/tree/main/packages/agent


## 使用rust完整复刻https://github.com/earendil-works/pi/tree/main/packages/coding-agent 
## 对https://github.com/0xPlaygrounds/rig/tree/main/crates/rig-core/src/providers 做一层thin wrapper 实现https://github.com/earendil-works/pi/tree/main/packages/ai
## 使用ratatui 实现https://github.com/earendil-works/pi/tree/main/packages/tui

## 上面四个拆分为四个crate分别是pi-agent-core, pi-coding-agent, pi-ai, pi-tui

## 代码复刻规范
### 第一步，分析仓库的架构，输出：
1. 模块列表和职责
2. 核心数据结构
3. 模块间依赖关系
4. 对外接口（API/函数签名）
不要写任何代码，只做分析。
### 第二步 ，按照模块进行重写

目标语言：rust
要求：
- 保持函数签名语义一致
- 先写类型定义
- 先写测试，再写实现

### 每个模块写完立刻验证
确保和原版行为一致：
- 相同输入输出相同结果
- 边界条件处理一致