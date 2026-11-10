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
        return FuzzyMatch {
            matches: true,
            score: 0,
        };
    }
    if query_lower.chars().count() > text_lower.chars().count() {
        return FuzzyMatch {
            matches: false,
            score: 0,
        };
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
            // TS `isWordBoundary = i === 0 || /[\s\-_./:]/.test(textLower[i-1])`
            // — `\s` covers tab/newline, not just a space.
            let is_word_boundary = i == 0
                || t[i - 1].is_whitespace()
                || matches!(t[i - 1], '-' | '_' | '.' | '/' | ':');
            // Reward consecutive matches.
            // All terms are scaled by 10 so the TS fractional positional
            // penalty (`i * 0.1`) keeps its exact ordering without changing
            // the public integer score type.
            // TS starts `lastMatchIndex` at -1, so a match at i=0 satisfies
            // `lastMatchIndex === i - 1` and receives the consecutive bonus.
            let is_consecutive = match last_match_index {
                None => i == 0,
                Some(prev) => prev + 1 == i,
            };
            if is_consecutive {
                consecutive_matches += 1;
                score -= consecutive_matches * 50;
            } else {
                consecutive_matches = 0;
                // Penalize gaps.
                if let Some(prev) = last_match_index {
                    score += (i - prev - 1) as i64 * 20;
                }
            }
            // Reward word-boundary matches.
            if is_word_boundary {
                score -= 100;
            }
            // Slight penalty for later matches (`i * 0.1` in TS).
            score += i as i64;
            last_match_index = Some(i);
            query_index += 1;
        }
    }

    if query_index < q.len() {
        return FuzzyMatch {
            matches: false,
            score: 0,
        };
    }
    if query_lower == text_lower {
        score -= 1000;
    }
    FuzzyMatch {
        matches: true,
        score,
    }
}

/// TS `/^(?<letters>[a-z]+)(?<digits>[0-9]+)$/` and the mirrored
/// `^(?<digits>[0-9]+)(?<letters>[a-z]+)$`: one contiguous run of letters and
/// one contiguous run of digits, in either order. Mixed runs (`a1b2`) are not
/// swap-eligible. Returns the swapped query (the two runs exchanged).
fn split_alnum_swap(q: &str) -> Option<String> {
    let first = q.chars().next()?;
    let first_is_letter = first.is_ascii_alphabetic();
    let boundary = q
        .char_indices()
        .find(|(_, c)| c.is_ascii_alphabetic() != first_is_letter)
        .map(|(i, _)| i)?;
    let (first_run, second_run) = q.split_at(boundary);
    if first_run.is_empty() || second_run.is_empty() {
        return None;
    }
    // TS `/^(?<letters>[a-z]+)(?<digits>[0-9]+)$/` (and the mirrored form):
    // both runs must be pure letters/digits. Punctuation is not swap-eligible.
    let valid = if first_is_letter {
        first_run.chars().all(|c| c.is_ascii_lowercase())
            && second_run.chars().all(|c| c.is_ascii_digit())
    } else {
        first_run.chars().all(|c| c.is_ascii_digit())
            && second_run.chars().all(|c| c.is_ascii_lowercase())
    };
    if !valid {
        return None;
    }
    Some(format!("{second_run}{first_run}"))
}

/// Port of `fuzzyMatch` (including the alphanumeric-swap fallback for
/// queries like `gpt4` ↔ `4gpt`-style pairs).
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let primary = match_query(query, text);
    if primary.matches {
        return primary;
    }
    let q = query.to_lowercase();
    let swapped: Option<String> = split_alnum_swap(&q);
    let Some(swapped) = swapped else {
        return primary;
    };
    let swapped_match = match_query(&swapped, text);
    if !swapped_match.matches {
        return primary;
    }
    FuzzyMatch {
        matches: true,
        score: swapped_match.score + 50,
    }
}

/// Filter and sort items by fuzzy-match quality (best matches first).
/// Supports whitespace- and slash-separated tokens: all tokens must match.
/// Returns `(index, score)` pairs into `items`, best first.
pub fn fuzzy_filter_indices<T, F: Fn(&T) -> String>(
    items: &[T],
    query: &str,
    get_text: F,
) -> Vec<(usize, i64)> {
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
        assert!(
            exact.score < prefix.score,
            "exact ({}) < prefix ({})",
            exact.score,
            prefix.score
        );
    }

    #[test]
    fn alphanumeric_swap_fallback() {
        let m = fuzzy_match("gpt4", "4gpt");
        assert!(m.matches, "gpt4 ↔ 4gpt swap");
    }

    /// TS splits the swap with `^(?<letters>[a-z]+)(?<digits>[0-9]+)$` (and the
    /// mirrored form), so interleaved runs are not swap-eligible.
    #[test]
    fn alphanumeric_swap_requires_contiguous_runs() {
        assert!(split_alnum_swap("gpt4").as_deref() == Some("4gpt"));
        assert!(split_alnum_swap("4gpt").as_deref() == Some("gpt4"));
        assert_eq!(split_alnum_swap("a1b2"), None);
        assert_eq!(split_alnum_swap("gpt"), None);
        assert_eq!(split_alnum_swap("42"), None);
        assert_eq!(split_alnum_swap(""), None);
        assert_eq!(split_alnum_swap("abc12!"), None);
        assert_eq!(split_alnum_swap("abc1a"), None);
        assert!(!fuzzy_match("a1b2", "2b1a").matches);
    }

    /// TS awards the consecutive bonus for a first-character match too
    /// (`lastMatchIndex` starts at -1).
    #[test]
    fn first_character_match_gets_consecutive_bonus() {
        let first = fuzzy_match("a", "a");
        let later = fuzzy_match("a", "xa");
        assert!(
            first.score < later.score,
            "first-char score ({}) should beat later ({})",
            first.score,
            later.score
        );
    }

    /// TS `isWordBoundary` uses `/[\s\-_./:]/`, so tab/newline count too.
    #[test]
    fn word_boundary_includes_all_whitespace() {
        let tab = fuzzy_match("ab", "x\tab");
        let space = fuzzy_match("ab", "x ab");
        assert_eq!(tab.score, space.score, "tab and space are both boundaries");
    }

    #[test]
    fn filter_sorts_best_first_and_supports_tokens() {
        let items = [
            "openai/gpt-4o",
            "anthropic/claude-sonnet",
            "openai/gpt-4o-mini",
        ];
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
