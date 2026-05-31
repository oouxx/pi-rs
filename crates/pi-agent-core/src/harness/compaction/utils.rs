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