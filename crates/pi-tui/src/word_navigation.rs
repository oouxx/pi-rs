#[derive(Debug, PartialEq, Clone, Copy)]
enum SegType {
    Word,
    Whitespace,
    Atomic,
}

#[derive(Clone)]
struct Segment {
    start: usize,
    end: usize,
    ty: SegType,
}

pub struct WordNavigationOptions<'a> {
    pub is_atomic_segment: Option<&'a dyn Fn(&str) -> bool>,
}

impl Default for WordNavigationOptions<'_> {
    fn default() -> Self {
        Self {
            is_atomic_segment: None,
        }
    }
}

/// Build an initial list of segments: whitespace runs and non-whitespace runs.
/// Then merge consecutive non-whitespace runs that form an atomic segment.
fn segment_words(text: &str, is_atomic: Option<&dyn Fn(&str) -> bool>) -> Vec<Segment> {
    let mut raw_segs: Vec<Segment> = Vec::new();
    if text.is_empty() {
        return raw_segs;
    }

    // First pass: segment by whitespace boundaries
    let mut pos = 0;
    while pos < text.len() {
        let rest = &text[pos..];
        let next_ws = rest.find(|c: char| c.is_whitespace());
        match next_ws {
            Some(0) => {
                // Whitespace run
                let ws_end = rest
                    .find(|c: char| !c.is_whitespace())
                    .unwrap_or(rest.len());
                raw_segs.push(Segment {
                    start: pos,
                    end: pos + ws_end,
                    ty: SegType::Whitespace,
                });
                pos += ws_end;
            }
            Some(n) => {
                raw_segs.push(Segment {
                    start: pos,
                    end: pos + n,
                    ty: SegType::Word,
                });
                pos += n;
            }
            None => {
                raw_segs.push(Segment {
                    start: pos,
                    end: text.len(),
                    ty: SegType::Word,
                });
                pos = text.len();
            }
        }
    }

    // If no atomic check, return raw segments
    let ia = match is_atomic {
        Some(f) => f,
        None => return raw_segs,
    };

    // Second pass: merge consecutive Word segments into Atomic where applicable
    // We iterate and look for Word segments that, when combined, satisfy the atomic predicate
    let mut merged: Vec<Segment> = Vec::new();
    let mut i = 0;
    while i < raw_segs.len() {
        if raw_segs[i].ty != SegType::Word {
            merged.push(Segment {
                start: raw_segs[i].start,
                end: raw_segs[i].end,
                ty: raw_segs[i].ty.clone(),
            });
            i += 1;
            continue;
        }

        // Check if this word segment + following Word/WS/Word... forms an atomic marker
        // We scan forward to find a span that satisfies the predicate
        let mut j = i;
        let mut best_end = raw_segs[i].end;
        let mut found_atomic = false;

        while j < raw_segs.len() {
            // Extend range to include this segment
            let range_end = raw_segs[j].end;
            let span = &text[raw_segs[i].start..range_end];
            if ia(span) {
                best_end = range_end;
                found_atomic = true;
            }
            // If this segment is Whitespace and the next one is Word, we can keep merging
            // If this is the last or next is WS, stop
            if j + 1 >= raw_segs.len() {
                break;
            }
            // If the current segment is Word and next is WS followed by Word, continue
            // If the current segment is WS and next is Word, continue
            // Otherwise stop
            if raw_segs[j].ty == SegType::Whitespace && raw_segs[j + 1].ty == SegType::Word {
                j += 1;
                continue;
            }
            if raw_segs[j].ty == SegType::Word && raw_segs[j + 1].ty == SegType::Whitespace {
                j += 1;
                continue;
            }
            break;
        }

        if found_atomic {
            merged.push(Segment {
                start: raw_segs[i].start,
                end: best_end,
                ty: SegType::Atomic,
            });
            // Advance past all segments consumed
            while i < raw_segs.len() && raw_segs[i].end <= best_end {
                i += 1;
            }
        } else {
            merged.push(Segment {
                start: raw_segs[i].start,
                end: raw_segs[i].end,
                ty: SegType::Word,
            });
            i += 1;
        }
    }

    merged
}

