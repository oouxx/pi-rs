use crate::harness::compaction::compaction::{estimate_tokens, serialize_conversation};
use crate::harness::messages::{
    create_branch_summary_message, create_compaction_summary_message, create_custom_message,
};
use crate::harness::types::{
    BranchSummaryError, BranchSummaryResult, FileOperations, GenerateBranchSummaryOptions,
    SessionTreeEntry,
};
use crate::pi_ai_types::ContentBlock;
use crate::types::AgentMessage;

use pi_ai::env_api_keys::get_env_api_key;
use pi_ai::stream::stream as pi_stream;

pub struct BranchPreparation {
    pub messages: Vec<AgentMessage>,
    pub file_ops: FileOperations,
    pub total_tokens: u64,
}

fn get_message_from_entry(entry: &SessionTreeEntry) -> Option<AgentMessage> {
    match entry {
        SessionTreeEntry::Message { message, .. } => {
            if let AgentMessage::ToolResult { .. } = message {
                None
            } else {
                Some(message.clone())
            }
        }
        SessionTreeEntry::CustomMessage {
            custom_type,
            content,
            display,
            details,
            timestamp: _,
            ..
        } => {
            let custom_content = match content {
                serde_json::Value::String(s) => crate::types::CustomContent::Text(s.clone()),
                _ => crate::types::CustomContent::Text(content.to_string()),
            };
            Some(create_custom_message(
                custom_type.clone(),
                custom_content,
                *display,
                details.clone(),
                chrono::Utc::now().timestamp_millis(),
            ))
        }
        SessionTreeEntry::BranchSummary {
            summary, from_id, ..
        } => Some(create_branch_summary_message(
            summary.clone(),
            from_id.clone(),
            chrono::Utc::now().timestamp_millis(),
        )),
        SessionTreeEntry::Compaction {
            summary,
            tokens_before,
            ..
        } => Some(create_compaction_summary_message(
            summary.clone(),
            *tokens_before,
            chrono::Utc::now().timestamp_millis(),
        )),
        _ => None,
    }
}

pub fn prepare_branch_entries(
    entries: &[SessionTreeEntry],
    token_budget: u64,
) -> BranchPreparation {
    let mut messages = Vec::new();
    let file_ops = FileOperations::new();
    let mut total_tokens = 0u64;

    for entry in entries.iter().rev() {
        let message = match get_message_from_entry(entry) {
            Some(m) => m,
            None => continue,
        };

        let tokens = estimate_tokens(&message);
        if token_budget > 0 && total_tokens + tokens > token_budget {
            break;
        }
        messages.insert(0, message);
        total_tokens += tokens;
    }

    BranchPreparation {
        messages,
        file_ops,
        total_tokens,
    }
}

const BRANCH_SUMMARY_PREAMBLE: &str =
    "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n";

const BRANCH_SUMMARY_PROMPT: &str = r#"Create a structured summary of this conversation branch for context when returning later.
Use this EXACT format:
## Goal
[What was the user trying to accomplish in this branch?]
## Constraints & Preferences
- [Any constraints preferences or requirements mentioned]
- [Or "(none)" if none were mentioned]
## Progress
### Done
- [x] [Completed tasks/changes]
### In Progress
- [ ] [Work that was started but not finished]
### Blocked
- [Issues preventing progress if any]
## Key Decisions
- **[Decision]**: [Brief rationale]
## Next Steps
1. [What should happen next to continue this work]
Keep each section concise. Preserve exact file paths function names and error messages."#;

