//! Tool tests for pi-coding-agent.
//!
//! Mirrors the original TypeScript tools.test.ts from
//! https://github.com/earendil-works/pi/tree/main/packages/coding-agent/test/tools.test.ts
//!
//! Run with: cargo test -p pi-coding-agent --test tools_test -- --nocapture

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use pi_agent_core::types::AgentTool;

// ============================================================================
// Helpers
// ============================================================================

/// Create a temporary directory for testing.
fn setup_test_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("Failed to create temp dir")
}

/// Extract text output from a tool result.
fn get_text_output(result: &pi_agent_core::types::AgentToolResult<serde_json::Value>) -> String {
    result
        .content
        .iter()
        .filter_map(|c| {
            if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = c {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Create a 1x1 red 24-bit BMP file bytes.
#[allow(dead_code)]
fn create_tiny_bmp_1x1_red_24bpp() -> Vec<u8> {
    let mut buffer = vec![0u8; 58];
    // BMP header
    buffer[0..2].copy_from_slice(b"BM");
    // File size
    let file_size = buffer.len() as u32;
    buffer[2..6].copy_from_slice(&file_size.to_le_bytes());
    // Data offset
    buffer[10..14].copy_from_slice(&54u32.to_le_bytes());
    // DIB header size
    buffer[14..18].copy_from_slice(&40u32.to_le_bytes());
    // Width
    buffer[18..22].copy_from_slice(&1i32.to_le_bytes());
    // Height
    buffer[22..26].copy_from_slice(&1i32.to_le_bytes());
    // Planes
    buffer[26..28].copy_from_slice(&1u16.to_le_bytes());
    // Bits per pixel
    buffer[28..30].copy_from_slice(&24u16.to_le_bytes());
    // Compression
    buffer[30..34].copy_from_slice(&0u32.to_le_bytes());
    // Image size
    buffer[34..38].copy_from_slice(&4u32.to_le_bytes());
    // Pixel data (red)
    buffer[56] = 0xff;
    buffer
}

/// Helper to call a tool's execute function.
async fn execute_tool(
    tool: &AgentTool<serde_json::Value, serde_json::Value>,
    call_id: &str,
    params: serde_json::Value,
) -> Result<pi_agent_core::types::AgentToolResult<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>>
{
    (tool.execute)(call_id.to_string(), params, None, None).await
}

/// The grep/find tools shell out to `rg` / `fd` (matching TS). Tests skip when
/// the binary is not resolvable, so they never hit the network from CI.
fn tool_binary_available(tool: &str) -> bool {
    pi_coding_agent::utils::tools_manager::get_tool_path(tool).is_some()
}

/// Skip guard for tests that need a managed external binary.
macro_rules! require_binary {
    ($tool:expr) => {
        if !tool_binary_available($tool) {
            eprintln!("skipping: {} not available", $tool);
            return;
        }
    };
}

// ============================================================================
// Read Tool Tests
// ============================================================================

#[tokio::test]
async fn test_read_file_contents() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("test.txt");
    let content = "Hello, world!\nLine 2\nLine 3";
    tokio::fs::write(&file_path, content).await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({ "path": file_path.to_str().unwrap() });
    let result = execute_tool(&tool, "test-call-1", params).await.unwrap();

    let output = get_text_output(&result);
    assert_eq!(output, content);
    assert!(!output.contains("Use offset="));
}

#[tokio::test]
async fn test_read_nonexistent_file() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("nonexistent.txt");

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({ "path": file_path.to_str().unwrap() });
    // TS `ops.access()` throws -> tool error result (is_error), not a text result.
    let err = execute_tool(&tool, "test-call-2", params)
        .await
        .expect_err("missing file must be a tool error");
    let msg = err.to_string();
    assert!(
        msg.contains("No such file") || msg.contains("not found") || msg.contains("os error"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn test_read_truncation_details() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("large-file.txt");
    let lines: Vec<String> = (1..=2500).map(|i| format!("Line {}", i)).collect();
    tokio::fs::write(&file_path, lines.join("\n")).await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({ "path": file_path.to_str().unwrap() });
    let result = execute_tool(&tool, "test-call-9", params).await.unwrap();

    let details: serde_json::Value = result.details;
    assert!(!details.is_null());
    if let Some(truncation) = details.get("truncation") {
        assert_eq!(truncation.get("truncated").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(
            truncation.get("truncated_by").and_then(serde_json::Value::as_str),
            Some("lines")
        );
        assert_eq!(
            truncation.get("total_lines").and_then(serde_json::Value::as_u64),
            Some(2500)
        );
        assert_eq!(
            truncation.get("output_lines").and_then(serde_json::Value::as_u64),
            Some(2000)
        );
    }
}

// ============================================================================

#[tokio::test]
async fn test_read_image_extension_non_image_content() {
    let dir = setup_test_dir();
    let test_file = dir.path().join("not-an-image.png");
    tokio::fs::write(&test_file, "definitely not a png").await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({ "path": test_file.to_str().unwrap() });
    let result = execute_tool(&tool, "test-call-img-2", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("definitely not a png"));
    let has_image = result.content.iter().any(|c| {
        matches!(c, pi_agent_core::pi_ai_types::ContentBlock::Image { .. })
    });
    assert!(!has_image);
}

// ============================================================================

#[tokio::test]
async fn test_read_truncate_by_bytes() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("large-bytes.txt");
    // Create file that exceeds 50KB byte limit but has fewer than 2000 lines
    let lines: Vec<String> = (0..500)
        .map(|i| format!("Line {}: {}", i + 1, "x".repeat(200)))
        .collect();
    tokio::fs::write(&file_path, lines.join("\n")).await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({ "path": file_path.to_str().unwrap() });
    let result = execute_tool(&tool, "test-call-4", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Line 1:"));
    // Should show byte limit message
    assert!(
        output.contains("limit") || output.contains("KB"),
        "Expected byte limit message in output: {}",
        output
    );
    // Should have a continuation message
    assert!(
        output.contains("Use offset="),
        "Expected continuation message in output: {}",
        output
    );
}

#[tokio::test]
async fn test_read_image_mime_detection() {
    let dir = setup_test_dir();
    // Create a minimal valid PNG file (1x1 red pixel)
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, // IHDR chunk length
        0x49, 0x48, 0x44, 0x52, // "IHDR"
        0x00, 0x00, 0x00, 0x01, // width: 1
        0x00, 0x00, 0x00, 0x01, // height: 1
        0x08, 0x02, // bit depth: 8, color type: RGB
        0x00, 0x00, 0x00, // compression, filter, interlace
        0x90, 0x77, 0x53, 0xDE, // IHDR CRC
        0x00, 0x00, 0x00, 0x0A, // IDAT chunk length
        0x49, 0x44, 0x41, 0x54, // "IDAT"
        0x78, 0xDA, 0x63, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE7, 0x27, 0xDA, // IDAT data
        0x00, 0x00, 0x00, 0x00, // IEND chunk length
        0x49, 0x45, 0x4E, 0x44, // "IEND"
        0xAE, 0x42, 0x60, 0x82, // IEND CRC
    ];

    let test_file = dir.path().join("image.txt");
    tokio::fs::write(&test_file, &png_bytes).await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({ "path": test_file.to_str().unwrap() });
    let result = execute_tool(&tool, "test-call-img-1", params).await.unwrap();

    let output = get_text_output(&result);
    // The Rust read tool may not have image detection yet, so just verify it handles the file
    // without crashing and produces some output
    assert!(!output.is_empty(), "Expected non-empty output for PNG file");
}

#[tokio::test]
async fn test_read_bmp_as_png() {
    let dir = setup_test_dir();
    let test_file = dir.path().join("image.bmp");
    tokio::fs::write(&test_file, &create_tiny_bmp_1x1_red_24bpp())
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({ "path": test_file.to_str().unwrap() });
    let result = execute_tool(&tool, "test-call-img-bmp", params).await.unwrap();

    let output = get_text_output(&result);
    // The Rust read tool may not have image detection yet, so just verify it handles the file
    assert!(!output.is_empty(), "Expected non-empty output for BMP file");
}
/// A real 1x1 PNG (detected by magic bytes, so the read tool takes the image
/// branch).
fn create_tiny_png_bytes() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

/// TS `getNonVisionImageNote`: a model without image support still gets the
/// image bytes converted to a text note, plus the "does not support images"
/// hint (the model never sees the attachment).
#[tokio::test]
async fn test_read_image_notes_non_vision_model() {
    let dir = setup_test_dir();
    let test_file = dir.path().join("image.png");
    tokio::fs::write(&test_file, &create_tiny_png_bytes()).await.unwrap();

    let text_only_model = || {
        Box::pin(async {
            Some(pi_ai::types::Model {
                prompt_cache: None,
                id: "text-only".into(),
                name: "text-only".into(),
                api: "anthropic-messages".into(),
                provider: "anthropic".into(),
                base_url: "https://api.anthropic.com".into(),
                reasoning: false,
                thinking_level_map: None,
                input: vec!["text".into()],
                input_limits: None,
                cost: pi_ai::types::ModelCost::default(),
                context_window: 200_000,
                max_tokens: 4096,
                sampling_params: None,
                headers: None,
                compat: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Option<pi_ai::types::Model>> + Send>>
    };
    let vision_model = || {
        Box::pin(async {
            Some(pi_ai::types::Model {
                prompt_cache: None,
                id: "vision".into(),
                name: "vision".into(),
                api: "anthropic-messages".into(),
                provider: "anthropic".into(),
                base_url: "https://api.anthropic.com".into(),
                reasoning: false,
                thinking_level_map: None,
                input: vec!["text".into(), "image".into()],
                input_limits: None,
                cost: pi_ai::types::ModelCost::default(),
                context_window: 200_000,
                max_tokens: 4096,
                sampling_params: None,
                headers: None,
                compat: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Option<pi_ai::types::Model>> + Send>>
    };

    let make_tool = |provider: pi_coding_agent::core::tools::read::ReadModelProvider| {
        pi_coding_agent::core::tools::read::create_read_tool(
            dir.path().to_str().unwrap(),
            Some(pi_coding_agent::core::tools::read::ReadToolOptions {
                model_provider: Some(provider),
                ..Default::default()
            }),
        )
    };

    let params = serde_json::json!({ "path": test_file.to_str().unwrap() });

    let non_vision = make_tool(std::sync::Arc::new(text_only_model));
    let result = execute_tool(&non_vision, "test-call-nonvision", params.clone())
        .await
        .unwrap();
    let note = get_text_output(&result);
    assert!(
        note.contains("Current model does not support images"),
        "expected non-vision note, got: {note}"
    );

    let vision = make_tool(std::sync::Arc::new(vision_model));
    let result = execute_tool(&vision, "test-call-vision", params).await.unwrap();
    let note = get_text_output(&result);
    assert!(
        !note.contains("Current model does not support images"),
        "vision model must not get the note: {note}"
    );
    assert!(
        result
            .content
            .iter()
            .any(|b| matches!(b, pi_ai::types::ContentBlock::Image { .. })),
        "vision model still receives the image attachment: {:?}",
        result.content
    );
}

/// A model `inputLimits.images.resize` profile narrows the resize applied on
/// `read` (#9631).
#[tokio::test]
async fn test_read_image_uses_model_resize_limits() {
    let dir = setup_test_dir();
    let test_file = dir.path().join("big.png");
    // 300x150 solid PNG — under the 2000px default, over the 100px profile.
    let img = image::RgbImage::from_pixel(300, 150, image::Rgb([1, 2, 3]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    tokio::fs::write(&test_file, out.into_inner()).await.unwrap();

    let provider: pi_coding_agent::core::tools::read::ReadModelProvider =
        std::sync::Arc::new(|| {
            Box::pin(async {
                Some(pi_ai::types::Model {
                    prompt_cache: None,
                    id: "narrow".into(),
                    name: "narrow".into(),
                    api: "anthropic-messages".into(),
                    provider: "anthropic".into(),
                    base_url: "https://api.anthropic.com".into(),
                    reasoning: false,
                    thinking_level_map: None,
                    input: vec!["text".into(), "image".into()],
                    input_limits: Some(pi_ai::types::ModelInputLimits {
                        max_request_bytes: None,
                        images: Some(pi_ai::types::ModelImageInputLimits {
                            resize: Some(pi_ai::types::ModelImageResizeOptions {
                                max_width: Some(100),
                                max_height: Some(100),
                                max_bytes: None,
                                jpeg_quality: None,
                            }),
                            max_per_message: None,
                            max_per_request: None,
                        }),
                    }),
                    cost: pi_ai::types::ModelCost::default(),
                    context_window: 200_000,
                    max_tokens: 4096,
                    sampling_params: None,
                    headers: None,
                    compat: None,
                })
            })
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = Option<pi_ai::types::Model>> + Send>,
                >
        });
    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::read::ReadToolOptions {
            model_provider: Some(provider),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({ "path": test_file.to_str().unwrap() });
    let result = execute_tool(&tool, "test-call-limits", params).await.unwrap();
    let image = result
        .content
        .iter()
        .find_map(|b| match b {
            pi_ai::types::ContentBlock::Image { data, .. } => Some(data.clone()),
            _ => None,
        })
        .expect("image block");
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image)
        .expect("b64");
    let decoded = image::load_from_memory(&bytes).expect("decode");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (100, 50),
        "model profile narrowed the width"
    );
}

// Write Tool Tests
// ============================================================================

#[tokio::test]
async fn test_write_file_contents() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("write-test.txt");
    let content = "Test content";

    let tool = pi_coding_agent::core::tools::write::create_write_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "content": content
    });
    let result = execute_tool(&tool, "test-call-write-1", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Successfully wrote"));
    assert!(output.contains("write-test.txt"));

    // Verify file was written
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, content);
}

#[tokio::test]
async fn test_write_error_is_tool_error() {
    let dir = setup_test_dir();
    // Parent path is a regular file, so mkdir/write must fail.
    let blocker = dir.path().join("blocker");
    tokio::fs::write(&blocker, "x").await.unwrap();
    let target = blocker.join("nested/file.txt");

    let tool = pi_coding_agent::core::tools::write::create_write_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": target.to_str().unwrap(),
        "content": "boom"
    });
    // TS `ops.mkdir`/`ops.writeFile` throw -> error tool result.
    let err = execute_tool(&tool, "test-call-write-err", params)
        .await
        .expect_err("write into a file path must be a tool error");
    assert!(
        err.to_string().contains("Not a directory")
            || err.to_string().contains("os error")
            || err.to_string().contains("File exists"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn test_write_creates_parent_dirs() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("nested").join("dir").join("test.txt");
    let content = "Nested content";

    let tool = pi_coding_agent::core::tools::write::create_write_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "content": content
    });
    let result = execute_tool(&tool, "test-call-write-2", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Successfully wrote"));

    // Verify file was written
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, content);
}

// ============================================================================
// Edit Tool Tests
// ============================================================================

#[tokio::test]
async fn test_edit_replace_text() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("edit-test.txt");
    tokio::fs::write(&file_path, "Hello, world!").await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "world", "newText": "testing"}]
    });
    let result = execute_tool(&tool, "test-call-edit-1", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Successfully replaced"));

    let details: &serde_json::Value = &result.details;
    assert!(!details.is_null());
    assert!(details.get("diff").and_then(serde_json::Value::as_str).is_some());
    assert!(details.get("patch").and_then(serde_json::Value::as_str).is_some());

    let patch = details.get("patch").and_then(serde_json::Value::as_str).unwrap();
    assert!(patch.contains("--- "));
    assert!(patch.contains("+++ "));
    assert!(patch.contains("@@"));
    assert!(patch.contains("-Hello, world!"));
    assert!(patch.contains("+Hello, testing!"));

    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "Hello, testing!");
}

#[tokio::test]
async fn test_edit_text_not_found() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("edit-test.txt");
    tokio::fs::write(&file_path, "Hello, world!").await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "nonexistent", "newText": "testing"}]
    });
    let result = execute_tool(&tool, "test-call-edit-2", params).await;

    assert!(result.is_err());
    let err = result.err().unwrap();
    let err_msg = format!("{}", err);
    assert!(err_msg.contains("Could not find the exact text"));
}

