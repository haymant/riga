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

    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        match key.code {
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
}
