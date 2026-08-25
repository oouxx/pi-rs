//! Fuzzy matching — port of TS `packages/tui/src/fuzzy.ts`.
//!
//! Matches if all query characters appear in order (not necessarily
//! consecutive). Lower score = better match.

/// Result of a fuzzy match: whether it matched and its score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub matches: bool,
    pub score: i64,
}

fn match_query(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    if query_lower.is_empty() {
        return FuzzyMatch { matches: true, score: 0 };
    }
    if query_lower.chars().count() > text_lower.chars().count() {
        return FuzzyMatch { matches: false, score: 0 };
    }

    let q: Vec<char> = query_lower.chars().collect();
    let t: Vec<char> = text_lower.chars().collect();
    let mut query_index = 0usize;
    let mut score: i64 = 0;
    let mut last_match_index: Option<usize> = None;
    let mut consecutive_matches = 0i64;

    for (i, &c) in t.iter().enumerate() {
        if query_index >= q.len() {
            break;
        }
        if c == q[query_index] {
            let is_word_boundary =
                i == 0 || matches!(t[i - 1], ' ' | '-' | '_' | '.' | '/' | ':');
            // Reward consecutive matches.
            if i > 0 && last_match_index == Some(i - 1) {
                consecutive_matches += 1;
                score -= consecutive_matches * 5;
            } else {
                consecutive_matches = 0;
                // Penalize gaps.
                if let Some(prev) = last_match_index {
                    score += (i - prev - 1) as i64 * 2;
                }
            }
            // Reward word-boundary matches.
            if is_word_boundary {
                score -= 10;
            }
            // Slight penalty for later matches.
            score += i as i64 / 10;
            last_match_index = Some(i);
            query_index += 1;
        }
    }

    if query_index < q.len() {
        return FuzzyMatch { matches: false, score: 0 };
    }
    if query_lower == text_lower {
        score -= 100;
    }
    FuzzyMatch { matches: true, score }
}

/// Port of `fuzzyMatch` (including the alphanumeric-swap fallback for
/// queries like `gpt4` ↔ `4gpt`-style pairs).
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let primary = match_query(query, text);
    if primary.matches {
        return primary;
    }
    let q = query.to_lowercase();
    let swapped: Option<String> = {
        let (letters, digits): (String, String) = q.chars().partition(|c| c.is_ascii_alphabetic());
        if letters.is_empty() || digits.is_empty() {
            None
        } else if q.len() == letters.len() + digits.len() {
            Some(format!("{digits}{letters}"))
        } else {
            None
        }
    };
    let Some(swapped) = swapped else { return primary };
    let swapped_match = match_query(&swapped, text);
    if !swapped_match.matches {
        return primary;
    }
    FuzzyMatch { matches: true, score: swapped_match.score + 5 }
}

/// Filter and sort items by fuzzy-match quality (best matches first).
/// Supports whitespace- and slash-separated tokens: all tokens must match.
/// Returns `(index, score)` pairs into `items`, best first.
pub fn fuzzy_filter_indices<T, F: Fn(&T) -> String>(items: &[T], query: &str, get_text: F) -> Vec<(usize, i64)> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return (0..items.len()).map(|i| (i, 0)).collect();
    }
    let tokens: Vec<&str> = trimmed
        .split(|c: char| c.is_whitespace() || c == '/')
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return (0..items.len()).map(|i| (i, 0)).collect();
    }

    let mut results: Vec<(usize, i64)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let text = get_text(item);
        let mut total: i64 = 0;
        let mut all_match = true;
        for token in &tokens {
            let m = fuzzy_match(token, &text);
            if m.matches {
                total += m.score;
            } else {
                all_match = false;
                break;
            }
        }
        if all_match {
            results.push((i, total));
        }
    }
    results.sort_by_key(|(_, score)| *score);
    results
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        let m = fuzzy_match("", "anything");
        assert!(m.matches);
        assert_eq!(m.score, 0);
    }

    #[test]
    fn subsequence_matches_in_order() {
        assert!(fuzzy_match("mdl", "model").matches);
        assert!(!fuzzy_match("dmo", "model").matches);
        assert!(!fuzzy_match("modelx", "model").matches);
    }

    #[test]
    fn exact_match_bonus_bests_prefix() {
        let exact = fuzzy_match("model", "model");
        let prefix = fuzzy_match("model", "model-x");
        assert!(exact.matches && prefix.matches);
        assert!(exact.score < prefix.score, "exact ({}) < prefix ({})", exact.score, prefix.score);
    }

    #[test]
    fn alphanumeric_swap_fallback() {
        let m = fuzzy_match("gpt4", "4gpt");
        assert!(m.matches, "gpt4 ↔ 4gpt swap");
    }

    #[test]
    fn filter_sorts_best_first_and_supports_tokens() {
        let items = ["openai/gpt-4o", "anthropic/claude-sonnet", "openai/gpt-4o-mini"];
        let idx = fuzzy_filter_indices(&items, "openai 4o", |s| s.to_string());
        assert_eq!(idx.len(), 2, "openai/* 4o* both match");
        let (first, _) = idx[0];
        assert_eq!(items[first], "openai/gpt-4o");
    }

    #[test]
    fn no_match_filters_out() {
        let items = ["one", "two"];
        assert!(fuzzy_filter_indices(&items, "zzz", |s| s.to_string()).is_empty());
    }
}
