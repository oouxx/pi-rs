use std::collections::HashMap;

use pi_agent_core::pi_ai_types::{ContentBlock, Message};
use pi_agent_core::types::AgentMessage;

use crate::core::messages;

const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a summarization assistant. Your job is to create concise summaries of coding agent conversations.

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

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.
Summarize the prefix to provide context for the retained suffix:
## Original Request
[What did the user ask for in this turn?]
## Early Progress
- [Key decisions and work done in the prefix]
## Context for Suffix
- [Information needed to understand the retained recent work]
Be concise. Focus on what's needed to understand the kept suffix."#;

#[derive(Debug, Clone)]
pub struct CompactionSettings {
    pub reserve_tokens: u64,
    pub compact_on_threshold: bool,
    pub threshold_ratio: f64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            reserve_tokens: 30000,
            compact_on_threshold: true,
            threshold_ratio: 0.8,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactionPreparation {
    pub first_kept_entry_id: Option<String>,
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: u64,
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
    pub settings: CompactionSettings,
}

#[derive(Debug, Clone, Default)]
pub struct FileOperations {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub details: CompactionDetails,
}

#[derive(Debug, Clone)]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CompactionCutPoint {
    pub turn_start_index: usize,
    pub first_kept_entry_index: usize,
    pub is_split_turn: bool,
}

pub fn should_compact(
    total_tokens: u64,
    context_window: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.compact_on_threshold {
        return false;
    }
    let threshold = (context_window as f64 * settings.threshold_ratio) as u64;
    total_tokens >= threshold
}

pub fn find_compaction_cut_point(
    messages: &[AgentMessage],
    keep_recent_turns: usize,
) -> CompactionCutPoint {
    let mut turn_starts: Vec<usize> = Vec::new();

    for (i, msg) in messages.iter().enumerate() {
        match msg {
            AgentMessage::User { .. } => {
                turn_starts.push(i);
            }
            AgentMessage::BranchSummary { .. } | AgentMessage::CompactionSummary { .. } => {
                turn_starts.push(i);
            }
            _ => {}
        }
    }

    if turn_starts.len() <= keep_recent_turns {
        return CompactionCutPoint {
            turn_start_index: 0,
            first_kept_entry_index: 0,
            is_split_turn: false,
        };
    }

    let cut_turn = turn_starts.len() - keep_recent_turns;
    let turn_start_index = turn_starts[cut_turn];

    CompactionCutPoint {
        turn_start_index: 0,
        first_kept_entry_index: turn_start_index,
        is_split_turn: false,
    }
}

pub fn prepare_compaction(
    messages: &[AgentMessage],
    keep_recent_turns: usize,
    settings: CompactionSettings,
) -> CompactionPreparation {
    let cut_point = find_compaction_cut_point(messages, keep_recent_turns);

    let messages_to_summarize: Vec<AgentMessage> = messages[..cut_point.first_kept_entry_index]
        .iter()
        .filter_map(|m| match m {
            AgentMessage::BashExecution {
                exclude_from_context,
                ..
            } if exclude_from_context.unwrap_or(false) => None,
            _ => Some(m.clone()),
        })
        .collect();

    let previous_summary = messages.iter().find_map(|m| match m {
        AgentMessage::CompactionSummary { summary, .. } => Some(summary.clone()),
        _ => None,
    });

    let file_ops = extract_file_operations(messages, None);

    let first_kept_entry_id = messages
        .get(cut_point.first_kept_entry_index)
        .and_then(|_| Some(format!("entry-{}", cut_point.first_kept_entry_index)));

    CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages: Vec::new(),
        is_split_turn: cut_point.is_split_turn,
        tokens_before: 0,
        previous_summary,
        file_ops,
        settings,
    }
}

