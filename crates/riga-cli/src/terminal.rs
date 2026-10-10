use std::io::{self, Stdout};

use crossterm::{
    event::{KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    pub fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| error.to_string())?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES,
            )
        ) {
            let _ = disable_raw_mode();
            return Err(error.to_string());
        }
        let terminal =
            Terminal::new(CrosstermBackend::new(stdout)).map_err(|error| error.to_string())?;
        Ok(Self { terminal })
    }

    pub fn draw<F>(&mut self, render: F) -> io::Result<ratatui::CompletedFrame<'_>>
    where
        F: FnOnce(&mut ratatui::Frame<'_>),
    {
        self.terminal.draw(render)
    }

    pub fn clear(&mut self) -> io::Result<()> {
        self.terminal.clear()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            PopKeyboardEnhancementFlags,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}
