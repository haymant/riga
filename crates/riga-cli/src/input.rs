use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextBuffer {
    text: String,
    cursor: usize,
}

impl TextBuffer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor_byte_position(&self) -> usize {
        self.cursor
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        let prefix = &self.text[..self.cursor];
        let line = prefix.matches('\n').count() as u16;
        let column = prefix
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count() as u16;
        (column, line)
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn replace(&mut self, value: &str) {
        self.text.clear();
        self.text.push_str(value);
        self.cursor = self.text.len();
    }

    pub fn insert(&mut self, value: char) {
        self.text.insert(self.cursor, value);
        self.cursor += value.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    pub fn delete_forward(&mut self) {
        if let Some(character) = self.text[self.cursor..].chars().next() {
            self.text
                .drain(self.cursor..self.cursor + character.len_utf8());
        }
    }

    pub fn replace_range(&mut self, start: usize, end: usize, replacement: &str) -> bool {
        if start > end
            || end > self.text.len()
            || !self.text.is_char_boundary(start)
            || !self.text.is_char_boundary(end)
        {
            return false;
        }
        self.text.replace_range(start..end, replacement);
        self.cursor = start + replacement.len();
        true
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.text[..self.cursor]
                .char_indices()
                .last()
                .map(|(index, _)| index)
                .unwrap_or(0);
        }
    }

    pub fn move_right(&mut self) {
        if let Some(character) = self.text[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map(|index| self.cursor + index)
            .unwrap_or(self.text.len());
    }

    pub fn move_vertical(&mut self, down: bool) -> bool {
        let prefix = &self.text[..self.cursor];
        let current_line_start = prefix.rfind('\n').map(|index| index + 1).unwrap_or(0);
        let column = prefix[current_line_start..].chars().count();
        let target_start = if down {
            let Some(offset) = self.text[self.cursor..].find('\n') else {
                return false;
            };
            self.cursor + offset + 1
        } else {
            if current_line_start == 0 {
                return false;
            }
            self.text[..current_line_start - 1]
                .rfind('\n')
                .map(|index| index + 1)
                .unwrap_or(0)
        };
        let target_end = self.text[target_start..]
            .find('\n')
            .map(|offset| target_start + offset)
            .unwrap_or(self.text.len());
        let target_column = self.text[target_start..target_end]
            .chars()
            .take(column)
            .map(char::len_utf8)
            .sum::<usize>();
        self.cursor = target_start + target_column;
        true
    }

    pub fn move_word_left(&mut self) {
        let prefix = &self.text[..self.cursor];
        let mut chars = prefix.char_indices().rev();
        while let Some((index, character)) = chars.next() {
            self.cursor = index;
            if !character.is_whitespace() {
                break;
            }
        }
        for (index, character) in chars {
            if character.is_whitespace() {
                break;
            }
            self.cursor = index;
        }
    }

    pub fn move_word_right(&mut self) {
        let suffix = &self.text[self.cursor..];
        let mut seen_word = false;
        for (offset, character) in suffix.char_indices() {
            if seen_word && character.is_whitespace() {
                self.cursor += offset;
                return;
            }
            if !character.is_whitespace() {
                seen_word = true;
            }
        }
        self.cursor = self.text.len();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        match key.code {
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_word_left();
                InputAction::Changed
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_word_right();
                InputAction::Changed
            }
            KeyCode::Home => {
                self.move_home();
                InputAction::Changed
            }
            KeyCode::End => {
                self.move_end();
                InputAction::Changed
            }
            KeyCode::Delete => {
                self.delete_forward();
                InputAction::Changed
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_word_left();
                self.delete_forward();
                InputAction::Changed
            }
            KeyCode::Char('\n' | '\r') => {
                self.insert('\n');
                InputAction::Changed
            }
            KeyCode::Char(_character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::Ignored
            }
            KeyCode::Char(character) => {
                self.insert(character);
                InputAction::Changed
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert('\n');
                InputAction::Changed
            }
            KeyCode::Enter => {
                if self.is_empty() {
                    InputAction::Ignored
                } else {
                    let value = self.text.clone();
                    self.clear();
                    InputAction::Submit(value)
                }
            }
            KeyCode::Backspace => {
                self.backspace();
                InputAction::Changed
            }
            KeyCode::Left => {
                self.move_left();
                InputAction::Changed
            }
            KeyCode::Right => {
                self.move_right();
                InputAction::Changed
            }
            _ => InputAction::Ignored,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    Changed,
    Submit(String),
    Ignored,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_editing_never_splits_a_codepoint() {
        let mut buffer = TextBuffer::default();
        for character in "你好".chars() {
            buffer.insert(character);
        }
        buffer.backspace();
        assert_eq!(buffer.text(), "你");
        buffer.move_left();
        buffer.insert('界');
        assert_eq!(buffer.text(), "界你");
    }

    #[test]
    fn enter_submits_and_shift_enter_inserts_newline() {
        let mut buffer = TextBuffer::default();
        buffer.insert('a');
        assert_eq!(
            buffer.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            InputAction::Changed
        );
        buffer.insert('b');
        assert_eq!(buffer.text(), "a\nb");
        assert_eq!(
            buffer.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            InputAction::Submit("a\nb".into())
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn cursor_range_replacement_preserves_text_after_cursor() {
        let mut buffer = TextBuffer::default();
        buffer.replace("ask @fi later");
        buffer.move_home();
        for _ in 0..7 {
            buffer.move_right();
        }
        assert!(buffer.replace_range(4, 7, "@src/main.rs"));
        assert_eq!(buffer.text(), "ask @src/main.rs later");
        assert_eq!(buffer.cursor_byte_position(), "ask @src/main.rs".len());
    }

    #[test]
    fn word_navigation_and_delete_are_unicode_safe() {
        let mut buffer = TextBuffer::default();
        buffer.replace("界 alpha βeta");
        buffer.move_word_left();
        assert_eq!(buffer.cursor_byte_position(), "界 alpha ".len());
        buffer.move_word_left();
        assert_eq!(buffer.cursor_byte_position(), "界 ".len());
        buffer.delete_forward();
        assert_eq!(buffer.text(), "界 lpha βeta");
    }

    #[test]
    fn vertical_navigation_keeps_the_requested_column_and_respects_lines() {
        let mut buffer = TextBuffer::default();
        buffer.replace("first\nsecond\nthird");
        while buffer.cursor_byte_position() > 0 {
            buffer.move_left();
        }
        for _ in 0..4 {
            buffer.move_right();
        }
        assert!(buffer.move_vertical(true));
        assert_eq!(buffer.cursor_position(), (4, 1));
        assert!(buffer.move_vertical(false));
        assert_eq!(buffer.cursor_position(), (4, 0));
        buffer.move_home();
        assert!(!buffer.move_vertical(false));
    }
}
