//! Text helpers (match TS `packages/ai/src/utils/text.ts`).

use crate::types::ContentBlock;

/// Extract and join text from message content (match TS `contentText`).
///
/// Only `text` blocks are included; thinking/tool-call/image blocks are skipped.
/// A plain string is returned unchanged.
pub fn content_text(content: &[ContentBlock], separator: &str) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_text_joins_text_blocks() {
        let content = vec![
            ContentBlock::Text {
                text: "hello".into(),
                text_signature: None,
            },
            ContentBlock::Text {
                text: "world".into(),
                text_signature: None,
            },
        ];
        assert_eq!(content_text(&content, "\n"), "hello\nworld");
    }

    #[test]
    fn test_content_text_skips_non_text_blocks() {
        let content = vec![
            ContentBlock::Text {
                text: "only".into(),
                text_signature: None,
            },
            ContentBlock::Thinking {
                thinking: "hidden".into(),
                thinking_signature: None,
                redacted: None,
            },
        ];
        assert_eq!(content_text(&content, "\n"), "only");
    }

    #[test]
    fn test_content_text_empty() {
        assert_eq!(content_text(&[], "\n"), "");
    }
}
