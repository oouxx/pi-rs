//! Context token estimation (port of `packages/ai/src/utils/estimate.ts`).
//!
//! Used by `clampMaxTokensToContext` (simple streaming options) to keep the
//! requested output under the model's context window.

use std::collections::HashSet;

use crate::types::{ContentBlock, Context, Message, StopReason, Tool, Usage};

const CHARS_PER_TOKEN: u64 = 4;
const ESTIMATED_IMAGE_CHARS: u64 = 4800;

/// Estimated context usage (match TS `ContextUsageEstimate`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextUsageEstimate {
    /// Estimated total context tokens.
    pub tokens: u64,
    /// Tokens reported by the most recent applicable assistant usage block.
    pub usage_tokens: u64,
    /// Estimated tokens after the most recent applicable usage block.
    pub trailing_tokens: u64,
    /// Index of the applicable message that provided usage.
    pub last_usage_index: Option<usize>,
}

/// TS `calculateContextTokens`.
#[must_use]
pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

fn ceil_div(value: u64, divisor: u64) -> u64 {
    value.div_ceil(divisor)
}

/// JS `String.length` counts UTF-16 code units.
fn utf16_len(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

/// TS `estimateTextTokens`.
#[must_use]
pub fn estimate_text_tokens(text: &str) -> u64 {
    ceil_div(utf16_len(text), CHARS_PER_TOKEN)
}

fn content_chars(content: &[ContentBlock]) -> u64 {
    content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text, .. } => utf16_len(text),
            ContentBlock::Image { .. } => ESTIMATED_IMAGE_CHARS,
            _ => 0,
        })
        .sum()
}

/// TS `estimateMessageTokens`.
#[must_use]
pub fn estimate_message_tokens(message: &Message) -> u64 {
    let chars = match message {
        Message::User { content, .. } | Message::ToolResult { content, .. } => content_chars(content),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text, .. } => utf16_len(text),
                ContentBlock::Thinking { thinking, .. } => utf16_len(thinking),
                ContentBlock::ToolCall {
                    name, arguments, ..
                } => utf16_len(name) + utf16_len(&safe_json_stringify(arguments)),
                _ => 0,
            })
            .sum(),
    };
    ceil_div(chars, CHARS_PER_TOKEN)
}

fn safe_json_stringify(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "undefined".to_string())
}

fn estimate_tools_tokens(tools: Option<&[Tool]>) -> u64 {
    match tools {
        None | Some([]) => 0,
        Some(tools) => estimate_text_tokens(
            &serde_json::to_string(tools).unwrap_or_else(|_| "undefined".to_string()),
        ),
    }
}

fn message_timestamp(message: &Message) -> i64 {
    match message {
        Message::User { timestamp, .. }
        | Message::Assistant { timestamp, .. }
        | Message::ToolResult { timestamp, .. } => *timestamp,
    }
}

fn get_last_assistant_usage_info(messages: &[Message]) -> Option<(Usage, usize)> {
    let mut latest_prefix_timestamp = i64::MIN;
    let mut usage_info = None;
    for (index, message) in messages.iter().enumerate() {
        if let Message::Assistant {
            usage,
            stop_reason,
            timestamp,
            ..
        } = message
        {
            let usage_applies = *timestamp >= latest_prefix_timestamp;
            if usage_applies
                && *stop_reason != StopReason::Aborted
                && *stop_reason != StopReason::Error
                && calculate_context_tokens(usage) > 0
            {
                usage_info = Some((usage.clone(), index));
            }
        }
        latest_prefix_timestamp = latest_prefix_timestamp.max(message_timestamp(message));
    }
    usage_info
}

fn estimate_messages(messages: &[Message]) -> ContextUsageEstimate {
    if let Some((usage, index)) = get_last_assistant_usage_info(messages) {
        let usage_tokens = calculate_context_tokens(&usage);
        let trailing_tokens: u64 = messages[index + 1..]
            .iter()
            .map(estimate_message_tokens)
            .sum();
        return ContextUsageEstimate {
            tokens: usage_tokens + trailing_tokens,
            usage_tokens,
            trailing_tokens,
            last_usage_index: Some(index),
        };
    }

    let tokens: u64 = messages.iter().map(estimate_message_tokens).sum();
    ContextUsageEstimate {
        tokens,
        usage_tokens: 0,
        trailing_tokens: tokens,
        last_usage_index: None,
    }
}

