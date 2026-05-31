use crate::harness::types::FileOperations;
use crate::pi_ai_types::ContentBlock;
use crate::types::AgentMessage;

pub fn create_file_ops() -> FileOperations {
    FileOperations::new()
}

pub fn extract_file_ops_from_message(message: &AgentMessage, file_ops: &mut FileOperations) {
    if let AgentMessage::Assistant { content, .. } = message {
        for block in content {
            if let ContentBlock::ToolCall {
                name, arguments, ..
            } = block
            {
                let args = arguments.as_object();
                if let Some(args_map) = args {
                    if let Some(path) = args_map.get("path").and_then(|v| v.as_str()) {
                        match name.as_str() {
                            "read" => file_ops.read.push(path.to_string()),
                            "write" => file_ops.written.push(path.to_string()),
                            "edit" => file_ops.edited.push(path.to_string()),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

pub fn compute_file_lists(file_ops: &FileOperations) -> (Vec<String>, Vec<String>) {
    let mut modified: Vec<String> = file_ops.edited.iter().chain(file_ops.written.iter()).cloned().collect();
    modified.sort();
    modified.dedup();

    let read_only: Vec<String> = file_ops
        .read
        .iter()
        .filter(|f| !modified.contains(f))
        .cloned()
        .collect();

    (read_only, modified)
}

pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections = Vec::new();

    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }

    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }

    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", sections.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_ai_types::ContentBlock;
    use crate::types::AgentMessage;

    fn create_assistant_with_tool_call(name: &str, path: &str) -> AgentMessage {
        AgentMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "tool-1".to_string(),
                name: name.to_string(),
                arguments: serde_json::json!({"path": path}),
            }],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage: crate::pi_ai_types::Usage::default(),
            stop_reason: Some(crate::pi_ai_types::StopReason::ToolUse),
            error_message: None,
            timestamp: 1000,
        }
    }

    #[test]
    fn test_create_file_ops() {
        let ops = create_file_ops();
        assert!(ops.read.is_empty());
        assert!(ops.written.is_empty());
        assert!(ops.edited.is_empty());
    }

    #[test]
    fn test_extract_file_ops_from_message_read() {
        let msg = create_assistant_with_tool_call("read", "src/main.rs");
        let mut file_ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut file_ops);
        assert_eq!(file_ops.read, vec!["src/main.rs"]);
        assert!(file_ops.written.is_empty());
        assert!(file_ops.edited.is_empty());
    }

    #[test]
    fn test_extract_file_ops_from_message_write() {
        let msg = create_assistant_with_tool_call("write", "src/new_file.rs");
        let mut file_ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut file_ops);
        assert!(file_ops.read.is_empty());
        assert_eq!(file_ops.written, vec!["src/new_file.rs"]);
        assert!(file_ops.edited.is_empty());
    }

    #[test]
    fn test_extract_file_ops_from_message_edit() {
        let msg = create_assistant_with_tool_call("edit", "src/edited.rs");
        let mut file_ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut file_ops);
        assert!(file_ops.read.is_empty());
        assert!(file_ops.written.is_empty());
        assert_eq!(file_ops.edited, vec!["src/edited.rs"]);
    }

    #[test]
    fn test_extract_file_ops_from_message_unknown_tool() {
        let msg = create_assistant_with_tool_call("bash", "/tmp/script.sh");
        let mut file_ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut file_ops);
        assert!(file_ops.read.is_empty());
        assert!(file_ops.written.is_empty());
        assert!(file_ops.edited.is_empty());
    }

    #[test]
    fn test_extract_file_ops_from_non_assistant_message() {
        let msg = AgentMessage::User {
            content: vec![ContentBlock::text("Hello")],
            timestamp: 1000,
        };
        let mut file_ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut file_ops);
        assert!(file_ops.read.is_empty());
    }

    #[test]
    fn test_extract_file_ops_from_message_no_path() {
        let msg = AgentMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "tool-1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"query": "test"}),
            }],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            usage: crate::pi_ai_types::Usage::default(),
            stop_reason: Some(crate::pi_ai_types::StopReason::ToolUse),
            error_message: None,
            timestamp: 1000,
        };
        let mut file_ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut file_ops);
        assert!(file_ops.read.is_empty());
    }

    #[test]
    fn test_compute_file_lists_modified_only() {
        let file_ops = FileOperations {
            read: vec!["read1.rs".to_string()],
            written: vec!["written1.rs".to_string()],
            edited: vec!["edited1.rs".to_string()],
        };
        let (read_only, modified) = compute_file_lists(&file_ops);
        assert!(read_only.contains(&"read1.rs".to_string()));
        assert!(modified.contains(&"written1.rs".to_string()));
        assert!(modified.contains(&"edited1.rs".to_string()));
    }

    #[test]
    fn test_compute_file_lists_read_and_modified_overlap() {
        let file_ops = FileOperations {
            read: vec!["shared.rs".to_string(), "read_only.rs".to_string()],
            written: vec![],
            edited: vec!["shared.rs".to_string()],
        };
        let (read_only, modified) = compute_file_lists(&file_ops);
        assert!(read_only.contains(&"read_only.rs".to_string()));
        assert!(!read_only.contains(&"shared.rs".to_string()));
        assert!(modified.contains(&"shared.rs".to_string()));
    }

    #[test]
    fn test_compute_file_lists_dedup() {
        let file_ops = FileOperations {
            read: vec![],
            written: vec!["file.rs".to_string()],
            edited: vec!["file.rs".to_string()],
        };
        let (read_only, modified) = compute_file_lists(&file_ops);
        assert!(read_only.is_empty());
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0], "file.rs");
    }

    #[test]
    fn test_compute_file_lists_sorted() {
        let file_ops = FileOperations {
            read: vec![],
            written: vec!["z.rs".to_string(), "a.rs".to_string()],
            edited: vec!["m.rs".to_string()],
        };
        let (_, modified) = compute_file_lists(&file_ops);
        assert_eq!(modified, vec!["a.rs", "m.rs", "z.rs"]);
    }

    #[test]
    fn test_format_file_operations_both() {
        let read_files = vec!["read1.rs".to_string()];
        let modified_files = vec!["mod1.rs".to_string()];
        let result = format_file_operations(&read_files, &modified_files);
        assert!(result.starts_with("\n\n"));
        assert!(result.contains("<read-files>"));
        assert!(result.contains("read1.rs"));
        assert!(result.contains("</read-files>"));
        assert!(result.contains("<modified-files>"));
        assert!(result.contains("mod1.rs"));
        assert!(result.contains("</modified-files>"));
    }

    #[test]
    fn test_format_file_operations_read_only() {
        let read_files = vec!["read1.rs".to_string()];
        let modified_files: Vec<String> = vec![];
        let result = format_file_operations(&read_files, &modified_files);
        assert!(result.contains("<read-files>"));
        assert!(!result.contains("<modified-files>"));
    }

    #[test]
    fn test_format_file_operations_modified_only() {
        let read_files: Vec<String> = vec![];
        let modified_files = vec!["mod1.rs".to_string()];
        let result = format_file_operations(&read_files, &modified_files);
        assert!(!result.contains("<read-files>"));
        assert!(result.contains("<modified-files>"));
    }

    #[test]
    fn test_format_file_operations_empty() {
        let read_files: Vec<String> = vec![];
        let modified_files: Vec<String> = vec![];
        let result = format_file_operations(&read_files, &modified_files);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_file_operations_multiple_files() {
        let read_files = vec!["a.rs".to_string(), "b.rs".to_string()];
        let modified_files = vec!["c.rs".to_string()];
        let result = format_file_operations(&read_files, &modified_files);
        assert!(result.contains("a.rs\nb.rs"));
        assert!(result.contains("c.rs"));
    }
}