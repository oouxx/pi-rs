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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::types::{CompactionSettings, SessionTreeEntry};
    use crate::pi_ai_types::{ContentBlock, Usage};

    fn create_user_message(text: &str) -> AgentMessage {
        AgentMessage::User {
            content: vec![ContentBlock::text(text)],
            timestamp: 1000,
        }
    }

    fn create_assistant_message(text: &str, usage: Usage) -> AgentMessage {
        AgentMessage::Assistant {
            content: vec![ContentBlock::text(text)],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage,
            stop_reason: Some(crate::pi_ai_types::StopReason::EndTurn),
            error_message: None,
            timestamp: 1000,
        }
    }

    fn create_tool_result_message(text: &str) -> AgentMessage {
        AgentMessage::ToolResult {
            tool_call_id: "tool-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::text(text)],
            details: serde_json::Value::Object(Default::default()),
            is_error: false,
            timestamp: 1000,
        }
    }

    fn create_mock_usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
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
        assert!(tokens > 0);
        assert_eq!(tokens, ("Hello world".len() as u64 / 4).max(1));
    }

    #[test]
    fn test_estimate_tokens_assistant_message() {
        let msg = create_assistant_message("Hi there", create_mock_usage(100, 50));
        let tokens = estimate_tokens(&msg);
        assert!(tokens > 0);
        assert_eq!(tokens, ("Hi there".len() as u64 / 4).max(1));
    }

    #[test]
    fn test_estimate_tokens_tool_result_message() {
        let msg = create_tool_result_message("File contents here");
        let tokens = estimate_tokens(&msg);
        assert!(tokens > 0);
        assert_eq!(tokens, ("File contents here".len() as u64 / 4).max(1));
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
        assert!(tokens > 0);
        let combined = "ls -la file1 file2";
        assert_eq!(tokens, (combined.len() as u64 / 4).max(1));
    }

    #[test]
    fn test_estimate_tokens_custom_text() {
        let msg = AgentMessage::Custom {
            custom_type: "note".to_string(),
            content: crate::types::CustomContent::Text("Custom note".to_string()),
            display: true,
            details: None,
            timestamp: 1000,
        };
        let tokens = estimate_tokens(&msg);
        assert!(tokens > 0);
        assert_eq!(tokens, ("Custom note".len() as u64 / 4).max(1));
    }

    #[test]
    fn test_estimate_tokens_custom_blocks() {
        let msg = AgentMessage::Custom {
            custom_type: "note".to_string(),
            content: crate::types::CustomContent::Blocks(vec![ContentBlock::text("Block content")]),
            display: true,
            details: None,
            timestamp: 1000,
        };
        let tokens = estimate_tokens(&msg);
        assert!(tokens > 0);
        assert_eq!(tokens, ("Block content".len() as u64 / 4).max(1));
    }

    #[test]
    fn test_estimate_tokens_branch_summary() {
        let msg = AgentMessage::BranchSummary {
            summary: "Branch summary text".to_string(),
            from_id: "from-id".to_string(),
            timestamp: 1000,
        };
        let tokens = estimate_tokens(&msg);
        assert!(tokens > 0);
        assert_eq!(tokens, ("Branch summary text".len() as u64 / 4).max(1));
    }

    #[test]
    fn test_estimate_tokens_compaction_summary() {
        let msg = AgentMessage::CompactionSummary {
            summary: "Compaction summary text".to_string(),
            tokens_before: 50000,
            timestamp: 1000,
        };
        let tokens = estimate_tokens(&msg);
        assert!(tokens > 0);
        assert_eq!(tokens, ("Compaction summary text".len() as u64 / 4).max(1));
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
        let expected: u64 = messages.iter().map(|m| estimate_tokens(m)).sum();
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
            reserve_tokens: 10000,
            keep_recent_tokens: 20000,
        };
        assert!(should_compact(95000, 100000, &settings));
    }

    #[test]
    fn test_should_compact_below_threshold() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 10000,
            keep_recent_tokens: 20000,
        };
        assert!(!should_compact(89000, 100000, &settings));
    }

    #[test]
    fn test_should_compact_disabled() {
        let settings = CompactionSettings {
            enabled: false,
            reserve_tokens: 10000,
            keep_recent_tokens: 20000,
        };
        assert!(!should_compact(95000, 100000, &settings));
    }

    #[test]
    fn test_should_compact_at_threshold() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 10000,
            keep_recent_tokens: 20000,
        };
        assert!(!should_compact(90000, 100000, &settings));
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
                    create_user_message(&format!("User {}", i)),
                    create_assistant_message(&format!("Assistant {}", i), create_mock_usage(0, 100)),
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
    fn test_find_cut_point_all_fit() {
        let messages = vec![
            create_user_message("Hi"),
            create_assistant_message("Hello", create_mock_usage(100, 50)),
        ];
        let cut_point = find_cut_point(&messages, 10000);
        assert_eq!(cut_point.first_kept_entry_index, 0);
        assert_eq!(cut_point.turn_start_index, 0);
        assert!(!cut_point.is_split_turn);
    }

    #[test]
    fn test_find_cut_point_split_turn() {
        let messages = vec![
            create_assistant_message("Short reply", create_mock_usage(100, 50)),
            create_user_message("Hello"),
            create_assistant_message("Response", create_mock_usage(100, 50)),
        ];
        let cut_point = find_cut_point(&messages, 10);
        assert!(cut_point.is_split_turn);
        assert_ne!(cut_point.first_kept_entry_index, cut_point.turn_start_index);
        assert!(cut_point.turn_start_index < messages.len());
    }

    #[test]
    fn test_find_cut_point_no_user_in_kept() {
        let messages = vec![
            create_assistant_message("Long assistant message here", create_mock_usage(100, 50)),
            create_tool_result_message("Tool result content here"),
        ];
        let cut_point = find_cut_point(&messages, 10000);
        assert_eq!(cut_point.turn_start_index, messages.len());
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
        let expected_system = (system_prompt.len() as u64 / 4).max(1);
        let expected_messages = estimate_context_tokens(&messages);
        assert_eq!(total, expected_system + expected_messages);
    }

    #[test]
    fn test_calculate_context_tokens_empty_system() {
        let messages = vec![create_user_message("Hello")];
        let total = calculate_context_tokens("", &messages);
        let expected = 1 + estimate_context_tokens(&messages);
        assert_eq!(total, expected);
    }

    #[test]
    fn test_should_compact_saturating_sub() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 200000,
            keep_recent_tokens: 20000,
        };
        assert!(should_compact(100, 100000, &settings));
    }

    #[test]
    fn test_serialize_conversation_user() {
        let messages = vec![Message::User {
            content: vec![ContentBlock::text("Hello")],
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("[User]: Hello"));
    }

    #[test]
    fn test_serialize_conversation_assistant() {
        let messages = vec![Message::Assistant {
            content: vec![ContentBlock::text("Hi there")],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage: create_mock_usage(100, 50),
            stop_reason: Some(crate::pi_ai_types::StopReason::EndTurn),
            error_message: None,
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("[Assistant]: Hi there"));
    }

    #[test]
    fn test_serialize_conversation_tool_result() {
        let messages = vec![Message::ToolResult {
            tool_call_id: "tool-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::text("File content")],
            details: serde_json::Value::Object(Default::default()),
            is_error: false,
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("[Tool result]: File content"));
    }

    #[test]
    fn test_serialize_conversation_thinking() {
        let messages = vec![Message::Assistant {
            content: vec![ContentBlock::Thinking {
                thinking: "Let me think...".to_string(),
                thinking_signature: None,
            }],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage: create_mock_usage(100, 50),
            stop_reason: Some(crate::pi_ai_types::StopReason::EndTurn),
            error_message: None,
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("[Assistant thinking]: Let me think..."));
    }

    #[test]
    fn test_serialize_conversation_tool_call() {
        let messages = vec![Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "tool-1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            }],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage: create_mock_usage(100, 50),
            stop_reason: Some(crate::pi_ai_types::StopReason::EndTurn),
            error_message: None,
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("[Assistant tool calls]:"));
        assert!(result.contains("read("));
    }

    #[test]
    fn test_serialize_conversation_truncation() {
        let long_content = "x".repeat(3000);
        let messages = vec![Message::ToolResult {
            tool_call_id: "tool-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::text(&long_content)],
            details: serde_json::Value::Object(Default::default()),
            is_error: false,
            timestamp: 1000,
        }];
        let result = serialize_conversation(&messages);
        assert!(result.contains("[...truncated]"));
    }

    #[test]
    fn test_prepare_compaction_no_compaction_needed() {
        let entries = vec![create_message_entry(
            create_user_message("Hello"),
            None,
        )];
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 100000,
            keep_recent_tokens: 20000,
        };
        let result = prepare_compaction(&entries, 200000, &settings);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CompactionError::NoCompactionNeeded));
    }

    #[test]
    fn test_prepare_compaction_with_messages() {
        let mut entries = Vec::new();
        let mut parent_id: Option<String> = None;
        for i in 0..5 {
            let user_entry = SessionTreeEntry::Message {
                id: format!("entry-{}-u", i),
                parent_id: parent_id.clone(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message(&format!("User message {} with some extra text to add tokens", i)),
            };
            parent_id = Some(format!("entry-{}-u", i));
            entries.push(user_entry);

            let assistant_entry = SessionTreeEntry::Message {
                id: format!("entry-{}-a", i),
                parent_id: parent_id.clone(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_assistant_message(
                    &format!("Assistant message {} with some extra text", i),
                    create_mock_usage(5000, 1000),
                ),
            };
            parent_id = Some(format!("entry-{}-a", i));
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
            first_kept_entry_id: "entry-u1".to_string(),
            tokens_before: 1234,
            details: None,
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
            message: create_assistant_message("Assistant msg 2 with more text", create_mock_usage(8000, 2000)),
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

    #[test]
    fn test_prepare_compaction_with_branch_summary() {
        let branch = SessionTreeEntry::BranchSummary {
            id: "entry-bs1".to_string(),
            parent_id: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            from_id: "branch-id".to_string(),
            summary: "Branch summary with enough text to contribute tokens to the context window".to_string(),
            details: None,
            from_hook: None,
        };
        let u1 = SessionTreeEntry::Message {
            id: "entry-u1".to_string(),
            parent_id: Some("entry-bs1".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_user_message("User message with enough text to go over the token limit and trigger compaction"),
        };
        let a1 = SessionTreeEntry::Message {
            id: "entry-a1".to_string(),
            parent_id: Some("entry-u1".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_assistant_message("Assistant response with enough text to increase token count significantly", create_mock_usage(5000, 1000)),
        };
        let u2 = SessionTreeEntry::Message {
            id: "entry-u2".to_string(),
            parent_id: Some("entry-a1".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_user_message("Another user message adding more tokens to the context window"),
        };
        let a2 = SessionTreeEntry::Message {
            id: "entry-a2".to_string(),
            parent_id: Some("entry-u2".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_assistant_message("Another assistant response with more text", create_mock_usage(5000, 1000)),
        };

        let entries = vec![branch, u1, a1, u2, a2];
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 30,
            keep_recent_tokens: 10,
        };
        let result = prepare_compaction(&entries, 50, &settings);
        assert!(result.is_ok(), "Expected compaction to be needed, got: {:?}", result);
    }
}