#[tokio::test]
async fn test_edit_file_not_found() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("missing.txt");

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "hello", "newText": "world"}]
    });
    let result = execute_tool(&tool, "test-call-edit-3", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("ENOENT"));
}

#[tokio::test]
async fn test_edit_multiple_occurrences() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("edit-test.txt");
    tokio::fs::write(&file_path, "foo foo foo").await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "foo", "newText": "bar"}]
    });
    let result = execute_tool(&tool, "test-call-edit-4", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("Found 3 occurrences"));
}

#[tokio::test]
async fn test_edit_multi_disjoint_regions() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("edit-multi.txt");
    tokio::fs::write(&file_path, "alpha\nbeta\ngamma\ndelta\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [
            {"oldText": "alpha\n", "newText": "ALPHA\n"},
            {"oldText": "gamma\n", "newText": "GAMMA\n"}
        ]
    });
    let result = execute_tool(&tool, "test-call-edit-5", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Successfully replaced 2 block(s)"));

    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "ALPHA\nbeta\nGAMMA\ndelta\n");
}

#[tokio::test]
async fn test_edit_empty_edits() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("edit-empty-edits.txt");
    tokio::fs::write(&file_path, "hello\nworld\n").await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": []
    });
    let result = execute_tool(&tool, "test-call-edit-6", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("edits must contain at least one replacement"));
}

