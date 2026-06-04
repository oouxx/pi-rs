//! Shared diff computation utilities for the edit tool.
//!
//! Port of `packages/coding-agent/src/core/tools/edit-diff.ts`.
//!
//! Provides fuzzy text matching, line-ending normalization, Unicode
//! normalization, edit application, and diff/patch generation.

use std::fmt;

use similar::TextDiff;
use unicode_normalization::UnicodeNormalization;

use super::path_utils;

// ============================================================================
// Types
// ============================================================================

/// Line ending style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    CrLf,
    Lf,
}

/// A single edit operation: replace `old_text` with `new_text`.
///
/// Equivalent to `Edit` in edit-diff.ts.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

/// Result of a fuzzy text search.
///
/// Equivalent to `FuzzyMatchResult` in edit-diff.ts.
#[derive(Debug, Clone)]
pub struct FuzzyMatchResult {
    /// Whether a match was found.
    pub found: bool,
    /// The index where the match starts.
    pub index: usize,
    /// Length of the matched text.
    pub match_length: usize,
    /// Whether fuzzy matching was used (false = exact match).
    pub used_fuzzy_match: bool,
    /// The content to use for replacement operations.
    /// When exact match: original content. When fuzzy match: normalized content.
    pub content_for_replacement: String,
}

/// Result of stripping a UTF-8 BOM.
#[derive(Debug, Clone)]
pub struct BomResult {
    /// The BOM character if present, empty string otherwise.
    pub bom: String,
    /// Content without the BOM.
    pub text: String,
}

/// Result of applying edits to normalized content.
///
/// Equivalent to `AppliedEditsResult` in edit-diff.ts.
#[derive(Debug, Clone)]
pub struct AppliedEditsResult {
    /// The base content (original or fuzzy-normalized) that edits were matched against.
    pub base_content: String,
    /// The new content after applying all edits.
    pub new_content: String,
}

/// Display-oriented diff result with line numbers and context.
///
/// Equivalent to `EditDiffResult` in edit-diff.ts.
#[derive(Debug, Clone)]
pub struct EditDiffResult {
    /// The formatted diff string.
    pub diff: String,
    /// The first changed line number (in the new file), if any.
    pub first_changed_line: Option<usize>,
}

/// Error from computing a diff for preview.
///
/// Equivalent to `EditDiffError` in edit-diff.ts.
#[derive(Debug, Clone)]
pub struct EditDiffError {
    pub error: String,
}

/// Display-oriented diff result (from `generate_diff_string`).
#[derive(Debug, Clone)]
pub struct DiffResult {
    pub diff: String,
    pub first_changed_line: Option<usize>,
}

/// Errors that can occur when applying edits.
///
/// Mirrors the error messages from edit-diff.ts.
#[derive(Debug, Clone)]
pub enum EditError {
    NotFound {
        path: String,
        edit_index: usize,
        total_edits: usize,
    },
    Duplicate {
        path: String,
        edit_index: usize,
        total_edits: usize,
        occurrences: usize,
    },
    EmptyOldText {
        path: String,
        edit_index: usize,
        total_edits: usize,
    },
    NoChange {
        path: String,
        total_edits: usize,
    },
    Overlap {
        path: String,
        first: usize,
        second: usize,
    },
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path, edit_index, total_edits } => {
                if *total_edits == 1 {
                    write!(
                        f,
                        "Could not find the exact text in {path}. \
                         The old text must match exactly including all whitespace and newlines."
                    )
                } else {
                    write!(
                        f,
                        "Could not find edits[{edit_index}] in {path}. \
                         The oldText must match exactly including all whitespace and newlines."
                    )
                }
            }
            Self::Duplicate { path, edit_index, total_edits, occurrences } => {
                if *total_edits == 1 {
                    write!(
                        f,
                        "Found {occurrences} occurrences of the text in {path}. \
                         The text must be unique. Please provide more context to make it unique."
                    )
                } else {
                    write!(
                        f,
                        "Found {occurrences} occurrences of edits[{edit_index}] in {path}. \
                         Each oldText must be unique. Please provide more context to make it unique."
                    )
                }
            }
            Self::EmptyOldText { path, edit_index, total_edits } => {
                if *total_edits == 1 {
                    write!(f, "oldText must not be empty in {path}.")
                } else {
                    write!(f, "edits[{edit_index}].oldText must not be empty in {path}.")
                }
            }
            Self::NoChange { path, total_edits: _ } => {
                write!(
                    f,
                    "No changes made to {path}. \
                     The replacement produced identical content. \
                     This might indicate an issue with special characters \
                     or the text not existing as expected."
                )
            }
            Self::Overlap { path, first, second } => {
                write!(
                    f,
                    "edits[{first}] and edits[{second}] overlap in {path}. \
                     Merge them into one edit or target disjoint regions."
                )
            }
        }
    }
}