pub fn find_word_backward(text: &str, cursor: usize, options: &WordNavigationOptions) -> usize {
    if cursor == 0 {
        return 0;
    }
    let text_before = &text[..cursor];
    let mut segs = segment_words(text_before, options.is_atomic_segment);
    if segs.is_empty() {
        return 0;
    }

    let ia = options.is_atomic_segment;
    while let Some(s) = segs.last() {
        let is_atom =
            s.ty == SegType::Atomic || ia.map_or(false, |f| f(&text_before[s.start..s.end]));
        if is_atom || s.ty != SegType::Whitespace {
            break;
        }
        segs.pop();
    }
    if segs.is_empty() {
        return 0;
    }

    segs.last().unwrap().start
}

pub fn find_word_forward(text: &str, cursor: usize, options: &WordNavigationOptions) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let text_after = &text[cursor..];
    let segs = segment_words(text_after, options.is_atomic_segment);
    if segs.is_empty() {
        return text.len();
    }

    let ia = options.is_atomic_segment;
    let mut pos = cursor;
    let mut idx = 0;

    while idx < segs.len() {
        let s = &segs[idx];
        let is_atom =
            s.ty == SegType::Atomic || ia.map_or(false, |f| f(&text_after[s.start..s.end]));
        if is_atom || s.ty != SegType::Whitespace {
            break;
        }
        pos += s.end - s.start;
        idx += 1;
    }
    if idx >= segs.len() {
        return pos;
    }

    pos += segs[idx].end - segs[idx].start;
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_text() {
        assert_eq!(
            find_word_backward("", 0, &WordNavigationOptions::default()),
            0
        );
        assert_eq!(
            find_word_forward("", 0, &WordNavigationOptions::default()),
            0
        );
    }

    #[test]
    fn test_backward_at_start() {
        assert_eq!(
            find_word_backward("hello world", 0, &WordNavigationOptions::default()),
            0
        );
    }

    #[test]
    fn test_forward_at_end() {
        assert_eq!(
            find_word_forward("hello world", 11, &WordNavigationOptions::default()),
            11
        );
    }

    #[test]
    fn test_backward_simple() {
        assert_eq!(
            find_word_backward("hello world", 11, &WordNavigationOptions::default()),
            6
        );
    }

    #[test]
    fn test_backward_trailing_whitespace() {
        assert_eq!(
            find_word_backward("hello world  ", 13, &WordNavigationOptions::default()),
            6
        );
    }

    #[test]
    fn test_forward_simple() {
        assert_eq!(
            find_word_forward("hello world", 0, &WordNavigationOptions::default()),
            5
        );
    }

    #[test]
    fn test_forward_skip_whitespace() {
        assert_eq!(
            find_word_forward("hello   world", 5, &WordNavigationOptions::default()),
            13
        );
    }

    #[test]
    fn test_backward_punctuation() {
        assert_eq!(
            find_word_backward("hello, world", 12, &WordNavigationOptions::default()),
            7
        );
    }

    #[test]
    fn test_forward_punctuation() {
        assert_eq!(
            find_word_forward("hello,world", 0, &WordNavigationOptions::default()),
            11
        );
    }

    #[test]
    fn test_with_atomic_segments() {
        let re = regex_lite::Regex::new(r"^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$").unwrap();
        let atomic: &dyn Fn(&str) -> bool = &|s| re.is_match(s);
        let opts = WordNavigationOptions {
            is_atomic_segment: Some(atomic),
        };
        let text = "hello [paste #1] world";
        // "[paste #1]" = 10 chars: 0=6,1=15. Atomic segment [6,16).
        // "world" starts at 17.
        // Backward from end: skip "world" → position 17
        let pos = find_word_backward(text, text.len(), &opts);
        assert_eq!(pos, 17);
    }

    #[test]
    fn test_words_separated_by_spaces() {
        assert_eq!(
            find_word_backward("a b c", 5, &WordNavigationOptions::default()),
            4
        );
        assert_eq!(
            find_word_forward("a b c", 0, &WordNavigationOptions::default()),
            1
        );
    }

    #[test]
    fn test_backward_single_word() {
        assert_eq!(
            find_word_backward("hello", 5, &WordNavigationOptions::default()),
            0
        );
    }

    #[test]
    fn test_forward_last_word() {
        assert_eq!(
            find_word_forward("hello world", 6, &WordNavigationOptions::default()),
            11
        );
    }

    #[test]
    fn test_backward_mid_word() {
        assert_eq!(
            find_word_backward("hello world", 8, &WordNavigationOptions::default()),
            6
        );
    }

    #[test]
    fn test_forward_mid_word() {
        assert_eq!(
            find_word_forward("hello", 2, &WordNavigationOptions::default()),
            5
        );
    }
}
