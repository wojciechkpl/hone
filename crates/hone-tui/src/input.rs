/// Tracks the text the user is currently composing, along with cursor position.
#[derive(Debug, Clone, Default)]
pub struct InputState {
    /// The raw text in the input box.
    pub buffer: String,
    /// Byte-offset cursor position within `buffer`.
    pub cursor_position: usize,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a character at the current cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor_position, c);
        self.cursor_position += c.len_utf8();
    }

    /// Delete the character immediately before the cursor (backspace semantics).
    pub fn delete_char(&mut self) {
        if self.cursor_position == 0 {
            return;
        }
        // Step back one char boundary.
        let new_pos = self
            .buffer[..self.cursor_position]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.buffer.drain(new_pos..self.cursor_position);
        self.cursor_position = new_pos;
    }

    /// Move the cursor one character to the left.
    pub fn move_cursor_left(&mut self) {
        if self.cursor_position == 0 {
            return;
        }
        self.cursor_position = self.buffer[..self.cursor_position]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    /// Move the cursor one character to the right.
    pub fn move_cursor_right(&mut self) {
        if self.cursor_position >= self.buffer.len() {
            return;
        }
        let ch = self.buffer[self.cursor_position..]
            .chars()
            .next()
            .expect("cursor is at a valid char boundary");
        self.cursor_position += ch.len_utf8();
    }

    /// Return the current buffer contents and reset to empty.
    pub fn take_input(&mut self) -> String {
        self.cursor_position = 0;
        std::mem::take(&mut self.buffer)
    }

    /// Returns `true` when the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}