fn extract_file_operations(
    messages: &[AgentMessage],
    _prev_compaction_index: Option<usize>,
) -> FileOperations {
    let mut read_files: Vec<String> = Vec::new();
    let mut modified_files: Vec<String> = Vec::new();
    let mut seen_read: HashMap<String, bool> = HashMap::new();
    let mut seen_modified: HashMap<String, bool> = HashMap::new();

    for msg in messages {
        match msg {
            AgentMessage::ToolResult {
                tool_name, content, ..
            } => match tool_name.as_str() {
                "read" => {
                    for block in content {
                        if let ContentBlock::Text { text, .. } = block {
                            for line in text.lines() {
                                if line.starts_with("File: ") {
                                    let path = line.trim_start_matches("File: ").trim();
                                    if seen_read.insert(path.to_string(), true).is_none() {
                                        read_files.push(path.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                "write" | "edit" => {
                    for block in content {
                        if let ContentBlock::Text { text, .. } = block {
                            for line in text.lines() {
                                if line.starts_with("File: ") || line.starts_with("Wrote ") {
                                    let path = line
                                        .trim_start_matches("File: ")
                                        .trim_start_matches("Wrote ")
                                        .trim();
                                    if seen_modified.insert(path.to_string(), true).is_none() {
                                        modified_files.push(path.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    FileOperations {
        read_files,
        modified_files,
    }
}

pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut result = String::new();

    if !read_files.is_empty() {
        result.push_str("\n\n## Files Read\n");
        for f in read_files {
            result.push_str(&format!("- {}\n", f));
        }
    }

    if !modified_files.is_empty() {
        result.push_str("\n\n## Files Modified\n");
        for f in modified_files {
            result.push_str(&format!("- {}\n", f));
        }
    }

    result
}

pub fn build_summarization_prompt(
    messages: &[AgentMessage],
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
) -> String {
    let llm_messages = messages::convert_to_llm(messages);
    let conversation_text = serialize_conversation(&llm_messages);

    let mut prompt = String::new();

    if let Some(prev) = previous_summary {
        prompt.push_str(&format!(
            "Previous summary of earlier conversation:\n<previous_summary>\n{}\n</previous_summary>\n\n",
            prev
        ));
    }

    if let Some(instructions) = custom_instructions {
        prompt.push_str(&format!("Focus on: {}\n\n", instructions));
    }

    prompt.push_str(&format!(
        "<conversation>\n{}\n</conversation>\n\nSummarize this conversation, preserving key context, file paths, code changes, and current state.",
        conversation_text
    ));

    prompt
}

fn serialize_conversation(messages: &[Message]) -> String {
    let mut text = String::new();
    for msg in messages {
        match msg {
            Message::User { content, .. } => {
                text.push_str("## User\n");
                for block in content {
                    if let ContentBlock::Text { text: t, .. } = block {
                        text.push_str(t);
                        text.push('\n');
                    }
                }
            }
            Message::Assistant { content, .. } => {
                text.push_str("## Assistant\n");
                for block in content {
                    match block {
                        ContentBlock::Text { text: t, .. } => {
                            text.push_str(t);
                            text.push('\n');
                        }
                        ContentBlock::ToolCall { name, .. } => {
                            text.push_str(&format!("[Tool Call: {}]\n", name));
                        }
                        _ => {}
                    }
                }
            }
            Message::ToolResult {
                tool_name, content, ..
            } => {
                text.push_str(&format!("## Tool Result ({})\n", tool_name));
                for block in content {
                    if let ContentBlock::Text { text: t, .. } = block {
                        text.push_str(t);
                        text.push('\n');
                    }
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_compact() {
        let settings = CompactionSettings::default();
        assert!(should_compact(80000, 100000, &settings));
        assert!(!should_compact(50000, 100000, &settings));
    }

    #[test]
    fn test_should_compact_disabled() {
        let settings = CompactionSettings {
            compact_on_threshold: false,
            ..Default::default()
        };
        assert!(!should_compact(99999, 100000, &settings));
    }

    #[test]
    fn test_find_compaction_cut_point() {
        let messages = vec![
            AgentMessage::User {
                content: vec![ContentBlock::text("hello")],
                timestamp: 1,
            },
            AgentMessage::Assistant {
                content: vec![ContentBlock::text("hi")],
                api: "test".into(),
                provider: "test".into(),
                model: "test".into(),
                usage: Default::default(),
                stop_reason: None,
                error_message: None,
                timestamp: 2,
            },
            AgentMessage::User {
                content: vec![ContentBlock::text("do something")],
                timestamp: 3,
            },
            AgentMessage::Assistant {
                content: vec![ContentBlock::text("done")],
                api: "test".into(),
                provider: "test".into(),
                model: "test".into(),
                usage: Default::default(),
                stop_reason: None,
                error_message: None,
                timestamp: 4,
            },
        ];

        let cut = find_compaction_cut_point(&messages, 1);
        assert_eq!(cut.first_kept_entry_index, 2);
    }

    #[test]
    fn test_format_file_operations() {
        let result = format_file_operations(
            &["/path/a.rs".to_string()],
            &["/path/b.rs".to_string(), "/path/c.rs".to_string()],
        );
        assert!(result.contains("Files Read"));
        assert!(result.contains("/path/a.rs"));
        assert!(result.contains("Files Modified"));
        assert!(result.contains("/path/b.rs"));
    }

    #[test]
    fn test_build_summarization_prompt() {
        let messages = vec![AgentMessage::User {
            content: vec![ContentBlock::text("hello")],
            timestamp: 1,
        }];
        let prompt = build_summarization_prompt(&messages, None, None);
        assert!(prompt.contains("<conversation>"));
        assert!(prompt.contains("hello"));
    }

    #[test]
    fn test_build_summarization_prompt_with_previous() {
        let messages = vec![AgentMessage::User {
            content: vec![ContentBlock::text("hello")],
            timestamp: 1,
        }];
        let prompt = build_summarization_prompt(&messages, Some("previous summary"), None);
        assert!(prompt.contains("previous summary"));
    }
}
