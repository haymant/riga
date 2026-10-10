use crate::{
    input::{InputAction, TextBuffer},
    model::{AppState, CatalogEntry, ProviderForm, UiPanel},
    transport::RigaTransport,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCommand {
    StartRun {
        session_id: String,
        prompt: String,
    },
    Approve {
        approval_id: String,
        always: bool,
    },
    Deny {
        approval_id: String,
    },
    CancelRun {
        run_id: String,
    },
    SelectSession {
        session_id: String,
    },
    CreateSession {
        title: String,
        workspace: String,
    },
    SaveProvider {
        config: riga_server::ws::ProviderConfig,
    },
    InsertCatalog {
        text: String,
    },
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
    pub panel: UiPanel,
    pub panel_cursor: usize,
    pub panel_input: TextBuffer,
    pub workspace_input: TextBuffer,
    pub creating_session: bool,
    pub session_field: usize,
    pub provider: ProviderForm,
    pub provider_field: usize,
    pub catalog: Vec<CatalogEntry>,
    pub catalog_query: TextBuffer,
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
        if self.panel != UiPanel::Transcript {
            return self.handle_panel_key(key);
        }
        match key.code {
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.panel = UiPanel::History;
                self.panel_cursor = 0;
                return None;
            }
            KeyCode::Char(',') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.panel = UiPanel::Settings;
                self.provider_field = 0;
                self.sync_provider_input();
                return None;
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.panel = UiPanel::Catalog;
                self.panel_cursor = 0;
                self.catalog_query.clear();
                return None;
            }
            KeyCode::Char('?') => {
                self.panel = UiPanel::Help;
                return None;
            }
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

    fn handle_panel_key(&mut self, key: KeyEvent) -> Option<UiCommand> {
        if key.code == KeyCode::Esc {
            self.panel = UiPanel::Transcript;
            self.creating_session = false;
            return None;
        }
        match self.panel {
            UiPanel::History => {
                if self.creating_session {
                    match key.code {
                        KeyCode::Enter
                            if !self.panel_input.is_empty() && !self.workspace_input.is_empty() =>
                        {
                            self.creating_session = false;
                            return Some(UiCommand::CreateSession {
                                title: self.panel_input.text().into(),
                                workspace: self.workspace_input.text().into(),
                            });
                        }
                        KeyCode::Tab => self.session_field = (self.session_field + 1) % 2,
                        _ => {
                            if self.session_field == 0 {
                                let _ = self.panel_input.handle_key(key);
                            } else {
                                let _ = self.workspace_input.handle_key(key);
                            }
                        }
                    }
                    return None;
                }
                match key.code {
                    KeyCode::Up => self.panel_cursor = self.panel_cursor.saturating_sub(1),
                    KeyCode::Down => {
                        self.panel_cursor = self
                            .panel_cursor
                            .saturating_add(1)
                            .min(self.state.sessions.len().saturating_sub(1))
                    }
                    KeyCode::Char('n') => {
                        self.creating_session = true;
                        self.panel_input.clear();
                        self.workspace_input.clear();
                        self.workspace_input.insert('.');
                        self.session_field = 0;
                    }
                    KeyCode::Enter => {
                        if let Some(session) = self.state.sessions.get(self.panel_cursor) {
                            self.panel = UiPanel::Transcript;
                            return Some(UiCommand::SelectSession {
                                session_id: session.id.clone(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            UiPanel::Settings => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                    self.commit_provider_input();
                    return Some(UiCommand::SaveProvider {
                        config: self.provider.to_config(),
                    });
                }
                match key.code {
                    KeyCode::Tab => {
                        self.commit_provider_input();
                        self.provider_field = (self.provider_field + 1) % 7;
                        self.sync_provider_input();
                    }
                    KeyCode::BackTab => {
                        self.commit_provider_input();
                        self.provider_field = self.provider_field.checked_sub(1).unwrap_or(6);
                        self.sync_provider_input();
                    }
                    KeyCode::Enter if self.provider_field == 3 => {
                        self.provider.reasoning_effort =
                            match self.provider.reasoning_effort.as_str() {
                                "low" => "medium",
                                "medium" => "high",
                                _ => "low",
                            }
                            .into();
                        self.sync_provider_input();
                    }
                    KeyCode::Enter if self.provider_field == 4 => {
                        self.provider.kind = match self.provider.kind {
                            riga_server::ws::ProviderKind::Remote => {
                                riga_server::ws::ProviderKind::Local
                            }
                            riga_server::ws::ProviderKind::Local => {
                                riga_server::ws::ProviderKind::Remote
                            }
                        };
                        self.sync_provider_input();
                    }
                    KeyCode::Enter if self.provider_field == 5 => {
                        self.provider.api = match self.provider.api {
                            riga_server::ws::ProviderApi::Chat => {
                                riga_server::ws::ProviderApi::Responses
                            }
                            riga_server::ws::ProviderApi::Responses => {
                                riga_server::ws::ProviderApi::Chat
                            }
                        };
                        self.sync_provider_input();
                    }
                    _ => {
                        let _ = self.panel_input.handle_key(key);
                        self.commit_provider_input();
                    }
                }
            }
            UiPanel::Catalog => match key.code {
                KeyCode::Up => self.panel_cursor = self.panel_cursor.saturating_sub(1),
                KeyCode::Down => self.panel_cursor = self.panel_cursor.saturating_add(1),
                KeyCode::Enter => {
                    if let Some(text) = self
                        .filtered_catalog()
                        .get(self.panel_cursor)
                        .map(|entry| entry.insert_text.clone())
                    {
                        self.panel = UiPanel::Transcript;
                        return Some(UiCommand::InsertCatalog { text });
                    }
                }
                _ => {
                    let _ = self.catalog_query.handle_key(key);
                    self.panel_cursor = 0;
                }
            },
            UiPanel::Help | UiPanel::Transcript => {}
        }
        None
    }

    fn sync_provider_input(&mut self) {
        let value = match self.provider_field {
            0 => self.provider.endpoint.clone(),
            1 => self.provider.api_key.clone(),
            2 => self.provider.model.clone(),
            3 => self.provider.reasoning_effort.clone(),
            4 => format!("{:?}", self.provider.kind),
            5 => format!("{:?}", self.provider.api),
            _ => self.provider.subagent_model.clone(),
        };
        self.panel_input.clear();
        for character in value.chars() {
            self.panel_input.insert(character);
        }
    }

    fn commit_provider_input(&mut self) {
        let value = self.panel_input.text().to_owned();
        match self.provider_field {
            0 => self.provider.endpoint = value,
            1 => self.provider.api_key = value,
            2 => self.provider.model = value,
            3 => self.provider.reasoning_effort = value,
            6 => self.provider.subagent_model = value,
            _ => {}
        }
    }

    pub fn filtered_catalog(&self) -> Vec<&CatalogEntry> {
        let query = self.catalog_query.text().to_lowercase();
        self.catalog
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.id.to_lowercase().contains(&query)
                    || entry.kind.to_lowercase().contains(&query)
                    || entry.description.to_lowercase().contains(&query)
            })
            .collect()
    }

    pub fn set_catalog(&mut self, catalog: Vec<CatalogEntry>) {
        self.catalog = catalog;
    }
    pub fn set_provider(&mut self, provider: ProviderForm) {
        self.provider = provider;
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
                UiCommand::SelectSession { session_id } => {
                    app.state.selected_session = Some(session_id);
                    app.state.active_run = None;
                }
                UiCommand::CreateSession { title, workspace } => {
                    let session = transport
                        .create_session(riga_server::CreateSessionRequest { title, workspace })
                        .await?;
                    app.state.selected_session = Some(session.id.clone());
                    app.state.sessions.push(session);
                }
                UiCommand::SaveProvider { config } => {
                    match transport.configure_provider(config).await {
                        Ok(_) => {
                            app.provider.error = None;
                            app.panel = UiPanel::Transcript;
                        }
                        Err(error) => app.provider.error = Some(error),
                    }
                }
                UiCommand::InsertCatalog { text } => {
                    for character in text.chars() {
                        app.draft.insert(character);
                    }
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

    #[test]
    fn history_panel_creates_a_valid_session_without_touching_the_draft() {
        let mut app = UiState::default();
        app.draft.insert('x');
        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert_eq!(app.panel, UiPanel::History);
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        for character in "demo".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::CreateSession {
                title: "demo".into(),
                workspace: ".".into()
            })
        );
        assert_eq!(app.draft.text(), "x");
    }

    #[test]
    fn settings_save_command_keeps_the_api_key_out_of_display_state() {
        let mut app = UiState::default();
        app.provider.endpoint = "https://model.test/v1".into();
        app.provider.api_key = "secret".into();
        app.provider.model = "model".into();
        app.handle_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::CONTROL));
        let Some(UiCommand::SaveProvider { config }) =
            app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        else {
            panic!("settings should emit an explicit save command")
        };
        assert_eq!(config.api_key, "secret");
        assert_eq!(app.provider.api_key, "secret");
    }

    #[test]
    fn catalog_filters_incrementally_and_inserts_server_text_only() {
        let mut app = UiState::default();
        app.set_catalog(vec![CatalogEntry {
            id: "read".into(),
            kind: "tools".into(),
            description: "read a file".into(),
            insert_text: "Use the read tool: ".into(),
            requires_approval: false,
        }]);
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::InsertCatalog {
                text: "Use the read tool: ".into()
            })
        );
    }
}
