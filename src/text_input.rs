//! A minimal single-line text editor shared by the commit box, the settings
//! form and the prompt dialog.

/// The cursor is kept as a byte offset that always sits on a `char` boundary.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    value: String,
    cursor: usize,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self { value, cursor }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Cursor position in `char`s, for rendering.
    pub fn char_cursor(&self) -> usize {
        self.value[..self.cursor].chars().count()
    }

    pub fn insert_char(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, text: &str) {
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn prev_boundary(&self) -> usize {
        self.value[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_boundary(&self) -> usize {
        self.value[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
            .unwrap_or(self.cursor)
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.prev_boundary();
            self.value.replace_range(prev..self.cursor, "");
            self.cursor = prev;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.value.len() {
            let next = self.next_boundary();
            self.value.replace_range(self.cursor..next, "");
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.prev_boundary();
    }

    pub fn move_right(&mut self) {
        self.cursor = self.next_boundary();
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.value.len();
    }

    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '.'
    }

    pub fn move_word_left(&mut self) {
        let mut idx = self.cursor;
        // Skip separators, then the word itself, landing on its first character.
        while idx > 0 {
            let prev = self.value[..idx].chars().next_back().unwrap();
            if Self::is_word_char(prev) {
                break;
            }
            idx -= prev.len_utf8();
        }
        while idx > 0 {
            let prev = self.value[..idx].chars().next_back().unwrap();
            if !Self::is_word_char(prev) {
                break;
            }
            idx -= prev.len_utf8();
        }
        self.cursor = idx;
    }

    pub fn move_word_right(&mut self) {
        let mut idx = self.cursor;
        while idx < self.value.len() {
            let next = self.value[idx..].chars().next().unwrap();
            if !Self::is_word_char(next) {
                break;
            }
            idx += next.len_utf8();
        }
        while idx < self.value.len() {
            let next = self.value[idx..].chars().next().unwrap();
            if Self::is_word_char(next) {
                break;
            }
            idx += next.len_utf8();
        }
        self.cursor = idx;
    }

    pub fn delete_word_left(&mut self) {
        let before = self.cursor;
        self.move_word_left();
        let target = self.cursor;
        self.value.replace_range(target..before, "");
        self.cursor = target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace() {
        let mut input = TextInput::new("ab");
        input.move_home();
        input.insert_char('x');
        assert_eq!(input.value(), "xab");
        input.delete();
        assert_eq!(input.value(), "xb");
        input.backspace();
        assert_eq!(input.value(), "b");
    }

    #[test]
    fn multibyte_cursor_is_safe() {
        let mut input = TextInput::new("héllo");
        input.move_end();
        input.backspace();
        assert_eq!(input.value(), "héll");
        input.move_home();
        input.move_right();
        input.insert_char('X');
        assert_eq!(input.value(), "hXéll");
    }

    #[test]
    fn word_movement() {
        let mut input = TextInput::new("fix the thing");
        input.move_word_left();
        assert_eq!(input.char_cursor(), 8);

        // From the start of "thing", deleting a word removes "the ".
        input.delete_word_left();
        assert_eq!(input.value(), "fix thing");

        // From the very end, deleting a word removes "thing" entirely.
        let mut input = TextInput::new("fix the thing");
        input.delete_word_left();
        assert_eq!(input.value(), "fix the ");
    }
}
