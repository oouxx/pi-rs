use std::path::{Path, PathBuf};

use crate::utils::paths::{normalize_path, resolve_path, PathOptions};

const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

fn try_macos_screenshot_path(file_path: &str) -> String {
    // Replace " AM" or " PM" (case-insensitive) with narrow no-break space variant
    #[allow(clippy::unwrap_used)] // literal pattern, compilation is infallible
    let re = regex::Regex::new(r"(?i) (AM|PM)\.").unwrap();
    re.replace_all(file_path, |caps: &regex::Captures| {
        format!("{}{}.", NARROW_NO_BREAK_SPACE, &caps[1])
    }).to_string()
}

fn try_nfd_variant(file_path: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    file_path.nfd().collect::<String>()
}

fn try_curly_quote_variant(file_path: &str) -> String {
    file_path.replace('\'', "\u{2019}")
}

fn file_exists(file_path: &str) -> bool {
    Path::new(file_path).exists()
}

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

pub fn expand_path(path: &str) -> PathBuf {
    let opts = PathOptions {
        normalize_unicode_spaces: true,
        strip_at_prefix: true,
        ..Default::default()
    };
    PathBuf::from(normalize_path(path, &opts))
}

pub fn resolve_to_cwd(path: &str, cwd: &str) -> PathBuf {
    let opts = PathOptions {
        normalize_unicode_spaces: true,
        strip_at_prefix: true,
        ..Default::default()
    };
    PathBuf::from(resolve_path(path, cwd, &opts))
}