// ============================================================================
// Text processing
// ============================================================================

/// Detect the dominant line ending style in `content`.
///
/// Equivalent to `detectLineEnding` in edit-diff.ts.
/// Returns `CrLf` if `\r\n` appears before the first standalone `\n`, else `Lf`.
pub fn detect_line_ending(content: &str) -> LineEnding {
    let crlf_idx = content.find("\r\n");
    let lf_idx = content.find('\n');
    match (crlf_idx, lf_idx) {
        (Some(crlf), Some(lf)) if crlf < lf => LineEnding::CrLf,
        _ => LineEnding::Lf,
    }
}

/// Normalize all line endings to LF (`\n`).
///
/// Equivalent to `normalizeToLF` in edit-diff.ts.
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Restore line endings to the original style.
///
/// Equivalent to `restoreLineEndings` in edit-diff.ts.
pub fn restore_line_endings(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::CrLf => text.replace('\n', "\r\n"),
        LineEnding::Lf => text.to_string(),
    }
}

/// Normalize text for fuzzy matching.
///
/// Applies progressive transformations:
/// - NFKC Unicode normalization
/// - Strip trailing whitespace from each line
/// - Normalize smart quotes to ASCII equivalents
/// - Normalize Unicode dashes/hyphens to ASCII hyphen
/// - Normalize special Unicode spaces to regular space
///
/// Equivalent to `normalizeForFuzzyMatch` in edit-diff.ts.
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    let nfkc: String = text.nfkc().collect();
    let mut result = String::with_capacity(nfkc.len());

    // Split into lines, trim trailing whitespace, join
    for line in nfkc.lines() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(line.trim_end());
    }
    // Handle trailing newline
    if nfkc.ends_with('\n') {
        result.push('\n');
    }

    // Smart single quotes → '
    result = result
        .replace('\u{2018}', "'")
        .replace('\u{2019}', "'")
        .replace('\u{201A}', "'")
        .replace('\u{201B}', "'");

    // Smart double quotes → "
    result = result
        .replace('\u{201C}', "\"")
        .replace('\u{201D}', "\"")
        .replace('\u{201E}', "\"")
        .replace('\u{201F}', "\"");

    // Various dashes/hyphens → -
    for ch in [
        '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}',
        '\u{2014}', '\u{2015}', '\u{2212}',
    ] {
        result = result.replace(ch, "-");
    }

    // Special spaces → regular space
    // NBSP \u{00A0}, various spaces \u{2002}-\u{200A},
    // narrow NBSP \u{202F}, medium math space \u{205F}, ideographic space \u{3000}
    let mut spaced = String::with_capacity(result.len());
    for ch in result.chars() {
        match ch {
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => {
                spaced.push(' ');
            }
            c => spaced.push(c),
        }
    }

    spaced
}

/// Strip UTF-8 BOM if present.
///
/// Equivalent to `stripBom` in edit-diff.ts.
pub fn strip_bom(content: &str) -> BomResult {
    if content.starts_with('\u{FEFF}') {
        BomResult {
            bom: "\u{FEFF}".to_string(),
            text: content.chars().skip(1).collect(),
        }
    } else {
        BomResult {
            bom: String::new(),
            text: content.to_string(),
        }
    }
}

// ============================================================================
// Text search and matching
// ============================================================================

