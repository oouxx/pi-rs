use crate::harness::compaction::compaction::{estimate_tokens, serialize_conversation};
use crate::harness::messages::{create_branch_summary_message, create_compaction_summary_message, create_custom_message};
use crate::harness::types::{
    BranchSummaryError, BranchSummaryResult, FileOperations,
    GenerateBranchSummaryOptions, SessionTreeEntry,
};
use crate::pi_ai_types::ContentBlock;
use crate::types::AgentMessage;

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

    let instructions = if options.replace_instructions.unwrap_or(false) && options.custom_instructions.is_some() {
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

    let _summarization_messages = vec![crate::pi_ai_types::Message::User {
        content: vec![ContentBlock::Text {
            text: prompt_text,
            text_signature: None,
        }],
        timestamp: chrono::Utc::now().timestamp_millis(),
    }];

    let summary = format!("{}{}", BRANCH_SUMMARY_PREAMBLE, "Branch summary placeholder");

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
            details,
            from_hook,
            ..
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
                        if let Some(modified) = obj.get("modifiedFiles").and_then(|v| v.as_array()) {
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