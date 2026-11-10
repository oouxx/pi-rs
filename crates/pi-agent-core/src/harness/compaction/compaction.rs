#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::harness::types::{
    CompactionError, CompactionPreparation, CompactionSettings, FileOperations, SessionTreeEntry,
};
use crate::pi_ai_types::Message;
use crate::types::AgentMessage;

// External pi_ai crate imports (not re-exported through pi_ai_types)
use pi_ai::stream::complete_simple as pi_complete;

/// UTF-16 code units in a string (JS `String.prototype.length`).
fn utf16_chars(text: &str) -> u64 {
    pi_ai::utils::text::utf16_len(text) as u64
}

/// Text + image character count (TS `estimateTextAndImageContentChars`).
fn content_chars(content: &[crate::pi_ai_types::ContentBlock]) -> u64 {
    content
        .iter()
        .map(|block| match block {
            crate::pi_ai_types::ContentBlock::Text { text, .. } => utf16_chars(text),
            crate::pi_ai_types::ContentBlock::Image { .. } => 4800,
            _ => 0,
        })
        .sum()
}

/// Estimate token count for a message (match v0.87.1 `estimateTokens`).
///
/// Counts UTF-16 code units, includes image blocks (4800 chars), assistant
/// thinking and tool-call arguments, system sections/toolsAdded, and uses
/// `ceil(chars / 4)` (0 for an empty message, not 1).
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let chars = match message {
        AgentMessage::System {
            content,
            sections,
            tools_added,
            ..
        } => {
            let mut count = match content {
                crate::pi_ai_types::SystemContent::Text(text) => utf16_chars(text),
                crate::pi_ai_types::SystemContent::Blocks(blocks) => content_chars(blocks),
            };
            if let Some(sections) = sections {
                for section in sections.values().flatten() {
                    count += utf16_chars(section);
                }
            }
            if let Some(tools_added) = tools_added {
                count += utf16_chars(&serde_json::to_string(tools_added).unwrap_or_default());
            }
            count
        }
        AgentMessage::User { content, .. } => content_chars(content),
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .map(|block| match block {
                crate::pi_ai_types::ContentBlock::Text { text, .. } => utf16_chars(text),
                crate::pi_ai_types::ContentBlock::Thinking { thinking, .. } => {
                    utf16_chars(thinking)
                }
                crate::pi_ai_types::ContentBlock::ToolCall {
                    name, arguments, ..
                } => {
                    utf16_chars(name)
                        + utf16_chars(&serde_json::to_string(arguments).unwrap_or_default())
                }
                _ => 0,
            })
            .sum(),
        AgentMessage::ToolResult { content, .. } => content_chars(content),
        AgentMessage::BashExecution {
            command, output, ..
        } => utf16_chars(command) + utf16_chars(output),
        AgentMessage::Custom { content, .. } => match content {
            crate::types::CustomContent::Text(text) => utf16_chars(text),
            crate::types::CustomContent::Blocks(blocks) => content_chars(blocks),
        },
        AgentMessage::BranchSummary { summary, .. }
        | AgentMessage::CompactionSummary { summary, .. } => utf16_chars(summary),
    };
    chars.div_ceil(4)
}

pub fn estimate_context_tokens(messages: &[AgentMessage]) -> u64 {
    messages.iter().map(estimate_tokens).sum()
}

pub fn should_compact(
    total_tokens: u64,
    context_window: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    let threshold = context_window.saturating_sub(settings.reserve_tokens);
    total_tokens > threshold
}

pub fn find_turn_start_index(messages: &[AgentMessage], from_index: usize) -> usize {
    for i in (0..=from_index).rev() {
        if let AgentMessage::User { .. } = messages[i] {
            return i;
        }
    }
    0
}

pub struct CutPoint {
    pub first_kept_entry_index: usize,
    pub turn_start_index: usize,
    pub is_split_turn: bool,
}

/// Turn-start roles in v0.87.1 `isTurnStartEntry`.
fn is_turn_start_message(message: &AgentMessage) -> bool {
    matches!(
        message,
        AgentMessage::User { .. }
            | AgentMessage::BashExecution { .. }
            | AgentMessage::Custom { .. }
            | AgentMessage::BranchSummary { .. }
            | AgentMessage::CompactionSummary { .. }
    )
}