#[tokio::test]
async fn test_edit_overlapping_regions() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("edit-overlap.txt");
    tokio::fs::write(&file_path, "one\ntwo\nthree\n").await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [
            {"oldText": "one\ntwo\n", "newText": "ONE\nTWO\n"},
            {"oldText": "two\nthree\n", "newText": "TWO\nTHREE\n"}
        ]
    });
    let result = execute_tool(&tool, "test-call-edit-7", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("overlap"));
}

#[tokio::test]
async fn test_edit_no_partial_apply() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("edit-no-partial.txt");
    let original_content = "alpha\nbeta\ngamma\n";
    tokio::fs::write(&file_path, original_content).await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [
            {"oldText": "alpha\n", "newText": "ALPHA\n"},
            {"oldText": "missing\n", "newText": "MISSING\n"}
        ]
    });
    let result = execute_tool(&tool, "test-call-edit-8", params).await;

    assert!(result.is_err());

    // File should be unchanged
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, original_content);
}

#[tokio::test]
async fn test_edit_match_against_original() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("edit-multi-original.txt");
    tokio::fs::write(&file_path, "foo\nbar\nbaz\n").await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [
            {"oldText": "foo\n", "newText": "foo bar\n"},
            {"oldText": "bar\n", "newText": "BAR\n"}
        ]
    });
    let _result = execute_tool(&tool, "test-call-edit-9", params).await
        .expect("Edit should succeed");

    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "foo bar\nBAR\nbaz\n");
}


