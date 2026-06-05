/// Result of a fuzzy match operation.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    pub matches: bool,
    pub score: f64,
}

/// Match characters of `query` against `text` in order.
/// Returns a match result with score (lower = better).
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();

    let primary = match_query(&query_lower, &text_lower);
    if primary.matches {
        return primary;
    }

    // Try swapped alphanumeric patterns like "abc123" -> "123abc"
    let swapped = try_swapped_query(&query_lower, &text_lower);
    match swapped {
        Some(result) => result,
        None => primary,
    }
}

fn match_query(query: &str, text: &str) -> FuzzyMatch {
    if query.is_empty() {
        return FuzzyMatch {
            matches: true,
            score: 0.0,
        };
    }

    if query.len() > text.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    let query_chars: Vec<char> = query.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();

    let mut query_idx = 0;
    let mut score = 0.0_f64;
    let mut last_match_idx: i32 = -1;
    let mut consecutive_matches = 0;

    for (i, tc) in text_chars.iter().enumerate() {
        if query_idx >= query_chars.len() {
            break;
        }
        if *tc == query_chars[query_idx] {
            let is_word_boundary = i == 0
                || text_chars[i - 1] == ' '
                || text_chars[i - 1] == '-'
                || text_chars[i - 1] == '_'
                || text_chars[i - 1] == '.'
                || text_chars[i - 1] == '/'
                || text_chars[i - 1] == ':';

            if last_match_idx == i as i32 - 1 {
                consecutive_matches += 1;
                score -= (consecutive_matches * 5) as f64;
            } else {
                consecutive_matches = 0;
                if last_match_idx >= 0 {
                    score += (i as i32 - last_match_idx - 1) as f64 * 2.0;
                }
            }

            if is_word_boundary {
                score -= 10.0;
            }

            score += i as f64 * 0.1;

            last_match_idx = i as i32;
            query_idx += 1;
        }
    }

    if query_idx < query_chars.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    if query == text {
        score -= 100.0;
    }

    FuzzyMatch {
        matches: true,
        score,
    }
}

fn try_swapped_query(query: &str, text: &str) -> Option<FuzzyMatch> {
    let chars: Vec<char> = query.chars().collect();

    // Try "abc123" -> split into alpha prefix + numeric suffix
    let alpha_numeric = try_split_pattern(&chars, |c| c.is_alphabetic(), |c| c.is_numeric());
    if let Some((a, n)) = alpha_numeric {
        let swapped: String = n.iter().chain(a.iter()).collect();
        let result = match_query(&swapped, text);
        if result.matches {
            return Some(FuzzyMatch {
                matches: true,
                score: result.score + 5.0,
            });
        }
    }

    // Try "123abc" -> split into numeric prefix + alpha suffix
    let numeric_alpha = try_split_pattern(&chars, |c| c.is_numeric(), |c| c.is_alphabetic());
    if let Some((n, a)) = numeric_alpha {
        let swapped: String = a.iter().chain(n.iter()).collect();
        let result = match_query(&swapped, text);
        if result.matches {
            return Some(FuzzyMatch {
                matches: true,
                score: result.score + 5.0,
            });
        }
    }

    None
}

fn try_split_pattern(
    chars: &[char],
    first_pred: fn(char) -> bool,
    second_pred: fn(char) -> bool,
) -> Option<(&[char], &[char])> {
    if chars.len() < 2 {
        return None;
    }
    let split_point = chars.iter().position(|c| !first_pred(*c))?;
    if split_point == 0 || split_point >= chars.len() {
        return None;
    }
    let (first, second) = chars.split_at(split_point);
    if first.iter().all(|c| first_pred(*c)) && second.iter().all(|c| second_pred(*c)) {
        Some((first, second))
    } else {
        None
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

    let tokens: Vec<&str> = trimmed.split_whitespace().filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return items.iter().collect();
    }

    let mut results: Vec<(&T, f64)> = Vec::new();

    'items: for item in items {
        let text = get_text(item);
        let mut total_score = 0.0_f64;

        for token in &tokens {
            let m = fuzzy_match(token, text);
            if m.matches {
                total_score += m.score;
            } else {
                continue 'items;
            }
        }

        results.push((item, total_score));
    }

    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
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
        assert!(result.score < -50.0); // exact match bonus
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
        assert!(word_boundary.score < no_boundary.score);
    }

    #[test]
    fn test_consecutive_bonus() {
        let el = fuzzy_match("el", "hello");
        let ho = fuzzy_match("ho", "hello");
        assert!(el.matches);
        assert!(ho.matches);
    }

    #[test]
    fn test_swapped_alphanumeric() {
        // "abc123" should match "123abc"
        let result = fuzzy_match("abc123", "xyz 123abc xyz");
        assert!(result.matches);
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
        // "hello" should come first (better score)
        assert_eq!(result[0], &"hello");
    }
}
