use std::collections::VecDeque;

/// A bounded ring buffer of commit SHAs supporting undo and redo.
pub struct UndoStack {
    entries: VecDeque<String>,
    max_size: usize,
    redo_stack: Vec<String>,
}

impl UndoStack {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_size,
            redo_stack: Vec::new(),
        }
    }

    /// Push a new SHA onto the stack. Clears the redo stack and evicts the
    /// oldest entry if the stack has reached `max_size`.
    pub fn push(&mut self, sha: String) {
        self.redo_stack.clear();
        if self.entries.len() == self.max_size {
            self.entries.pop_front();
        }
        self.entries.push_back(sha);
    }

    /// Pop the most recent SHA for undo. The popped SHA is saved so it can be
    /// restored by `redo`. Returns `None` when the stack is empty.
    pub fn undo(&mut self) -> Option<String> {
        let sha = self.entries.pop_back()?;
        self.redo_stack.push(sha.clone());
        Some(sha)
    }

    /// Re-apply the most recently undone SHA. Returns `None` when there is
    /// nothing to redo.
    pub fn redo(&mut self) -> Option<String> {
        let sha = self.redo_stack.pop()?;
        self.entries.push_back(sha.clone());
        Some(sha)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounded_at_max_size() {
        let mut stack = UndoStack::new(3);
        stack.push("a".into());
        stack.push("b".into());
        stack.push("c".into());
        stack.push("d".into());
        assert_eq!(stack.len(), 3);
        // "a" should have been evicted; undo returns "d", "c", "b" in that order
        assert_eq!(stack.undo(), Some("d".into()));
        assert_eq!(stack.undo(), Some("c".into()));
        assert_eq!(stack.undo(), Some("b".into()));
        assert_eq!(stack.undo(), None);
    }

    #[test]
    fn test_redo_cleared_on_new_push() {
        let mut stack = UndoStack::new(10);
        stack.push("a".into());
        stack.push("b".into());
        stack.undo();
        // New push clears the redo stack
        stack.push("c".into());
        assert_eq!(stack.redo(), None);
    }
}