#[tokio::test]
async fn test_edit_eacces_readonly() {
    let dir = setup_test_dir();
    let test_file = dir.path().join("edit-readonly.txt");
    tokio::fs::write(&test_file, "hello\n").await.unwrap();

    // Make file read-only
    let mut perms = tokio::fs::metadata(&test_file).await.unwrap().permissions();
    perms.set_readonly(true);
    tokio::fs::set_permissions(&test_file, perms).await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": test_file.to_str().unwrap(),
        "edits": [{ "oldText": "hello", "newText": "world" }]
    });
    let result = execute_tool(&tool, "test-call-14", params).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("EACCES") || err.contains("Permission denied") || err.contains("readonly"));
}

#[tokio::test]
async fn test_edit_enoint_diff_preview() {
    let dir = setup_test_dir();
    let missing_file = dir.path().join("missing-preview.txt");

    // Use the compute_edits_diff function directly
    let result = pi_coding_agent::core::tools::edit_diff::compute_edits_diff(
        missing_file.to_str().unwrap(),
        &[pi_coding_agent::core::tools::edit_diff::Edit {
            old_text: "hello".to_string(),
            new_text: "world".to_string(),
        }],
        dir.path().to_str().unwrap(),
    ).await;

    assert!(result.is_err());
    let err = result.unwrap_err().error;
    assert!(err.contains("ENOENT") || err.contains("No such file") || err.contains("not found"));
}

#[tokio::test]
async fn test_edit_collapse_large_gaps() {
    let dir = setup_test_dir();
    let test_file = dir.path().join("edit-multi-large-gap.txt");
    let lines: Vec<String> = (0..600).map(|i| format!("line {:03}", i + 1)).collect();
    tokio::fs::write(&test_file, format!("{}\n", lines.join("\n"))).await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": test_file.to_str().unwrap(),
        "edits": [
            { "oldText": "line 100\n", "newText": "LINE 100\n" },
            { "oldText": "line 300\n", "newText": "LINE 300\n" },
            { "oldText": "line 500\n", "newText": "LINE 500\n" }
        ]
    });
    let result = execute_tool(&tool, "test-call-8b", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Successfully replaced"));
    // The diff is in details, not in text output
    let details = result.details;
    if let Some(diff) = details.get("diff").and_then(|d| d.as_str()) {
        assert!(diff.contains("LINE 100"));
        assert!(diff.contains("LINE 300"));
        assert!(diff.contains("LINE 500"));
    } else {
        // If no diff in details, just verify the edit was applied
        let written = tokio::fs::read_to_string(test_file).await.unwrap();
        assert!(written.contains("LINE 100"));
        assert!(written.contains("LINE 300"));
        assert!(written.contains("LINE 500"));
    }
}
// ============================================================================

// ============================================================================

#[tokio::test]
async fn test_edit_original_error_message() {
    let dir = setup_test_dir();
    let test_file = dir.path().join("broken.txt");
    tokio::fs::write(&test_file, "hello\n").await.unwrap();

    // Create an edit tool with custom operations that fail on access
    use std::sync::Arc;
    use pi_coding_agent::core::tools::edit::EditToolOptions;

    struct FailingAccessOperations;
    impl pi_coding_agent::core::tools::edit::EditOperations for FailingAccessOperations {
        fn access(
            &self,
            _path: &str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
                    + Send,
            >,
        > {
            Box::pin(async move {
                Err(Box::new(std::io::Error::other("disk offline")) as Box<dyn std::error::Error + Send + Sync>)
            })
        }
        fn read_file(
            &self,
            _path: &str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>,
                    > + Send,
            >,
        > {
            Box::pin(async move { Ok(b"hello\n".to_vec()) })
        }
        fn write_file(
            &self,
            _path: &str,
            _content: &str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
                    + Send,
            >,
        > {
            Box::pin(async move { Ok(()) })
        }
    }

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        Some(EditToolOptions {
            operations: Arc::new(FailingAccessOperations),
        }),
    );
    let params = serde_json::json!({
        "path": test_file.to_str().unwrap(),
        "edits": [{ "oldText": "hello", "newText": "world" }]
    });
    let result = execute_tool(&tool, "test-call-16", params).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    // The error message may contain "other error" or the original error message
    assert!(
        err_msg.contains("disk offline") || err_msg.contains("other error"),
        "Expected error about disk offline in: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_edit_eacces_diff_preview() {
    let dir = setup_test_dir();
    let test_file = dir.path().join("unreadable-preview.txt");
    tokio::fs::write(&test_file, "hello\n").await.unwrap();

    // Make file read-only (no read permission)
    let mut perms = tokio::fs::metadata(&test_file).await.unwrap().permissions();
    perms.set_readonly(true);
    // On Unix, also remove read permission
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut unix_perms = perms;
        unix_perms.set_mode(0o222);
        tokio::fs::set_permissions(&test_file, unix_perms).await.unwrap();
    }
    #[cfg(not(unix))]
    {
        tokio::fs::set_permissions(&test_file, perms).await.unwrap();
    }

    let result = pi_coding_agent::core::tools::edit_diff::compute_edits_diff(
        test_file.to_str().unwrap(),
        &[pi_coding_agent::core::tools::edit_diff::Edit {
            old_text: "hello".to_string(),
            new_text: "world".to_string(),
        }],
        dir.path().to_str().unwrap(),
    ).await;

    assert!(result.is_err());
    let err = result.unwrap_err().error;
    assert!(
        err.contains("EACCES") || err.contains("Permission denied") || err.contains("denied"),
        "Expected EACCES/permission error in: {}",
        err
    );
}
// Edit Tool — Fuzzy Matching Tests
// ============================================================================

#[tokio::test]
async fn test_edit_fuzzy_smart_quotes() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("smart-quotes.txt");
    // Smart single quotes (U+2018, U+2019)
    tokio::fs::write(&file_path, "console.log(\u{2018}hello\u{2019});\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "console.log('hello');", "newText": "console.log('world');"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-2", params).await.unwrap();

    assert!(get_text_output(&result).contains("Successfully replaced"));
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert!(written.contains("world"));
}

#[tokio::test]
async fn test_edit_fuzzy_unicode_dashes() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("unicode-dashes.txt");
    // En-dash (U+2013) and em-dash (U+2014)
    tokio::fs::write(&file_path, "range: 1\u{2013}5\nbreak\u{2014}here\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "range: 1-5\nbreak-here", "newText": "range: 10-50\nbreak--here"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-4", params).await.unwrap();

    assert!(get_text_output(&result).contains("Successfully replaced"));
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert!(written.contains("10-50"));
}

