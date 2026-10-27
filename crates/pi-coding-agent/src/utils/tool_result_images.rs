//! Normalize image blocks returned by tool results (TS
//! `utils/tool-result-images.ts`).
//!
//! The `read` tool and `@file` attachments already run their images through
//! `process_image`, but tools that produce images themselves (extensions, MCP
//! bridges, screenshot tools) hand back arbitrary base64 payloads that would go
//! straight into session history and every later provider request. Oversized
//! images make the provider reject the whole conversation, so they are
//! normalized once as they enter history.
//!
//! Returns `None` when nothing changed, so callers can skip rewriting the
//! result.

use base64::Engine;
use pi_ai::types::{ContentBlock, ModelImageResizeOptions};

/// TS `normalizeToolResultImages`.
#[must_use]
pub fn normalize_tool_result_images(
    content: Vec<ContentBlock>,
    auto_resize_images: bool,
    resize_options: Option<&ModelImageResizeOptions>,
) -> Option<Vec<ContentBlock>> {
    if !content
        .iter()
        .any(|block| matches!(block, ContentBlock::Image { .. }))
    {
        return None;
    }

    let mut normalized: Vec<ContentBlock> = Vec::with_capacity(content.len());
    let mut changed = false;

    for block in content {
        let ContentBlock::Image { data, mime_type } = &block else {
            normalized.push(block);
            continue;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
            // Undecodable payload: keep the block (the provider will report it),
            // matching TS's "pass the original through" behavior.
            normalized.push(block);
            continue;
        };
        let processed = crate::utils::image_process::process_image(
            &bytes,
            mime_type,
            auto_resize_images,
            resize_options,
        );
        let Ok(processed) = processed else {
            // Unlike `read`, keep the original block: the tool already produced
            // this image and the failure may just be a missing image backend.
            normalized.push(block);
            continue;
        };
        if processed.data == *data && processed.mime_type == *mime_type && processed.hints.is_empty() {
            normalized.push(block);
            continue;
        }
        normalized.push(ContentBlock::Image {
            data: processed.data,
            mime_type: processed.mime_type,
        });
        if !processed.hints.is_empty() {
            normalized.push(ContentBlock::Text {
                text: processed.hints.join("\n"),
                text_signature: None,
            });
        }
        changed = true;
    }

    if changed {
        Some(normalized)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use base64::Engine;

    fn png_base64(width: u32, height: u32) -> String {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb([10, 20, 30]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    /// Content without images is left untouched (`None` = nothing changed).
    #[test]
    fn text_only_content_is_unchanged() {
        let content = vec![ContentBlock::text("hello")];
        assert!(normalize_tool_result_images(content, true, None).is_none());
    }

    /// A tool that returns an oversized image has it resized once, with the
    /// dimension note appended (TS `normalizeToolResultImages`).
    #[test]
    fn oversized_tool_image_is_resized_with_note() {
        let content = vec![
            ContentBlock::text("screenshot"),
            ContentBlock::Image {
                data: png_base64(900, 400),
                mime_type: "image/png".into(),
            },
        ];
        let profile = pi_ai::types::ModelImageResizeOptions {
            max_width: Some(100),
            max_height: Some(100),
            max_bytes: None,
            jpeg_quality: None,
        };
        let normalized =
            normalize_tool_result_images(content, true, Some(&profile)).expect("changed");
        assert_eq!(normalized.len(), 3, "image + hints text appended: {normalized:?}");
        let ContentBlock::Image { data, .. } = &normalized[1] else {
            panic!("expected image block, got {:?}", normalized[1]);
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("b64");
        let img = image::load_from_memory(&bytes).expect("decode");
        assert_eq!((img.width(), img.height()), (100, 44), "resized, aspect kept");
        let ContentBlock::Text { text, .. } = &normalized[2] else {
            panic!("expected hints text");
        };
        assert!(text.contains("original 900x400"), "got {text}");
    }

    /// Undecodable payloads are kept as-is (a missing image backend must not
    /// silently delete a tool's output).
    #[test]
    fn undecodable_image_is_preserved() {
        let content = vec![ContentBlock::Image {
            data: "not-base64!!".into(),
            mime_type: "image/png".into(),
        }];
        assert!(normalize_tool_result_images(content, true, None).is_none());
    }
}
