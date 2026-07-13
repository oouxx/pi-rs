use std::collections::HashMap;

use pi_agent_core::pi_ai_types::{ContentBlock, Message};
use pi_agent_core::types::AgentMessage;
use serde::Serialize;

use crate::core::messages;

pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a summarization assistant. Your job is to create concise summaries of coding agent conversations.

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

#[derive(Debug, Clone, Default, Serialize)]
pub struct FileOperations {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub details: CompactionDetails,
}

#[derive(Debug, Clone, Default, Serialize)]
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

pub fn serialize_compaction_summary(summary: &CompactionResult) -> Result<String, String> {
    serde_json::to_string(summary).map_err(|e| format!("Failed to serialize compaction summary: {}", e))
}

pub fn save_compaction_summary(summary: &CompactionResult, file_path: &str) -> Result<(), String> {
    let json_line = serialize_compaction_summary(summary)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)
        .map_err(|e| format!("Failed to open file {}: {}", file_path, e))
        .and_then(|mut file| {
            use std::io::Write;
            writeln!(file, "{}", json_line)
                .map_err(|e| format!("Failed to write to file {}: {}", file_path, e))
        })
}

// ============================================================================
// Token estimation utilities
// ============================================================================

/// Roughly estimate the number of tokens in a text string.
/// Uses a simple heuristic: ~4 characters per token for English text.
pub fn estimate_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    // Rough estimate: 4 chars per token for English, 1.5 for CJK
    let mut char_count = 0u64;
    let mut cjk_count = 0u64;
    for ch in text.chars() {
        char_count += 1;
        if ch as u32 >= 0x4E00 && ch as u32 <= 0x9FFF {
            cjk_count += 1;
        }
    }
    // CJK characters are roughly 1.5 tokens each, others ~0.25 tokens each
    let non_cjk = char_count.saturating_sub(cjk_count);
    (cjk_count * 3 / 2) + (non_cjk / 4)
}

/// Estimate the number of tokens in a conversation message.
pub fn estimate_message_tokens(msg: &pi_agent_core::pi_ai_types::Message) -> u64 {
    let mut total = 0u64;
    match msg {
        pi_agent_core::pi_ai_types::Message::User { content, .. } => {
            for block in content {
                total += estimate_content_block_tokens(block);
            }
        }
        pi_agent_core::pi_ai_types::Message::Assistant { content, .. } => {
            for block in content {
                total += estimate_content_block_tokens(block);
            }
        }
        pi_agent_core::pi_ai_types::Message::ToolResult { content, .. } => {
            for block in content {
                total += estimate_content_block_tokens(block);
            }
        }
    }
    total
}

fn estimate_content_block_tokens(block: &pi_agent_core::pi_ai_types::ContentBlock) -> u64 {
    match block {
        pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } => {
            estimate_text_tokens(text)
        }
        pi_agent_core::pi_ai_types::ContentBlock::ToolCall { name, arguments, .. } => {
            estimate_text_tokens(name) + estimate_text_tokens(&arguments.to_string())
        }
        pi_agent_core::pi_ai_types::ContentBlock::Image { .. } => 100, // rough estimate per image
        pi_agent_core::pi_ai_types::ContentBlock::Thinking { thinking, .. } => {
            estimate_text_tokens(thinking)
        }
    }
}

/// Calculate the total token count for a list of messages.
pub fn calculate_context_tokens(messages: &[pi_agent_core::pi_ai_types::Message]) -> u64 {
    messages.iter().map(|m| estimate_message_tokens(m)).sum()
}

/// Estimate tokens for AgentMessages (converts to LLM messages first).
pub fn estimate_agent_messages_tokens(messages: &[pi_agent_core::types::AgentMessage]) -> u64 {
    let llm_messages = crate::core::messages::convert_to_llm(messages);
    calculate_context_tokens(&llm_messages)
}

// ============================================================================
// Message extraction from session entries
// ============================================================================

/// Extract an AgentMessage from a session entry, if the entry is a message type.
pub fn get_message_from_entry(entry: &crate::core::session_manager::SessionEntry) -> Option<pi_agent_core::types::AgentMessage> {
    match entry {
        crate::core::session_manager::SessionEntry::Message { message, .. } => {
            serde_json::from_value(message.clone()).ok()
        }
        _ => None,
    }
}

// ============================================================================
// Branch summarization
// ============================================================================

/// Collect entries for branch summary generation.
/// Returns the messages that should be summarized for a branch.
pub fn collect_entries_for_branch_summary(
    entries: &[crate::core::session_manager::SessionEntry],
    from_id: &str,
) -> Vec<crate::core::session_manager::SessionEntry> {
    let mut collected = Vec::new();
    let mut found_from = false;

    for entry in entries {
        if entry.id() == from_id {
            found_from = true;
        }
        if found_from {
            collected.push(entry.clone());
        }
    }

    collected
}

/// Build a summarization prompt for branch summary generation.
pub fn build_branch_summary_prompt(
    entries: &[crate::core::session_manager::SessionEntry],
    from_id: &str,
) -> String {
    let collected = collect_entries_for_branch_summary(entries, from_id);
    let messages: Vec<pi_agent_core::types::AgentMessage> = collected
        .iter()
        .filter_map(|e| get_message_from_entry(e))
        .collect();

    let llm_messages = crate::core::messages::convert_to_llm(&messages);
    let conversation_text = serialize_conversation(&llm_messages);

    format!(
        r#"Summarize the following conversation branch, focusing on:
1. What was the original request?
2. What work was done?
3. What files were changed?
4. What is the current state?

<conversation>
{}
</conversation>

Provide a concise branch summary."#,
        conversation_text
    )
}

// ============================================================================
// File operation utilities
// ============================================================================

