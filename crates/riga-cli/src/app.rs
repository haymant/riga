use crate::{
    input::{InputAction, TextBuffer},
    model::{AppState, CatalogEntry, DeckLens, ProviderForm, TranscriptItem, UiPanel},
    transport::RigaTransport,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};

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
    ResumeRun {
        run_id: String,
        session_id: String,
    },
    LocalModelAction {
        action: String,
        model_id: Option<String>,
        path: Option<String>,
    },
    UploadAttachment {
        path: String,
    },
    Quit,
}

#[derive(Debug, Default)]
pub struct UiState {
    pub state: AppState,
    pub draft: TextBuffer,
    pub last_submitted: Option<String>,
    pub prompt_history: Vec<String>,
    pub prompt_history_cursor: Option<usize>,
    pub should_quit: bool,
    pub follow_output: bool,
    pub transcript_scroll: usize,
    pub transcript_focused: bool,
    pub new_events: usize,
    pub reasoning_collapsed: bool,
    pub tools_collapsed: bool,
    pub(crate) approval_submission: Option<String>,
    pub approval_focused: bool,
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
    pub deck_lens: DeckLens,
    pub local_models: Option<riga_server::LocalModelOverview>,
    pub local_model_error: Option<String>,
    pub local_model_status: Option<String>,
    pub local_model_progress: Option<f64>,
    pub local_attachment_input: bool,
    pub notice: Option<String>,
    pub confirm_quit: bool,
    pub busy: Option<String>,
    pub busy_tick: u64,
    pub clear_screen: bool,
    pub current_prompt: Option<String>,
    pub rundeck_open: bool,
    pub completion_trigger: Option<char>,
    pub completion_cursor: usize,
    pub command_candidates: Vec<String>,
}

enum BackgroundResult {
    StartRun {
        run_id: String,
        session_id: String,
        result: Result<broadcast::Receiver<riga_kernel::events::RigaEventEnvelope>, String>,
    },
    LocalModel {
        action: String,
        model_id: Option<String>,
        result: Result<(), String>,
    },
}

impl UiState {
    pub fn set_session_history(&mut self, turns: Vec<riga_server::ws::ConversationTurn>) {
        self.state.session_history = turns
            .into_iter()
            .map(|turn| match turn.role.as_str() {
                "user" => TranscriptItem::UserText(turn.content),
                "assistant" => TranscriptItem::AssistantText(turn.content),
                "reasoning" => TranscriptItem::AssistantReasoning(turn.content),
                role => TranscriptItem::System(format!("{role}: {}", turn.content)),
            })
            .collect();
        self.jump_to_bottom();
    }

    fn navigate_prompt_history(&mut self, down: bool) -> bool {
        if self.prompt_history.is_empty() {
            return false;
        }
        if down {
            let Some(index) = self.prompt_history_cursor else {
                return false;
            };
            if index + 1 >= self.prompt_history.len() {
                self.prompt_history_cursor = None;
                self.draft.clear();
            } else {
                let next = index + 1;
                self.prompt_history_cursor = Some(next);
                self.draft.replace(&self.prompt_history[next]);
            }
        } else {
            let next = self
                .prompt_history_cursor
                .map(|index| index.saturating_sub(1))
                .unwrap_or(self.prompt_history.len().saturating_sub(1));
            self.prompt_history_cursor = Some(next);
            self.draft.replace(&self.prompt_history[next]);
        }
        self.refresh_completion();
        true
    }

    pub fn set_command_candidates(&mut self, candidates: Vec<String>) {
        self.command_candidates = candidates;
    }

    fn matching_candidates(&self, token: &str) -> Vec<String> {
        self.command_candidates
            .iter()
            .filter(|candidate| candidate.starts_with(token))
            .cloned()
            .collect()
    }