/// Find `old_text` in `content`, trying exact match first, then fuzzy match.
///
/// When fuzzy matching is used, the returned `content_for_replacement` is the
/// fuzzy-normalized version of the content.
///
/// Equivalent to `fuzzyFindText` in edit-diff.ts.
pub fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatchResult {
    // Try exact match first
    if let Some(index) = content.find(old_text) {
        return FuzzyMatchResult {
            found: true,
            index,
            match_length: old_text.len(),
            used_fuzzy_match: false,
            content_for_replacement: content.to_string(),
        };
    }

    // Try fuzzy match
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);

    if let Some(index) = fuzzy_content.find(&fuzzy_old_text) {
        FuzzyMatchResult {
            found: true,
            index,
            match_length: fuzzy_old_text.len(),
            used_fuzzy_match: true,
            content_for_replacement: fuzzy_content,
        }
    } else {
        FuzzyMatchResult {
            found: false,
            index: 0,
            match_length: 0,
            used_fuzzy_match: false,
            content_for_replacement: content.to_string(),
        }
    }
}

/// Count occurrences of `old_text` in `content` using fuzzy matching.
///
/// Equivalent to the `countOccurrences` helper in edit-diff.ts.
pub fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);

    if fuzzy_old_text.is_empty() {
        return 0;
    }

    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = fuzzy_content[start..].find(&fuzzy_old_text) {
        count += 1;
        start += pos + 1;
    }
    count
}

// ============================================================================
// Edit application
// ============================================================================

struct MatchedEdit {
    edit_index: usize,
    match_index: usize,
    match_length: usize,
    new_text: String,
}

/// Apply one or more exact-text replacements to LF-normalized content.
///
/// All edits are matched against the same original content. Replacements are
/// then applied in reverse order so offsets remain stable.
///
/// Equivalent to `applyEditsToNormalizedContent` in edit-diff.ts.
pub fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<AppliedEditsResult, EditError> {
    let total = edits.len();

    // Normalize edit texts to LF
    let normalized_edits: Vec<Edit> = edits
        .iter()
        .map(|e| Edit {
            old_text: normalize_to_lf(&e.old_text),
            new_text: normalize_to_lf(&e.new_text),
        })
        .collect();

    // Check for empty oldText
    for (i, edit) in normalized_edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(EditError::EmptyOldText {
                path: path.to_string(),
                edit_index: i,
                total_edits: total,
            });
        }
    }

    // Find all matches
    let initial_matches: Vec<FuzzyMatchResult> = normalized_edits
        .iter()
        .map(|edit| fuzzy_find_text(normalized_content, &edit.old_text))
        .collect();

    let base_content = if initial_matches.iter().any(|m| m.used_fuzzy_match) {
        normalize_for_fuzzy_match(normalized_content)
    } else {
        normalized_content.to_string()
    };

    let mut matched_edits: Vec<MatchedEdit> = Vec::with_capacity(normalized_edits.len());

    for (i, edit) in normalized_edits.iter().enumerate() {
        let match_result = fuzzy_find_text(&base_content, &edit.old_text);

        if !match_result.found {
            return Err(EditError::NotFound {
                path: path.to_string(),
                edit_index: i,
                total_edits: total,
            });
        }

        let occurrences = count_occurrences(&base_content, &edit.old_text);
        if occurrences > 1 {
            return Err(EditError::Duplicate {
                path: path.to_string(),
                edit_index: i,
                total_edits: total,
                occurrences,
            });
        }

        matched_edits.push(MatchedEdit {
            edit_index: i,
            match_index: match_result.index,
            match_length: match_result.match_length,
            new_text: edit.new_text.clone(),
        });
    }

    // Check for overlaps
    matched_edits.sort_by_key(|m| m.match_index);
    for i in 1..matched_edits.len() {
        let prev = &matched_edits[i - 1];
        let curr = &matched_edits[i];
        if prev.match_index + prev.match_length > curr.match_index {
            return Err(EditError::Overlap {
                path: path.to_string(),
                first: prev.edit_index,
                second: curr.edit_index,
            });
        }
    }

    // Apply edits in reverse order to preserve offsets
    let mut new_content = base_content.clone();
    for m in matched_edits.into_iter().rev() {
        new_content = format!(
            "{}{}{}",
            &new_content[..m.match_index],
            m.new_text,
            &new_content[m.match_index + m.match_length..]
        );
    }

    if base_content == new_content {
        return Err(EditError::NoChange {
            path: path.to_string(),
            total_edits: total,
        });
    }

    Ok(AppliedEditsResult {
        base_content,
        new_content,
    })
}

