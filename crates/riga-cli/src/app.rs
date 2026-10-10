use crate::{
    input::{InputAction, TextBuffer},
    model::AppState,
    transport::RigaTransport,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCommand {
    StartRun { session_id: String, prompt: String },
    Approve { approval_id: String, always: bool },
    Deny { approval_id: String },
    CancelRun { run_id: String },
    Quit,
}

#[derive(Debug, Default)]
pub struct UiState {
    pub state: AppState,
    pub draft: TextBuffer,
    pub last_submitted: Option<String>,
    pub should_quit: bool,
    pub follow_output: bool,
    pub transcript_scroll: usize,
    pub new_events: usize,
    pub reasoning_collapsed: bool,
    pub tools_collapsed: bool,
    pub(crate) approval_submission: Option<String>,
}

impl UiState {
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<UiCommand> {
        if let Some(approval) = self.state.pending_approval().cloned() {
            if self.approval_submission.is_some() {
                return None;
            }
            let command = match key.code {
                KeyCode::Char('y') => Some(UiCommand::Approve {
                    approval_id: approval.approval_id,
                    always: false,
                }),
                KeyCode::Char('a') => Some(UiCommand::Approve {
                    approval_id: approval.approval_id,
                    always: true,
                }),
                KeyCode::Char('n') => Some(UiCommand::Deny {
                    approval_id: approval.approval_id,
                }),
                KeyCode::Esc => None,
                _ => return None,
            };
            if let Some(command) = command {
                let approval_id = match &command {
                    UiCommand::Approve { approval_id, .. } | UiCommand::Deny { approval_id } => {
                        approval_id.clone()
                    }
                    _ => return None,
                };
                self.approval_submission = Some(approval_id);
                return Some(command);
            }
        }
        match key.code {
            KeyCode::PageUp => {
                self.follow_output = false;
                self.transcript_scroll = self.transcript_scroll.saturating_add(3);
                self.new_events = 0;
                return None;
            }
            KeyCode::PageDown => {
                self.transcript_scroll = self.transcript_scroll.saturating_sub(3);
                return None;
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.follow_output = false;
                self.transcript_scroll = self.transcript_scroll.saturating_add(1);
                return None;
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.transcript_scroll = self.transcript_scroll.saturating_sub(1);
                return None;
            }
            KeyCode::Home => {
                self.follow_output = false;
                self.transcript_scroll = usize::MAX;
                return None;
            }
            KeyCode::End => {
                self.jump_to_bottom();
                return None;
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => {
                self.reasoning_collapsed = !self.reasoning_collapsed;
                return None;
            }
            KeyCode::Char('t') if key.modifiers.is_empty() => {
                self.tools_collapsed = !self.tools_collapsed;
                return None;
            }
            KeyCode::Char('q') if self.draft.is_empty() => {
                self.should_quit = true;
                return Some(UiCommand::Quit);
            }
            KeyCode::Esc => {
                if let Some(run_id) = self.state.active_run.clone() {
                    return Some(UiCommand::CancelRun { run_id });
                }
                self.should_quit = true;
                return Some(UiCommand::Quit);
            }
            _ => {}
        }
        if let InputAction::Submit(prompt) = self.draft.handle_key(key) {
            self.last_submitted = Some(prompt.clone());
            return Some(UiCommand::StartRun {
                session_id: self.state.selected_session.clone().unwrap_or_default(),
                prompt,
            });
        }
        None
    }

    pub fn apply_event(&mut self, envelope: riga_kernel::events::RigaEventEnvelope) {
        let followed = self.follow_output;
        let outcome = self.state.apply_event(envelope);
        if followed {
            self.jump_to_bottom();
        } else if matches!(
            outcome,
            crate::model::ApplyOutcome::Applied { .. } | crate::model::ApplyOutcome::Gap { .. }
        ) {
            self.new_events = self.new_events.saturating_add(1);
        }
    }

    pub fn approval_command_finished(&mut self, approval_id: &str) {
        if self.approval_submission.as_deref() == Some(approval_id) {
            self.approval_submission = None;
        }
    }

    pub fn jump_to_bottom(&mut self) {
        self.follow_output = true;
        self.transcript_scroll = 0;
        self.new_events = 0;
    }
}

pub async fn run_with_transport<T: RigaTransport>(
    mut app: UiState,
    transport: T,
) -> Result<(), String> {
    let mut terminal = crate::terminal::TerminalGuard::enter()?;
    let mut events: Option<broadcast::Receiver<riga_kernel::events::RigaEventEnvelope>> = None;
    loop {
        while let Some(receiver) = events.as_mut() {
            match receiver.try_recv() {
                Ok(envelope) => app.apply_event(envelope),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    app.new_events = app.new_events.saturating_add(skipped as usize);
                    break;
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    events = None;
                    break;
                }
            }
        }
        terminal
            .draw(|frame| crate::ui::render(frame, &app))
            .map_err(|error| error.to_string())?;
        if app.should_quit {
            break;
        }
        if event::poll(Duration::from_millis(60)).map_err(|error| error.to_string())?
            && let Event::Key(key) = event::read().map_err(|error| error.to_string())?
            && let Some(command) = app.handle_key(key)
        {
            match command {
                UiCommand::StartRun { session_id, prompt } => {
                    let run_id = format!(
                        "cli-{}",
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    );
                    let receiver = transport
                        .start_run(run_id.clone(), session_id, prompt)
                        .await?;
                    app.state.select_run(Some(run_id));
                    events = Some(receiver);
                }
                UiCommand::Approve {
                    approval_id,
                    always,
                } => {
                    let _ = transport
                        .respond_to_approval(approval_id.clone(), true, always)
                        .await;
                    app.approval_command_finished(&approval_id);
                }
                UiCommand::Deny { approval_id } => {
                    let _ = transport
                        .respond_to_approval(approval_id.clone(), false, false)
                        .await;
                    app.approval_command_finished(&approval_id);
                }
                UiCommand::CancelRun { run_id } => {
                    let _ = transport.cancel_run(run_id).await;
                }
                UiCommand::Quit => break,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use riga_kernel::{
        PROTOCOL_VERSION,
        events::{RigaEvent, RigaEventEnvelope},
    };
    fn approval_app() -> UiState {
        let mut app = UiState::default();
        app.state.select_run(Some("run-1".into()));
        app.apply_event(RigaEventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            event_id: "e1".into(),
            session_id: "session-1".into(),
            run_id: "run-1".into(),
            sequence: 1,
            timestamp: "now".into(),
            event: RigaEvent::ApprovalRequested {
                approval_id: "approval-1".into(),
                task_id: "task-1".into(),
                tool: "write".into(),
                summary: "edit file".into(),
            },
        });
        app
    }
    #[test]
    fn enter_emits_start_command_and_clears_draft() {
        let mut app = UiState::default();
        app.state.selected_session = Some("session-1".into());
        app.draft.insert('h');
        app.draft.insert('i');
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::StartRun {
                session_id: "session-1".into(),
                prompt: "hi".into()
            })
        );
        assert!(app.draft.is_empty());
    }
    #[test]
    fn approval_keys_emit_once_and_always_without_duplicate_submission() {
        let mut app = approval_app();
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(UiCommand::Approve {
                approval_id: "approval-1".into(),
                always: true
            })
        );
        assert!(
            app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
                .is_none()
        );
        app.approval_command_finished("approval-1");
        assert!(
            app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
                .is_some()
        );
    }
    #[test]
    fn scroll_up_disables_follow_and_jump_to_bottom_resumes_it() {
        let mut app = UiState {
            follow_output: true,
            ..UiState::default()
        };
        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert!(!app.follow_output);
        app.new_events = 2;
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(app.follow_output);
        assert_eq!(app.new_events, 0);
    }
}