/// TS `estimateContextTokens`.
#[must_use]
pub fn estimate_context_tokens(context: &Context) -> ContextUsageEstimate {
    let estimate = estimate_messages(&context.messages);
    if let Some(index) = estimate.last_usage_index {
        let added_names: HashSet<&str> = context.messages[index + 1..]
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult {
                    added_tool_names, ..
                } => added_tool_names.as_ref(),
                _ => None,
            })
            .flatten()
            .map(String::as_str)
            .collect();
        let added_tools: Vec<Tool> = context
            .tools
            .as_ref()
            .map(|tools| {
                tools
                    .iter()
                    .filter(|tool| added_names.contains(tool.name.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let added_tool_tokens = estimate_tools_tokens(if added_tools.is_empty() {
            None
        } else {
            Some(&added_tools)
        });
        return ContextUsageEstimate {
            tokens: estimate.tokens + added_tool_tokens,
            usage_tokens: estimate.usage_tokens,
            trailing_tokens: estimate.trailing_tokens + added_tool_tokens,
            last_usage_index: estimate.last_usage_index,
        };
    }

    let prefix_tokens = context
        .system_prompt
        .as_deref()
        .map_or(0, estimate_text_tokens)
        + estimate_tools_tokens(context.tools.as_deref());
    ContextUsageEstimate {
        tokens: estimate.tokens + prefix_tokens,
        usage_tokens: estimate.usage_tokens,
        trailing_tokens: estimate.trailing_tokens + prefix_tokens,
        last_usage_index: estimate.last_usage_index,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::types::{ContentBlock, UsageCost};

    fn user(text: &str, timestamp: i64) -> Message {
        Message::User {
            content: vec![ContentBlock::Text {
                text: text.into(),
                text_signature: None,
            }],
            timestamp,
        }
    }

    fn assistant(usage: Usage, stop_reason: StopReason, timestamp: i64) -> Message {
        Message::Assistant {
            content: vec![],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "m".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage,
            stop_reason,
            error_message: None,
            timestamp,
        }
    }

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 0,
            cost: UsageCost::default(),
        }
    }

    fn context(messages: Vec<Message>) -> Context {
        Context {
            system_prompt: None,
            messages,
            tools: None,
        }
    }

    #[test]
    fn test_calculate_context_tokens_prefers_total() {
        let mut u = usage(10, 20);
        assert_eq!(calculate_context_tokens(&u), 30);
        u.total_tokens = 99;
        assert_eq!(calculate_context_tokens(&u), 99);
    }

    #[test]
    fn test_estimate_text_tokens() {
        assert_eq!(estimate_text_tokens("Hello world"), 3); // 11/4 -> 3
        assert_eq!(estimate_text_tokens(""), 0);
    }

    #[test]
    fn test_estimate_uses_last_assistant_usage_plus_trailing() {
        // usage=100, then a trailing user message of 12 chars -> 3 tokens.
        let ctx = context(vec![
            assistant(usage(60, 40), StopReason::Stop, 1),
            user("Hello world!", 2),
        ]);
        let estimate = estimate_context_tokens(&ctx);
        assert_eq!(estimate.usage_tokens, 100);
        assert_eq!(estimate.trailing_tokens, 3);
        assert_eq!(estimate.tokens, 103);
        assert_eq!(estimate.last_usage_index, Some(0));
    }

    /// An aborted/errored assistant usage must not be used (match TS).
    #[test]
    fn test_estimate_ignores_aborted_usage() {
        let ctx = context(vec![
            assistant(usage(1000, 0), StopReason::Aborted, 1),
            user("abcd", 2),
        ]);
        let estimate = estimate_context_tokens(&ctx);
        assert_eq!(estimate.last_usage_index, None);
        assert_eq!(estimate.tokens, 1);
    }

    #[test]
    fn test_estimate_includes_system_prompt_and_tools_without_usage() {
        let mut ctx = context(vec![user("abcd", 1)]);
        ctx.system_prompt = Some("12345678".into()); // 8 chars -> 2 tokens
        assert_eq!(estimate_context_tokens(&ctx).tokens, 1 + 2);
    }
}
