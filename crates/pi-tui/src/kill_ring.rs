/// Ring buffer for Emacs-style kill/yank operations.
///
/// Tracks killed (deleted) text entries. Consecutive kills can accumulate
/// into a single entry. Supports yank (paste most recent) and yank-pop
/// (cycle through older entries).
#[derive(Debug, Clone)]
pub struct KillRing {
    ring: Vec<String>,
}

impl KillRing {
    pub fn new() -> Self {
        Self { ring: Vec::new() }
    }

    /// Add text to the kill ring.
    ///
    /// * `text` - The killed text to add
    /// * `prepend` - If accumulating, prepend (backward deletion) or append (forward deletion)
    /// * `accumulate` - Merge with the most recent entry instead of creating a new one
    pub fn push(&mut self, text: String, prepend: bool, accumulate: bool) {
        if text.is_empty() {
            return;
        }

        if accumulate && !self.ring.is_empty() {
            let last = self.ring.pop().unwrap();
            if prepend {
                self.ring.push(text + &last);
            } else {
                self.ring.push(last + &text);
            }
        } else {
            self.ring.push(text);
        }
    }

    /// Get most recent entry without modifying the ring.
    pub fn peek(&self) -> Option<&str> {
        self.ring.last().map(|s| s.as_str())
    }

    /// Move last entry to front (for yank-pop cycling).
    pub fn rotate(&mut self) {
        if self.ring.len() > 1 {
            let last = self.ring.pop().unwrap();
            self.ring.insert(0, last);
        }
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

impl Default for KillRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_kill_ring() {
        let kr = KillRing::new();
        assert!(kr.is_empty());
        assert_eq!(kr.len(), 0);
        assert!(kr.peek().is_none());
    }

    #[test]
    fn test_push_and_peek() {
        let mut kr = KillRing::new();
        kr.push("hello".to_string(), false, false);
        assert_eq!(kr.len(), 1);
        assert_eq!(kr.peek(), Some("hello"));
    }

    #[test]
    fn test_accumulate_append() {
        let mut kr = KillRing::new();
        kr.push("hello".to_string(), false, false);
        kr.push(" world".to_string(), false, true);
        assert_eq!(kr.len(), 1);
        assert_eq!(kr.peek(), Some("hello world"));
    }

    #[test]
    fn test_accumulate_prepend() {
        let mut kr = KillRing::new();
        kr.push("world".to_string(), false, false);
        kr.push("hello ".to_string(), true, true);
        assert_eq!(kr.len(), 1);
        assert_eq!(kr.peek(), Some("hello world"));
    }

    #[test]
    fn test_no_accumulate() {
        let mut kr = KillRing::new();
        kr.push("a".to_string(), false, false);
        kr.push("b".to_string(), false, false);
        assert_eq!(kr.len(), 2);
    }

    #[test]
    fn test_empty_text_noop() {
        let mut kr = KillRing::new();
        kr.push("".to_string(), false, false);
        assert_eq!(kr.len(), 0);
    }

    #[test]
    fn test_rotate() {
        let mut kr = KillRing::new();
        kr.push("a".to_string(), false, false);
        kr.push("b".to_string(), false, false);
        kr.push("c".to_string(), false, false);
        assert_eq!(kr.len(), 3);
        assert_eq!(kr.peek(), Some("c"));

        kr.rotate();
        assert_eq!(kr.peek(), Some("b"));

        kr.rotate();
        assert_eq!(kr.peek(), Some("a"));

        kr.rotate();
        assert_eq!(kr.peek(), Some("c"));
    }

    #[test]
    fn test_rotate_single_entry() {
        let mut kr = KillRing::new();
        kr.push("only".to_string(), false, false);
        kr.rotate();
        assert_eq!(kr.peek(), Some("only"));
    }

    #[test]
    fn test_rotate_empty() {
        let mut kr = KillRing::new();
        kr.rotate();
        assert!(kr.peek().is_none());
    }
}