pub fn resolve_read_path(path: &str, cwd: &str) -> PathBuf {
    let resolved = resolve_to_cwd(path, cwd);
    let resolved_str = resolved.to_string_lossy().to_string();

    if file_exists(&resolved_str) {
        return resolved;
    }

    // Try macOS AM/PM variant (narrow no-break space before AM/PM)
    let am_pm_variant = try_macos_screenshot_path(&resolved_str);
    if am_pm_variant != resolved_str && file_exists(&am_pm_variant) {
        return PathBuf::from(am_pm_variant);
    }

    // Try NFD variant (macOS stores filenames in NFD form)
    let nfd_variant = try_nfd_variant(&resolved_str);
    if nfd_variant != resolved_str && file_exists(&nfd_variant) {
        return PathBuf::from(nfd_variant);
    }

    // Try curly quote variant (macOS uses U+2019 in screenshot names)
    let curly_variant = try_curly_quote_variant(&resolved_str);
    if curly_variant != resolved_str && file_exists(&curly_variant) {
        return PathBuf::from(curly_variant);
    }

    // Try combined NFD + curly quote (for French macOS screenshots like "Capture d'écran")
    let nfd_curly_variant = try_curly_quote_variant(&nfd_variant);
    if nfd_curly_variant != resolved_str && file_exists(&nfd_curly_variant) {
        return PathBuf::from(nfd_curly_variant);
    }

    resolved
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ------------------------------------------------------------------
    // expandPath
    // ------------------------------------------------------------------

    #[test]
    fn test_expand_path_tilde() {
        let result = expand_path("~");
        assert!(!result.to_string_lossy().contains('~'));
    }

    #[test]
    fn test_expand_path_tilde_path() {
        let result = expand_path("~/Documents/file.txt");
        let s = result.to_string_lossy();
        assert!(!s.contains("~/"));
        assert!(s.contains("Documents/file.txt"));
    }

    #[test]
    fn test_expand_path_tilde_prefixed_filename() {
        // "~draft.md" should be kept literal (not expanded)
        let result = expand_path("~draft.md");
        assert_eq!(result, PathBuf::from("~draft.md"));

        // "@~draft.md" should strip @ prefix and keep tilde literal
        let result = expand_path("@~draft.md");
        assert_eq!(result, PathBuf::from("~draft.md"));
    }

    #[test]
    fn test_expand_path_unicode_spaces() {
        // Non-breaking space (U+00A0) should become regular space
        let with_nbsp = "file name.txt".to_string();
        let result = expand_path(&with_nbsp);
        assert_eq!(result, PathBuf::from("file name.txt"));
    }

    // ------------------------------------------------------------------
    // resolveToCwd
    // ------------------------------------------------------------------

    #[test]
    fn test_resolve_to_cwd_absolute() {
        let result = resolve_to_cwd("/usr/bin/test", "/home/user/project");
        assert_eq!(result, PathBuf::from("/usr/bin/test"));
    }

    #[test]
    fn test_resolve_to_cwd_relative() {
        let result = resolve_to_cwd("relative/file.txt", "/some/cwd");
        assert_eq!(result, PathBuf::from("/some/cwd/relative/file.txt"));
    }

    #[test]
    fn test_resolve_to_cwd_tilde_prefixed() {
        // "~draft.md" should be resolved against cwd (not expanded as home dir)
        let cwd = "/tmp/pi-path-utils-cwd";
        let result = resolve_to_cwd("~draft.md", cwd);
        assert_eq!(result, PathBuf::from("/tmp/pi-path-utils-cwd/~draft.md"));

        let result = resolve_to_cwd("@~draft.md", cwd);
        assert_eq!(result, PathBuf::from("/tmp/pi-path-utils-cwd/~draft.md"));
    }

    // ------------------------------------------------------------------
    // resolveReadPath
    // ------------------------------------------------------------------

    #[test]
    fn test_resolve_read_path_existing_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test-file.txt");
        fs::write(&file_path, "content").unwrap();

        let result = resolve_read_path("test-file.txt", dir.path().to_str().unwrap());
        assert_eq!(result, file_path);
    }

    #[test]
    fn test_resolve_read_path_nfd_vs_nfc() {
        // NFD: e (U+0065) + combining acute accent (U+0301)
        let nfd_file_name = "file\u{0065}\u{0301}.txt";
        // NFC: é as single character (U+00E9)
        let nfc_file_name = "file\u{00E9}.txt";

        // Verify they have different byte sequences
        assert_ne!(nfd_file_name, nfc_file_name);

        let dir = TempDir::new().unwrap();
        let nfd_path = dir.path().join(nfd_file_name);
        fs::write(&nfd_path, "content").unwrap();

        // User provides NFC path - should find the file
        let result = resolve_read_path(nfc_file_name, dir.path().to_str().unwrap());
        let result_str = result.to_string_lossy().to_string();
        assert!(result_str.contains(dir.path().to_str().unwrap()));
        assert!(result_str.ends_with(".txt"));
    }

    #[test]
    fn test_resolve_read_path_curly_quotes() {
        // macOS uses curly apostrophe (U+2019) in screenshot filenames
        let curly_quote_name = "Capture d\u{2019}cran.txt";
        let straight_quote_name = "Capture d'cran.txt";

        assert_ne!(curly_quote_name, straight_quote_name);

        let dir = TempDir::new().unwrap();
        let curly_path = dir.path().join(curly_quote_name);
        fs::write(&curly_path, "content").unwrap();

        // User provides straight quote path - should find the curly quote file
        let result = resolve_read_path(straight_quote_name, dir.path().to_str().unwrap());
        assert_eq!(result, curly_path);
    }

    #[test]
    fn test_resolve_read_path_combined_nfc_curly_quote() {
        // Full macOS screenshot filename with NFC é and curly quote
        let nfc_curly_name = "Capture d\u{2019}\u{00E9}cran.txt";
        let nfc_straight_name = "Capture d'\u{00E9}cran.txt";

        assert_ne!(nfc_curly_name, nfc_straight_name);

        let dir = TempDir::new().unwrap();
        let nfc_curly_path = dir.path().join(nfc_curly_name);
        fs::write(&nfc_curly_path, "content").unwrap();

        // User provides straight quote path - should find the curly quote file
        let result = resolve_read_path(nfc_straight_name, dir.path().to_str().unwrap());
        assert_eq!(result, nfc_curly_path);
    }

    #[test]
    fn test_resolve_read_path_macos_screenshot_am_pm() {
        // macOS uses narrow no-break space (U+202F) before AM/PM
        let macos_name = "Screenshot 2024-01-01 at 10.00.00\u{202F}AM.png";
        let user_name = "Screenshot 2024-01-01 at 10.00.00 AM.png";

        let dir = TempDir::new().unwrap();
        let macos_path = dir.path().join(macos_name);
        fs::write(&macos_path, "content").unwrap();

        let result = resolve_read_path(user_name, dir.path().to_str().unwrap());
        assert_eq!(result, macos_path);
    }

    #[test]
    fn test_resolve_read_path_macos_screenshot_lowercase_am_pm() {
        // Some locales like en_AU use lowercase am/pm
        let macos_name = "Screenshot 2024-01-01 at 10.00.00\u{202F}am.png";
        let user_name = "Screenshot 2024-01-01 at 10.00.00 am.png";

        let dir = TempDir::new().unwrap();
        let macos_path = dir.path().join(macos_name);
        fs::write(&macos_path, "content").unwrap();

        let result = resolve_read_path(user_name, dir.path().to_str().unwrap());
        assert_eq!(result, macos_path);
    }
}