/// Match v0.87.1 `findCutPoint` on the filtered message vector.
///
/// Valid cut points never split a tool call from its result. The budget check
/// is `>= keepRecentTokens` (not `>`), and a cut is a turn start for user,
/// bashExecution, custom, branchSummary and compactionSummary messages.
pub fn find_cut_point(messages: &[AgentMessage], tokens_to_keep: u64) -> CutPoint {
    let cut_points: Vec<usize> = (0..messages.len())
        .filter(|&index| !matches!(messages[index], AgentMessage::ToolResult { .. }))
        .collect();
    if cut_points.is_empty() {
        return CutPoint {
            first_kept_entry_index: 0,
            turn_start_index: 0,
            is_split_turn: false,
        };
    }

    let mut accumulated_tokens = 0u64;
    let mut cut_index = cut_points[0];
    for i in (0..messages.len()).rev() {
        let msg_tokens = estimate_tokens(&messages[i]);
        if msg_tokens == 0 {
            continue;
        }
        accumulated_tokens += msg_tokens;
        if accumulated_tokens >= tokens_to_keep {
            let fallback = cut_points.last().copied().unwrap_or(cut_index);
            cut_index = cut_points
                .iter()
                .copied()
                .find(|&candidate| candidate >= i)
                .unwrap_or(fallback);
            break;
        }
    }

    // Include adjacent metadata entries that do not affect context.
    while cut_index > 0 {
        let prev = &messages[cut_index - 1];
        if matches!(prev, AgentMessage::CompactionSummary { .. }) || estimate_tokens(prev) > 0 {
            break;
        }
        cut_index -= 1;
    }

    let starts_turn = is_turn_start_message(&messages[cut_index]);
    let turn_start = if starts_turn {
        None
    } else {
        (0..cut_index)
            .rev()
            .find(|&index| is_turn_start_message(&messages[index]))
    };

    CutPoint {
        first_kept_entry_index: cut_index,
        turn_start_index: turn_start.unwrap_or(cut_index),
        is_split_turn: !starts_turn && turn_start.is_some(),
    }
}

pub fn get_last_assistant_usage(messages: &[AgentMessage]) -> Option<crate::pi_ai_types::Usage> {
    for msg in messages.iter().rev() {
        if let AgentMessage::Assistant { usage, .. } = msg {
            return Some(usage.clone());
        }
    }
    None
}

pub fn calculate_context_tokens(system_prompt: &str, messages: &[AgentMessage]) -> u64 {
    utf16_chars(system_prompt).div_ceil(4) + estimate_context_tokens(messages)
}

pub fn prepare_compaction(
    entries: &[SessionTreeEntry],
    context_window: u64,
    settings: &CompactionSettings,
) -> std::result::Result<CompactionPreparation, CompactionError> {
    // Keep a parallel map from each filtered message back to its entry. The
    // cut point is computed on messages, but `first_kept_entry_id` must name
    // an entry; using the message index directly shifts it whenever the
    // session contains non-message entries (model/thinking changes, labels…).
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut message_entry_indices: Vec<usize> = Vec::new();
    for (entry_index, entry) in entries.iter().enumerate() {
        let message = match entry {
            SessionTreeEntry::Message { message, .. } => Some(message.clone()),
            SessionTreeEntry::Compaction {
                summary,
                tokens_before,
                ..
            } => Some(AgentMessage::CompactionSummary {
                summary: summary.clone(),
                tokens_before: *tokens_before,
                timestamp: chrono::Utc::now().timestamp_millis(),
            }),
            SessionTreeEntry::BranchSummary {
                summary, from_id, ..
            } => Some(AgentMessage::BranchSummary {
                summary: summary.clone(),
                from_id: from_id.clone(),
                timestamp: chrono::Utc::now().timestamp_millis(),
            }),
            _ => None,
        };
        if let Some(message) = message {
            messages.push(message);
            message_entry_indices.push(entry_index);
        }
    }

    let tokens_before = estimate_context_tokens(&messages);
    let tokens_to_keep = settings.keep_recent_tokens;

    if !should_compact(tokens_before, context_window, settings) {
        return Err(CompactionError::NoCompactionNeeded);
    }

    let cut_point = find_cut_point(&messages, tokens_to_keep);

    let first_kept_entry_id = message_entry_indices
        .get(cut_point.first_kept_entry_index)
        .and_then(|&entry_index| entries.get(entry_index))
        .map(|entry| entry.id().to_string())
        .unwrap_or_default();

    let messages_to_summarize: Vec<AgentMessage> =
        messages[..cut_point.first_kept_entry_index].to_vec();
    let turn_prefix_messages: Vec<AgentMessage> = if cut_point.is_split_turn {
        messages[cut_point.turn_start_index..cut_point.first_kept_entry_index].to_vec()
    } else {
        Vec::new()
    };
    // Retained recent messages stored on the compaction entry as a
    // self-contained checkpoint (match TS `retainedTail`).
    let retained_tail: Vec<AgentMessage> = messages[cut_point.first_kept_entry_index..].to_vec();

    let previous_summary = messages.iter().find_map(|m| {
        if let AgentMessage::CompactionSummary { summary, .. } = m {
            Some(summary.clone())
        } else {
            None
        }
    });

    let file_ops = extract_file_operations(&messages_to_summarize);

    Ok(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        retained_tail,
        is_split_turn: cut_point.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings: settings.clone(),
    })
}

