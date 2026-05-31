

use crate::pi_ai_types::{ContentBlock, Message};
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
                usage: usage.clone(),
                stop_reason: stop_reason.clone(),
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
                details: details.clone(),
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