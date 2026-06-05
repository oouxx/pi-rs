/// Generic undo stack with clone-on-push semantics.
///
/// Stores clones of state snapshots. Popped snapshots are returned
/// directly (no re-cloning) since they are already detached.
#[derive(Debug, Clone)]
pub struct UndoStack<S> {
    stack: Vec<S>,
}

impl<S: Clone> UndoStack<S> {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a clone of the given state onto the stack.
    pub fn push(&mut self, state: &S) {
        self.stack.push(state.clone());
    }

    /// Pop and return the most recent snapshot, or None if empty.
    pub fn pop(&mut self) -> Option<S> {
        self.stack.pop()
    }

    pub fn clear(&mut self) {
        self.stack.clear();
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}

impl<S: Clone> Default for UndoStack<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_stack() {
        let mut stack: UndoStack<String> = UndoStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
        assert!(stack.pop().is_none());
    }

    #[test]
    fn test_push_and_pop() {
        let mut stack = UndoStack::new();
        stack.push(&"hello".to_string());
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.pop(), Some("hello".to_string()));
        assert!(stack.is_empty());
    }

    #[test]
    fn test_clone_semantics() {
        let mut stack = UndoStack::new();
        let s = "hello".to_string();
        stack.push(&s);
        // Mutating original should not affect stack
        drop(s);
        assert_eq!(stack.pop(), Some("hello".to_string()));
    }

    #[test]
    fn test_multiple_values() {
        let mut stack = UndoStack::new();
        stack.push(&"a".to_string());
        stack.push(&"b".to_string());
        stack.push(&"c".to_string());
        assert_eq!(stack.len(), 3);
        assert_eq!(stack.pop(), Some("c".to_string()));
        assert_eq!(stack.pop(), Some("b".to_string()));
        assert_eq!(stack.pop(), Some("a".to_string()));
        assert!(stack.is_empty());
    }

    #[test]
    fn test_clear() {
        let mut stack = UndoStack::new();
        stack.push(&"a".to_string());
        stack.push(&"b".to_string());
        stack.clear();
        assert!(stack.is_empty());
        assert!(stack.pop().is_none());
    }

    #[test]
    fn test_with_struct() {
        #[derive(Clone, Debug, PartialEq)]
        struct State {
            lines: Vec<String>,
            cursor: usize,
        }

        let mut stack = UndoStack::new();
        let state = State {
            lines: vec!["hello".to_string()],
            cursor: 0,
        };
        stack.push(&state);
        assert_eq!(stack.len(), 1);

        let popped = stack.pop().unwrap();
        assert_eq!(popped.lines, vec!["hello".to_string()]);
        assert_eq!(popped.cursor, 0);
    }
}