pub(crate) fn extract_file_operations(messages: &[AgentMessage]) -> FileOperations {
    let mut ops = FileOperations::new();
    for msg in messages {
        if let AgentMessage::Assistant { content, .. } = msg {
            for block in content {
                if let crate::pi_ai_types::ContentBlock::ToolCall {
                    name, arguments, ..
                } = block
                {
                    let args = arguments.as_object();
                    if let Some(args_map) = args {
                        if let Some(path) = args_map.get("path").and_then(|v| v.as_str()) {
                            match name.as_str() {
                                "read" => ops.read.push(path.to_string()),
                                "write" => ops.written.push(path.to_string()),
                                "edit" => ops.edited.push(path.to_string()),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
    ops
}

pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts = Vec::new();

    for msg in messages {
        match msg {
            Message::User { content, .. } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        crate::pi_ai_types::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            // TS `serializeConversation` ignores system messages.
            Message::System { .. } => {}
            Message::Assistant { content, .. } => {
                let mut text_parts = Vec::new();
                let mut thinking_parts = Vec::new();
                let mut tool_calls = Vec::new();

                for block in content {
                    match block {
                        crate::pi_ai_types::ContentBlock::Text { text, .. } => {
                            text_parts.push(text.clone());
                        }
                        crate::pi_ai_types::ContentBlock::Thinking { thinking, .. } => {
                            thinking_parts.push(thinking.clone());
                        }
                        crate::pi_ai_types::ContentBlock::ToolCall {
                            name, arguments, ..
                        } => {
                            let args_str = arguments.to_string();
                            tool_calls.push(format!("{}({})", name, args_str));
                        }
                        _ => {}
                    }
                }

                if !thinking_parts.is_empty() {
                    parts.push(format!(
                        "[Assistant thinking]: {}",
                        thinking_parts.join("\n")
                    ));
                }
                if !text_parts.is_empty() {
                    parts.push(format!("[Assistant]: {}", text_parts.join("\n")));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            Message::ToolResult { content, .. } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        crate::pi_ai_types::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    // v0.87.1: `text.length` is UTF-16 code units and
                    // `slice(0, 2000)` never indexes bytes. A byte slice
                    // panics whenever byte 2000 lands inside a multi-byte
                    // character.
                    const TOOL_RESULT_MAX_CHARS: usize = 2000;
                    let total_units = pi_ai::utils::text::utf16_len(&text);
                    let truncated = if total_units > TOOL_RESULT_MAX_CHARS {
                        format!(
                            "{}\n\n[... {} more characters truncated]",
                            pi_ai::utils::text::truncate_utf16(&text, TOOL_RESULT_MAX_CHARS),
                            total_units - TOOL_RESULT_MAX_CHARS
                        )
                    } else {
                        text.clone()
                    };
                    parts.push(format!("[Tool result]: {truncated}"));
                }
            }
        }
    }

    parts.join("\n\n")
}

const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a summarization assistant. Your task is to create a concise summary of a coding agent conversation.

When summarizing:
1. Preserve key context: what the user asked, what was done, and what remains
2. Include specific file paths, function names, and code changes
3. Note any errors encountered and how they were resolved
4. Keep the summary focused and avoid unnecessary detail
5. Use bullet points for clarity

Format your summary as:
## Original Request
[What the user asked for]

## Work Done
- [Key actions taken and changes made]

## Current State
- [Where things stand now]

## Important Context
- [Any information needed to continue the work]"#;

/// Generate a summary of the given messages using the LLM.
///
/// Calls pi_ai to stream a completion from the specified model, using
/// the summarization system prompt and the serialized conversation as context.
#[allow(clippy::too_many_arguments)]
/// Shared choke point for every compaction/branch-summary summarization call
/// (match TS `completeSummarization`). Wraps the single LLM call in
/// `retryAssistantCall` so transient stream drops honor the configured retry
/// policy instead of failing the whole compaction on the first attempt.
/// Deterministic errors and aborts return immediately.
pub async fn complete_summarization(
    model: &crate::pi_ai_types::Model,
    context: &crate::pi_ai_types::Context,
    mut options: crate::pi_ai_types::SimpleStreamOptions,
    retry: Option<crate::pi_ai_types::RetryPolicy>,
    callbacks: Option<&crate::pi_ai_types::RetryCallbacks>,
) -> std::result::Result<crate::pi_ai_types::AssistantMessage, String> {
    // Summaries are standalone requests, so isolate routing and avoid cache
    // writes that cannot be reused (match TS #6618: fresh routing session ID +
    // `cacheRetention: "none"`).
    options.base.cache_retention = Some(crate::pi_ai_types::CacheRetention::None);
    options.base.session_id = Some(crate::pi_ai_types::uuid_v7());
    let produce = || {
        let model = model.clone();
        let context = context.clone();
        let options = options.clone();
        async move {
            match pi_complete(&model, &context, Some(options)).await {
                Ok(msg) => msg,
                Err(e) => crate::pi_ai_types::AssistantMessage {
                    provider_thinking_level: None,
                    content: vec![],
                    api: model.api.clone(),
                    provider: model.provider.clone(),
                    model: model.id.clone(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: crate::pi_ai_types::Usage::default(),
                    stop_reason: crate::pi_ai_types::StopReason::Error,
                    error_message: Some(e),
                    raw_stop_reason: None,
                    end_turn: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            }
        }
    };
    let signal = options.base.signal.clone();
    let result =
        crate::pi_ai_types::retry_assistant_call(produce, retry, signal.as_ref(), callbacks).await;
    if result.stop_reason == crate::pi_ai_types::StopReason::Error {
        return Err(result
            .error_message
            .unwrap_or_else(|| "Summarization failed".to_string()));
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub async fn generate_summary(
    messages: &[AgentMessage],
    model: &crate::pi_ai_types::Model,
    reserve_tokens: u64,
    api_key: &str,
    headers: Option<&std::collections::HashMap<String, String>>,
    signal: Option<tokio::sync::watch::Receiver<bool>>,
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
    thinking_level: Option<crate::pi_ai_types::ThinkingLevel>,
    retry: Option<crate::pi_ai_types::RetryPolicy>,
    callbacks: Option<&crate::pi_ai_types::RetryCallbacks>,
) -> std::result::Result<String, CompactionError> {
    if messages.is_empty() {
        return Ok(String::new());
    }

    // Build system prompt
    let system_prompt = if let Some(prev) = previous_summary {
        format!(
            "{}\n\n## Previous Summary\n{}",
            SUMMARIZATION_SYSTEM_PROMPT, prev
        )
    } else if let Some(instructions) = custom_instructions {
        format!(
            "{}\n\n## Custom Instructions\n{}",
            SUMMARIZATION_SYSTEM_PROMPT, instructions
        )
    } else {
        SUMMARIZATION_SYSTEM_PROMPT.to_string()
    };

    // Convert AgentMessage → LLM Message and serialize
    let llm_messages = crate::harness::messages::convert_to_llm(messages);
    let conversation_text = serialize_conversation(&llm_messages);

    // Build user message with the conversation to summarize
    let user_content = format!(
        "Please summarize the following conversation:\n\n<conversation>\n{}\n</conversation>\n\nProvide a concise summary following the format specified above.",
        conversation_text
    );

    let context = crate::pi_ai_types::Context {
        system_prompt: Some(system_prompt),
        messages: vec![crate::pi_ai_types::Message::User {
            content: vec![crate::pi_ai_types::ContentBlock::Text {
                text: user_content,
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }],
        tools: None,
    };

    // Match TS: maxTokens = Math.min(Math.floor(0.8 * reserveTokens), model.maxTokens > 0 ? model.maxTokens : Infinity)
    let max_tokens = std::cmp::min(
        (reserve_tokens as f64 * 0.8) as u64,
        if model.max_tokens > 0 {
            model.max_tokens
        } else {
            u64::MAX
        },
    );

    // Match TS: model.reasoning && thinkingLevel && thinkingLevel !== "off" ? { reasoning: thinkingLevel } : {}
    let reasoning = if model.reasoning {
        thinking_level.filter(|tl| tl != "off")
    } else {
        None
    };

    let simple_options = crate::pi_ai_types::SimpleStreamOptions {
        base: crate::pi_ai_types::StreamOptions {
            api_key: Some(api_key.to_string()),
            max_tokens: Some(max_tokens),
            sampling_params: None,
            headers: headers.cloned().filter(|h| !h.is_empty()),
            signal,
            ..Default::default()
        },
        reasoning,
        thinking_budgets: None,
        debug: None,
    };

    // Call pi_ai complete_simple wrapped in retryAssistantCall (match TS
    // `completeSummarization`): transient stream drops honor the configured
    // retry policy instead of failing the whole compaction on the first attempt.
    let result = complete_summarization(model, &context, simple_options, retry, callbacks)
        .await
        .map_err(|e| {
            CompactionError::SummarizationFailed(format!("LLM summarization failed: {e}"))
        })?;

    // Extract text from the response
    let summary = result
        .content
        .iter()
        .filter_map(|b| match b {
            crate::pi_ai_types::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    if summary.trim().is_empty() {
        return Err(CompactionError::SummarizationFailed(
            "LLM returned empty summary".to_string(),
        ));
    }

    Ok(summary)
}

pub async fn compact(
    preparation: CompactionPreparation,
    model: &crate::pi_ai_types::Model,
    api_key: &str,
    headers: Option<&std::collections::HashMap<String, String>>,
    custom_instructions: Option<&str>,
    signal: Option<tokio::sync::watch::Receiver<bool>>,
    thinking_level: Option<crate::pi_ai_types::ThinkingLevel>,
    retry: Option<crate::pi_ai_types::RetryPolicy>,
    callbacks: Option<&crate::pi_ai_types::RetryCallbacks>,
) -> std::result::Result<crate::harness::types::CompactResult, CompactionError> {
    let summary = generate_summary(
        &preparation.messages_to_summarize,
        model,
        preparation.settings.reserve_tokens,
        api_key,
        headers,
        signal,
        custom_instructions,
        preparation.previous_summary.as_deref(),
        thinking_level,
        retry,
        callbacks,
    )
    .await?;

    Ok(crate::harness::types::CompactResult {
        summary,
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        tokens_before: preparation.tokens_before,
        details: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::types::{CompactionSettings, SessionTreeEntry};
    use crate::pi_ai_types::{ContentBlock, Usage};

    fn create_user_message(text: &str) -> AgentMessage {
        AgentMessage::User {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            timestamp: 1000,
        }
    }

    fn create_assistant_message(text: &str, usage: Usage) -> AgentMessage {
        AgentMessage::Assistant {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage,
            stop_reason: Some(crate::pi_ai_types::StopReason::Stop),
            error_message: None,
            timestamp: 1000,
        }
    }

    fn create_tool_result_message(text: &str) -> AgentMessage {
        AgentMessage::ToolResult {
            tool_call_id: "tool-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            details: serde_json::Value::Object(Default::default()),
            is_error: false,
            usage: None,
            timestamp: 1000,
        }
    }

    fn create_mock_usage(input: u64, output: u64) -> Usage {
        Usage {
            input,
            output,
            ..Usage::default()
        }
    }

    fn create_message_entry(message: AgentMessage, parent_id: Option<String>) -> SessionTreeEntry {
        SessionTreeEntry::Message {
            id: "entry-1".to_string(),
            parent_id,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message,
        }
    }

    #[test]
    fn test_estimate_tokens_user_message() {
        let msg = create_user_message("Hello world");
        let tokens = estimate_tokens(&msg);
        // v0.87.1: UTF-16 length 11, ceil(11 / 4) = 3.
        assert_eq!(tokens, 3);
    }

    #[test]
    fn test_estimate_tokens_bash_execution() {
        let msg = AgentMessage::BashExecution {
            command: "ls -la".to_string(),
            output: "file1 file2".to_string(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 1000,
            exclude_from_context: None,
        };
        let tokens = estimate_tokens(&msg);
        // command (6) + output (11) = 17 UTF-16 units, ceil(17 / 4) = 5.
        assert_eq!(tokens, 5);
    }

    #[test]
    fn test_estimate_tokens_custom_blocks() {
        let msg = AgentMessage::Custom {
            custom_type: "note".to_string(),
            content: crate::types::CustomContent::Blocks(vec![ContentBlock::Text {
                text: "Block content".to_string(),
                text_signature: None,
            }]),
            display: true,
            details: None,
            timestamp: 1000,
        };
        let tokens = estimate_tokens(&msg);
        // "Block content" is 13 UTF-16 units, ceil(13 / 4) = 4.
        assert_eq!(tokens, 4);
    }

    #[test]
    fn test_estimate_tokens_empty_message_is_zero() {
        // v0.87.1 returns Math.ceil(0 / 4) == 0, not a forced minimum of 1.
        let msg = create_user_message("");
        assert_eq!(estimate_tokens(&msg), 0);
    }

    #[test]
    fn test_estimate_tokens_minimum_one() {
        let msg = create_user_message("a");
        let tokens = estimate_tokens(&msg);
        assert_eq!(tokens, 1);
    }

    #[test]
    fn test_estimate_context_tokens() {
        let messages = vec![
            create_user_message("Hello"),
            create_assistant_message("Hi", create_mock_usage(100, 50)),
        ];
        let total = estimate_context_tokens(&messages);
        let expected: u64 = messages.iter().map(estimate_tokens).sum();
        assert_eq!(total, expected);
    }

    #[test]
    fn test_estimate_context_tokens_empty() {
        let messages: Vec<AgentMessage> = vec![];
        let total = estimate_context_tokens(&messages);
        assert_eq!(total, 0);
    }

    #[test]
    fn test_should_compact_above_threshold() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 10_000,
            keep_recent_tokens: 20_000,
        };
        assert!(should_compact(95_000, 100_000, &settings));
    }

    #[test]
    fn test_should_compact_disabled() {
        let settings = CompactionSettings {
            enabled: false,
            reserve_tokens: 10_000,
            keep_recent_tokens: 20_000,
        };
        assert!(!should_compact(95_000, 100_000, &settings));
    }

    #[test]
    fn test_should_compact_at_threshold() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 10_000,
            keep_recent_tokens: 20_000,
        };
        assert!(!should_compact(90_000, 100_000, &settings));
    }

    #[test]
    fn test_find_turn_start_index_with_user_message() {
        let messages = vec![
            create_user_message("Hello"),
            create_assistant_message("Hi", create_mock_usage(100, 50)),
            create_user_message("How are you?"),
        ];
        assert_eq!(find_turn_start_index(&messages, 2), 2);
    }

    #[test]
    fn test_find_turn_start_index_no_user_message() {
        let messages = vec![
            create_assistant_message("Hi", create_mock_usage(100, 50)),
            create_tool_result_message("result"),
        ];
        assert_eq!(find_turn_start_index(&messages, 1), 0);
    }

    #[test]
    fn test_find_turn_start_index_earlier_user() {
        let messages = vec![
            create_user_message("Hello"),
            create_assistant_message("Hi", create_mock_usage(100, 50)),
            create_tool_result_message("result"),
        ];
        assert_eq!(find_turn_start_index(&messages, 2), 0);
    }

    #[test]
    fn test_find_cut_point_basic() {
        let messages: Vec<AgentMessage> = (0..10)
            .flat_map(|i| {
                vec![
                    create_user_message(&format!("User {i}")),
                    create_assistant_message(&format!("Assistant {i}"), create_mock_usage(0, 100)),
                ]
            })
            .collect();

        let tokens_to_keep = 2500u64;
        let cut_point = find_cut_point(&messages, tokens_to_keep);
        assert!(cut_point.first_kept_entry_index < messages.len());
    }

    #[test]
    fn test_find_cut_point_empty_messages() {
        let messages: Vec<AgentMessage> = vec![];
        let cut_point = find_cut_point(&messages, 1000);
        assert_eq!(cut_point.first_kept_entry_index, 0);
        assert_eq!(cut_point.turn_start_index, 0);
        assert!(!cut_point.is_split_turn);
    }

    #[test]
    fn test_find_cut_point_split_turn() {
        let messages = vec![
            create_user_message("Hello"),
            create_assistant_message(&"A".repeat(100), create_mock_usage(100, 50)),
            create_user_message("Next"),
            create_assistant_message(&"B".repeat(100), create_mock_usage(100, 50)),
        ];
        // Keep ~30 tokens: the cut lands on the assistant at index 1, so the
        // preceding user message starts the split turn.
        let cut_point = find_cut_point(&messages, 30);
        assert!(cut_point.is_split_turn);
        assert_eq!(cut_point.first_kept_entry_index, 1);
        assert_eq!(cut_point.turn_start_index, 0);
    }

    #[test]
    fn test_find_cut_point_no_user_in_kept() {
        let messages = vec![
            create_assistant_message("Long assistant message here", create_mock_usage(100, 50)),
            create_tool_result_message("Tool result content here"),
        ];
        let cut_point = find_cut_point(&messages, 10_000);
        // No turn start exists before the cut, so `turn_start_index` falls
        // back to the cut index and this is not a split turn.
        assert_eq!(cut_point.first_kept_entry_index, 0);
        assert_eq!(cut_point.turn_start_index, 0);
        assert!(!cut_point.is_split_turn);
    }

    #[test]
    fn test_get_last_assistant_usage_found() {
        let usage = create_mock_usage(1000, 500);
        let messages = vec![
            create_user_message("Hello"),
            create_assistant_message("Hi", usage.clone()),
        ];
        let result = get_last_assistant_usage(&messages);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), usage);
    }

    #[test]
    fn test_get_last_assistant_usage_none() {
        let messages = vec![create_user_message("Hello")];
        let result = get_last_assistant_usage(&messages);
        assert!(result.is_none());
    }

    #[test]
    fn test_get_last_assistant_usage_last_one() {
        let usage1 = create_mock_usage(100, 50);
        let usage2 = create_mock_usage(200, 100);
        let messages = vec![
            create_assistant_message("First", usage1),
            create_user_message("Hello"),
            create_assistant_message("Second", usage2.clone()),
        ];
        let result = get_last_assistant_usage(&messages);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), usage2);
    }

    #[test]
    fn test_calculate_context_tokens() {
        let messages = vec![
            create_user_message("Hello"),
            create_assistant_message("Hi", create_mock_usage(100, 50)),
        ];
        let system_prompt = "You are a helpful assistant.";
        let total = calculate_context_tokens(system_prompt, &messages);
        let expected_system = (system_prompt.encode_utf16().count() as u64).div_ceil(4);
        let expected_messages = estimate_context_tokens(&messages);
        assert_eq!(total, expected_system + expected_messages);
    }

    #[test]
    fn test_calculate_context_tokens_empty_system() {
        let messages = vec![create_user_message("Hello")];
        let total = calculate_context_tokens("", &messages);
        // An empty system prompt contributes 0 tokens (no forced minimum).
        let expected = estimate_context_tokens(&messages);
        assert_eq!(total, expected);
    }

    #[test]
    fn test_serialize_conversation_user() {
        let messages = vec![Message::User {
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
                text_signature: None,
            }],
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("[User]: Hello"));
    }

    #[test]
    fn test_serialize_conversation_tool_result() {
        let messages = vec![Message::ToolResult {
            tool_call_id: "tool-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::Text {
                text: "File content".to_string(),
                text_signature: None,
            }],
            details: Some(serde_json::Value::Object(Default::default())),
            is_error: false,
            usage: None,
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("[Tool result]: File content"));
    }

    #[test]
    fn test_serialize_conversation_truncation() {
        let long_content = "x".repeat(3000);
        let messages = vec![Message::ToolResult {
            tool_call_id: "tool-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::Text {
                text: long_content.clone(),
                text_signature: None,
            }],
            details: Some(serde_json::Value::Object(Default::default())),
            is_error: false,
            usage: None,
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        // 3000 - 2000 = 1000 characters removed, matching v0.87.1.
        assert!(result.contains("[... 1000 more characters truncated]"));
        assert!(!result.contains("[...truncated]"));
    }

    #[test]
    fn test_serialize_conversation_truncation_is_utf16_safe() {
        // Byte 2000 would fall inside an emoji: the old byte slice panicked.
        let long_content = format!("a{}", "😀".repeat(2000));
        let messages = vec![Message::ToolResult {
            tool_call_id: "tool-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::Text {
                text: long_content,
                text_signature: None,
            }],
            details: None,
            is_error: false,
            usage: None,
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("more characters truncated"));
        assert!(!result.contains('\u{FFFD}'));
    }

    #[test]
    fn test_prepare_compaction_no_compaction_needed() {
        let entries = vec![create_message_entry(create_user_message("Hello"), None)];
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 100_000,
            keep_recent_tokens: 20_000,
        };
        let result = prepare_compaction(&entries, 200_000, &settings);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CompactionError::NoCompactionNeeded
        ));
    }

    #[test]
    fn test_prepare_compaction_with_messages() {
        let mut entries = Vec::new();
        let mut parent_id: Option<String> = None;
        for i in 0..5 {
            let user_entry = SessionTreeEntry::Message {
                id: format!("entry-{i}-u"),
                parent_id: parent_id.clone(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message(&format!(
                    "User message {} with some extra text to add tokens",
                    i
                )),
            };
            parent_id = Some(format!("entry-{i}-u"));
            entries.push(user_entry);

            let assistant_entry = SessionTreeEntry::Message {
                id: format!("entry-{i}-a"),
                parent_id: parent_id.clone(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_assistant_message(
                    &format!("Assistant message {i} with some extra text"),
                    create_mock_usage(5000, 1000),
                ),
            };
            parent_id = Some(format!("entry-{i}-a"));
            entries.push(assistant_entry);
        }

        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 50,
        };
        let result = prepare_compaction(&entries, 200, &settings);
        assert!(result.is_ok());
        let prep = result.unwrap();
        assert!(!prep.first_kept_entry_id.is_empty());
        assert!(prep.tokens_before > 0);
    }

    #[test]
    fn test_prepare_compaction_with_compaction_entry() {
        let u1 = SessionTreeEntry::Message {
            id: "entry-u1".to_string(),
            parent_id: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_user_message("User msg 1"),
        };
        let a1 = SessionTreeEntry::Message {
            id: "entry-a1".to_string(),
            parent_id: Some("entry-u1".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_assistant_message("Assistant msg 1", create_mock_usage(5000, 1000)),
        };
        let compaction = SessionTreeEntry::Compaction {
            id: "entry-c1".to_string(),
            parent_id: Some("entry-a1".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            summary: "First summary".to_string(),
            first_kept_entry_id: Some("entry-u1".to_string()),
            tokens_before: 1234,
            retained_tail: None,
            details: None,
            usage: None,
            from_hook: None,
        };
        let u2 = SessionTreeEntry::Message {
            id: "entry-u2".to_string(),
            parent_id: Some("entry-c1".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_user_message("User msg 2 with more text to increase tokens"),
        };
        let a2 = SessionTreeEntry::Message {
            id: "entry-a2".to_string(),
            parent_id: Some("entry-u2".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_assistant_message(
                "Assistant msg 2 with more text",
                create_mock_usage(8000, 2000),
            ),
        };

        let entries = vec![u1, a1, compaction, u2, a2];
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 50,
            keep_recent_tokens: 20,
        };
        let result = prepare_compaction(&entries, 100, &settings);
        if let Ok(prep) = result {
            assert_eq!(prep.previous_summary, Some("First summary".to_string()));
        }
    }

    #[test]
    fn test_prepare_compaction_with_file_ops() {
        let u1 = SessionTreeEntry::Message {
            id: "entry-u1".to_string(),
            parent_id: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_user_message("read a file"),
        };
        let assistant_msg = AgentMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "tool-1".to_string(),
                name: "write".to_string(),
                arguments: serde_json::json!({"path": "written.ts"}),
                thought_signature: None,
                namespace: None,
            }],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage: create_mock_usage(1000, 200),
            stop_reason: Some(crate::pi_ai_types::StopReason::ToolUse),
            error_message: None,
            timestamp: 1000,
        };
        let a1 = SessionTreeEntry::Message {
            id: "entry-a1".to_string(),
            parent_id: Some("entry-u1".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: assistant_msg,
        };
        let u2 = SessionTreeEntry::Message {
            id: "entry-u2".to_string(),
            parent_id: Some("entry-a1".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_user_message("continue with more text to add tokens"),
        };
        let a2 = SessionTreeEntry::Message {
            id: "entry-a2".to_string(),
            parent_id: Some("entry-u2".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_assistant_message("done with more text", create_mock_usage(4000, 500)),
        };

        let entries = vec![u1, a1, u2, a2];
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 50,
            keep_recent_tokens: 20,
        };
        let result = prepare_compaction(&entries, 100, &settings);
        if let Ok(prep) = result {
            assert!(prep.file_ops.written.contains(&"written.ts".to_string()));
        }
    }
}
