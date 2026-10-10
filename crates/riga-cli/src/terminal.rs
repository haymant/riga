use std::io::{self, Stdout};

use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::Write;

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
            EnableMouseCapture,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
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

    pub fn size(&self) -> io::Result<ratatui::layout::Size> {
        self.terminal.size()
    }

    pub fn clear(&mut self) -> io::Result<()> {
        self.terminal.clear()
    }

    pub fn copy_to_clipboard(&mut self, text: &str) -> io::Result<()> {
        use base64::Engine;

        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let mut stdout = io::stdout();
        write!(stdout, "\x1b]52;c;{encoded}\x07")?;
        stdout.flush()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}