/// Compute file lists from a set of messages, separating read and modified files.
pub fn compute_file_lists(
    messages: &[pi_agent_core::types::AgentMessage],
) -> (Vec<String>, Vec<String>) {
    let mut read_files: Vec<String> = Vec::new();
    let mut modified_files: Vec<String> = Vec::new();
    let mut seen_read: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut seen_modified: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

    for msg in messages {
        if let pi_agent_core::types::AgentMessage::ToolResult {
            tool_name, content, ..
        } = msg
        {
            match tool_name.as_str() {
                "read" => {
                    for block in content {
                        if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = block {
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
                        if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = block {
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
            }
        }
    }

    (read_files, modified_files)
}

/// Create file operations from a list of read and modified files.
pub fn create_file_ops(read_files: &[String], modified_files: &[String]) -> FileOperations {
    FileOperations {
        read_files: read_files.to_vec(),
        modified_files: modified_files.to_vec(),
    }
}

/// Extract file operations from a single message.
pub fn extract_file_ops_from_message(
    msg: &pi_agent_core::types::AgentMessage,
) -> FileOperations {
    let (read_files, modified_files) = compute_file_lists(&[msg.clone()]);
    FileOperations {
        read_files,
        modified_files,
    }
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

    #[test]
    fn test_serialize_compaction_summary() {
        let result = CompactionResult {
            summary: "test summary".into(),
            first_kept_entry_id: "entry-5".into(),
            tokens_before: 10000,
            details: CompactionDetails {
                read_files: vec!["/a.rs".into()],
                modified_files: vec!["/b.rs".into()],
            },
        };
        let json = serialize_compaction_summary(&result).unwrap();
        assert!(json.contains("test summary"));
        assert!(json.contains("entry-5"));
    }

    #[test]
    fn test_save_compaction_summary() {
        let result = CompactionResult {
            summary: "test".into(),
            first_kept_entry_id: "entry-1".into(),
            tokens_before: 5000,
            details: CompactionDetails::default(),
        };
        let path = "/tmp/test_compaction_summary.jsonl";
        let _ = std::fs::remove_file(path);
        save_compaction_summary(&result, path).unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("test"));
        std::fs::remove_file(path).ok();
    }

    // ============================================================
    // Token estimation
    // ============================================================

    #[test]
    fn test_estimate_text_tokens_empty() {
        assert_eq!(estimate_text_tokens(""), 0);
    }

    #[test]
    fn test_estimate_text_tokens_english() {
        // ~4 chars per token for English
        let tokens = estimate_text_tokens("Hello world, this is a test message with about twenty words in it for token estimation");
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_text_tokens_cjk() {
        // CJK characters are ~1.5 tokens each
        let tokens = estimate_text_tokens("你好世界这是一条测试消息");
        assert!(tokens > 0);
    }

    #[test]
    fn test_calculate_context_tokens() {
        use pi_agent_core::pi_ai_types::{ContentBlock, Message, StopReason, Usage};
        let messages = vec![
            Message::User {
                content: vec![ContentBlock::text("hello")],
                timestamp: 1000,
            },
            Message::Assistant {
                content: vec![ContentBlock::text("world")],
                api: "test".into(),
                provider: "test".into(),
                model: "test".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 1000,
            },
        ];
        let tokens = calculate_context_tokens(&messages);
        assert!(tokens > 0);
    }

    // ============================================================
    // File operations
    // ============================================================

    #[test]
    fn test_compute_file_lists_empty() {
        let (read, modified) = compute_file_lists(&[]);
        assert!(read.is_empty());
        assert!(modified.is_empty());
    }

    #[test]
    fn test_create_file_ops() {
        let ops = create_file_ops(&["/a.rs".into()], &["/b.rs".into()]);
        assert_eq!(ops.read_files.len(), 1);
        assert_eq!(ops.modified_files.len(), 1);
    }

    #[test]
    fn test_extract_file_ops_from_message_no_tool() {
        let msg = pi_agent_core::types::AgentMessage::User {
            content: vec![],
            timestamp: 1000,
        };
        let ops = extract_file_ops_from_message(&msg);
        assert!(ops.read_files.is_empty());
        assert!(ops.modified_files.is_empty());
    }

    // ============================================================
    // Branch summarization
    // ============================================================

    #[test]
    fn test_collect_entries_for_branch_summary() {
        use crate::core::session_manager::SessionEntry;
        let entries = vec![
            SessionEntry::Message {
                id: "1".into(),
                parent_id: None,
                timestamp: "t1".into(),
                message: serde_json::json!({"role": "user", "content": "hello"}),
            },
            SessionEntry::Message {
                id: "2".into(),
                parent_id: Some("1".into()),
                timestamp: "t2".into(),
                message: serde_json::json!({"role": "assistant", "content": "world"}),
            },
        ];
        let collected = collect_entries_for_branch_summary(&entries, "1");
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn test_get_message_from_entry() {
        use crate::core::session_manager::SessionEntry;
        let entry = SessionEntry::Message {
            id: "1".into(),
            parent_id: None,
            timestamp: "t1".into(),
            message: serde_json::json!({"role": "user", "content": [{"type": "text", "text": "hello"}], "timestamp": 1000}),
        };
        let msg = get_message_from_entry(&entry);
        assert!(msg.is_some());
    }

    #[test]
    fn test_get_message_from_entry_non_message() {
        use crate::core::session_manager::SessionEntry;
        let entry = SessionEntry::ThinkingLevelChange {
            id: "1".into(),
            parent_id: None,
            timestamp: "t1".into(),
            thinking_level: "medium".into(),
        };
        let msg = get_message_from_entry(&entry);
        assert!(msg.is_none());
    }
}
