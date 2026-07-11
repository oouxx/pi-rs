//! Tool tests for pi-coding-agent.
//!
//! Mirrors the original TypeScript tools.test.ts from
//! https://github.com/earendil-works/pi/tree/main/packages/coding-agent/test/tools.test.ts
//!
//! Run with: cargo test -p pi-coding-agent --test tools_test -- --nocapture

use pi_agent_core::types::AgentTool;
use tokio::io::AsyncWriteExt;

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
    let result = execute_tool(&tool, "test-call-2", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Error reading file") || output.contains("not found"));
}

#[tokio::test]
async fn test_read_truncate_by_lines() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("large.txt");
    let lines: Vec<String> = (1..=2500).map(|i| format!("Line {}", i)).collect();
    tokio::fs::write(&file_path, lines.join("\n")).await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({ "path": file_path.to_str().unwrap() });
    let result = execute_tool(&tool, "test-call-3", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Line 1"));
    assert!(output.contains("Line 2000"));
    assert!(!output.contains("Line 2001"));
    assert!(output.contains("[Showing lines 1-2000 of 2500. Use offset=2001 to continue.]"));
}

#[tokio::test]
async fn test_read_with_offset() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("offset-test.txt");
    let lines: Vec<String> = (1..=100).map(|i| format!("Line {}", i)).collect();
    tokio::fs::write(&file_path, lines.join("\n")).await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "offset": 51
    });
    let result = execute_tool(&tool, "test-call-5", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(!output.contains("Line 50"));
    assert!(output.contains("Line 51"));
    assert!(output.contains("Line 100"));
    assert!(!output.contains("Use offset="));
}

#[tokio::test]
async fn test_read_with_limit() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("limit-test.txt");
    let lines: Vec<String> = (1..=100).map(|i| format!("Line {}", i)).collect();
    tokio::fs::write(&file_path, lines.join("\n")).await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "limit": 10
    });
    let result = execute_tool(&tool, "test-call-6", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(output.contains("Line 1"));
    assert!(output.contains("Line 10"));
    assert!(!output.contains("Line 11"));
    assert!(output.contains("[90 more lines in file. Use offset=11 to continue.]"));
}

#[tokio::test]
async fn test_read_with_offset_and_limit() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("offset-limit-test.txt");
    let lines: Vec<String> = (1..=100).map(|i| format!("Line {}", i)).collect();
    tokio::fs::write(&file_path, lines.join("\n")).await.unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "offset": 41,
        "limit": 20
    });
    let result = execute_tool(&tool, "test-call-7", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(!output.contains("Line 40"));
    assert!(output.contains("Line 41"));
    assert!(output.contains("Line 60"));
    assert!(!output.contains("Line 61"));
    assert!(output.contains("[40 more lines in file. Use offset=61 to continue.]"));
}

#[tokio::test]
async fn test_read_offset_beyond_file() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("short.txt");
    tokio::fs::write(&file_path, "Line 1\nLine 2\nLine 3")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::read::create_read_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "offset": 100
    });
    let result = execute_tool(&tool, "test-call-8", params).await.unwrap();

    let output = get_text_output(&result);
    assert!(
        output.contains("Offset 100 is beyond end of file (3 lines total)")
            || output.contains("Offset 100 is beyond end of file")
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
        assert_eq!(truncation.get("truncated").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            truncation.get("truncated_by").and_then(|v| v.as_str()),
            Some("lines")
        );
        assert_eq!(
            truncation.get("total_lines").and_then(|v| v.as_u64()),
            Some(2500)
        );
        assert_eq!(
            truncation.get("output_lines").and_then(|v| v.as_u64()),
            Some(2000)
        );
    }
}

// ============================================================================
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
    assert!(details.get("diff").and_then(|v| v.as_str()).is_some());
    assert!(details.get("patch").and_then(|v| v.as_str()).is_some());

    let patch = details.get("patch").and_then(|v| v.as_str()).unwrap();
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
    let result = execute_tool(&tool, "test-call-edit-9", params).await
        .expect("Edit should succeed");

    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "foo bar\nBAR\nbaz\n");
}

// ============================================================================
// Edit Tool — Fuzzy Matching Tests
// ============================================================================

#[tokio::test]
async fn test_edit_fuzzy_trailing_whitespace() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("trailing-ws.txt");
    tokio::fs::write(&file_path, "line one   \nline two  \nline three\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "line one\nline two\n", "newText": "replaced\n"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-1", params).await.unwrap();

    assert!(get_text_output(&result).contains("Successfully replaced"));
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "replaced\nline three\n");
}

#[tokio::test]
async fn test_edit_fuzzy_chinese_punctuation() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("chinese-punctuation.txt");
    tokio::fs::write(&file_path, "你好，世界\n你好（世界）\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "你好,世界\n你好(世界)\n", "newText": "你好，pi\n你好(pi)\n"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-chinese", params).await.unwrap();

    assert!(get_text_output(&result).contains("Successfully replaced"));
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(written, "你好，pi\n你好(pi)\n");
}

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
async fn test_edit_fuzzy_smart_double_quotes() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("smart-double-quotes.txt");
    // Smart double quotes (U+201C, U+201D)
    tokio::fs::write(&file_path, "const msg = \u{201C}Hello World\u{201D};\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "const msg = \"Hello World\";", "newText": "const msg = \"Goodbye\";"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-3", params).await.unwrap();

    assert!(get_text_output(&result).contains("Successfully replaced"));
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert!(written.contains("Goodbye"));
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
async fn test_edit_fuzzy_non_breaking_space() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("nbsp.txt");
    // Non-breaking space (U+00A0)
    tokio::fs::write(&file_path, "hello\u{00A0}world\n").await.unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "hello world", "newText": "hello universe"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-5", params).await.unwrap();

    assert!(get_text_output(&result).contains("Successfully replaced"));
    let written = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert!(written.contains("universe"));
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
async fn test_edit_fuzzy_duplicates_after_normalization() {
    let dir = setup_test_dir();
    let file_path = dir.path().join("fuzzy-dups.txt");
    tokio::fs::write(&file_path, "hello world   \nhello world\n")
        .await
        .unwrap();

    let tool = pi_coding_agent::core::tools::edit::create_edit_tool(
        dir.path().to_str().unwrap(),
        None,
    );
    let params = serde_json::json!({
        "path": file_path.to_str().unwrap(),
        "edits": [{"oldText": "hello world", "newText": "replaced"}]
    });
    let result = execute_tool(&tool, "test-fuzzy-8", params).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("Found 2 occurrences"));
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

// ============================================================================
// Grep Tool Tests
// ============================================================================

#[tokio::test]
async fn test_grep_filename_in_output() {
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
}

#[tokio::test]
async fn test_grep_flag_like_pattern() {
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

// ============================================================================
// Find Tool Tests
// ============================================================================

#[tokio::test]
async fn test_find_hidden_files() {
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
    let output_lines: Vec<&str> = output.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
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