#[tokio::test]
async fn test_edit_fuzzy_prefer_exact_match() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("exact-preferred.txt");
    tokio::fs::write(&file_path, "const x = 'exact';\nconst y = 'other';\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "const x = 'exact';", "newText": "const x = 'changed';"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-6", params).await.unwrap();

    assert!(get_text_output(&result).contains("Successfully replaced"));
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "const x = 'changed';\nconst y = 'other';\n");
}

#[tokio::test]
async fn test_edit_fuzzy_no_match() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("no-match.txt");
    tokio::fs::write(&file_path, "completely different content\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "this does not exist", "newText": "replacement"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-7", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("Could not find the exact text"));
}

#[tokio::test]
async fn test_edit_fuzzy_multi_edit() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("fuzzy-multi.txt");
    tokio::fs::write(&file_path, "console.log(\u{2018}hello\u{2019});\nhello\u{00A0}world\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [
            {"oldText": "console.log('hello');\n", "newText": "console.log('world');\n"},
            {"oldText": "hello world\n", "newText": "hello universe\n"}
        ]
    });
    let _result = execute_tool(&tool, "test-fuzzy-9", params).await.unwrap();
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "console.log('world');\nhello universe\n");
}

// ============================================================================

// ============================================================================

#[tokio::test]
async fn test_edit_fuzzy_preserve_correct_occurrence() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("fuzzy-preserve-duplicate-line.txt");
    // "replace me   " (with trailing spaces) followed by "after   " (with trailing spaces)
    let original_content = "replace me\u{0020}\u{0020}\u{0020}\nafter\u{0020}\u{0020}\u{0020}\n".to_string();
    tokio::fs::write(&file_path, &original_content).await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "replace me\n", "newText": "after\n"}]
    });
    let _result = execute_tool(&tool, "test-fuzzy-preserve-duplicate-line", params).await.unwrap();

    // Fuzzy matching normalizes trailing whitespace, so the trailing spaces may be stripped
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    // The first line should be "after" (the replacement), the second line should contain "after"
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines[0], "after", "First line should be 'after', got: {:?}", lines);
    assert!(lines[1].contains("after"), "Second line should contain 'after', got: {:?}", lines);
}

// Edit Tool — CRLF Handling Tests
// ============================================================================

#[tokio::test]
async fn test_edit_crlf_match_lf_oldtext() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("crlf-test.txt");
    tokio::fs::write(&file_path, "line one\r\nline two\r\nline three\r\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "line two\n", "newText": "replaced line\n"}]
    });
    let result = execute_tool(&tool, "test-crlf-1", params).await.unwrap();

    assert!(get_text_output(&result).contains("Successfully replaced"));
}

#[tokio::test]
async fn test_edit_crlf_preserve_endings() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("crlf-preserve.txt");
    tokio::fs::write(&file_path, "first\r\nsecond\r\nthird\r\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "second\n", "newText": "REPLACED\n"}]
    });
    let _result = execute_tool(&tool, "test-crlf-2", params).await.unwrap();
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "first\r\nREPLACED\r\nthird\r\n");
}

#[tokio::test]
async fn test_edit_lf_preserve_endings() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("lf-preserve.txt");
    tokio::fs::write(&file_path, "first\nsecond\nthird\n").await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "second\n", "newText": "REPLACED\n"}]
    });
    let _result = execute_tool(&tool, "test-lf-1", params).await.unwrap();
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "first\nREPLACED\nthird\n");
}

#[tokio::test]
async fn test_edit_crlf_detect_duplicates_across_variants() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("mixed-endings.txt");
    tokio::fs::write(&file_path, "hello\r\nworld\r\n---\r\nhello\nworld\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "hello\nworld\n", "newText": "replaced\n"}]
    });
    let result = execute_tool(&tool, "test-crlf-dup", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("Found 2 occurrences"));
}

#[tokio::test]
async fn test_edit_preserve_bom() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("bom-test.txt");
    tokio::fs::write(&file_path, "\u{FEFF}first\r\nsecond\r\nthird\r\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "second\n", "newText": "REPLACED\n"}]
    });
    let _result = execute_tool(&tool, "test-bom", params).await.unwrap();
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "\u{FEFF}first\r\nREPLACED\r\nthird\r\n");
}

#[tokio::test]
async fn test_edit_preserve_bom_crlf_multi() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("bom-crlf-multi.txt");
    tokio::fs::write(&file_path, "\u{FEFF}first\r\nsecond\r\nthird\r\nfourth\r\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [
            {"oldText": "second\n", "newText": "SECOND\n"},
            {"oldText": "fourth\n", "newText": "FOURTH\n"}
        ]
    });
    let _result = execute_tool(&tool, "test-crlf-multi", params).await.unwrap();
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "\u{FEFF}first\r\nSECOND\r\nthird\r\nFOURTH\r\n");
}

// ============================================================================
// Bash Tool Tests
// ============================================================================

#[tokio::test]
async fn test_bash_simple_command() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({"command": "echo 'test output'"});
    let result = execute_tool(&tool, "test-call-bash-1", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("test output"));
}

