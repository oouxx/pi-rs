use crate::harness::types::{
    CompactionError, CompactionPreparation, CompactionSettings, FileOperations, SessionTreeEntry,
};
use crate::pi_ai_types::Message;
use crate::types::AgentMessage;

pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let text = match message {
        AgentMessage::User { content, .. } => content
            .iter()
            .map(|b| match b {
                crate::pi_ai_types::ContentBlock::Text { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .map(|b| match b {
                crate::pi_ai_types::ContentBlock::Text { text, .. } => text.clone(),
                crate::pi_ai_types::ContentBlock::ToolCall { name, arguments, .. } => {
                    format!("{} {}", name, arguments)
                }
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        AgentMessage::ToolResult { content, .. } => content
            .iter()
            .map(|b| match b {
                crate::pi_ai_types::ContentBlock::Text { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        AgentMessage::BashExecution {
            command, output, ..
        } => format!("{} {}", command, output),
        AgentMessage::Custom { content, .. } => match content {
            crate::types::CustomContent::Text(t) => t.clone(),
            crate::types::CustomContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    crate::pi_ai_types::ContentBlock::Text { text, .. } => text.clone(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        },
        AgentMessage::BranchSummary { summary, .. } => summary.clone(),
        AgentMessage::CompactionSummary { summary, .. } => summary.clone(),
    };

    (text.len() as u64 / 4).max(1)
}

pub fn estimate_context_tokens(messages: &[AgentMessage]) -> u64 {
    messages.iter().map(|m| estimate_tokens(m)).sum()
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

pub fn find_cut_point(
    messages: &[AgentMessage],
    tokens_to_keep: u64,
) -> CutPoint {
    let mut accumulated_tokens = 0u64;
    let mut first_kept_entry_index = messages.len();
    let mut turn_start_index = messages.len();

    for i in (0..messages.len()).rev() {
        let msg_tokens = estimate_tokens(&messages[i]);
        if accumulated_tokens + msg_tokens > tokens_to_keep {
            break;
        }
        accumulated_tokens += msg_tokens;
        first_kept_entry_index = i;

        if let AgentMessage::User { .. } = messages[i] {
            turn_start_index = i;
        }
    }

    let is_split_turn = first_kept_entry_index != turn_start_index && turn_start_index < messages.len();

    CutPoint {
        first_kept_entry_index,
        turn_start_index,
        is_split_turn,
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

pub fn calculate_context_tokens(
    system_prompt: &str,
    messages: &[AgentMessage],
) -> u64 {
    let system_tokens = (system_prompt.len() as u64 / 4).max(1);
    system_tokens + estimate_context_tokens(messages)
}

pub fn prepare_compaction(
    entries: &[SessionTreeEntry],
    context_window: u64,
    settings: &CompactionSettings,
) -> std::result::Result<CompactionPreparation, CompactionError> {
    let messages: Vec<AgentMessage> = entries
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Message { message, .. } => Some(message.clone()),
            SessionTreeEntry::Compaction { summary, tokens_before, .. } => {
                Some(AgentMessage::CompactionSummary {
                    summary: summary.clone(),
                    tokens_before: *tokens_before,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                })
            }
            SessionTreeEntry::BranchSummary {
                summary, from_id, ..
            } => Some(AgentMessage::BranchSummary {
                summary: summary.clone(),
                from_id: from_id.clone(),
                timestamp: chrono::Utc::now().timestamp_millis(),
            }),
            _ => None,
        })
        .collect();

    let tokens_before = estimate_context_tokens(&messages);
    let tokens_to_keep = settings.keep_recent_tokens;

    if !should_compact(tokens_before, context_window, settings) {
        return Err(CompactionError::NoCompactionNeeded);
    }

    let cut_point = find_cut_point(&messages, tokens_to_keep);

    let first_kept_entry_id = entries
        .get(cut_point.first_kept_entry_index)
        .map(|e| e.id().to_string())
        .unwrap_or_default();

    let messages_to_summarize: Vec<AgentMessage> = messages[..cut_point.first_kept_entry_index].to_vec();
    let turn_prefix_messages: Vec<AgentMessage> = if cut_point.is_split_turn {
        messages[cut_point.turn_start_index..cut_point.first_kept_entry_index].to_vec()
    } else {
        Vec::new()
    };

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
        is_split_turn: cut_point.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings: settings.clone(),
    })
}

fn extract_file_operations(messages: &[AgentMessage]) -> FileOperations {
    let mut ops = FileOperations::new();
    for msg in messages {
        if let AgentMessage::Assistant { content, .. } = msg {
            for block in content {
                if let crate::pi_ai_types::ContentBlock::ToolCall { name, arguments, .. } = block {
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
                    parts.push(format!("[User]: {}", text));
                }
            }
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
                    parts.push(format!("[Assistant thinking]: {}", thinking_parts.join("\n")));
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
                    let truncated = if text.len() > 2000 {
                        format!("{}[...truncated]", &text[..2000])
                    } else {
                        text.to_string()
                    };
                    parts.push(format!("[Tool result]: {}", truncated));
                }
            }
        }
    }

    parts.join("\n\n")
}

pub async fn generate_summary(
    _messages: &[AgentMessage],
    _model: &crate::pi_ai_types::Model,
    _reserve_tokens: u64,
    _api_key: &str,
    _headers: Option<&std::collections::HashMap<String, String>>,
    _signal: Option<tokio::sync::watch::Receiver<bool>>,
    _custom_instructions: Option<&str>,
    _previous_summary: Option<&str>,
    _thinking_level: Option<crate::pi_ai_types::ThinkingLevel>,
) -> std::result::Result<String, CompactionError> {
    Ok("Compaction summary placeholder".to_string())
}

pub async fn compact(
    preparation: CompactionPreparation,
    model: &crate::pi_ai_types::Model,
    api_key: &str,
    headers: Option<&std::collections::HashMap<String, String>>,
    custom_instructions: Option<&str>,
    signal: Option<tokio::sync::watch::Receiver<bool>>,
    thinking_level: Option<crate::pi_ai_types::ThinkingLevel>,
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
    )
    .await?;

    Ok(crate::harness::types::CompactResult {
        summary,
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        tokens_before: preparation.tokens_before,
        details: None,
    })
}