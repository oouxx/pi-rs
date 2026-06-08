use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

/// Result of a fuzzy match operation.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    pub matches: bool,
    pub score: f64,
}

static MATCHER: std::sync::LazyLock<SkimMatcherV2> =
    std::sync::LazyLock::new(|| SkimMatcherV2::default().ignore_case());

/// Match characters of `query` against `text` in order.
/// Returns a match result with score (higher = better).
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    if query.is_empty() {
        return FuzzyMatch {
            matches: true,
            score: 0.0,
        };
    }
    match MATCHER.fuzzy_match(text, query) {
        Some(s) => FuzzyMatch {
            matches: true,
            score: s as f64,
        },
        None => FuzzyMatch {
            matches: false,
            score: 0.0,
        },
    }
}

/// Filter and sort items by fuzzy match quality (best matches first).
/// Supports space-separated tokens: all tokens must match.
pub fn fuzzy_filter<'a, T, F>(items: &'a [T], query: &str, get_text: F) -> Vec<&'a T>
where
    F: Fn(&T) -> &str,
{
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return items.iter().collect();
    }

    let tokens: Vec<&str> = trimmed
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return items.iter().collect();
    }

    let mut results: Vec<(&T, i64)> = Vec::new();

    'items: for item in items {
        let text = get_text(item);
        let mut total_score = 0_i64;

        for token in &tokens {
            match MATCHER.fuzzy_match(text, token) {
                Some(score) => total_score += score,
                None => continue 'items,
            }
        }

        results.push((item, total_score));
    }

    results.sort_by(|a, b| b.1.cmp(&a.1));
    results.into_iter().map(|r| r.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_matches_everything() {
        let result = fuzzy_match("", "hello world");
        assert!(result.matches);
        assert_eq!(result.score, 0.0);
    }

    #[test]
    fn test_exact_match() {
        let result = fuzzy_match("hello", "hello");
        assert!(result.matches);
        assert!(result.score > 50.0);
    }

    #[test]
    fn test_subsequence_match() {
        let result = fuzzy_match("hlo", "hello");
        assert!(result.matches);
    }

    #[test]
    fn test_no_match() {
        let result = fuzzy_match("xyz", "hello");
        assert!(!result.matches);
    }

    #[test]
    fn test_case_insensitive() {
        let result = fuzzy_match("HELLO", "hello world");
        assert!(result.matches);
    }

    #[test]
    fn test_word_boundary_bonus() {
        let word_boundary = fuzzy_match("hc", "hello_charlie");
        let no_boundary = fuzzy_match("hc", "xhellocharliex");
        assert!(word_boundary.matches);
        assert!(no_boundary.matches);
        assert!(word_boundary.score > no_boundary.score);
    }

    #[test]
    fn test_consecutive_bonus() {
        let el = fuzzy_match("el", "hello");
        let ho = fuzzy_match("ho", "hello");
        assert!(el.matches);
        assert!(ho.matches);
    }

    #[test]
    fn test_fuzzy_filter_empty_query() {
        let items = vec!["apple", "banana", "cherry"];
        let result = fuzzy_filter(&items, "", |s| s);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_fuzzy_filter_basic() {
        let items = vec!["hello world", "help desk", "world peace"];
        let result = fuzzy_filter(&items, "he", |s| s);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&&"hello world"));
        assert!(result.contains(&&"help desk"));
    }

    #[test]
    fn test_fuzzy_filter_multi_token() {
        let items = vec!["hello world", "help desk", "world hello"];
        let result = fuzzy_filter(&items, "hello world", |s| s);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_fuzzy_filter_no_match() {
        let items = vec!["apple", "banana", "cherry"];
        let result = fuzzy_filter(&items, "xyz", |s| s);
        assert!(result.is_empty());
    }

    #[test]
    fn test_fuzzy_filter_sorted() {
        let items = vec!["zzz hello", "hello"];
        let result = fuzzy_filter(&items, "hello", |s| s);
        assert_eq!(result[0], &"hello");
    }
}