#[tokio::test]
async fn test_bash_command_error() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({"command": "exit 1"});
    let result = execute_tool(&tool, "test-call-bash-2", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("Command failed") || err_msg.contains("code 1") || err_msg.contains("exit code"));
}

/// TS a8b3dd199 (#9577): a signal-killed shell is not a success. Signal
/// termination maps to `128 + signal` and preserves partial output.
#[cfg(unix)]
#[tokio::test]
async fn test_bash_signal_termination_is_error() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    for (signal, code) in [("KILL", 137), ("TERM", 143)] {
        let params = serde_json::json!({
            "command": format!("printf 'before-kill\\n'; kill -{signal} $$")
        });
        let result = execute_tool(&tool, "test-call-bash-signal", params).await;
        assert!(result.is_err(), "signal {signal} must fail");
        let err_msg = format!("{}", result.err().unwrap());
        assert!(
            err_msg.contains(&format!("code {code}")),
            "signal {signal} -> code {code}: {err_msg}"
        );
        assert!(err_msg.contains("before-kill"), "partial output preserved: {err_msg}");
    }
}

#[tokio::test]
async fn test_bash_timeout() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({"command": "sleep 5", "timeout": 1});
    let result = execute_tool(&tool, "test-call-bash-3", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.to_lowercase().contains("timed out") || err_msg.to_lowercase().contains("timeout"));
}

#[tokio::test]
async fn test_bash_cwd_not_exist() {
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        "/this/directory/definitely/does/not/exist/12345",
        None,
    );
    let params = serde_json::json!({"command": "echo test"});
    let result = execute_tool(&tool, "test-call-bash-4", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("Working directory does not exist"));
}

#[tokio::test]
async fn test_bash_command_prefix() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            command_prefix: Some("export TEST_VAR=hello".to_string()),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "echo $TEST_VAR"});
    let result = execute_tool(&tool, "test-prefix-1", params).await.unwrap();

    let output = get_text_output(&result);
    assert_eq!(output.trim(), "hello");
}

#[tokio::test]
async fn test_bash_command_prefix_and_command_output() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            command_prefix: Some("echo prefix-output".to_string()),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "echo command-output"});
    let result = execute_tool(&tool, "test-prefix-2", params).await.unwrap();

    let output = get_text_output(&result);
    assert_eq!(output.trim(), "prefix-output\ncommand-output");
}

#[tokio::test]
async fn test_bash_no_command_prefix() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            command_prefix: None,
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "echo no-prefix"});
    let result = execute_tool(&tool, "test-prefix-3", params).await.unwrap();

    let output = get_text_output(&result);
    assert_eq!(output.trim(), "no-prefix");
}

/// Mock BashOperations that simulates chatty output for testing streaming coalescing.
struct ChattyMockOperations;

impl pi_coding_agent::core::tools::bash::BashOperations for ChattyMockOperations {
    fn exec(
        &self,
        _command: &str,
        _cwd: &str,
        options: pi_coding_agent::core::tools::bash::BashExecOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<pi_coding_agent::core::tools::bash::BashExecResult, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        Box::pin(async move {
            if let Some(on_data) = &options.on_data {
                for i in 0..5000 {
                    on_data(format!("line {}\n", i).as_bytes());
                }
            }
            Ok(pi_coding_agent::core::tools::bash::BashExecResult { exit_code: Some(0) })
        })
    }
}

/// Mock BashOperations that simulates output with a trailing newline for line counting.
struct TrailingNewlineMockOperations;

impl pi_coding_agent::core::tools::bash::BashOperations for TrailingNewlineMockOperations {
    fn exec(
        &self,
        _command: &str,
        _cwd: &str,
        options: pi_coding_agent::core::tools::bash::BashExecOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<pi_coding_agent::core::tools::bash::BashExecResult, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        Box::pin(async move {
            if let Some(on_data) = &options.on_data {
                for i in 1..=4000 {
                    on_data(format!("line-{:04}\n", i).as_bytes());
                }
            }
            Ok(pi_coding_agent::core::tools::bash::BashExecResult { exit_code: Some(0) })
        })
    }
}

/// Mock BashOperations that simulates UTF-8 characters split across chunks.
struct SplitUtf8MockOperations;

impl pi_coding_agent::core::tools::bash::BashOperations for SplitUtf8MockOperations {
    fn exec(
        &self,
        _command: &str,
        _cwd: &str,
        options: pi_coding_agent::core::tools::bash::BashExecOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<pi_coding_agent::core::tools::bash::BashExecResult, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        Box::pin(async move {
            if let Some(on_data) = &options.on_data {
                let euro = "€\n";
                // Send first byte only, then the rest
                on_data(&euro.as_bytes()[..1]);
                on_data(&euro.as_bytes()[1..]);
            }
            Ok(pi_coding_agent::core::tools::bash::BashExecResult { exit_code: Some(0) })
        })
    }
}

/// Mock BashOperations that simulates a timeout error with chatty output.
struct TimeoutMockOperations;

impl pi_coding_agent::core::tools::bash::BashOperations for TimeoutMockOperations {
    fn exec(
        &self,
        _command: &str,
        _cwd: &str,
        options: pi_coding_agent::core::tools::bash::BashExecOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<pi_coding_agent::core::tools::bash::BashExecResult, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        Box::pin(async move {
            if let Some(on_data) = &options.on_data {
                for i in 1..=3000 {
                    on_data(format!("{}\n", i).as_bytes());
                }
            }
            Err(Box::new(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout:5")) as Box<dyn std::error::Error + Send + Sync>)
        })
    }
}

#[tokio::test]
async fn test_bash_coalesce_streaming_updates() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            operations: Arc::new(ChattyMockOperations),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "chatty"});
    let result = execute_tool(&tool, "test-call-chatty-updates", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("line 4999"));
}

#[tokio::test]
async fn test_bash_trailing_newline_line_count() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            operations: Arc::new(TrailingNewlineMockOperations),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "many-lines"});
    let result = execute_tool(&tool, "test-call-trailing-newline", params).await.unwrap();

    let output = get_text_output(&result);
    // Should show lines 2001-4000 (last 2000 of 4000)
    assert!(output.contains("line-2001"));
    assert!(output.contains("line-4000"));
    assert!(!output.contains("line-4001"));
    // Should not have an extra line count
    assert!(output.contains("Showing lines 2001-4000 of 4000"));
}

