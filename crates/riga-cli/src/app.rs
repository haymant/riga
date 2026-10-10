use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent};

use crate::{
    input::{InputAction, TextBuffer},
    model::AppState,
};

#[derive(Debug, Default)]
pub struct UiState {
    pub state: AppState,
    pub draft: TextBuffer,
    pub last_submitted: Option<String>,
    pub should_quit: bool,
}

impl UiState {
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<String> {
        if key.code == KeyCode::Char('q') && self.draft.is_empty() {
            self.should_quit = true;
            return None;
        }
        if key.code == KeyCode::Esc {
            self.should_quit = true;
            return None;
        }
        if let InputAction::Submit(prompt) = self.draft.handle_key(key) {
            self.last_submitted = Some(prompt.clone());
            return Some(prompt);
        }
        None
    }
}

pub fn run(mut app: UiState) -> Result<(), String> {
    let mut terminal = crate::terminal::TerminalGuard::enter()?;
    loop {
        terminal
            .draw(|frame| crate::ui::render(frame, &app))
            .map_err(|error| error.to_string())?;
        if app.should_quit {
            break;
        }
        if event::poll(Duration::from_millis(100)).map_err(|error| error.to_string())?
            && let Event::Key(key) = event::read().map_err(|error| error.to_string())?
        {
            let _ = app.handle_key(key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn enter_emits_one_submit_and_clears_the_draft() {
        let mut app = UiState::default();
        app.draft.insert('h');
        app.draft.insert('i');
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some("hi".into())
        );
        assert!(app.draft.is_empty());
        assert_eq!(app.last_submitted.as_deref(), Some("hi"));
    }

    #[test]
    fn shift_enter_only_changes_the_draft() {
        let mut app = UiState::default();
        app.draft.insert('a');
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(app.draft.text(), "a\n");
        assert!(!app.should_quit);
    }

    #[test]
    fn unknown_keys_do_not_mutate_or_quit() {
        let mut app = UiState::default();
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE)),
            None
        );
        assert!(app.draft.is_empty());
        assert!(!app.should_quit);
    }
}
