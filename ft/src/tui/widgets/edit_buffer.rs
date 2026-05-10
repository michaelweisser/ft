//! Single-line edit buffer with char-precise cursor handling. Used by every
//! TUI surface that takes typed input (search query bar, edit popup fields,
//! the new-task quickline, the fuzzy picker).
//!
//! The buffer stores a `String` plus a *character* cursor (not a byte
//! offset) so the math stays simple for multi-byte glyphs. Methods are
//! deliberately tiny so callers can compose readline-style behavior
//! (Ctrl+W word delete, Home/End jump, etc.) without forking the type.

#[derive(Debug, Clone, Default)]
pub struct EditBuffer {
    pub text: String,
    /// Cursor position as a character offset (not byte offset).
    pub cursor: usize,
}

impl EditBuffer {
    pub fn from(text: &str) -> Self {
        let cursor = text.chars().count();
        Self {
            text: text.to_string(),
            cursor,
        }
    }

    pub fn insert(&mut self, c: char) {
        let byte_idx = self
            .text
            .char_indices()
            .nth(self.cursor)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len());
        self.text.insert(byte_idx, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev_char = self
            .text
            .char_indices()
            .nth(self.cursor - 1)
            .map(|(b, c)| (b, c.len_utf8()));
        if let Some((b, len)) = prev_char {
            self.text.replace_range(b..b + len, "");
            self.cursor -= 1;
        }
    }

    pub fn delete(&mut self) {
        let target = self
            .text
            .char_indices()
            .nth(self.cursor)
            .map(|(b, c)| (b, c.len_utf8()));
        if let Some((b, len)) = target {
            self.text.replace_range(b..b + len, "");
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        let max = self.text.chars().count();
        if self.cursor < max {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    /// Delete from the cursor leftward to the start of the previous word.
    /// Matches bash/readline `unix-word-rubout`: skip trailing whitespace,
    /// then skip non-whitespace, then erase the span.
    pub fn delete_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let start_byte: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
        let end_byte: usize = chars[..self.cursor].iter().map(|c| c.len_utf8()).sum();
        self.text.replace_range(start_byte..end_byte, "");
        self.cursor = i;
    }
}
