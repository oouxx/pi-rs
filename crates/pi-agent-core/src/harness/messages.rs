

use crate::pi_ai_types::{ContentBlock, Message, StopReason};
use crate::types::AgentMessage;

pub const COMPACTION_SUMMARY_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:\n<summary>\n";
pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";
pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n<summary>\n";
pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

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

pub fn create_branch_summary_message(summary: String, from_id: String, timestamp: i64) -> AgentMessage {
    AgentMessage::BranchSummary {
        summary,
        from_id,
        timestamp,
    }
}

pub fn create_compaction_summary_message(summary: String, tokens_before: u64, timestamp: i64) -> AgentMessage {
    AgentMessage::CompactionSummary {
        summary,
        tokens_before,
        timestamp,
    }
}

pub fn create_custom_message(
    custom_type: String,
    content: crate::types::CustomContent,
    display: bool,
    details: Option<serde_json::Value>,
    timestamp: i64,
) -> AgentMessage {
    AgentMessage::Custom {
        custom_type,
        content,
        display,
        details,
        timestamp,
    }
}

pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|m| match m {
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
                if *exclude_from_context == Some(true) {
                    None
                } else {
                    let text = bash_execution_to_text(
                        command,
                        output,
                        *exit_code,
                        *cancelled,
                        *truncated,
                        full_output_path.as_deref(),
                    );
                    Some(Message::User {
                        content: vec![ContentBlock::Text {
                            text,
                            text_signature: None,
                        }],
                        timestamp: *timestamp,
                    })
                }
            }
            AgentMessage::Custom {
                content,
                timestamp,
                ..
            } => {
                let blocks = match content {
                    crate::types::CustomContent::Text(t) => vec![ContentBlock::Text {
                        text: t.clone(),
                        text_signature: None,
                    }],
                    crate::types::CustomContent::Blocks(blocks) => blocks.clone(),
                };
                Some(Message::User {
                    content: blocks,
                    timestamp: *timestamp,
                })
            }
            AgentMessage::BranchSummary {
                summary,
                timestamp,
                ..
            } => Some(Message::User {
                content: vec![ContentBlock::Text {
                    text: format!(
                        "{}{}{}",
                        BRANCH_SUMMARY_PREFIX, summary, BRANCH_SUMMARY_SUFFIX
                    ),
                    text_signature: None,
                }],
                timestamp: *timestamp,
            }),
            AgentMessage::CompactionSummary {
                summary,
                timestamp,
                ..
            } => Some(Message::User {
                content: vec![ContentBlock::Text {
                    text: format!(
                        "{}{}{}",
                        COMPACTION_SUMMARY_PREFIX, summary, COMPACTION_SUMMARY_SUFFIX
                    ),
                    text_signature: None,
                }],
                timestamp: *timestamp,
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_ai_types::Usage;

    fn create_user_message(text: &str) -> AgentMessage {
        AgentMessage::User {
            content: vec![ContentBlock::Text { text: text.to_string(), text_signature: None }],
            timestamp: 1000,
        }
    }

    fn create_assistant_message(text: &str) -> AgentMessage {
        AgentMessage::Assistant {
            content: vec![ContentBlock::Text { text: text.to_string(), text_signature: None }],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage: Usage::default(),
            stop_reason: None,
            error_message: None,
            timestamp: 1000,
        }
    }

    #[test]
    fn test_bash_execution_to_text_with_output() {
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
    fn test_bash_execution_to_text_no_output() {
        let text = bash_execution_to_text("true", "", Some(0), false, false, None);
        assert!(text.contains("(no output)"));
    }

    #[test]
    fn test_bash_execution_to_text_error_exit_code() {
        let text = bash_execution_to_text("false", "error output", Some(1), false, false, None);
        assert!(text.contains("Command exited with code 1"));
    }

    #[test]
    fn test_bash_execution_to_text_zero_exit_code() {
        let text = bash_execution_to_text("echo ok", "ok", Some(0), false, false, None);
        assert!(!text.contains("Command exited"));
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
    fn test_bash_execution_to_text_cancelled_takes_precedence_over_exit_code() {
        let text = bash_execution_to_text("cmd", "out", Some(1), true, false, None);
        assert!(text.contains("(command cancelled)"));
        assert!(!text.contains("Command exited with code 1"));
    }

    #[test]
    fn test_convert_to_llm_user_message() {
        let messages = vec![create_user_message("Hello")];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        if let Message::User { content, .. } = &result[0] {
            if let ContentBlock::Text { text, .. } = &content[0] {
                assert_eq!(text, "Hello");
            }
        }
    }

    #[test]
    fn test_convert_to_llm_assistant_message() {
        let messages = vec![create_assistant_message("Hi there")];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        if let Message::Assistant { content, model, .. } = &result[0] {
            if let ContentBlock::Text { text, .. } = &content[0] {
                assert_eq!(text, "Hi there");
            }
            assert_eq!(model, "claude-sonnet-4-5");
        }
    }

    #[test]
    fn test_convert_to_llm_tool_result_message() {
        let messages = vec![AgentMessage::ToolResult {
            tool_call_id: "tc-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::Text { text: "file contents".to_string(), text_signature: None }],
            details: serde_json::Value::Object(Default::default()),
            is_error: false,
            timestamp: 1000,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        if let Message::ToolResult { tool_call_id, tool_name, .. } = &result[0] {
            assert_eq!(tool_call_id, "tc-1");
            assert_eq!(tool_name, "read");
        }
    }

    #[test]
    fn test_convert_to_llm_excludes_bash_with_exclude_flag() {
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
    fn test_convert_to_llm_includes_bash_without_exclude_flag() {
        let messages = vec![AgentMessage::BashExecution {
            command: "echo hello".to_string(),
            output: "hello".to_string(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 1000,
            exclude_from_context: None,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        if let Message::User { content, .. } = &result[0] {
            if let ContentBlock::Text { text, .. } = &content[0] {
                assert!(text.contains("echo hello"));
            }
        }
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
                assert!(text.contains(BRANCH_SUMMARY_SUFFIX.trim()));
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
                assert!(text.contains(COMPACTION_SUMMARY_PREFIX.trim()));
                assert!(text.contains(COMPACTION_SUMMARY_SUFFIX.trim()));
            }
        }
    }

    #[test]
    fn test_convert_to_llm_custom_text() {
        let messages = vec![AgentMessage::Custom {
            custom_type: "note".to_string(),
            content: crate::types::CustomContent::Text("Hello".to_string()),
            display: true,
            details: None,
            timestamp: 1000,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        if let Message::User { content, .. } = &result[0] {
            if let ContentBlock::Text { text, .. } = &content[0] {
                assert_eq!(text, "Hello");
            }
        }
    }

    #[test]
    fn test_convert_to_llm_custom_blocks() {
        let messages = vec![AgentMessage::Custom {
            custom_type: "note".to_string(),
            content: crate::types::CustomContent::Blocks(vec![ContentBlock::Text { text: "Block content".to_string(), text_signature: None }]),
            display: true,
            details: None,
            timestamp: 1000,
        }];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 1);
        if let Message::User { content, .. } = &result[0] {
            if let ContentBlock::Text { text, .. } = &content[0] {
                assert_eq!(text, "Block content");
            }
        }
    }

    #[test]
    fn test_convert_to_llm_mixed_messages() {
        let messages = vec![
            create_user_message("Hello"),
            create_assistant_message("Hi"),
            create_user_message("How are you?"),
        ];
        let result = convert_to_llm(&messages);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_create_branch_summary_message() {
        let msg = create_branch_summary_message("Summary".to_string(), "from-id".to_string(), 1234);
        match msg {
            AgentMessage::BranchSummary { summary, from_id, timestamp } => {
                assert_eq!(summary, "Summary");
                assert_eq!(from_id, "from-id");
                assert_eq!(timestamp, 1234);
            }
            _ => panic!("Expected BranchSummary"),
        }
    }

    #[test]
    fn test_create_compaction_summary_message() {
        let msg = create_compaction_summary_message("Summary".to_string(), 50000, 1234);
        match msg {
            AgentMessage::CompactionSummary { summary, tokens_before, timestamp } => {
                assert_eq!(summary, "Summary");
                assert_eq!(tokens_before, 50000);
                assert_eq!(timestamp, 1234);
            }
            _ => panic!("Expected CompactionSummary"),
        }
    }

    #[test]
    fn test_create_custom_message() {
        let msg = create_custom_message(
            "note".to_string(),
            crate::types::CustomContent::Text("Hello".to_string()),
            true,
            None,
            1234,
        );
        match msg {
            AgentMessage::Custom { custom_type, content, display, details, timestamp } => {
                assert_eq!(custom_type, "note");
                assert!(display);
                assert!(details.is_none());
                assert_eq!(timestamp, 1234);
                if let crate::types::CustomContent::Text(t) = &content {
                    assert_eq!(t, "Hello");
                }
            }
            _ => panic!("Expected Custom"),
        }
    }
}