    pub fn completion_items(&self) -> Vec<String> {
        let text = self.draft.text();
        let start = text
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, _)| index + 1)
            .unwrap_or(0);
        self.matching_candidates(&text[start..])
    }

    fn refresh_completion(&mut self) {
        let text = self.draft.text();
        let start = text
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, _)| index + 1)
            .unwrap_or(0);
        let token = &text[start..];
        if !matches!(token.chars().next(), Some('/' | '@')) {
            self.completion_trigger = None;
            return;
        }
        self.completion_trigger = token.chars().next();
        self.completion_cursor = self
            .completion_cursor
            .min(self.matching_candidates(token).len().saturating_sub(1));
    }

    fn accept_completion(&mut self) -> bool {
        let Some(_) = self.completion_trigger else {
            return false;
        };
        let text = self.draft.text().to_owned();
        let start = text
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, _)| index + 1)
            .unwrap_or(0);
        let token = &text[start..];
        let matches = self.matching_candidates(token);
        let Some(candidate) = matches.get(self.completion_cursor) else {
            return false;
        };
        for _ in token.chars() {
            self.draft.backspace();
        }
        for character in candidate.chars() {
            self.draft.insert(character);
        }
        self.draft.insert(' ');
        self.completion_trigger = None;
        true
    }

    fn archive_active_run(&mut self) {
        if let Some(prompt) = self.current_prompt.take() {
            self.state
                .session_history
                .push(TranscriptItem::UserText(prompt));
        }
        if let Some(run_id) = self.state.active_run.as_deref()
            && let Some(run) = self.state.run(run_id)
        {
            self.state.session_history.extend(run.transcript.clone());
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent, terminal_height: u16) {
        match mouse.kind {
            MouseEventKind::Down(_) => {
                self.transcript_focused = mouse.row < terminal_height.saturating_sub(4);
            }
            MouseEventKind::ScrollUp => {
                self.transcript_focused = true;
                self.follow_output = false;
                self.transcript_scroll = self.transcript_scroll.saturating_add(3);
            }
            MouseEventKind::ScrollDown => {
                self.transcript_focused = true;
                self.transcript_scroll = self.transcript_scroll.saturating_sub(3);
            }
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<UiCommand> {
        if self.confirm_quit {
            match key.code {
                KeyCode::Char('y') => {
                    self.confirm_quit = false;
                    self.should_quit = true;
                    return Some(UiCommand::Quit);
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.confirm_quit = false;
                    self.notice = None;
                    return None;
                }
                _ => return None,
            }
        }
        if key.code == KeyCode::Esc && self.busy.is_some() && self.state.active_run.is_some() {
            return Some(UiCommand::CancelRun {
                run_id: self.state.active_run.clone().unwrap_or_default(),
            });
        }
        if let Some(approval) = self.state.pending_approval().cloned() {
            if self.approval_submission.is_some() {
                return None;
            }
            if key.code == KeyCode::Tab {
                self.approval_focused = true;
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
        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.transcript_focused = false;
        }
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
            let _ = self.draft.handle_key(key);
            return None;
        }
        if let KeyCode::Char(character @ ('\n' | '\r')) = key.code {
            if character == '\r' {
                self.draft.insert('\n');
            } else {
                let _ = self.draft.handle_key(key);
            }
            return None;
        }
        match key.code {
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.panel = UiPanel::History;
                self.panel_cursor = 0;
                return None;
            }
            KeyCode::Char('\u{8}') => {
                self.panel = UiPanel::History;
                self.panel_cursor = 0;
                return None;
            }
            KeyCode::Backspace if self.draft.is_empty() => {
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
            KeyCode::F(2) => {
                self.panel = UiPanel::Settings;
                self.provider_field = 0;
                self.sync_provider_input();
                return None;
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.draft.insert('\n');
                self.refresh_completion();
                return None;
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.rundeck_open = !self.rundeck_open;
                return None;
            }
            KeyCode::Char('\u{4}') => {
                self.rundeck_open = !self.rundeck_open;
                return None;
            }
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.panel = UiPanel::LocalModels;
                self.panel_cursor = 0;
                self.local_attachment_input = false;
                return None;
            }
            KeyCode::Char('?') => {
                self.panel = UiPanel::Help;
                return None;
            }
            KeyCode::Up if self.completion_trigger.is_some() => {
                self.completion_cursor = self.completion_cursor.saturating_sub(1);
                return None;
            }
            KeyCode::Down if self.completion_trigger.is_some() => {
                self.completion_cursor = self.completion_cursor.saturating_add(1);
                return None;
            }
            KeyCode::Enter if self.completion_trigger.is_some() => {
                if self.accept_completion() {
                    return None;
                }
            }
            KeyCode::Up if self.transcript_focused => {
                self.follow_output = false;
                self.transcript_scroll = self.transcript_scroll.saturating_add(1);
                return None;
            }
            KeyCode::Down if self.transcript_focused => {
                self.transcript_scroll = self.transcript_scroll.saturating_sub(1);
                return None;
            }
            KeyCode::Up => {
                self.navigate_prompt_history(false);
                return None;
            }
            KeyCode::Down => {
                self.navigate_prompt_history(true);
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
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.reasoning_collapsed = !self.reasoning_collapsed;
                return None;
            }
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tools_collapsed = !self.tools_collapsed;
                return None;
            }
            KeyCode::Char('q') if self.draft.is_empty() => {
                self.should_quit = true;
                return Some(UiCommand::Quit);
            }
            KeyCode::Esc => {
                self.confirm_quit = true;
                self.notice = Some(if self.state.active_run.is_some() {
                    "Quit the TUI? Press y to quit or n to continue; use x in RunDeck to stop the run.".into()
                } else {
                    "Quit the TUI? Press y to quit or n/Esc to continue.".into()
                });
                return None;
            }
            _ => {}
        }
        match self.draft.handle_key(key) {
            InputAction::Changed => self.prompt_history_cursor = None,
            InputAction::Submit(prompt) => {
                self.last_submitted = Some(prompt.clone());
                self.prompt_history.push(prompt.clone());
                self.prompt_history_cursor = None;
                return Some(UiCommand::StartRun {
                    session_id: self.state.selected_session.clone().unwrap_or_default(),
                    prompt,
                });
            }
            InputAction::Ignored => {}
        }
        self.refresh_completion();
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
            UiPanel::RunDeck => match key.code {
                KeyCode::Left | KeyCode::Char('l') => {
                    self.deck_lens = match self.deck_lens {
                        DeckLens::Execution => DeckLens::Knowledge,
                        DeckLens::Evidence => DeckLens::Execution,
                        DeckLens::Knowledge => DeckLens::Evidence,
                    }
                }
                KeyCode::Right | KeyCode::Char('r') | KeyCode::Tab => {
                    self.deck_lens = match self.deck_lens {
                        DeckLens::Execution => DeckLens::Evidence,
                        DeckLens::Evidence => DeckLens::Knowledge,
                        DeckLens::Knowledge => DeckLens::Execution,
                    }
                }
                KeyCode::Up => self.panel_cursor = self.panel_cursor.saturating_sub(1),
                KeyCode::Down => self.panel_cursor = self.panel_cursor.saturating_add(1),
                KeyCode::Char('x') => {
                    if let Some(active) = self.state.active_runs.get(self.panel_cursor) {
                        return Some(UiCommand::CancelRun {
                            run_id: active.run_id.clone(),
                        });
                    }
                }
                KeyCode::Enter => {
                    if let Some(active) = self.state.active_runs.get(self.panel_cursor) {
                        self.panel = UiPanel::Transcript;
                        return Some(UiCommand::ResumeRun {
                            run_id: active.run_id.clone(),
                            session_id: active.session_id.clone(),
                        });
                    }
                }
                _ => {}
            },
            UiPanel::LocalModels => {
                if self.local_attachment_input {
                    match key.code {
                        KeyCode::Enter if !self.panel_input.is_empty() => {
                            self.local_attachment_input = false;
                            return Some(UiCommand::UploadAttachment {
                                path: self.panel_input.text().into(),
                            });
                        }
                        _ => {
                            let _ = self.panel_input.handle_key(key);
                        }
                    }
                } else {
                    match key.code {
                        KeyCode::Up => self.panel_cursor = self.panel_cursor.saturating_sub(1),
                        KeyCode::Down => self.panel_cursor = self.panel_cursor.saturating_add(1),
                        KeyCode::Char('a') => {
                            self.local_attachment_input = true;
                            self.panel_input.clear();
                        }
                        KeyCode::Char('u') => {
                            return Some(UiCommand::LocalModelAction {
                                action: "unload".into(),
                                model_id: None,
                                path: None,
                            });
                        }
                        KeyCode::Char('x') => {
                            if let Some(overview) = &self.local_models {
                                let index =
                                    self.panel_cursor.saturating_sub(overview.installed.len());
                                if let Some(model) = overview.catalog.get(index) {
                                    return Some(UiCommand::LocalModelAction {
                                        action: "cancel".into(),
                                        model_id: Some(model.id.clone()),
                                        path: None,
                                    });
                                }
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Enter => {
                            if let Some(overview) = &self.local_models {
                                if let Some(model) = overview.installed.get(self.panel_cursor) {
                                    return Some(UiCommand::LocalModelAction {
                                        action: "load".into(),
                                        model_id: Some(model.id.clone()),
                                        path: Some(model.path.clone()),
                                    });
                                }
                                let index =
                                    self.panel_cursor.saturating_sub(overview.installed.len());
                                if let Some(model) = overview.catalog.get(index) {
                                    return Some(UiCommand::LocalModelAction {
                                        action: "download".into(),
                                        model_id: Some(model.id.clone()),
                                        path: None,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
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
            self.approval_focused = false;
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
    let mut local_events = transport.subscribe_local_models();
    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel();
    loop {
        app.busy_tick = app.busy_tick.wrapping_add(1);
        while let Ok(result) = completion_rx.try_recv() {
            match result {
                BackgroundResult::StartRun {
                    run_id,
                    session_id,
                    result,
                } => match result {
                    Ok(receiver) => {
                        app.notice = None;
                        app.busy = Some("Thinking...".into());
                        app.state.active_runs.push(riga_server::ws::ActiveRun {
                            run_id: run_id.clone(),
                            session_id,
                            local: false,
                        });
                        app.state.select_run(Some(run_id));
                        events = Some(receiver);
                    }
                    Err(error) => {
                        app.busy = None;
                        app.notice = Some(format!("Unable to start run: {error}"));
                    }
                },
                BackgroundResult::LocalModel {
                    action,
                    model_id,
                    result,
                } => match result {
                    Ok(()) => {
                        app.local_models = Some(transport.local_models().await);
                        app.local_model_error = None;
                        if action == "load" {
                            app.busy = None;
                            app.clear_screen = true;
                            if let Some(id) = model_id {
                                app.provider.kind = riga_server::ws::ProviderKind::Local;
                                app.provider.model = id;
                                if let Err(error) =
                                    transport.configure_provider(app.provider.to_config()).await
                                {
                                    app.local_model_error = Some(error);
                                }
                            }
                        } else if action == "unload" {
                            app.busy = None;
                        }
                    }
                    Err(error) => {
                        app.busy = None;
                        app.clear_screen = true;
                        app.local_model_error = Some(error);
                    }
                },
            }
        }
        while let Ok(event) = local_events.try_recv() {
            match event {
                riga_server::local_model::LocalModelEvent::DownloadProgress(progress) => {
                    app.local_model_status =
                        Some(format!("{}: {:.1}%", progress.model_id, progress.percent));
                    app.local_model_progress = Some((progress.percent / 100.0).clamp(0.0, 1.0));
                    app.busy = Some(format!("Downloading {}...", progress.model_id));
                }
                riga_server::local_model::LocalModelEvent::DownloadFinished {
                    model_id, ..
                } => {
                    app.local_model_status = Some(format!("{model_id}: downloaded"));
                    app.local_model_progress = Some(1.0);
                    app.busy = None;
                    app.local_models = Some(transport.local_models().await);
                }
                riga_server::local_model::LocalModelEvent::DownloadFailed { model_id, message } => {
                    app.local_model_status = Some(format!("{model_id}: failed — {message}"));
                    app.local_model_progress = None;
                    app.busy = None;
                }
                riga_server::local_model::LocalModelEvent::Token { .. } => {}
            }
        }
        while let Some(receiver) = events.as_mut() {
            match receiver.try_recv() {
                Ok(envelope) => {
                    app.apply_event(envelope);
                    let terminal_run = app.state.active_run.as_deref().and_then(|run_id| {
                        app.state.run(run_id).and_then(|run| {
                            matches!(
                                run.status,
                                crate::model::RunStatus::Completed
                                    | crate::model::RunStatus::Failed
                                    | crate::model::RunStatus::Cancelled
                            )
                            .then_some(run_id.to_owned())
                        })
                    });
                    if let Some(run_id) = terminal_run {
                        app.busy = None;
                        app.archive_active_run();
                        app.state.active_runs.retain(|active| active.run_id != run_id);
                        app.state.select_run(None);
                        app.current_prompt = None;
                        events = None;
                    } else if let Some(run_id) = app.state.active_run.as_deref()
                        && let Some(run) = app.state.run(run_id)
                    {
                        app.busy = match run.status {
                            crate::model::RunStatus::Running => Some("Thinking...".into()),
                            crate::model::RunStatus::WaitingForApproval => {
                                Some("Waiting for approval...".into())
                            }
                            crate::model::RunStatus::Unknown => Some("Starting...".into()),
                            crate::model::RunStatus::Completed
                            | crate::model::RunStatus::Failed
                            | crate::model::RunStatus::Cancelled => None,
                        };
                    }
                }
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
        if app.clear_screen {
            terminal.clear().map_err(|error| error.to_string())?;
            app.clear_screen = false;
        }
        terminal
            .draw(|frame| crate::ui::render(frame, &app))
            .map_err(|error| error.to_string())?;
        if app.should_quit {
            break;
        }
        if event::poll(Duration::from_millis(60)).map_err(|error| error.to_string())? {
            match event::read().map_err(|error| error.to_string())? {
                Event::Mouse(mouse) => {
                    let height = terminal
                        .size()
                        .map_err(|error| error.to_string())?
                        .height;
                    app.handle_mouse(mouse, height);
                }
                Event::Key(key) => {
                    let Some(command) = app.handle_key(key) else {
                        continue;
                    };
                    match command {
                UiCommand::StartRun { session_id, prompt } => {
                    if app.busy.as_deref() == Some("Stopping...") {
                        app.notice = Some("The previous run is still stopping; please wait a moment.".into());
                        continue;
                    }
                    app.archive_active_run();
                    app.current_prompt = Some(prompt.clone());
                    let run_id = format!(
                        "cli-{}",
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    );
                    app.busy = Some("Starting...".into());
                    let tx = completion_tx.clone();
                    let transport = transport.clone();
                    tokio::spawn(async move {
                        let result = transport
                            .start_run(run_id.clone(), session_id.clone(), prompt)
                            .await;
                        let _ = tx.send(BackgroundResult::StartRun {
                            run_id,
                            session_id,
                            result,
                        });
                    });
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
                    if transport.cancel_run(run_id).await {
                        app.busy = Some("Stopping...".into());
                    } else {
                        app.busy = None;
                        app.notice = Some("The run could not be cancelled.".into());
                    }
                }
                UiCommand::SelectSession { session_id } => {
                    app.set_session_history(transport.session_history(session_id.clone()).await);
                    app.state.selected_session = Some(session_id);
                    app.state.active_run = None;
                    app.current_prompt = None;
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
                UiCommand::ResumeRun { run_id, session_id } => {
                    match transport.subscribe_run(run_id.clone(), 0).await {
                        Ok(receiver) => {
                            app.notice = None;
                            app.state.selected_session = Some(session_id);
                            app.state.select_run(Some(run_id));
                            events = Some(receiver);
                        }
                        Err(error) => app.notice = Some(format!("Unable to resume run: {error}")),
                    }
                }
                UiCommand::LocalModelAction {
                    action,
                    model_id,
                    path,
                } => {
                    app.busy = Some(match action.as_str() {
                        "load" => "Loading local model...".into(),
                        "download" => "Starting download...".into(),
                        "unload" => "Unloading local model...".into(),
                        _ => "Working...".into(),
                    });
                    if action == "download" {
                        app.local_model_status =
                            model_id.clone().map(|id| format!("{id}: download started"));
                        app.local_model_progress = Some(0.0);
                    }
                    let tx = completion_tx.clone();
                    let transport = transport.clone();
                    tokio::spawn(async move {
                        let result = transport
                            .local_model_action(action.clone(), model_id.clone(), path)
                            .await;
                        let _ = tx.send(BackgroundResult::LocalModel {
                            action,
                            model_id,
                            result,
                        });
                    });
                }
                UiCommand::UploadAttachment { path } => {
                    let file_name = std::path::Path::new(&path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("attachment")
                        .to_owned();
                    match tokio::fs::read(&path).await {
                        Ok(bytes) => match transport
                            .upload_attachment(riga_server::ipc::IpcAttachment {
                                name: file_name,
                                bytes,
                                session_id: app.state.selected_session.clone(),
                            })
                            .await
                        {
                            Ok(attachment) => {
                                for character in
                                    format!("\n[attachment: {}]", attachment.path).chars()
                                {
                                    app.draft.insert(character);
                                }
                                app.local_model_error = None;
                            }
                            Err(error) => app.local_model_error = Some(error),
                        },
                        Err(error) => {
                            app.local_model_error = Some(format!("attachment read failed: {error}"))
                        }
                    }
                }
                        UiCommand::Quit => break,
                    }
                }
                _ => {}
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
    fn tab_focuses_pending_approval_without_submitting_it() {
        let mut app = approval_app();
        assert!(app
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .is_none());
        assert!(app.approval_focused);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            Some(UiCommand::Approve {
                approval_id: "approval-1".into(),
                always: false
            })
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
    fn loading_session_history_projects_user_and_assistant_messages() {
        let mut app = UiState::default();
        app.set_session_history(vec![
            riga_server::ws::ConversationTurn {
                role: "user".into(),
                content: "previous question".into(),
            },
            riga_server::ws::ConversationTurn {
                role: "assistant".into(),
                content: "previous answer".into(),
            },
        ]);

        assert_eq!(
            app.state.session_history,
            vec![
                crate::model::TranscriptItem::UserText("previous question".into()),
                crate::model::TranscriptItem::AssistantText("previous answer".into()),
            ]
        );
    }

    #[test]
    fn composer_up_down_navigates_submitted_prompt_history() {
        let mut app = UiState::default();
        app.state.selected_session = Some("session-1".into());
        for prompt in ["first prompt", "second prompt"] {
            for character in prompt.chars() {
                app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
            }
            assert!(matches!(
                app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                Some(UiCommand::StartRun { .. })
            ));
        }

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "second prompt");
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "first prompt");
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "second prompt");
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.draft.is_empty());
    }

    #[test]
    fn focused_transcript_up_down_controls_follow_offset() {
        let mut app = UiState {
            transcript_focused: true,
            follow_output: true,
            ..UiState::default()
        };
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(!app.follow_output);
        assert_eq!(app.transcript_scroll, 1);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.transcript_scroll, 0);
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

    #[test]
    fn rundeck_lists_active_runs_and_resume_preserves_scope() {
        let mut app = UiState::default();
        app.state.active_runs.push(riga_server::ws::ActiveRun {
            run_id: "run-live".into(),
            session_id: "session-1".into(),
            local: true,
        });
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(app.rundeck_open);
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(!app.rundeck_open);
        app.panel = UiPanel::RunDeck;
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.deck_lens, DeckLens::Evidence);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::ResumeRun {
                run_id: "run-live".into(),
                session_id: "session-1".into(),
            })
        );
    }

    #[test]
    fn local_model_panel_loads_selected_installed_model() {
        let mut app = UiState {
            local_models: Some(riga_server::LocalModelOverview {
                accelerator: "CPU (OpenMP)",
                catalog: Vec::new(),
                installed: vec![riga_server::local_model::InstalledModel {
                    id: "local-7b".into(),
                    name: "Local 7B".into(),
                    file_name: "local.gguf".into(),
                    path: "/models/local.gguf".into(),
                    size_bytes: 1,
                    curated: true,
                    recommended_context: Some(8192),
                    license_url: None,
                }],
                loaded: None,
            }),
            ..UiState::default()
        };
        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert_eq!(app.panel, UiPanel::LocalModels);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::LocalModelAction {
                action: "load".into(),
                model_id: Some("local-7b".into()),
                path: Some("/models/local.gguf".into()),
            })
        );
    }

    #[test]
    fn selected_downloaded_model_can_start_multiple_chat_threads() {
        let mut app = UiState {
            local_models: Some(riga_server::LocalModelOverview {
                accelerator: "CPU (OpenMP)",
                catalog: Vec::new(),
                installed: vec![riga_server::local_model::InstalledModel {
                    id: "downloaded-model".into(),
                    name: "Downloaded model".into(),
                    file_name: "model.gguf".into(),
                    path: "/models/model.gguf".into(),
                    size_bytes: 1,
                    curated: true,
                    recommended_context: Some(8192),
                    license_url: None,
                }],
                loaded: Some("model.gguf".into()),
            }),
            ..UiState::default()
        };
        app.state.selected_session = Some("session-chat".into());
        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        let Some(UiCommand::LocalModelAction { model_id, .. }) =
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("installed model should produce a load command")
        };
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.provider.kind = riga_server::ws::ProviderKind::Local;
        app.provider.model = model_id.expect("selected model id");
        for prompt in ["first thread", "second thread"] {
            for character in prompt.chars() {
                app.draft.insert(character);
            }
            assert_eq!(
                app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                Some(UiCommand::StartRun {
                    session_id: "session-chat".into(),
                    prompt: prompt.into(),
                })
            );
        }
    }

    #[test]
    fn composer_accepts_r_and_t_and_shift_enter_without_submitting() {
        let mut app = UiState::default();
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "rt,\nx");
        assert!(!app.reasoning_collapsed);
        assert!(!app.tools_collapsed);
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        assert!(app.reasoning_collapsed);
        assert!(app.tools_collapsed);
    }

    #[test]
    fn starting_a_new_run_archives_the_previous_visible_turn() {
        let mut app = UiState {
            current_prompt: Some("first".into()),
            ..UiState::default()
        };
        app.state.active_run = Some("run-1".into());
        app.state
            .apply_event(riga_kernel::events::RigaEventEnvelope {
                protocol_version: riga_kernel::PROTOCOL_VERSION,
                event_id: "event-1".into(),
                session_id: "session-1".into(),
                run_id: "run-1".into(),
                sequence: 1,
                timestamp: "now".into(),
                event: riga_kernel::events::RigaEvent::TextDelta {
                    delta: "reply".into(),
                },
            });
        app.archive_active_run();
        assert!(matches!(
            app.state.session_history.as_slice(),
            [
                crate::model::TranscriptItem::UserText(prompt),
                crate::model::TranscriptItem::AssistantText(reply)
            ] if prompt == "first" && reply == "reply"
        ));
    }

    #[test]
    fn raw_shift_enter_adds_a_newline_and_control_shortcuts_select_panels() {
        let mut app = UiState::default();
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('\n'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('\r'), KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "a\nb\n");

        app.handle_key(KeyEvent::new(KeyCode::Char('\u{4}'), KeyModifiers::NONE));
        assert!(app.rundeck_open);
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(app.draft.text(), "a\nb\n\n");
        app.panel = UiPanel::Transcript;
        app.handle_key(KeyEvent::new(KeyCode::Char('\u{8}'), KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::History);
    }

    #[test]
    fn slash_and_at_commands_filter_and_insert_candidates() {
        let mut app = UiState {
            command_candidates: vec![
                "/tools/read".into(),
                "/tools/write".into(),
                "@README.md".into(),
            ],
            ..UiState::default()
        };
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert_eq!(app.completion_items(), vec!["/tools/read", "/tools/write"]);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "/tools/read ");
        app.handle_key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE));
        assert_eq!(app.completion_items(), vec!["@README.md"]);
    }

    #[test]
    fn escape_cancels_a_thinking_run_instead_of_quitting() {
        let mut app = UiState {
            busy: Some("Thinking...".into()),
            ..UiState::default()
        };
        app.state.active_run = Some("run-1".into());
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(UiCommand::CancelRun {
                run_id: "run-1".into()
            })
        );
        assert!(!app.should_quit);
    }

    #[test]
    fn escape_requires_confirmation_and_f2_opens_settings() {
        let mut app = UiState::default();
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::Settings);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.should_quit);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(app.should_quit);
    }
}
