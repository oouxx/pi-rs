# DEVIATIONS.md — pi-ai

按 CLAUDE.md 阶段四的偏差日志格式记录。对齐检查遇到下列差异时按对应
"确认状态"处理。

| # | 位置（文件/函数） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
|---|------------------|-----------|---------------|----------|----------|
| 1 | 模型生成入口 `crates/xtask` / `crates/pi-ai/build.rs` / `data/models_generated.json` | `scripts/generate-models.ts` + `scripts/generate-image-models.ts` 作为 pre-publish 脚本抓取并生成 `src/providers/*.models.ts` + `src/models.generated.ts`，产物入仓，下游零网络 | 维护者跑 `cargo run -p xtask -- generate-models` 抓取并写盘 `crates/pi-ai/data/models_generated.json`，提交进仓；`build.rs` 不联网，运行时 `include_str!` 嵌入 | 对齐原版"生成是维护者动作、产物入仓、下游零网络、可复现"的设计；避免每次编译联网、离线构建静默退化成 0 模型的隐患 | 已确认保留 |
| 2 | 模型来源/覆盖度 `xtask/src/generate_models.rs::process_models_dev` 等 | `generate-models.ts` 覆盖 models.dev 的全部 provider（含 amazon-bedrock、google-vertex、kimi-coding、moonshotai/-cn、xiaomi/token-plan-*、opencode/-go、nvidia），外加 Vercel AI Gateway、NVIDIA NIM、`generate-image-models.ts` 图像模型，以及大批手工补充模型与 compat/thinking 元数据计算 | 当前仅覆盖 13 个 provider（anthropic/openai/google/deepseek/groq/cerebras/xai/mistral/together/fireworks-ai/github-copilot/minimax/minimax-cn）+ OpenRouter，无 AI Gateway/NIM/图像模型，无手工补充模型，compat/thinkingLevelMap/headers/tiers 等元数据基本未生成 | 阶段性未完成（本次仅落地 xtask 架构，覆盖度留后续补齐） | 待确认 |

| 3 | `mistral-conversations` API / mistral provider | 原版有独立 `mistral-conversations` API 实现（官方 Mistral Conversations API + 消息转换 + tool-call id 归一化）及 `mistral` provider 模型 | 已删除：不再注册 `mistral-conversations` 后端，不再生成 `mistral` provider 模型，移除 `MISTRAL_API_KEY` 映射与默认模型/display name | 用户反馈 Mistral 使用面小、且原 Rust 实现是错误路由到 openai-completions（行为与原版不符）；与其保留错误实现，不如显式移除。需要时按原版补完整 `mistral-conversations` 后端再恢复 | 已确认保留 |
| 4 | `ollama` provider / `providers/ollama.rs` | 原版无内置 Ollama 支持（用户须自行在 `models.json` 配 OpenAI 兼容条目指向 Ollama） | 新增运行时自动发现：`discover_ollama_models()` 探测本机 `http://localhost:11434/api/tags`，把已安装模型注册为 `api=openai-completions` 的 `ollama` provider 模型；探测失败静默返回空（Ollama 未运行属合法状态）；端点可用 `OLLAMA_BASE_URL`/`OLLAMA_HOST` 覆盖 | 本地 LLM 是 pi-rs 的常用场景，自动发现降低使用门槛；原版 `models.json` 路径仍可用（且可覆盖自动发现的默认值，如 contextWindow） | 已确认保留（pi-rs 增强，非原版行为） |

> 备注：#2 属于"未完成"而非"有意偏差"。补齐时按原版逻辑逐项移植，并
> 在补完后把对应条目状态改为"已确认保留"或删除。
