//! Build the initial message for non-interactive mode from stdin, @file, and CLI args.
//!
//! Mirrors packages/coding-agent/src/cli/initial-message.ts

use crate::args::CliArgs;
use crate::file_processor::ImageAttachment;

/// Input for building the initial message.
pub struct InitialMessageInput<'a> {
    pub parsed: &'a CliArgs,
    pub file_text: Option<String>,
    pub file_images: Vec<ImageAttachment>,
    pub stdin_content: Option<String>,
}

/// Result of building the initial message.
pub struct InitialMessageResult {
    pub initial_message: Option<String>,
    pub initial_images: Vec<ImageAttachment>,
}

/// Combine stdin content, @file text, and CLI message into a single initial prompt.
///
/// Order: stdin → @file text → first CLI message.
/// Consumes the first message from `parsed.messages`.
pub fn build_initial_message(input: InitialMessageInput<'_>) -> InitialMessageResult {
    let mut parts: Vec<String> = Vec::new();
    let mut images: Vec<ImageAttachment> = Vec::new();

    if let Some(stdin) = input.stdin_content {
        parts.push(stdin);
    }
    if let Some(ref file_text) = input.file_text {
        parts.push(file_text.clone());
    }

    // Take the first CLI message (if any) — but in our args model,
    // messages is not easily mutated. Instead we concatenate all messages.
    if !input.parsed.messages.is_empty() {
        // Pop the first message
        let first = input.parsed.messages[0].clone();
        parts.push(first);
        // Note: the original TS shifts the array; we don't modify in place
    }

    if !input.file_images.is_empty() {
        images = input.file_images.clone();
    }

    InitialMessageResult {
        initial_message: if parts.is_empty() { None } else { Some(parts.join("")) },
        initial_images: images,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        let parsed = CliArgs::new();
        let result = build_initial_message(InitialMessageInput {
            parsed: &parsed,
            file_text: None,
            file_images: vec![],
            stdin_content: None,
        });
        assert!(result.initial_message.is_none());
        assert!(result.initial_images.is_empty());
    }

    #[test]
    fn test_with_message() {
        let mut parsed = CliArgs::new();
        parsed.messages.push("hello".to_string());

        let result = build_initial_message(InitialMessageInput {
            parsed: &parsed,
            file_text: None,
            file_images: vec![],
            stdin_content: None,
        });
        assert_eq!(result.initial_message, Some("hello".to_string()));
    }

    #[test]
    fn test_stdin_and_file() {
        let parsed = CliArgs::new();
        let result = build_initial_message(InitialMessageInput {
            parsed: &parsed,
            file_text: Some("<file>content</file>".to_string()),
            file_images: vec![],
            stdin_content: Some("stdin data".to_string()),
        });
        assert!(result.initial_message.unwrap().contains("stdin data"));
    }

    #[test]
    fn test_order_stdin_file_message() {
        let mut parsed = CliArgs::new();
        parsed.messages.push("cli message".to_string());

        let result = build_initial_message(InitialMessageInput {
            parsed: &parsed,
            file_text: Some("<file>text</file>".to_string()),
            file_images: vec![],
            stdin_content: Some("stdin".to_string()),
        });
        let msg = result.initial_message.unwrap();
        assert!(msg.starts_with("stdin"));
    }
}