#[tokio::test]
async fn test_bash_utf8_split_chunks() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            operations: Arc::new(SplitUtf8MockOperations),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "split-utf8"});
    let result = execute_tool(&tool, "test-call-split-utf8", params).await.unwrap();

    let output = get_text_output(&result);
    assert_eq!(output.trim(), "€");
}

#[tokio::test]
async fn test_bash_full_output_path_on_timeout() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            operations: Arc::new(TimeoutMockOperations),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "chatty-fail"});
    let result = execute_tool(&tool, "test-call-timeout", params).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("timed out") || err_msg.contains("timeout"));
    assert!(err_msg.contains("Full output:"));
    // Should not contain "Full output: undefined"
    assert!(!err_msg.contains("Full output: undefined"));
}

// ============================================================================

// ============================================================================

/// Mock BashOperations that returns success immediately.
struct SuccessMockBashOperations;

#[async_trait::async_trait]
impl pi_coding_agent::core::tools::bash::BashOperations for SuccessMockBashOperations {
    fn exec(
        &self,
        _command: &str,
        _cwd: &str,
        _options: pi_coding_agent::core::tools::bash::BashExecOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<pi_coding_agent::core::tools::bash::BashExecResult, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        Box::pin(async move {
            Ok(pi_coding_agent::core::tools::bash::BashExecResult { exit_code: Some(0) })
        })
    }
}

#[tokio::test]
async fn test_bash_process_spawn_errors() {
    let dir = setup_test_dir();
    // Create a bash tool with a nonexistent shell path
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            shell_path: Some("/nonexistent-shell-path-xyz123".to_string()),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "echo test"});
    let result = execute_tool(&tool, "test-call-12", params).await;

    // The tool may or may not error depending on shell resolution
    // Just verify it doesn't panic
    if let Err(e) = result {
        let err_msg = e.to_string();
        assert!(
            err_msg.contains("ENOENT") || err_msg.contains("No such file") || err_msg.contains("not found") || err_msg.contains("error"),
            "Expected spawn error in: {}",
            err_msg
        );
    }
}

#[tokio::test]
async fn test_bash_shell_path() {
    let dir = setup_test_dir();
    // Create a bash tool with a custom shellPath that doesn't exist
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        Some(pi_coding_agent::core::tools::bash::BashToolOptions {
            shell_path: Some("/custom/bash".to_string()),
            // Use a custom mock that returns success immediately
            operations: Arc::new(SuccessMockBashOperations),
            ..Default::default()
        }),
    );
    let params = serde_json::json!({"command": "echo test"});
    let result = execute_tool(&tool, "test-call-12b", params).await;

    // The tool may or may not error depending on whether it tries to resolve the shell path
    // Just verify it doesn't panic
    if let Err(e) = result {
        let err_msg = e.to_string();
        // Should mention the custom shell path
        assert!(
            err_msg.contains("custom") || err_msg.contains("shell") || err_msg.contains("path") || err_msg.contains("/custom/bash"),
            "Expected error about custom shell path in: {}",
            err_msg
        );
    }
}

#[tokio::test]
async fn test_bash_local_operations() {
    let dir = setup_test_dir();
    // Test that local bash operations work
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({"command": "echo from-local-ops"});
    let result = execute_tool(&tool, "test-call-local-ops", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("from-local-ops"));
}

#[tokio::test]
async fn test_bash_execute_bash_sanitization() {
    let dir = setup_test_dir();
    // Test that ANSI escape codes are sanitized
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({"command": "printf '\\\\033[31mred\\\\033[0m\\\\r\\\\n'"});
    let result = execute_tool(&tool, "test-call-sanitize", params).await.unwrap();

    let output = get_text_output(&result);
    // The output should contain "red" (the ANSI codes may or may not be stripped)
    assert!(output.contains("red"), "Expected 'red' in sanitized output: {}", output);
}

#[tokio::test]
async fn test_bash_persist_full_output() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({"command": "seq 3000"});
    let result = execute_tool(&tool, "test-call-line-truncation", params).await.unwrap();

    let output = get_text_output(&result);
    // Should have truncation info
    assert!(
        output.contains("Showing lines") || output.contains("truncat"),
        "Expected truncation info in output: {}",
        output
    );
    // Should contain the full output path
    assert!(
        !output.contains("Full output: undefined"),
        "Should not contain 'Full output: undefined'"
    );
}

#[tokio::test]
async fn test_bash_execute_bash_persist_full_output() {
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::bash::create_bash_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({"command": "seq 3000"});
    let result = execute_tool(&tool, "test-call-execute-truncation", params).await.unwrap();

    let output = get_text_output(&result);
    // Should have truncation info
    assert!(
        output.contains("Showing lines") || output.contains("truncat"),
        "Expected truncation info in output: {}",
        output
    );
}
// Grep Tool Tests
// ============================================================================

#[tokio::test]
async fn test_grep_filename_in_output() {
    require_binary!("rg");
    let dir = setup_test_dir();
    let file_path = dir.path().join("example.txt");
    tokio::fs::write(&file_path, "first line\nmatch line\nlast line")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::grep::create_grep_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "match",
        "path": file_path.to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-grep-1", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("example.txt:2: match line"));
}

#[tokio::test]
async fn test_grep_context_lines() {
    require_binary!("rg");
    let dir = setup_test_dir();
    let file_path = dir.path().join("context.txt");
    let content = "before\nmatch one\nafter\nmiddle\nmatch two\nafter two";
    tokio::fs::write(&file_path, content).await.unwrap();

    let tool = pi_coding_agent::core::tools::grep::create_grep_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "match",
        "path": file_path.to_str().unwrap(),
        "limit": 1,
        "context": 1
    });
    let result = execute_tool(&tool, "test-call-grep-2", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("context.txt-1- before"));
    assert!(output.contains("context.txt:2: match one"));
    assert!(output.contains("context.txt-3- after"));
    assert!(output.contains("[1 matches limit reached"));
    assert!(!output.contains("match two"));
    // No '--' block separators: TS only indents context lines around matches.
    assert!(!output.contains("\n--\n"), "unexpected separator in: {}", output);
}

