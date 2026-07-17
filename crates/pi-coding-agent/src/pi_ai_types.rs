//! Re-exports from pi-agent-core's pi_ai_types for downstream crates.
//!
//! Downstream consumers (pi-cli, pi-tui) should access pi-ai-level types
//! through this module rather than depending on pi-agent-core or pi-ai directly.
//! This preserves the one-directional dependency chain:
//!
//!   pi-ai → pi-agent-core → pi-coding-agent → pi-cli / pi-tui

pub use pi_agent_core::pi_ai_types::{
    get_env_api_key, get_env_var_name, AnthropicMessagesCompat, AssistantMessage,
    AssistantMessageDiagnostic, AssistantMessageEvent, CacheRetention, ContentBlock, Context,
    ImagesModel, Message, Model, ModelCompat, ModelCost, OpenAICompletionsCompat,
    OpenAIResponsesCompat, OpenRouterRouting, ProviderResponse, SimpleStreamOptions, StopReason,
    StreamOptions, ThinkingBudgets, ThinkingLevel, ThinkingLevelMap, Tool, ToolCall, ToolExecutionMode,
    Transport, Usage, UsageCost, VercelGatewayRouting, StreamResponse,
};