/// Generate a branch summary by calling the LLM.
///
/// Sends the serialized conversation to the configured model and extracts a
/// structured summary of the branch content.
pub async fn generate_branch_summary(
    entries: &[SessionTreeEntry],
    options: &GenerateBranchSummaryOptions,
) -> std::result::Result<BranchSummaryResult, BranchSummaryError> {
    let context_window = options.model.context_window.max(128000);
    let reserve_tokens = options.reserve_tokens.unwrap_or(16384);
    let token_budget = context_window.saturating_sub(reserve_tokens);

    let preparation = prepare_branch_entries(entries, token_budget);

    if preparation.messages.is_empty() {
        return Ok(BranchSummaryResult {
            summary: "No content to summarize".to_string(),
            read_files: Vec::new(),
            modified_files: Vec::new(),
        });
    }

    let llm_messages = crate::harness::messages::convert_to_llm(&preparation.messages);
    let conversation_text = serialize_conversation(&llm_messages);

    let instructions =
        if options.replace_instructions.unwrap_or(false) && options.custom_instructions.is_some() {
            options.custom_instructions.clone().unwrap()
        } else if let Some(custom) = &options.custom_instructions {
            format!("{}\n\nAdditional focus: {}", BRANCH_SUMMARY_PROMPT, custom)
        } else {
            BRANCH_SUMMARY_PROMPT.to_string()
        };

    let prompt_text = format!(
        "<conversation>\n{}\n</conversation>\n\n{}",
        conversation_text, instructions
    );

    let summarization_messages = vec![crate::pi_ai_types::Message::User {
        content: vec![ContentBlock::Text {
            text: prompt_text,
            text_signature: None,
        }],
        timestamp: chrono::Utc::now().timestamp_millis(),
    }];

    // Call the LLM to generate the summary
    let api_key = get_env_api_key(&options.model.provider).unwrap_or_default();

    let context = crate::pi_ai_types::Context {
        system_prompt: Some(
            "You are a code context summarizer. Follow the user instructions exactly and produce a structured summary."
                .to_string(),
        ),
        messages: summarization_messages,
        tools: None,
    };

    let stream_options = crate::pi_ai_types::StreamOptions {
        api_key: if api_key.is_empty() {
            None
        } else {
            Some(api_key)
        },
        ..Default::default()
    };

    // Try to call the LLM; fall back to a basic placeholder on failure
    let summary = match pi_stream(&options.model, &context, Some(stream_options))
        .result()
        .await
    {
        Ok(response) => {
            let text = response
                .content
                .iter()
                .filter_map(|b| match b {
                    crate::pi_ai_types::ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            if text.trim().is_empty() {
                format!("{}{}", BRANCH_SUMMARY_PREAMBLE, "No summary generated")
            } else {
                format!("{}{}", BRANCH_SUMMARY_PREAMBLE, text)
            }
        }
        Err(_) => {
            format!(
                "{}Branch summary unavailable ({} entries)",
                BRANCH_SUMMARY_PREAMBLE,
                preparation.messages.len()
            )
        }
    };

    let mut read_files = preparation.file_ops.read.clone();
    let mut modified_files = preparation.file_ops.written.clone();
    modified_files.extend(preparation.file_ops.edited.clone());
    read_files.sort();
    modified_files.sort();
    read_files.retain(|f| !modified_files.contains(f));

    Ok(BranchSummaryResult {
        summary,
        read_files,
        modified_files,
    })
}

pub fn collect_entries_for_branch_summary(
    entries: &[SessionTreeEntry],
) -> (Vec<SessionTreeEntry>, FileOperations) {
    let mut file_ops = FileOperations::new();

    for entry in entries {
        if let SessionTreeEntry::BranchSummary {
            details, from_hook, ..
        } = entry
        {
            if !from_hook.unwrap_or(false) {
                if let Some(ref d) = details {
                    if let Some(obj) = d.as_object() {
                        if let Some(read) = obj.get("readFiles").and_then(|v| v.as_array()) {
                            for f in read.iter().filter_map(|v| v.as_str()) {
                                file_ops.read.push(f.to_string());
                            }
                        }
                        if let Some(modified) = obj.get("modifiedFiles").and_then(|v| v.as_array())
                        {
                            for f in modified.iter().filter_map(|v| v.as_str()) {
                                file_ops.edited.push(f.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    (entries.to_vec(), file_ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::types::SessionTreeEntry;
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

    fn create_assistant_message(text: &str) -> AgentMessage {
        AgentMessage::Assistant {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage: Usage::default(),
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
            timestamp: 1000,
        }
    }

    #[test]
    fn test_prepare_branch_entries_basic() {
        let entries = vec![
            SessionTreeEntry::Message {
                id: "e1".to_string(),
                parent_id: None,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message("Hello"),
            },
            SessionTreeEntry::Message {
                id: "e2".to_string(),
                parent_id: Some("e1".to_string()),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_assistant_message("Hi"),
            },
        ];
        let prep = prepare_branch_entries(&entries, 0);
        assert_eq!(prep.messages.len(), 2);
        assert!(prep.total_tokens > 0);
    }

    #[test]
    fn test_prepare_branch_entries_with_token_budget() {
        let entries = vec![
            SessionTreeEntry::Message {
                id: "e1".to_string(),
                parent_id: None,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message("Hello world this is a longer message"),
            },
            SessionTreeEntry::Message {
                id: "e2".to_string(),
                parent_id: Some("e1".to_string()),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_assistant_message("Short reply"),
            },
            SessionTreeEntry::Message {
                id: "e3".to_string(),
                parent_id: Some("e2".to_string()),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message("Another message here"),
            },
        ];
        let prep = prepare_branch_entries(&entries, 3);
        assert!(prep.messages.len() < entries.len());
        assert!(prep.total_tokens <= 3);
    }

    #[test]
    fn test_prepare_branch_entries_skips_tool_results() {
        let entries = vec![
            SessionTreeEntry::Message {
                id: "e1".to_string(),
                parent_id: None,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message("Hello"),
            },
            SessionTreeEntry::Message {
                id: "e2".to_string(),
                parent_id: Some("e1".to_string()),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_tool_result_message("result"),
            },
        ];
        let prep = prepare_branch_entries(&entries, 0);
        assert_eq!(prep.messages.len(), 1);
    }

    #[test]
    fn test_prepare_branch_entries_with_compaction() {
        let entries = vec![
            SessionTreeEntry::Compaction {
                id: "e1".to_string(),
                parent_id: None,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                summary: "Previous summary".to_string(),
                first_kept_entry_id: "e0".to_string(),
                tokens_before: 1000,
                details: None,
                from_hook: None,
            },
            SessionTreeEntry::Message {
                id: "e2".to_string(),
                parent_id: Some("e1".to_string()),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message("Hello"),
            },
        ];
        let prep = prepare_branch_entries(&entries, 0);
        assert_eq!(prep.messages.len(), 2);
        assert!(matches!(
            prep.messages[0],
            AgentMessage::CompactionSummary { .. }
        ));
    }

    #[test]
    fn test_prepare_branch_entries_with_branch_summary() {
        let entries = vec![
            SessionTreeEntry::BranchSummary {
                id: "e1".to_string(),
                parent_id: None,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                from_id: "branch-id".to_string(),
                summary: "Branch summary".to_string(),
                details: None,
                from_hook: None,
            },
            SessionTreeEntry::Message {
                id: "e2".to_string(),
                parent_id: Some("e1".to_string()),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message("Hello"),
            },
        ];
        let prep = prepare_branch_entries(&entries, 0);
        assert_eq!(prep.messages.len(), 2);
        assert!(matches!(
            prep.messages[0],
            AgentMessage::BranchSummary { .. }
        ));
    }

    #[test]
    fn test_prepare_branch_entries_empty() {
        let entries: Vec<SessionTreeEntry> = vec![];
        let prep = prepare_branch_entries(&entries, 0);
        assert!(prep.messages.is_empty());
        assert_eq!(prep.total_tokens, 0);
    }

    #[test]
    fn test_prepare_branch_entries_preserves_order() {
        let entries = vec![
            SessionTreeEntry::Message {
                id: "e1".to_string(),
                parent_id: None,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_user_message("First"),
            },
            SessionTreeEntry::Message {
                id: "e2".to_string(),
                parent_id: Some("e1".to_string()),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: create_assistant_message("Second"),
            },
        ];
        let prep = prepare_branch_entries(&entries, 0);
        match &prep.messages[0] {
            AgentMessage::User { content, .. } => {
                if let ContentBlock::Text { text, .. } = &content[0] {
                    assert_eq!(text, "First");
                }
            }
            _ => panic!("Expected User message first"),
        }
    }

    #[test]
    fn test_collect_entries_for_branch_summary_basic() {
        let entries = vec![SessionTreeEntry::Message {
            id: "e1".to_string(),
            parent_id: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: create_user_message("Hello"),
        }];
        let (result_entries, file_ops) = collect_entries_for_branch_summary(&entries);
        assert_eq!(result_entries.len(), 1);
        assert!(file_ops.read.is_empty());
        assert!(file_ops.written.is_empty());
        assert!(file_ops.edited.is_empty());
    }

    #[test]
    fn test_collect_entries_for_branch_summary_with_details() {
        let entries = vec![SessionTreeEntry::BranchSummary {
            id: "e1".to_string(),
            parent_id: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            from_id: "branch-id".to_string(),
            summary: "Summary".to_string(),
            details: Some(serde_json::json!({
                "readFiles": ["read1.rs", "read2.rs"],
                "modifiedFiles": ["mod1.rs"]
            })),
            from_hook: Some(false),
        }];
        let (_, file_ops) = collect_entries_for_branch_summary(&entries);
        assert_eq!(file_ops.read, vec!["read1.rs", "read2.rs"]);
        assert_eq!(file_ops.edited, vec!["mod1.rs"]);
    }

    #[test]
    fn test_collect_entries_for_branch_summary_skips_hook_entries() {
        let entries = vec![SessionTreeEntry::BranchSummary {
            id: "e1".to_string(),
            parent_id: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            from_id: "branch-id".to_string(),
            summary: "Summary".to_string(),
            details: Some(serde_json::json!({
                "readFiles": ["read1.rs"],
                "modifiedFiles": ["mod1.rs"]
            })),
            from_hook: Some(true),
        }];
        let (_, file_ops) = collect_entries_for_branch_summary(&entries);
        assert!(file_ops.read.is_empty());
        assert!(file_ops.edited.is_empty());
    }

    #[test]
    fn test_collect_entries_for_branch_summary_no_details() {
        let entries = vec![SessionTreeEntry::BranchSummary {
            id: "e1".to_string(),
            parent_id: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            from_id: "branch-id".to_string(),
            summary: "Summary".to_string(),
            details: None,
            from_hook: Some(false),
        }];
        let (_, file_ops) = collect_entries_for_branch_summary(&entries);
        assert!(file_ops.read.is_empty());
    }
}