// ============================================================================
// Diff generation
// ============================================================================

/// Generate a standard unified patch.
///
/// Equivalent to `generateUnifiedPatch` in edit-diff.ts.
pub fn generate_unified_patch(
    path: &str,
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> String {
    let diff = TextDiff::from_lines(old_content, new_content);
    let mut buf: Vec<u8> = Vec::new();
    diff.unified_diff()
        .context_radius(context_lines)
        .header(path, path)
        .to_writer(&mut buf)
        .expect("writing to vec should not fail");
    String::from_utf8(buf).unwrap_or_default()
}

/// Generate a display-oriented diff string with line numbers and context.
///
/// Equivalent to `generateDiffString` in edit-diff.ts.
pub fn generate_diff_string(
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> DiffResult {
    let diff = TextDiff::from_lines(old_content, new_content);
    let grouped_ops = diff.grouped_ops(context_lines);

    let old_lines: Vec<&str> = old_content.split('\n').collect();
    let new_lines: Vec<&str> = new_content.split('\n').collect();
    let max_line_num = old_lines.len().max(new_lines.len());
    let line_num_width = if max_line_num > 0 {
        (max_line_num as f64).log10().floor() as usize + 1
    } else {
        1
    };

    let mut output: Vec<String> = Vec::new();
    let mut first_changed_line: Option<usize> = None;

    for group in &grouped_ops {
        let first_op = &group[0];

        let old_start = first_op.old_range().start;
        let new_start = first_op.new_range().start;

        let mut old_line = old_start;
        let mut new_line = new_start;

        for op in group {
            let (old_range, new_range) = (op.old_range(), op.new_range());

            match op {
                similar::DiffOp::Equal { .. } => {
                    let len = old_range.len();
                    for i in 0..len {
                        let idx = old_range.start + i;
                        let line = if idx < old_lines.len() { old_lines[idx] } else { "" };
                        if old_line == old_start && new_line == new_start {
                            let ln = pad_number(old_line, line_num_width);
                            output.push(format!(" {} {}", ln, line));
                        } else if i < context_lines || i >= len - context_lines {
                            let ln = pad_number(old_line, line_num_width);
                            output.push(format!(" {} {}", ln, line));
                        } else if i == context_lines {
                            output.push(format!(" {} ...", " ".repeat(line_num_width)));
                        }
                        old_line += 1;
                        new_line += 1;
                    }
                }
                similar::DiffOp::Delete { .. } => {
                    for i in 0..old_range.len() {
                        let idx = old_range.start + i;
                        let line = if idx < old_lines.len() { old_lines[idx] } else { "" };
                        let ln = pad_number(old_line, line_num_width);
                        output.push(format!("-{} {}", ln, line));
                        old_line += 1;
                    }
                }
                similar::DiffOp::Insert { .. } => {
                    if first_changed_line.is_none() {
                        first_changed_line = Some(new_line);
                    }
                    for i in 0..new_range.len() {
                        let idx = new_range.start + i;
                        let line = if idx < new_lines.len() { new_lines[idx] } else { "" };
                        let ln = pad_number(new_line, line_num_width);
                        output.push(format!("+{} {}", ln, line));
                        new_line += 1;
                    }
                }
                similar::DiffOp::Replace { .. } => {
                    if first_changed_line.is_none() {
                        first_changed_line = Some(new_line);
                    }
                    for i in 0..old_range.len() {
                        let idx = old_range.start + i;
                        let line = if idx < old_lines.len() { old_lines[idx] } else { "" };
                        let ln = pad_number(old_line, line_num_width);
                        output.push(format!("-{} {}", ln, line));
                        old_line += 1;
                    }
                    for i in 0..new_range.len() {
                        let idx = new_range.start + i;
                        let line = if idx < new_lines.len() { new_lines[idx] } else { "" };
                        let ln = pad_number(new_line, line_num_width);
                        output.push(format!("+{} {}", ln, line));
                        new_line += 1;
                    }
                }
            }
        }
    }

    DiffResult {
        diff: output.join("\n"),
        first_changed_line,
    }
}

fn pad_number(n: usize, width: usize) -> String {
    format!("{:>width$}", n, width = width)
}

// ============================================================================
// Async diff computation
// ============================================================================

/// Compute the diff for one or more edit operations without applying them.
///
/// Used for preview rendering before the tool executes.
///
/// Equivalent to `computeEditsDiff` in edit-diff.ts.
pub async fn compute_edits_diff(
    path: &str,
    edits: &[Edit],
    cwd: &str,
) -> Result<EditDiffResult, EditDiffError> {
    let absolute_path = path_utils::resolve_to_cwd(path, cwd);

    // Read the file
    let raw_content = match tokio::fs::read_to_string(&absolute_path).await {
        Ok(c) => c,
        Err(e) => {
            return Err(EditDiffError {
                error: format!("Could not edit file: {path}. {e}"),
            });
        }
    };

    // Strip BOM before matching
    let BomResult { text: content, .. } = strip_bom(&raw_content);
    let normalized_content = normalize_to_lf(&content);

    match apply_edits_to_normalized_content(&normalized_content, edits, path) {
        Ok(result) => {
            let diff_result = generate_diff_string(&result.base_content, &result.new_content, 4);
            Ok(EditDiffResult {
                diff: diff_result.diff,
                first_changed_line: diff_result.first_changed_line,
            })
        }
        Err(e) => Err(EditDiffError {
            error: e.to_string(),
        }),
    }
}

/// Compute the diff for a single edit operation without applying it.
///
/// Equivalent to `computeEditDiff` in edit-diff.ts.
pub async fn compute_edit_diff(
    path: &str,
    old_text: &str,
    new_text: &str,
    cwd: &str,
) -> Result<EditDiffResult, EditDiffError> {
    compute_edits_diff(path, &[Edit {
        old_text: old_text.to_string(),
        new_text: new_text.to_string(),
    }], cwd).await
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- detect_line_ending ---

    #[test]
    fn test_detect_line_ending_lf() {
        assert_eq!(detect_line_ending("hello\nworld\n"), LineEnding::Lf);
    }

    #[test]
    fn test_detect_line_ending_crlf() {
        assert_eq!(detect_line_ending("hello\r\nworld\r\n"), LineEnding::CrLf);
    }

    #[test]
    fn test_detect_line_ending_crlf_before_lf() {
        assert_eq!(detect_line_ending("hello\r\nworld\n"), LineEnding::CrLf);
    }

    #[test]
    fn test_detect_line_ending_no_newlines() {
        assert_eq!(detect_line_ending("hello world"), LineEnding::Lf);
    }

    // --- normalize_to_lf ---

    #[test]
    fn test_normalize_to_lf_crlf() {
        assert_eq!(normalize_to_lf("hello\r\nworld\r\n"), "hello\nworld\n");
    }

    #[test]
    fn test_normalize_to_lf_cr() {
        assert_eq!(normalize_to_lf("hello\rworld\r"), "hello\nworld\n");
    }

    #[test]
    fn test_normalize_to_lf_already_lf() {
        assert_eq!(normalize_to_lf("hello\nworld\n"), "hello\nworld\n");
    }

    // --- restore_line_endings ---

    #[test]
    fn test_restore_line_endings_to_crlf() {
        assert_eq!(
            restore_line_endings("hello\nworld\n", LineEnding::CrLf),
            "hello\r\nworld\r\n"
        );
    }

    #[test]
    fn test_restore_line_endings_to_lf() {
        assert_eq!(
            restore_line_endings("hello\nworld\n", LineEnding::Lf),
            "hello\nworld\n"
        );
    }

    // --- normalize_for_fuzzy_match ---

    #[test]
    fn test_normalize_for_fuzzy_match_trailing_whitespace() {
        let result = normalize_for_fuzzy_match("hello   \nworld  \n");
        assert_eq!(result, "hello\nworld\n");
    }

    #[test]
    fn test_normalize_for_fuzzy_match_smart_quotes() {
        let result = normalize_for_fuzzy_match("\u{2018}hello\u{2019} \u{201C}world\u{201D}");
        assert_eq!(result, "'hello' \"world\"");
    }

    #[test]
    fn test_normalize_for_fuzzy_match_dashes() {
        let result = normalize_for_fuzzy_match("hello\u{2014}world");
        assert_eq!(result, "hello-world");
    }

    #[test]
    fn test_normalize_for_fuzzy_match_nbsp() {
        let result = normalize_for_fuzzy_match("hello\u{00A0}world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_normalize_for_fuzzy_match_nfkc() {
        // U+2126 (Ω) normalizes to U+03A9 (Ω) under NFKC
        let result = normalize_for_fuzzy_match("\u{2126}");
        assert_eq!(result, "\u{03A9}");
    }

    // --- strip_bom ---

    #[test]
    fn test_strip_bom_present() {
        let result = strip_bom("\u{FEFF}hello");
        assert_eq!(result.bom, "\u{FEFF}");
        assert_eq!(result.text, "hello");
    }

    #[test]
    fn test_strip_bom_absent() {
        let result = strip_bom("hello");
        assert_eq!(result.bom, "");
        assert_eq!(result.text, "hello");
    }

    #[test]
    fn test_strip_bom_empty() {
        let result = strip_bom("");
        assert_eq!(result.bom, "");
        assert_eq!(result.text, "");
    }

    // --- fuzzy_find_text ---

    #[test]
    fn test_fuzzy_find_text_exact_match() {
        let result = fuzzy_find_text("hello world", "world");
        assert!(result.found);
        assert_eq!(result.index, 6);
        assert_eq!(result.match_length, 5);
        assert!(!result.used_fuzzy_match);
    }

    #[test]
    fn test_fuzzy_find_text_exact_no_match() {
        let result = fuzzy_find_text("hello world", "xyz");
        assert!(!result.found);
    }

    #[test]
    fn test_fuzzy_find_text_fuzzy_match() {
        // Content has trailing space, oldText doesn't
        let result = fuzzy_find_text("hello world \nnext line", "world\nnext");
        assert!(result.found);
        assert!(result.used_fuzzy_match);
    }

    #[test]
    fn test_fuzzy_find_text_fuzzy_quotes() {
        let result = fuzzy_find_text("hello \u{201C}world\u{201D}", "\"world\"");
        assert!(result.found);
        assert!(result.used_fuzzy_match);
    }

    // --- count_occurrences ---

    #[test]
    fn test_count_occurrences_single() {
        assert_eq!(count_occurrences("hello world", "world"), 1);
    }

    #[test]
    fn test_count_occurrences_multiple() {
        assert_eq!(count_occurrences("hello hello hello", "hello"), 3);
    }

    #[test]
    fn test_count_occurrences_none() {
        assert_eq!(count_occurrences("hello world", "xyz"), 0);
    }

    #[test]
    fn test_count_occurrences_fuzzy() {
        // NBSP is normalized to regular space → should match via fuzzy
        assert_eq!(count_occurrences("hello\u{00A0}world", "hello world"), 1);
    }

    // --- apply_edits_to_normalized_content ---

    #[test]
    fn test_apply_edits_single() {
        let result = apply_edits_to_normalized_content(
            "hello world",
            &[Edit { old_text: "world".into(), new_text: "rust".into() }],
            "test.txt",
        ).unwrap();
        assert_eq!(result.new_content, "hello rust");
    }

    #[test]
    fn test_apply_edits_multiple() {
        let result = apply_edits_to_normalized_content(
            "foo bar baz",
            &[
                Edit { old_text: "foo".into(), new_text: "one".into() },
                Edit { old_text: "baz".into(), new_text: "three".into() },
            ],
            "test.txt",
        ).unwrap();
        assert_eq!(result.new_content, "one bar three");
    }

    #[test]
    fn test_apply_edits_not_found() {
        let result = apply_edits_to_normalized_content(
            "hello world",
            &[Edit { old_text: "notfound".into(), new_text: "replaced".into() }],
            "test.txt",
        );
        assert!(matches!(result, Err(EditError::NotFound { .. })));
    }

    #[test]
    fn test_apply_edits_duplicate() {
        let result = apply_edits_to_normalized_content(
            "hello hello",
            &[Edit { old_text: "hello".into(), new_text: "hi".into() }],
            "test.txt",
        );
        assert!(matches!(result, Err(EditError::Duplicate { .. })));
    }

    #[test]
    fn test_apply_edits_no_change() {
        let result = apply_edits_to_normalized_content(
            "hello world",
            &[Edit { old_text: "hello".into(), new_text: "hello".into() }],
            "test.txt",
        );
        assert!(matches!(result, Err(EditError::NoChange { .. })));
    }

    #[test]
    fn test_apply_edits_empty_old_text() {
        let result = apply_edits_to_normalized_content(
            "hello world",
            &[Edit { old_text: "".into(), new_text: "hi".into() }],
            "test.txt",
        );
        assert!(matches!(result, Err(EditError::EmptyOldText { .. })));
    }

    #[test]
    fn test_apply_edits_overlap() {
        let result = apply_edits_to_normalized_content(
            "hello world",
            &[
                Edit { old_text: "hello".into(), new_text: "hi".into() },
                Edit { old_text: "hello w".into(), new_text: "hey".into() },
            ],
            "test.txt",
        );
        assert!(matches!(result, Err(EditError::Overlap { .. })));
    }

    #[test]
    fn test_apply_edits_reverse_order() {
        // Edits applied in reverse order should not affect offsets
        let result = apply_edits_to_normalized_content(
            "a b c",
            &[
                Edit { old_text: "a".into(), new_text: "x".into() },
                Edit { old_text: "c".into(), new_text: "z".into() },
            ],
            "test.txt",
        ).unwrap();
        assert_eq!(result.new_content, "x b z");
    }

    #[test]
    fn test_apply_edits_fuzzy_match() {
        // Content has trailing whitespace, oldText doesn't → fuzzy match
        let result = apply_edits_to_normalized_content(
            "hello world  \nnext line",
            &[Edit { old_text: "hello world\nnext".into(), new_text: "hi world\nnext".into() }],
            "test.txt",
        ).unwrap();
        assert!(result.new_content.contains("hi world"));
    }

    #[test]
    fn test_apply_edits_error_messages_single() {
        let err = EditError::NotFound {
            path: "f.txt".into(),
            edit_index: 0,
            total_edits: 1,
        };
        let msg = err.to_string();
        assert!(msg.contains("Could not find the exact text"));
        assert!(!msg.contains("edits["));
    }

    #[test]
    fn test_apply_edits_error_messages_multi() {
        let err = EditError::NotFound {
            path: "f.txt".into(),
            edit_index: 0,
            total_edits: 2,
        };
        let msg = err.to_string();
        assert!(msg.contains("edits[0]"));
    }

    // --- generate_unified_patch ---

    #[test]
    fn test_generate_unified_patch_basic() {
        let patch = generate_unified_patch("test.txt", "hello\nworld\n", "hello\nrust\n", 3);
        assert!(patch.contains("test.txt"));
        assert!(patch.contains("+rust"));
        assert!(patch.contains("-world"));
    }

    #[test]
    fn test_generate_unified_patch_no_changes() {
        let patch = generate_unified_patch("test.txt", "hello\n", "hello\n", 3);
        // No changes → no hunks
        assert!(!patch.contains("@@"));
    }

    // --- generate_diff_string ---

    #[test]
    fn test_generate_diff_string_basic() {
        let result = generate_diff_string("hello\nworld\n", "hello\nrust\n", 3);
        // Output format is: "-<ln> world" and "+<ln> rust"
        assert!(
            result.diff.contains("world"),
            "expected diff to contain 'world', got: {}",
            result.diff
        );
        assert!(
            result.diff.contains("rust"),
            "expected diff to contain 'rust', got: {}",
            result.diff
        );
        assert!(result.first_changed_line.is_some());
    }

    #[test]
    fn test_generate_diff_string_no_changes() {
        let result = generate_diff_string("hello\n", "hello\n", 3);
        assert_eq!(result.diff, "");
        assert!(result.first_changed_line.is_none());
    }

    // --- EditError Display ---

    #[test]
    fn test_edit_error_display_not_found() {
        let err = EditError::NotFound {
            path: "f.txt".into(),
            edit_index: 0,
            total_edits: 1,
        };
        assert!(err.to_string().contains("f.txt"));
    }

    #[test]
    fn test_edit_error_display_duplicate() {
        let err = EditError::Duplicate {
            path: "f.txt".into(),
            edit_index: 0,
            total_edits: 1,
            occurrences: 3,
        };
        assert!(err.to_string().contains("3 occurrences"));
    }
}
