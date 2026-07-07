use pi_agent_core::pi_ai_types::{ContentBlock, Message, StopReason};
use pi_agent_core::types::{AgentMessage, CustomContent};

/// Normalize message content at ingestion boundaries.
/// Ensures empty/null content is consistently represented.
pub fn normalize_ingested_message(message: &mut Message) {
    match message {
        Message::User { content, .. } => {
            if content.is_empty() {
                *content = vec![ContentBlock::Text {
                    text: String::new(),
                    text_signature: None,
                }];
            }
        }
        Message::Assistant { content, .. } => {
            // Assistant messages with empty content are valid (tool-use only responses)
            // No normalization needed
        }
        _ => {}
    }
}

pub const COMPACTION_SUMMARY_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:\n<summary>\n";
pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";
pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n<summary>\n";
pub const BRANCH_SUMMARY_SUFFIX: &str = "\n</summary>";

pub fn bash_execution_to_text(
    command: &str,
    output: &str,
    exit_code: Option<i32>,
    cancelled: bool,
    truncated: bool,
    full_output_path: Option<&str>,
) -> String {
    let mut text = format!("Ran `{}`\n", command);
    if !output.is_empty() {
        text.push_str(&format!("```\n{}\n```", output));
    } else {
        text.push_str("(no output)");
    }
    if cancelled {
        text.push_str("\n\n(command cancelled)");
    } else if let Some(code) = exit_code {
        if code != 0 {
            text.push_str(&format!("\n\nCommand exited with code {}", code));
        }
    }
    if truncated {
        if let Some(path) = full_output_path {
            text.push_str(&format!("\n\n[Output truncated. Full output: {}]", path));
        }
    }
    text
}

pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    let mut result: Vec<Message> = messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::BashExecution {
                command,
                output,
                exit_code,
                cancelled,
                truncated,
                full_output_path,
                timestamp,
                exclude_from_context,
            } => {
                if exclude_from_context.unwrap_or(false) {
                    return None;
                }
                let text = bash_execution_to_text(
                    command,
                    output,
                    *exit_code,
                    *cancelled,
                    *truncated,
                    full_output_path.as_deref(),
                );
                Some(Message::User {
                    content: vec![ContentBlock::text(text)],
                    timestamp: *timestamp,
                })
            }
            AgentMessage::Custom {
                content, timestamp, ..
            } => {
                let blocks = match content {
                    CustomContent::Text(t) => vec![ContentBlock::text(t)],
                    CustomContent::Blocks(blocks) => blocks.clone(),
                };
                Some(Message::User {
                    content: blocks,
                    timestamp: *timestamp,
                })
            }
            AgentMessage::BranchSummary {
                summary, timestamp, ..
            } => Some(Message::User {
                content: vec![ContentBlock::text(format!(
                    "{}{}{}",
                    BRANCH_SUMMARY_PREFIX, summary, BRANCH_SUMMARY_SUFFIX
                ))],
                timestamp: *timestamp,
            }),
            AgentMessage::CompactionSummary {
                summary, timestamp, ..
            } => Some(Message::User {
                content: vec![ContentBlock::text(format!(
                    "{}{}{}",
                    COMPACTION_SUMMARY_PREFIX, summary, COMPACTION_SUMMARY_SUFFIX
                ))],
                timestamp: *timestamp,
            }),
            AgentMessage::User { content, timestamp } => Some(Message::User {
                content: content.clone(),
                timestamp: *timestamp,
            }),
            AgentMessage::Assistant {
                content,
                api,
                provider,
                model,
                usage,
                stop_reason,
                error_message,
                timestamp,
            } => Some(Message::Assistant {
                content: content.clone(),
                api: api.clone(),
                provider: provider.clone(),
                model: model.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: usage.clone(),
                stop_reason: stop_reason.clone().unwrap_or(StopReason::Error),
                error_message: error_message.clone(),
                timestamp: *timestamp,
            }),
            AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                content,
                details,
                is_error,
                timestamp,
            } => Some(Message::ToolResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                content: content.clone(),
                details: Some(details.clone()),
                is_error: *is_error,
                timestamp: *timestamp,
            }),
        })
        .collect();
    for msg in result.iter_mut() {
        normalize_ingested_message(msg);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_agent_core::pi_ai_types::Usage;

    #[test]
    fn test_bash_execution_to_text() {
        let text = bash_execution_to_text(
            "ls -la",
            "file1.txt\nfile2.txt",
            Some(0),
            false,
            false,
            None,
        );
        assert!(text.contains("Ran `ls -la`"));
        assert!(text.contains("file1.txt"));
        assert!(!text.contains("Command exited"));
    }

    #[test]
    fn test_bash_execution_to_text_error() {
        let text = bash_execution_to_text("false", "error output", Some(1), false, false, None);
        assert!(text.contains("Command exited with code 1"));
    }

    #[test]
    fn test_bash_execution_to_text_cancelled() {
        let text = bash_execution_to_text("sleep 10", "", None, true, false, None);
        assert!(text.contains("(command cancelled)"));
    }

    #[test]
    fn test_bash_execution_to_text_truncated() {
        let text = bash_execution_to_text(
            "cat bigfile",
            "output...",
            Some(0),
            false,
            true,
            Some("/tmp/output.log"),
        );
        assert!(text.contains("Output truncated"));
        assert!(text.contains("/tmp/output.log"));
    }

    #[test]
    fn test_convert_to_llm_excludes_bash() {
        let messages = vec![AgentMessage::BashExecution {
            command: "secret".to_string(),
            output: "hidden".to_string(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 1000,
            exclude_from_context: Some(true),
        }];
        let result = convert_to_llm(&messages);
        assert!(result.is_empty());
    }

    #[test]
    fn test_convert_to_llm_branch_summary() {
        let messages = vec![AgentMessage::BranchSummary {
            summary: "Previous work".to_string(),
            from_id: "id1".to_string(),
            timestamp: 1000,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        if let Message::User { content, .. } = &result[0] {
            if let ContentBlock::Text { text, .. } = &content[0] {
                assert!(text.contains("Previous work"));
                assert!(text.contains(BRANCH_SUMMARY_PREFIX.trim()));
            }
        }
    }

    #[test]
    fn test_convert_to_llm_compaction_summary() {
        let messages = vec![AgentMessage::CompactionSummary {
            summary: "Summarized context".to_string(),
            tokens_before: 50000,
            timestamp: 1000,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        if let Message::User { content, .. } = &result[0] {
            if let ContentBlock::Text { text, .. } = &content[0] {
                assert!(text.contains("Summarized context"));
            }
        }
    }

    #[test]
    fn test_convert_to_llm_custom_text() {
        let messages = vec![AgentMessage::Custom {
            custom_type: "note".to_string(),
            content: CustomContent::Text("Hello".to_string()),
            display: true,
            details: None,
            timestamp: 1000,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
    }

    // --- normalize_ingested_message tests ---

    #[test]
    fn test_normalize_user_message_empty_content() {
        let mut msg = Message::User {
            content: vec![],
            timestamp: 0,
        };
        normalize_ingested_message(&mut msg);
        match &msg {
            Message::User { content, .. } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentBlock::Text { text, .. } => {
                        assert_eq!(text, "");
                    }
                    _ => panic!("Expected Text content block"),
                }
            }
            _ => panic!("Should still be User"),
        }
    }

    #[test]
    fn test_normalize_user_message_non_empty_unchanged() {
        let mut msg = Message::User {
            content: vec![ContentBlock::text("hello")],
            timestamp: 0,
        };
        normalize_ingested_message(&mut msg);
        match &msg {
            Message::User { content, .. } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentBlock::Text { text, .. } => {
                        assert_eq!(text, "hello");
                    }
                    _ => panic!("Expected Text content block"),
                }
            }
            _ => panic!("Should still be User"),
        }
    }

    #[test]
    fn test_normalize_assistant_message_unchanged() {
        let mut msg = Message::Assistant {
            content: vec![],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        };
        normalize_ingested_message(&mut msg);
        match &msg {
            Message::Assistant { content, .. } => {
                // Assistant messages with empty content should remain empty
                // (tool-use only responses are valid with no text content)
                assert_eq!(content.len(), 0);
            }
            _ => panic!("Should still be Assistant"),
        }
    }

    #[test]
    fn test_normalize_tool_result_unchanged() {
        let mut msg = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "test".into(),
            content: vec![],
            details: None,
            is_error: false,
            timestamp: 0,
        };
        normalize_ingested_message(&mut msg);
        match &msg {
            Message::ToolResult { content, .. } => {
                // ToolResult messages should not be normalized
                assert_eq!(content.len(), 0);
            }
            _ => panic!("Should still be ToolResult"),
        }
    }

    #[test]
    fn test_convert_to_llm_normalizes_empty_user_content() {
        let messages = vec![AgentMessage::User {
            content: vec![],
            timestamp: 1000,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        match &result[0] {
            Message::User { content, .. } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentBlock::Text { text, .. } => {
                        assert_eq!(text, "");
                    }
                    _ => panic!("Expected Text content block"),
                }
            }
            _ => panic!("Expected User message"),
        }
    }

    #[test]
    fn test_convert_to_llm_keeps_assistant_empty_content() {
        let messages = vec![AgentMessage::Assistant {
            content: vec![],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            usage: Usage::default(),
            stop_reason: Some(StopReason::ToolUse),
            error_message: None,
            timestamp: 1000,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        match &result[0] {
            Message::Assistant { content, .. } => {
                // Assistant messages with empty content should remain empty
                assert_eq!(content.len(), 0);
            }
            _ => panic!("Expected Assistant message"),
        }
    }
}