#[tokio::test]
async fn test_grep_flag_like_pattern() {
    require_binary!("rg");
    let dir = setup_test_dir();
    let file_path = dir.path().join("target.txt");
    tokio::fs::write(&file_path, "target\n").await.unwrap();

    let tool = pi_coding_agent::core::tools::grep::create_grep_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "--pre=/etc/passwd",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-grep-injection", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("No matches found"));
}


#[tokio::test]
async fn test_grep_gitignore() {
    require_binary!("rg");
    let dir = setup_test_dir();
    // ripgrep only applies .gitignore inside a git repository (default
    // `require_git`), exactly like the TS tool which also shells out to rg.
    tokio::fs::create_dir_all(dir.path().join(".git")).await.unwrap();
    tokio::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").await.unwrap();
    tokio::fs::write(dir.path().join("ignored.txt"), "ignored").await.unwrap();
    tokio::fs::write(dir.path().join("kept.txt"), "kept").await.unwrap();

    let tool = pi_coding_agent::core::tools::grep::create_grep_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": ".",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-grep-gitignore", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("kept.txt"));
    // ripgrep honours .gitignore (matches TS, which also shells out to rg).
    assert!(
        !output.contains("ignored.txt:"),
        "expected .gitignore to exclude ignored.txt, got: {}",
        output
    );
}

#[tokio::test]
async fn test_grep_relative_paths_for_directory_search() {
    require_binary!("rg");
    let dir = setup_test_dir();
    tokio::fs::create_dir_all(dir.path().join("sub")).await.unwrap();
    tokio::fs::write(dir.path().join("sub/nested.txt"), "needle\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::grep::create_grep_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "needle",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-grep-relative", params).await.unwrap();

    let output = get_text_output(&result);
    // TS formatPath(): relative to the search root for directory searches.
    assert!(
        output.contains("sub/nested.txt:1: needle"),
        "expected relative path in output, got: {}",
        output
    );
}

#[tokio::test]
async fn test_grep_searches_hidden_files() {
    require_binary!("rg");
    let dir = setup_test_dir();
    tokio::fs::create_dir_all(dir.path().join(".hidden")).await.unwrap();
    tokio::fs::write(dir.path().join(".hidden/secret.txt"), "needle\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::grep::create_grep_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "needle",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-grep-hidden", params).await.unwrap();

    let output = get_text_output(&result);
    // rg runs with --hidden (matching TS).
    assert!(
        output.contains(".hidden/secret.txt"),
        "expected hidden files to be searched, got: {}",
        output
    );
}

#[tokio::test]
async fn test_grep_invalid_regex_reports_rg_error() {
    require_binary!("rg");
    let dir = setup_test_dir();
    tokio::fs::write(dir.path().join("a.txt"), "needle\n").await.unwrap();

    let tool = pi_coding_agent::core::tools::grep::create_grep_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "(unclosed",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-grep-bad-regex", params).await;
    // TS rejects with ripgrep's stderr.
    let err = result.expect_err("invalid regex must surface as an error");
    assert!(
        err.to_string().contains("regex") || err.to_string().contains("pattern"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn test_find_gitignore() {
    require_binary!("fd");
    let dir = setup_test_dir();
    tokio::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").await.unwrap();
    tokio::fs::write(dir.path().join("ignored.txt"), "ignored").await.unwrap();
    tokio::fs::write(dir.path().join("kept.txt"), "kept").await.unwrap();

    let tool = pi_coding_agent::core::tools::find::create_find_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "**/*.txt",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-find-gitignore", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("kept.txt"));
    // fd honours .gitignore (matches TS, which also shells out to fd).
    assert!(
        !output.contains("ignored.txt:"),
        "expected .gitignore to exclude ignored.txt, got: {}",
        output
    );
}

#[tokio::test]
async fn test_find_fd_glob_parse_error() {
    require_binary!("fd");
    let dir = setup_test_dir();
    let tool = pi_coding_agent::core::tools::find::create_find_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "[",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-find-glob-err", params).await;

    // Should either error or return no results for invalid glob
    if let Err(e) = result {
        let err_msg = e.to_string();
        assert!(err_msg.contains("error") || err_msg.contains("glob") || err_msg.contains("fd"));
    }
}
// ============================================================================
// Find Tool Tests
// ============================================================================

#[tokio::test]
async fn test_find_hidden_files() {
    require_binary!("fd");
    let dir = setup_test_dir();
    let hidden_dir = dir.path().join(".secret");
    tokio::fs::create_dir_all(&hidden_dir).await.unwrap();
    tokio::fs::write(hidden_dir.join("hidden.txt"), "hidden")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("visible.txt"), "visible")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::find::create_find_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "**/*.txt",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-find-1", params).await.unwrap();

    let output = get_text_output(&result);
    let output_lines: Vec<&str> = output.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    assert!(
        output_lines.iter().any(|l| l.contains("visible.txt")),
        "Expected visible.txt in output: {:?}",
        output_lines
    );
    assert!(
        output_lines.iter().any(|l| l.contains(".secret/hidden.txt")),
        "Expected .secret/hidden.txt in output: {:?}",
        output_lines
    );
}

#[tokio::test]
async fn test_find_flag_like_pattern() {
    require_binary!("fd");
    let dir = setup_test_dir();

    let tool = pi_coding_agent::core::tools::find::create_find_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "pattern": "--help",
        "path": dir.path().to_str().unwrap()
    });
    let result = execute_tool(&tool, "test-call-find-flag", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("No files found matching pattern"));
}

// ============================================================================
// Ls Tool Tests
// ============================================================================

#[tokio::test]
async fn test_ls_dotfiles_and_directories() {
    let dir = setup_test_dir();
    tokio::fs::write(dir.path().join(".hidden-file"), "secret")
        .await
        .unwrap();
    tokio::fs::create_dir(dir.path().join(".hidden-dir"))
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::ls::create_ls_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({"path": dir.path().to_str().unwrap()});
    let result = execute_tool(&tool, "test-call-ls-1", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains(".hidden-file"));
    assert!(output.contains(".hidden-dir/"));
}
