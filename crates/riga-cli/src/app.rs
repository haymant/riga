use crate::{
    input::{InputAction, TextBuffer},
    model::{
        AppState, CatalogEntry, CompletionItem, DeckLens, ProviderForm, TranscriptItem, UiPanel,
    },
    transport::RigaTransport,
};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;
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
    RenameSession {
        session_id: String,
        title: String,
    },
    SetSessionArchived {
        session_id: String,
        archived: bool,
    },
    DeleteSession {
        session_id: String,
        confirmation_name: String,
    },
    SaveProvider {
        config: riga_server::ws::ProviderConfig,
    },
    SaveProviderProfile {
        input: riga_server::ipc::ProviderProfileInput,
    },
    DeleteProviderProfile {
        provider_id: String,
        confirmation_name: String,
    },
    LoadRemoteModels,
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
    SendShellInput {
        call_id: String,
        text: String,
    },
    ExportConversation {
        path: PathBuf,
        jsonl: String,
        record_count: usize,
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
    pub last_sidebar_panel: Option<UiPanel>,
    pub sidebar_width: u16,
    pub selection_anchor: Option<u16>,
    pub selection_cursor: Option<u16>,
    pub panel_cursor: usize,
    pub panel_input: TextBuffer,
    pub workspace_input: TextBuffer,
    pub creating_session: bool,
    pub renaming_session: Option<String>,
    pub deleting_session: Option<String>,
    pub history_archived_view: bool,
    pub session_field: usize,
    pub provider: ProviderForm,
    pub provider_field: usize,
    pub provider_profiles: Vec<riga_server::ipc::ProviderProfileSummary>,
    pub deleting_provider_profile: Option<String>,
    pub catalog: Vec<CatalogEntry>,
    pub catalog_query: TextBuffer,
    pub catalog_scope: Option<String>,
    pub deck_lens: DeckLens,
    pub deck_details_expanded: bool,
    pub local_models_expanded: bool,
    pub model_picker_open: bool,
    pub remote_models: Vec<RemoteModelChoice>,
    pub remote_models_loading: bool,
    pub remote_models_error: Option<String>,
    pub remote_model_query: TextBuffer,
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
    pub completion_range: Option<(usize, usize)>,
    pub completion_candidates: Vec<CompletionItem>,
    pub shell_input_mode: bool,
    pub shell_input: TextBuffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteModelChoice {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
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
    RemoteModels(
        Result<
            (
                Vec<riga_server::ipc::ProviderProfileSummary>,
                Vec<RemoteModelChoice>,
                Vec<String>,
            ),
            String,
        >,
    ),
}

enum SlashCommandResult {
    NotCommand,
    Consumed,
    Execute(UiCommand),
}

impl UiState {
    fn active_shell_call_id(&self) -> Option<String> {
        let run_id = self.state.active_run.as_deref()?;
        let run = self.state.run(run_id)?;
        let mut active_calls = Vec::<String>::new();
        for item in &run.transcript {
            match item {
                TranscriptItem::ToolCall { call_id, tool, .. }
                    if matches!(tool.to_ascii_lowercase().as_str(), "bash" | "shell") =>
                {
                    active_calls.push(call_id.clone());
                }
                TranscriptItem::ToolResult {
                    call_id: Some(call_id),
                    ..
                } => active_calls.retain(|active| active != call_id),
                _ => {}
            }
        }
        active_calls.pop()
    }

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

    fn conversation_export_items(&self) -> Vec<TranscriptItem> {
        let mut items = self.state.session_history.clone();
        if let Some(prompt) = &self.current_prompt {
            items.push(TranscriptItem::UserText(prompt.clone()));
        }
        if let Some(run_id) = self.state.active_run.as_deref()
            && let Some(run) = self.state.run(run_id)
            && self.state.selected_session.as_deref() == Some(run.session_id.as_str())
        {
            items.extend(run.transcript.clone());
        }
        items
    }

    fn visible_history_sessions(&self) -> Vec<riga_kernel::state::Session> {
        self.state
            .sessions
            .iter()
            .filter(|session| session.archived == self.history_archived_view)
            .cloned()
            .collect()
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

    pub fn set_completion_candidates(&mut self, candidates: Vec<CompletionItem>) {
        self.completion_candidates = candidates;
        self.refresh_completion();
    }

    fn matching_candidates(&self, token: &str) -> Vec<CompletionItem> {
        let query = token.to_lowercase();
        self.completion_candidates
            .iter()
            .filter(|candidate| {
                candidate.trigger == token.chars().next().unwrap_or_default()
                    && candidate.label.to_lowercase().starts_with(&query)
            })
            .cloned()
            .collect()
    }

    pub fn completion_items(&self) -> Vec<CompletionItem> {
        let Some((start, end)) = self.completion_range else {
            return Vec::new();
        };
        let text = self.draft.text();
        let cursor = self.draft.cursor_byte_position();
        self.matching_candidates(&text[start..cursor.min(end)])
    }

    fn active_completion_range(&self) -> Option<(char, usize, usize)> {
        let text = self.draft.text();
        let cursor = self.draft.cursor_byte_position();
        let prefix = &text[..cursor];
        let start = prefix
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, character)| index + character.len_utf8())
            .unwrap_or(0);
        let token = &text[start..cursor];
        let trigger = token.chars().next()?;
        if !matches!(trigger, '/' | '@') {
            return None;
        }
        if start > 0
            && !matches!(
                text[..start].chars().next_back(),
                Some(' ' | '\t' | '\n' | '\r')
            )
        {
            return None;
        }
        let end = text[cursor..]
            .char_indices()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, _)| cursor + index)
            .unwrap_or(text.len());
        Some((trigger, start, end))
    }

    fn refresh_completion(&mut self) {
        let Some((trigger, start, end)) = self.active_completion_range() else {
            self.completion_trigger = None;
            self.completion_range = None;
            return;
        };
        self.completion_trigger = Some(trigger);
        self.completion_range = Some((start, end));
        self.completion_cursor = self
            .completion_cursor
            .min(self.completion_items().len().saturating_sub(1));
    }

    fn accept_completion(&mut self) -> bool {
        let Some((start, end)) = self.completion_range else {
            return false;
        };
        let matches = self.completion_items();
        let Some(candidate) = matches.get(self.completion_cursor) else {
            return false;
        };
        let text = self.draft.text().to_owned();
        let mut insertion = candidate.insert_text.clone();
        if !insertion.chars().last().is_some_and(char::is_whitespace)
            && !text[end..].chars().next().is_some_and(char::is_whitespace)
        {
            insertion.push(' ');
        }
        if !self.draft.replace_range(start, end, &insertion) {
            return false;
        }
        self.completion_trigger = None;
        self.completion_range = None;
        self.refresh_completion();
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

    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        terminal_width: u16,
        terminal_height: u16,
    ) -> Option<UiCommand> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = crate::ui::history_session_index_at(
                    self,
                    terminal_width,
                    terminal_height,
                    mouse.column,
                    mouse.row,
                ) {
                    self.panel = UiPanel::History;
                    self.panel_cursor = index;
                    return self
                        .handle_panel_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
                let wide_sidebar = terminal_width >= 112 && terminal_height >= 12;
                let sidebar_width = if self.sidebar_width == 0 {
                    34
                } else {
                    self.sidebar_width.clamp(24, 56)
                };
                if wide_sidebar && mouse.column >= terminal_width.saturating_sub(sidebar_width) {
                    if (1..=5).contains(&mouse.row) {
                        let panel = match mouse.row {
                            1 => UiPanel::History,
                            2 => UiPanel::Settings,
                            3 => UiPanel::LocalModels,
                            4 => UiPanel::Help,
                            5 => UiPanel::RunDeck,
                            _ => unreachable!(),
                        };
                        self.show_sidebar_panel(panel);
                    } else {
                        self.panel = self.last_sidebar_panel.unwrap_or(UiPanel::History);
                        self.transcript_focused = false;
                        self.clear_selection();
                    }
                } else {
                    let composer_height = self
                        .draft
                        .text()
                        .lines()
                        .count()
                        .max(1)
                        .saturating_add(2)
                        .min(terminal_height.saturating_sub(5).max(3) as usize)
                        as u16;
                    let composer_top = terminal_height.saturating_sub(composer_height + 1);
                    if mouse.row >= composer_top && mouse.row < terminal_height.saturating_sub(1) {
                        self.focus_composer();
                        self.clear_selection();
                    } else if !wide_sidebar
                        && (self.panel != UiPanel::Transcript || self.rundeck_open)
                        && mouse.row < composer_top.saturating_sub(2)
                    {
                        self.transcript_focused = false;
                        self.clear_selection();
                    } else if mouse.row > 0 && mouse.row < composer_top {
                        if self.panel != UiPanel::Transcript {
                            self.last_sidebar_panel = Some(self.panel);
                        }
                        self.panel = UiPanel::Transcript;
                        self.rundeck_open = false;
                        self.transcript_focused = true;
                        self.selection_anchor = Some(mouse.row);
                        self.selection_cursor = Some(mouse.row);
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.selection_anchor.is_some() {
                    self.selection_cursor = Some(mouse.row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.selection_anchor.is_some() {
                    self.selection_cursor = Some(mouse.row);
                }
            }
            MouseEventKind::ScrollUp => {
                if self.panel == UiPanel::Transcript {
                    self.transcript_focused = true;
                    self.follow_output = false;
                    self.transcript_scroll = self.transcript_scroll.saturating_add(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.panel == UiPanel::Transcript {
                    self.transcript_focused = true;
                    self.transcript_scroll = self.transcript_scroll.saturating_sub(3);
                }
            }
            _ => {}
        }
        None
    }

    fn show_sidebar_panel(&mut self, panel: UiPanel) {
        self.panel = panel;
        self.model_picker_open = false;
        self.last_sidebar_panel = Some(panel);
        self.rundeck_open = panel == UiPanel::RunDeck;
        self.transcript_focused = false;
        self.clear_selection();
    }

    fn focus_composer(&mut self) {
        self.model_picker_open = false;
        if self.panel != UiPanel::Transcript {
            self.last_sidebar_panel = Some(self.panel);
        }
        self.panel = UiPanel::Transcript;
        self.transcript_focused = false;
        self.rundeck_open = false;
    }

    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_cursor = None;
    }

    pub fn handle_paste(&mut self, text: &str) {
        if self.shell_input_mode {
            self.shell_input.insert_str(text);
            return;
        }
        if self.panel == UiPanel::Transcript {
            self.draft.insert_str(text);
            self.prompt_history_cursor = None;
            self.refresh_completion();
            return;
        }
        match self.panel {
            UiPanel::History if self.deleting_session.is_some() => {
                self.panel_input.insert_str(text)
            }
            UiPanel::History if self.creating_session && self.session_field == 1 => {
                self.workspace_input.insert_str(text)
            }
            UiPanel::History if self.creating_session || self.renaming_session.is_some() => {
                self.panel_input.insert_str(text)
            }
            UiPanel::Settings => {
                if self.model_picker_open {
                    self.remote_model_query.insert_str(text);
                    self.panel_cursor = 0;
                } else {
                    self.panel_input.insert_str(text);
                    self.commit_provider_input();
                }
            }
            UiPanel::Catalog => self.catalog_query.insert_str(text),
            UiPanel::LocalModels if self.local_attachment_input => {
                self.panel_input.insert_str(text)
            }
            _ => self.draft.insert_str(text),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<UiCommand> {
        // Some terminals encode Backspace as Ctrl+H (or the legacy U+0008
        // character). Normalize both before shortcut routing so deleting at
        // the start of an empty composer can never open session history.
        let key = if key.code == KeyCode::Char('\u{8}')
            || (matches!(key.code, KeyCode::Char('h' | 'H'))
                && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
        } else {
            key
        };
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
        if matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SHIFT)
        {
            if let Some(run_id) = self.state.active_run.clone() {
                self.shell_input_mode = false;
                self.notice = Some("Stopping active run…".into());
                return Some(UiCommand::CancelRun { run_id });
            }
            self.notice = Some("No active run to stop.".into());
            return None;
        }
        if self.shell_input_mode {
            if key.code == KeyCode::Esc
                || (key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                self.shell_input_mode = false;
                self.notice = Some("Shell input mode closed; the run is still active.".into());
                return None;
            }
            let Some(call_id) = self.active_shell_call_id() else {
                self.shell_input_mode = false;
                self.notice = Some("The shell command has already finished.".into());
                return None;
            };
            return match self.shell_input.handle_key(key) {
                InputAction::Submit(text) => Some(UiCommand::SendShellInput {
                    call_id,
                    text: format!("{text}\n"),
                }),
                InputAction::Changed | InputAction::Ignored => None,
            };
        }
        if key.code == KeyCode::Esc && self.panel != UiPanel::Transcript {
            return self.handle_panel_key(key);
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
        if key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.panel != UiPanel::Transcript {
                self.focus_composer();
            }
            if self.active_shell_call_id().is_some() {
                self.shell_input_mode = true;
                self.shell_input.clear();
                self.transcript_focused = false;
                self.notice = Some(
                    "Shell input · type a response and press Enter; Esc returns to chat.".into(),
                );
            } else {
                self.notice = Some("No active Bash/shell command is waiting for input.".into());
            }
            return None;
        }
        if key.code == KeyCode::Tab
            && self.panel == UiPanel::Transcript
            && self.completion_trigger.is_none()
        {
            if self.transcript_focused {
                self.transcript_focused = false;
            } else {
                let panel = self.last_sidebar_panel.unwrap_or(UiPanel::History);
                self.show_sidebar_panel(panel);
            }
            return None;
        }
        if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.panel != UiPanel::Transcript {
                self.focus_composer();
                return None;
            }
        }
        if key.code == KeyCode::Tab && self.panel != UiPanel::Transcript {
            let panel_tab_navigates = match self.panel {
                UiPanel::Settings if self.model_picker_open => true,
                UiPanel::Settings => false,
                UiPanel::History if self.creating_session => false,
                UiPanel::History if self.renaming_session.is_some() => true,
                UiPanel::LocalModels if self.local_attachment_input => true,
                _ => true,
            };
            if panel_tab_navigates {
                self.focus_composer();
                return None;
            }
        }
        if self.panel == UiPanel::Transcript && self.transcript_focused {
            match key.code {
                KeyCode::Up => {
                    self.follow_output = false;
                    self.transcript_scroll = self.transcript_scroll.saturating_add(1);
                    return None;
                }
                KeyCode::Down => {
                    self.transcript_scroll = self.transcript_scroll.saturating_sub(1);
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
                KeyCode::Home => {
                    self.follow_output = false;
                    self.transcript_scroll = usize::MAX;
                    return None;
                }
                KeyCode::End => {
                    self.jump_to_bottom();
                    return None;
                }
                KeyCode::Esc => {
                    self.transcript_focused = false;
                    self.clear_selection();
                    return None;
                }
                _ if key.modifiers.contains(KeyModifiers::CONTROL)
                    || matches!(key.code, KeyCode::F(1) | KeyCode::F(3)) =>
                {
                    self.transcript_focused = false;
                }
                _ => return None,
            }
        }
        if self.panel != UiPanel::Transcript {
            let panel_is_editor = matches!(self.panel, UiPanel::Settings | UiPanel::Catalog)
                || (self.panel == UiPanel::History
                    && (self.creating_session || self.renaming_session.is_some()))
                || (self.panel == UiPanel::LocalModels && self.local_attachment_input);
            if !panel_is_editor {
                match key.code {
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        let current = if self.sidebar_width == 0 {
                            34
                        } else {
                            self.sidebar_width
                        };
                        self.sidebar_width = current.saturating_add(4).clamp(24, 56);
                        return None;
                    }
                    KeyCode::Char('-') => {
                        let current = if self.sidebar_width == 0 {
                            34
                        } else {
                            self.sidebar_width
                        };
                        self.sidebar_width = current.saturating_sub(4).clamp(24, 56);
                        return None;
                    }
                    _ => {}
                }
            }
            match key.code {
                KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.panel = UiPanel::Help;
                    return None;
                }
                KeyCode::F(1) => {
                    self.panel = UiPanel::Help;
                    return None;
                }
                KeyCode::F(3) => {
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
                KeyCode::Char('s')
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && self.panel != UiPanel::Settings =>
                {
                    self.panel = UiPanel::Settings;
                    self.provider_field = 0;
                    self.sync_provider_input();
                    return None;
                }
                KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.panel = UiPanel::LocalModels;
                    self.panel_cursor = 0;
                    self.local_attachment_input = false;
                    return None;
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.panel = UiPanel::Catalog;
                    self.panel_cursor = 0;
                    self.catalog_query.clear();
                    self.catalog_scope = None;
                    return None;
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if self.panel == UiPanel::RunDeck {
                        self.last_sidebar_panel = Some(UiPanel::RunDeck);
                        self.panel = UiPanel::Transcript;
                        self.rundeck_open = false;
                        self.transcript_focused = false;
                    } else {
                        self.show_sidebar_panel(UiPanel::RunDeck);
                    }
                    return None;
                }
                _ => {}
            }
            return self.handle_panel_key(key);
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
            KeyCode::F(3) => {
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
                self.catalog_scope = None;
                return None;
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.draft.insert('\n');
                self.refresh_completion();
                return None;
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.panel == UiPanel::RunDeck || self.rundeck_open {
                    self.last_sidebar_panel = Some(UiPanel::RunDeck);
                    self.panel = UiPanel::Transcript;
                    self.rundeck_open = false;
                } else {
                    self.show_sidebar_panel(UiPanel::RunDeck);
                }
                return None;
            }
            KeyCode::Char('\u{4}') => {
                if self.panel == UiPanel::RunDeck || self.rundeck_open {
                    self.last_sidebar_panel = Some(UiPanel::RunDeck);
                    self.panel = UiPanel::Transcript;
                    self.rundeck_open = false;
                } else {
                    self.show_sidebar_panel(UiPanel::RunDeck);
                }
                return None;
            }
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.panel = UiPanel::LocalModels;
                self.panel_cursor = 0;
                self.local_attachment_input = false;
                return None;
            }
            KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.panel = UiPanel::Help;
                return None;
            }
            KeyCode::F(1) => {
                self.panel = UiPanel::Help;
                return None;
            }
            KeyCode::Up if self.completion_trigger.is_some() => {
                self.completion_cursor = self.completion_cursor.saturating_sub(1);
                return None;
            }
            KeyCode::Down if self.completion_trigger.is_some() => {
                self.completion_cursor = self
                    .completion_cursor
                    .saturating_add(1)
                    .min(self.completion_items().len().saturating_sub(1));
                return None;
            }
            KeyCode::Tab if self.completion_trigger.is_some() => {
                if self.accept_completion() {
                    return None;
                }
            }
            KeyCode::Enter
                if self.completion_trigger.is_some()
                    && !is_native_slash_command(self.draft.text()) =>
            {
                if self.accept_completion() {
                    return None;
                }
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
                if self.draft.text().contains('\n') {
                    self.draft.move_vertical(false);
                    self.refresh_completion();
                    return None;
                }
                self.navigate_prompt_history(false);
                return None;
            }
            KeyCode::Down => {
                if self.draft.text().contains('\n') {
                    self.draft.move_vertical(true);
                    self.refresh_completion();
                    return None;
                }
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
            KeyCode::Home => {
                if !self.draft.is_empty() {
                    self.draft.move_home();
                    self.refresh_completion();
                    return None;
                }
                self.follow_output = false;
                self.transcript_scroll = usize::MAX;
                return None;
            }
            KeyCode::End => {
                if !self.draft.is_empty() {
                    self.draft.move_end();
                    self.refresh_completion();
                    return None;
                }
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
                match self.handle_slash_command(&prompt) {
                    SlashCommandResult::NotCommand => {}
                    SlashCommandResult::Consumed => return None,
                    SlashCommandResult::Execute(command) => return Some(command),
                }
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
        if key.code == KeyCode::Esc && self.panel == UiPanel::Settings && self.model_picker_open {
            self.model_picker_open = false;
            self.panel = UiPanel::Transcript;
            self.last_sidebar_panel = Some(UiPanel::Settings);
            self.transcript_focused = false;
            return None;
        }
        if key.code == KeyCode::Esc {
            self.model_picker_open = false;
            self.last_sidebar_panel = Some(self.panel);
            self.panel = UiPanel::Transcript;
            self.rundeck_open = false;
            self.transcript_focused = false;
            self.creating_session = false;
            self.renaming_session = None;
            self.deleting_session = None;
            self.panel_input.clear();
            return None;
        }
        match self.panel {
            UiPanel::History => {
                if let Some(session_id) = self.deleting_session.clone() {
                    if key.code == KeyCode::Enter {
                        let Some(session) = self
                            .state
                            .sessions
                            .iter()
                            .find(|session| session.id == session_id)
                        else {
                            self.deleting_session = None;
                            self.panel_input.clear();
                            self.notice = Some("The session no longer exists.".into());
                            return None;
                        };
                        if self.panel_input.text() == session.title {
                            let confirmation_name = self.panel_input.text().to_owned();
                            self.deleting_session = None;
                            self.panel_input.clear();
                            return Some(UiCommand::DeleteSession {
                                session_id,
                                confirmation_name,
                            });
                        }
                        self.notice =
                            Some("Session name did not match; nothing was deleted.".into());
                        return None;
                    }
                    let _ = self.panel_input.handle_key(key);
                    return None;
                }
                if let Some(session_id) = self.renaming_session.clone() {
                    if key.code == KeyCode::Enter && !self.panel_input.is_empty() {
                        self.renaming_session = None;
                        return Some(UiCommand::RenameSession {
                            session_id,
                            title: self.panel_input.text().into(),
                        });
                    }
                    let _ = self.panel_input.handle_key(key);
                    return None;
                }
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
                            .min(self.visible_history_sessions().len().saturating_sub(1))
                    }
                    KeyCode::Char('v') => {
                        self.history_archived_view = !self.history_archived_view;
                        self.panel_cursor = 0;
                    }
                    KeyCode::Char('a') if !self.history_archived_view => {
                        if let Some(session) =
                            self.visible_history_sessions().get(self.panel_cursor)
                        {
                            return Some(UiCommand::SetSessionArchived {
                                session_id: session.id.clone(),
                                archived: true,
                            });
                        }
                    }
                    KeyCode::Char('u') if self.history_archived_view => {
                        if let Some(session) =
                            self.visible_history_sessions().get(self.panel_cursor)
                        {
                            return Some(UiCommand::SetSessionArchived {
                                session_id: session.id.clone(),
                                archived: false,
                            });
                        }
                    }
                    KeyCode::Char('x') => {
                        if let Some(session) =
                            self.visible_history_sessions().get(self.panel_cursor)
                        {
                            self.deleting_session = Some(session.id.clone());
                            self.panel_input.clear();
                            self.notice = Some(format!(
                                "Type the exact session name {:?} to permanently delete it.",
                                session.title
                            ));
                        }
                    }
                    KeyCode::Char('n') => {
                        self.creating_session = true;
                        self.panel_input.clear();
                        self.workspace_input.clear();
                        self.workspace_input.insert('.');
                        self.session_field = 0;
                    }
                    KeyCode::Char('r') => {
                        if let Some(session) =
                            self.visible_history_sessions().get(self.panel_cursor)
                        {
                            self.renaming_session = Some(session.id.clone());
                            self.panel_input.clear();
                            for character in session.title.chars() {
                                self.panel_input.insert(character);
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(session) =
                            self.visible_history_sessions().get(self.panel_cursor)
                        {
                            self.last_sidebar_panel = Some(UiPanel::History);
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
                if let Some(provider_id) = self.deleting_provider_profile.clone() {
                    let Some(profile) = self
                        .provider_profiles
                        .iter()
                        .find(|item| item.id == provider_id)
                    else {
                        self.deleting_provider_profile = None;
                        self.panel_input.clear();
                        self.notice = Some("Provider profile no longer exists.".into());
                        return None;
                    };
                    if key.code == KeyCode::Esc {
                        self.deleting_provider_profile = None;
                        self.panel_input.clear();
                        return None;
                    }
                    if key.code == KeyCode::Enter {
                        if self.panel_input.text() == profile.name {
                            let confirmation_name = self.panel_input.text().to_owned();
                            self.deleting_provider_profile = None;
                            self.panel_input.clear();
                            return Some(UiCommand::DeleteProviderProfile {
                                provider_id,
                                confirmation_name,
                            });
                        }
                        self.notice =
                            Some("Provider name did not match; nothing was deleted.".into());
                        return None;
                    }
                    let _ = self.panel_input.handle_key(key);
                    return None;
                }
                if self.model_picker_open {
                    match key.code {
                        KeyCode::Up => self.panel_cursor = self.panel_cursor.saturating_sub(1),
                        KeyCode::Down => {
                            self.panel_cursor = self
                                .panel_cursor
                                .saturating_add(1)
                                .min(self.filtered_remote_models().len().saturating_sub(1));
                        }
                        KeyCode::Enter => {
                            let models = self.filtered_remote_models();
                            let choice = models
                                .get(self.panel_cursor)
                                .map(|choice| (*choice).clone());
                            if let Some(choice) = choice {
                                if let Some(profile) = self
                                    .provider_profiles
                                    .iter()
                                    .find(|profile| profile.id == choice.provider_id)
                                {
                                    self.provider.profile_name = profile.name.clone();
                                    self.provider.provider_id = Some(profile.id.clone());
                                    self.provider.provider_type = profile.provider_type;
                                    self.provider.endpoint = profile.endpoint.clone();
                                    self.provider.api = profile.api;
                                    self.provider.reasoning_effort =
                                        profile.reasoning_effort.clone();
                                    self.provider.subagent_model =
                                        profile.subagent_model.clone().unwrap_or_default();
                                }
                                self.provider.kind = riga_server::ws::ProviderKind::Remote;
                                self.provider.model = choice.model.clone();
                                self.provider.error = None;
                                self.model_picker_open = false;
                                self.panel = UiPanel::Transcript;
                                self.last_sidebar_panel = Some(UiPanel::Settings);
                                self.notice = Some(format!(
                                    "Switching to {} · {}…",
                                    choice.provider_name, choice.model
                                ));
                                return Some(UiCommand::SaveProvider {
                                    config: self.provider.to_config(),
                                });
                            }
                            if !self.remote_model_query.text().trim().is_empty() {
                                self.provider.model =
                                    self.remote_model_query.text().trim().to_owned();
                                self.provider.kind = riga_server::ws::ProviderKind::Remote;
                                self.model_picker_open = false;
                                self.panel = UiPanel::Transcript;
                                self.last_sidebar_panel = Some(UiPanel::Settings);
                                return Some(UiCommand::SaveProvider {
                                    config: self.provider.to_config(),
                                });
                            }
                        }
                        _ => {
                            let _ = self.remote_model_query.handle_key(key);
                            self.panel_cursor = 0;
                        }
                    }
                    return None;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
                    self.provider = ProviderForm::default();
                    self.provider.profile_name = "New provider".into();
                    self.provider_field = 0;
                    self.sync_provider_input();
                    self.notice = Some("Editing a new remote provider profile.".into());
                    return None;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
                    if self.provider_profiles.is_empty() {
                        self.notice =
                            Some("No saved provider profiles. Press Ctrl+N to add one.".into());
                    } else {
                        let current = self
                            .provider
                            .provider_id
                            .as_deref()
                            .and_then(|id| {
                                self.provider_profiles
                                    .iter()
                                    .position(|profile| profile.id == id)
                            })
                            .unwrap_or(usize::MAX);
                        let next = if current == usize::MAX {
                            0
                        } else {
                            (current + 1) % self.provider_profiles.len()
                        };
                        if let Some(profile) = self.provider_profiles.get(next) {
                            let id = profile.id.clone();
                            let name = profile.name.clone();
                            self.load_provider_profile(&id);
                            self.provider_field = 0;
                            self.sync_provider_input();
                            self.notice = Some(format!("Editing provider profile {name}."));
                        }
                    }
                    return None;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x') {
                    if let Some(profile) = self.provider.provider_id.as_ref().and_then(|id| {
                        self.provider_profiles
                            .iter()
                            .find(|profile| &profile.id == id)
                    }) {
                        self.deleting_provider_profile = Some(profile.id.clone());
                        self.panel_input.clear();
                        self.notice = Some(format!(
                            "Type the exact provider name {:?} to delete it.",
                            profile.name
                        ));
                    } else {
                        self.notice =
                            Some("Load a saved provider profile before deleting it.".into());
                    }
                    return None;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                    self.commit_provider_input();
                    return Some(UiCommand::SaveProviderProfile {
                        input: riga_server::ipc::ProviderProfileInput {
                            id: self.provider.provider_id.clone(),
                            name: self.provider.profile_name.trim().to_owned(),
                            provider_type: self.provider.provider_type,
                            endpoint: self.provider.endpoint.trim().to_owned(),
                            api_key: self.provider.api_key.clone(),
                            model: self.provider.model.trim().to_owned(),
                            reasoning_effort: self.provider.reasoning_effort.clone(),
                            api: self.provider.api,
                            subagent_model: (!self.provider.subagent_model.trim().is_empty())
                                .then(|| self.provider.subagent_model.trim().to_owned()),
                        },
                    });
                }
                match key.code {
                    KeyCode::Tab => {
                        self.commit_provider_input();
                        self.provider_field = (self.provider_field + 1) % 8;
                        self.sync_provider_input();
                    }
                    KeyCode::BackTab => {
                        self.commit_provider_input();
                        self.provider_field = self.provider_field.checked_sub(1).unwrap_or(7);
                        self.sync_provider_input();
                    }
                    KeyCode::Enter if self.provider_field == 4 => {
                        self.provider.reasoning_effort =
                            match self.provider.reasoning_effort.as_str() {
                                "low" => "medium",
                                "medium" => "high",
                                _ => "low",
                            }
                            .into();
                        self.sync_provider_input();
                    }
                    KeyCode::Enter if self.provider_field == 5 => {
                        self.provider.provider_type = match self.provider.provider_type {
                            riga_server::ws::ProviderType::OpenAiCompatible => {
                                riga_server::ws::ProviderType::OpencodeOpenAiCompatible
                            }
                            riga_server::ws::ProviderType::OpencodeOpenAiCompatible => {
                                riga_server::ws::ProviderType::OpenAiCompatible
                            }
                        };
                        if self.provider.provider_type
                            == riga_server::ws::ProviderType::OpencodeOpenAiCompatible
                        {
                            self.provider.api = riga_server::ws::ProviderApi::Responses;
                        }
                        self.sync_provider_input();
                    }
                    KeyCode::Enter if self.provider_field == 6 => {
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
                KeyCode::Char('v') => {
                    self.deck_details_expanded = !self.deck_details_expanded;
                }
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
                        KeyCode::Char('v') => {
                            self.local_models_expanded = !self.local_models_expanded;
                        }
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
            0 => self.provider.profile_name.clone(),
            1 => self.provider.endpoint.clone(),
            2 => self.provider.api_key.clone(),
            3 => self.provider.model.clone(),
            4 => self.provider.reasoning_effort.clone(),
            5 => format!("{:?}", self.provider.provider_type),
            6 => format!("{:?}", self.provider.api),
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
            0 => self.provider.profile_name = value,
            1 => self.provider.endpoint = value,
            2 => self.provider.api_key = value,
            3 => self.provider.model = value,
            4 => self.provider.reasoning_effort = value,
            7 => self.provider.subagent_model = value,
            _ => {}
        }
    }

    fn load_provider_profile(&mut self, profile_id: &str) -> bool {
        let Some(profile) = self
            .provider_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
        else {
            return false;
        };
        self.provider.profile_name = profile.name.clone();
        self.provider.provider_id = Some(profile.id.clone());
        self.provider.provider_type = profile.provider_type;
        self.provider.endpoint = profile.endpoint.clone();
        self.provider.api_key.clear();
        self.provider.model = profile.model.clone();
        self.provider.reasoning_effort = profile.reasoning_effort.clone();
        self.provider.kind = riga_server::ws::ProviderKind::Remote;
        self.provider.api = profile.api;
        self.provider.subagent_model = profile.subagent_model.clone().unwrap_or_default();
        self.provider.error = None;
        self.sync_provider_input();
        true
    }

    pub fn filtered_catalog(&self) -> Vec<&CatalogEntry> {
        let query = self.catalog_query.text().to_lowercase();
        let scope = self
            .catalog_scope
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        self.catalog
            .iter()
            .filter(|entry| {
                (scope.is_empty() || entry.kind.to_lowercase().contains(&scope))
                    && (query.is_empty()
                        || entry.id.to_lowercase().contains(&query)
                        || entry.kind.to_lowercase().contains(&query)
                        || entry.description.to_lowercase().contains(&query))
            })
            .collect()
    }

    pub fn set_catalog(&mut self, catalog: Vec<CatalogEntry>) {
        self.catalog = catalog;
    }
    pub fn set_provider(&mut self, provider: ProviderForm) {
        self.provider = provider;
    }

    pub fn filtered_remote_models(&self) -> Vec<&RemoteModelChoice> {
        let query = self.remote_model_query.text().trim().to_lowercase();
        self.remote_models
            .iter()
            .filter(|choice| {
                query.is_empty()
                    || choice.model.to_lowercase().contains(&query)
                    || choice.provider_name.to_lowercase().contains(&query)
                    || choice.provider_id.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn open_model_picker(&mut self) -> UiCommand {
        self.show_sidebar_panel(UiPanel::Settings);
        self.model_picker_open = true;
        self.remote_models.clear();
        self.remote_models_loading = true;
        self.remote_models_error = None;
        self.remote_model_query.clear();
        self.panel_cursor = 0;
        self.notice = Some("Loading registered providers and their remote models…".into());
        UiCommand::LoadRemoteModels
    }

    fn open_catalog_scope(&mut self, scope: &str, query: &str) {
        self.show_sidebar_panel(UiPanel::Catalog);
        self.catalog_scope = Some(scope.into());
        self.catalog_query.clear();
        self.catalog_query.insert_str(query);
        self.panel_cursor = 0;
    }

    fn handle_slash_command(&mut self, prompt: &str) -> SlashCommandResult {
        let trimmed = prompt.trim();
        if !trimmed.starts_with('/') {
            return SlashCommandResult::NotCommand;
        }
        let mut parts = trimmed.split_whitespace();
        let name = parts.next().unwrap_or_default().to_ascii_lowercase();
        let arguments = parts.collect::<Vec<_>>().join(" ");

        match name.as_str() {
            "/model" | "/models" => {
                let model = arguments.strip_prefix("set ").unwrap_or(&arguments).trim();
                if model.is_empty() || model.eq_ignore_ascii_case("list") {
                    return SlashCommandResult::Execute(self.open_model_picker());
                }
                if self.provider.endpoint.trim().is_empty() {
                    self.show_sidebar_panel(UiPanel::Settings);
                    self.provider_field = 0;
                    self.sync_provider_input();
                    self.notice = Some(
                        "Configure a provider endpoint in Settings before selecting a remote model."
                            .into(),
                    );
                    return SlashCommandResult::Consumed;
                }
                self.provider.kind = riga_server::ws::ProviderKind::Remote;
                self.provider.model = model.to_owned();
                self.provider.error = None;
                self.notice = Some(format!("Switching to remote model {model}…"));
                SlashCommandResult::Execute(UiCommand::SaveProvider {
                    config: self.provider.to_config(),
                })
            }
            "/export" => {
                let items = self.conversation_export_items();
                let session_id = self.state.selected_session.clone();
                let session_title = session_id.as_deref().and_then(|session_id| {
                    self.state
                        .sessions
                        .iter()
                        .find(|session| session.id == session_id)
                        .map(|session| session.title.clone())
                });
                let jsonl =
                    conversation_jsonl(&items, session_id.as_deref(), session_title.as_deref());
                SlashCommandResult::Execute(UiCommand::ExportConversation {
                    path: jsonl_export_path(&arguments),
                    jsonl,
                    record_count: items.len(),
                })
            }
            "/new" | "/clear" => {
                if self.state.active_run.is_some() || self.busy.is_some() {
                    self.notice = Some(
                        "Wait for the active operation to finish before starting a new session."
                            .into(),
                    );
                    return SlashCommandResult::Consumed;
                }
                let workspace = self
                    .state
                    .selected_session
                    .as_ref()
                    .and_then(|selected| {
                        self.state
                            .sessions
                            .iter()
                            .find(|session| &session.id == selected)
                    })
                    .map(|session| session.workspace.clone())
                    .unwrap_or_else(|| ".".into());
                SlashCommandResult::Execute(UiCommand::CreateSession {
                    title: if arguments.is_empty() {
                        "New session".into()
                    } else {
                        arguments
                    },
                    workspace,
                })
            }
            "/resume" | "/sessions" => {
                self.show_sidebar_panel(UiPanel::History);
                if let Some(selected) = &self.state.selected_session
                    && let Some(index) = self
                        .state
                        .sessions
                        .iter()
                        .position(|session| &session.id == selected)
                {
                    self.panel_cursor = index;
                }
                SlashCommandResult::Consumed
            }
            "/tools" => {
                self.open_catalog_scope("tools", &arguments);
                SlashCommandResult::Consumed
            }
            "/mcp" => {
                self.open_catalog_scope("mcp", &arguments);
                SlashCommandResult::Consumed
            }
            "/skills" => {
                self.open_catalog_scope("skills", &arguments);
                SlashCommandResult::Consumed
            }
            "/settings" => {
                self.show_sidebar_panel(UiPanel::Settings);
                self.provider_field = 0;
                self.sync_provider_input();
                SlashCommandResult::Consumed
            }
            "/local-models" => {
                self.show_sidebar_panel(UiPanel::LocalModels);
                self.panel_cursor = 0;
                self.local_attachment_input = false;
                SlashCommandResult::Consumed
            }
            "/rename" => {
                let Some(selected) = self.state.selected_session.clone() else {
                    self.notice = Some("There is no session to rename yet.".into());
                    return SlashCommandResult::Consumed;
                };
                let Some(index) = self
                    .state
                    .sessions
                    .iter()
                    .position(|session| session.id == selected)
                else {
                    self.notice = Some("The selected session could not be found.".into());
                    return SlashCommandResult::Consumed;
                };
                self.show_sidebar_panel(UiPanel::History);
                self.panel_cursor = index;
                if arguments.is_empty() {
                    self.renaming_session = Some(selected);
                    self.panel_input.clear();
                    for character in self.state.sessions[index].title.chars() {
                        self.panel_input.insert(character);
                    }
                    SlashCommandResult::Consumed
                } else {
                    SlashCommandResult::Execute(UiCommand::RenameSession {
                        session_id: selected,
                        title: arguments,
                    })
                }
            }
            "/status" => {
                let model = if self.provider.model.is_empty() {
                    "not configured"
                } else {
                    self.provider.model.as_str()
                };
                let session = self
                    .state
                    .selected_session
                    .as_ref()
                    .and_then(|selected| {
                        self.state
                            .sessions
                            .iter()
                            .find(|session| &session.id == selected)
                    })
                    .map(|session| session.title.as_str())
                    .unwrap_or("none");
                self.notice = Some(format!(
                    "Connection {:?} · provider {:?} · model {model} · session {session}",
                    self.state.connection, self.provider.kind
                ));
                SlashCommandResult::Consumed
            }
            "/help" => {
                self.show_sidebar_panel(UiPanel::Help);
                SlashCommandResult::Consumed
            }
            "/exit" | "/quit" => {
                self.should_quit = true;
                SlashCommandResult::Execute(UiCommand::Quit)
            }
            _ => SlashCommandResult::NotCommand,
        }
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

fn conversation_jsonl(
    items: &[TranscriptItem],
    session_id: Option<&str>,
    session_title: Option<&str>,
) -> String {
    let mut records = Vec::with_capacity(items.len() + 1);
    records.push(serde_json::json!({
        "type": "conversation",
        "format": "riga-jsonl-v1",
        "session_id": session_id,
        "session_title": session_title,
    }));
    for item in items {
        records.push(match item {
            TranscriptItem::UserText(content) => {
                serde_json::json!({ "type": "message", "role": "user", "content": content })
            }
            TranscriptItem::AssistantText(content) => {
                serde_json::json!({ "type": "message", "role": "assistant", "content": content })
            }
            TranscriptItem::AssistantReasoning(content) => {
                serde_json::json!({ "type": "reasoning", "content": content })
            }
            TranscriptItem::ToolCall {
                call_id,
                tool,
                call,
            } => serde_json::json!({
                "type": "tool_call",
                "call_id": call_id,
                "tool": tool,
                "arguments": call,
            }),
            TranscriptItem::ToolOutput { call_id, output } => serde_json::json!({
                "type": "tool_output",
                "call_id": call_id,
                "output": output,
            }),
            TranscriptItem::ToolResult { call_id, result } => serde_json::json!({
                "type": "tool_result",
                "call_id": call_id,
                "result": result,
            }),
            TranscriptItem::TaskResult {
                task_id,
                ok,
                result,
            } => serde_json::json!({
                "type": "task_result",
                "task_id": task_id,
                "ok": ok,
                "result": result,
            }),
            TranscriptItem::Approval {
                approval_id,
                tool,
                summary,
            } => serde_json::json!({
                "type": "approval",
                "approval_id": approval_id,
                "tool": tool,
                "summary": summary,
            }),
            TranscriptItem::System(content) => {
                serde_json::json!({ "type": "message", "role": "system", "content": content })
            }
        });
    }
    let mut jsonl = records
        .iter()
        .map(|record| serde_json::to_string(record).unwrap_or_else(|_| "{}".into()))
        .collect::<Vec<_>>()
        .join("\n");
    jsonl.push('\n');
    jsonl
}

fn jsonl_export_path(argument: &str) -> PathBuf {
    let supplied = argument.trim();
    let supplied = supplied
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            supplied
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(supplied);
    let mut path = if supplied.is_empty() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(format!("riga-conversation-{stamp}.jsonl"))
    } else {
        PathBuf::from(supplied)
    };
    if path.extension().is_none() {
        path.set_extension("jsonl");
    }
    path
}

async fn write_jsonl_export(path: &Path, jsonl: &str) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = file.write_all(jsonl.as_bytes()).await {
        drop(file);
        let _ = tokio::fs::remove_file(path).await;
        return Err(error.to_string());
    }
    file.flush().await.map_err(|error| error.to_string())
}

fn session_title_from_response(response: &str) -> Option<String> {
    let line = response
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let cleaned = line
        .trim_start_matches(|character: char| "#>*- ".contains(character))
        .trim()
        .trim_matches('`');
    if cleaned.is_empty() {
        return None;
    }
    let end = cleaned
        .find(|character: char| matches!(character, '.' | '!' | '?'))
        .map(|index| index + 1)
        .unwrap_or(cleaned.len());
    let title = cleaned[..end].trim().trim_matches('"').trim();
    if title.is_empty() {
        return None;
    }
    if title.chars().count() > 80 {
        Some(format!("{}…", title.chars().take(79).collect::<String>()))
    } else {
        Some(title.to_owned())
    }
}

fn is_native_slash_command(prompt: &str) -> bool {
    matches!(
        prompt.split_whitespace().next().unwrap_or_default(),
        "/model"
            | "/models"
            | "/export"
            | "/tools"
            | "/mcp"
            | "/skills"
            | "/settings"
            | "/local-models"
            | "/new"
            | "/clear"
            | "/resume"
            | "/sessions"
            | "/rename"
            | "/status"
            | "/help"
            | "/exit"
            | "/quit"
    )
}

pub async fn run_with_transport<T: RigaTransport>(
    mut app: UiState,
    transport: T,
) -> Result<(), String> {
    if let Ok(profiles) = transport.provider_profiles().await {
        if app.provider.provider_id.is_none()
            && let Some(profile) = profiles.iter().find(|profile| {
                profile.endpoint == app.provider.endpoint && profile.model == app.provider.model
            })
        {
            app.provider.provider_id = Some(profile.id.clone());
            app.provider.provider_type = profile.provider_type;
            app.provider.profile_name = profile.name.clone();
        }
        app.provider_profiles = profiles;
    }
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
                BackgroundResult::RemoteModels(result) => {
                    app.remote_models_loading = false;
                    match result {
                        Ok((profiles, models, errors)) => {
                            app.provider_profiles = profiles;
                            app.remote_models = models;
                            app.remote_models_error = if errors.is_empty() {
                                None
                            } else {
                                Some(errors.join(" · "))
                            };
                            if let Some(index) = app.remote_models.iter().position(|choice| {
                                choice.model == app.provider.model
                                    && Some(choice.provider_id.as_str())
                                        == app.provider.provider_id.as_deref()
                            }) {
                                app.panel_cursor = index;
                            }
                            app.notice = Some(format!(
                                "Loaded {} model{} across {} provider{}{}",
                                app.remote_models.len(),
                                if app.remote_models.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                },
                                app.provider_profiles.len(),
                                if app.provider_profiles.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                },
                                if errors.is_empty() {
                                    ".".into()
                                } else {
                                    format!("; {} provider catalog(s) unavailable.", errors.len())
                                }
                            ));
                        }
                        Err(error) => {
                            app.remote_models_error = Some(error.clone());
                            app.notice = Some(error);
                        }
                    }
                }
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
                        let first_reply = app.state.run(&run_id).and_then(|run| {
                            let is_untitled = app
                                .state
                                .sessions
                                .iter()
                                .find(|session| session.id == run.session_id)
                                .is_some_and(|session| session.title == "New session");
                            (run.status == crate::model::RunStatus::Completed
                                && is_untitled
                                && app.state.session_history.is_empty())
                            .then(|| (run.session_id.clone(), run.output.clone()))
                        });
                        if let Some((session_id, response)) = first_reply
                            && let Some(title) = session_title_from_response(&response)
                        {
                            match transport.rename_session(session_id.clone(), title).await {
                                Ok(session) => {
                                    if let Some(existing) = app
                                        .state
                                        .sessions
                                        .iter_mut()
                                        .find(|existing| existing.id == session_id)
                                    {
                                        *existing = session;
                                    }
                                }
                                Err(error) => {
                                    app.notice = Some(format!("Could not name session: {error}"))
                                }
                            }
                        }
                        app.archive_active_run();
                        app.state
                            .active_runs
                            .retain(|active| active.run_id != run_id);
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
                    let size = terminal.size().map_err(|error| error.to_string())?;
                    if let Some(UiCommand::SelectSession { session_id }) =
                        app.handle_mouse(mouse, size.width, size.height)
                    {
                        app.set_session_history(
                            transport.session_history(session_id.clone()).await,
                        );
                        app.state.selected_session = Some(session_id);
                        app.state.active_run = None;
                        app.current_prompt = None;
                        events = None;
                    }
                }
                Event::Paste(text) => {
                    app.handle_paste(&text);
                }
                Event::Key(key) => {
                    if matches!(key.code, KeyCode::Char('c' | 'C'))
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.modifiers.contains(KeyModifiers::SHIFT)
                    {
                        if app.selection_anchor.is_none() {
                            app.notice = Some(
                                "Select transcript text first, then press Ctrl+Shift+C.".into(),
                            );
                            continue;
                        }
                        let size = terminal.size().map_err(|error| error.to_string())?;
                        let text =
                            crate::ui::selected_transcript_text(&app, size.width, size.height);
                        if text.is_empty() {
                            app.notice = Some("No transcript rows selected.".into());
                        } else if let Err(error) = terminal.copy_to_clipboard(&text) {
                            app.notice = Some(format!("Clipboard copy failed: {error}"));
                        } else {
                            app.notice = Some(format!(
                                "Copied {} characters to the terminal clipboard.",
                                text.chars().count()
                            ));
                        }
                        app.clear_selection();
                        continue;
                    }
                    let Some(command) = app.handle_key(key) else {
                        continue;
                    };
                    match command {
                        UiCommand::StartRun { session_id, prompt } => {
                            if app.busy.as_deref() == Some("Stopping...") {
                                app.notice = Some(
                                    "The previous run is still stopping; please wait a moment."
                                        .into(),
                                );
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
                            app.set_session_history(
                                transport.session_history(session_id.clone()).await,
                            );
                            app.state.selected_session = Some(session_id);
                            app.state.active_run = None;
                            app.current_prompt = None;
                            events = None;
                        }
                        UiCommand::CreateSession { title, workspace } => {
                            let session = transport
                                .create_session(riga_server::CreateSessionRequest {
                                    title,
                                    workspace,
                                })
                                .await?;
                            if app.state.active_run.is_none() {
                                app.set_session_history(Vec::new());
                                app.current_prompt = None;
                            }
                            app.state.selected_session = Some(session.id.clone());
                            app.state.sessions.push(session);
                        }
                        UiCommand::RenameSession { session_id, title } => {
                            match transport.rename_session(session_id.clone(), title).await {
                                Ok(session) => {
                                    if let Some(existing) = app
                                        .state
                                        .sessions
                                        .iter_mut()
                                        .find(|existing| existing.id == session_id)
                                    {
                                        *existing = session;
                                    }
                                    app.notice = Some("Session name updated.".into());
                                }
                                Err(error) => {
                                    app.notice = Some(format!("Could not rename session: {error}"))
                                }
                            }
                        }
                        UiCommand::SetSessionArchived {
                            session_id,
                            archived,
                        } => {
                            match transport
                                .set_session_archived(session_id.clone(), archived)
                                .await
                            {
                                Ok(session) => {
                                    if let Some(existing) = app
                                        .state
                                        .sessions
                                        .iter_mut()
                                        .find(|existing| existing.id == session_id)
                                    {
                                        *existing = session;
                                    }
                                    app.panel_cursor = app.panel_cursor.min(
                                        app.visible_history_sessions().len().saturating_sub(1),
                                    );
                                    app.notice = Some(if archived {
                                        "Session archived. Press v to browse archived sessions."
                                            .into()
                                    } else {
                                        "Session restored to active history.".into()
                                    });
                                }
                                Err(error) => {
                                    app.notice = Some(format!("Could not update session: {error}"));
                                }
                            }
                        }
                        UiCommand::DeleteSession {
                            session_id,
                            confirmation_name,
                        } => {
                            let deleted = app
                                .state
                                .sessions
                                .iter()
                                .find(|session| session.id == session_id)
                                .cloned();
                            match transport
                                .delete_session(session_id.clone(), confirmation_name)
                                .await
                            {
                                Ok(()) => {
                                    app.state
                                        .sessions
                                        .retain(|session| session.id != session_id);
                                    app.panel_cursor = app.panel_cursor.min(
                                        app.visible_history_sessions().len().saturating_sub(1),
                                    );
                                    if app.state.selected_session.as_deref()
                                        == Some(session_id.as_str())
                                    {
                                        let replacement = app
                                            .state
                                            .sessions
                                            .iter()
                                            .find(|session| !session.archived)
                                            .or_else(|| app.state.sessions.first())
                                            .cloned();
                                        if let Some(session) = replacement {
                                            app.history_archived_view = session.archived;
                                            app.state.selected_session = Some(session.id.clone());
                                            app.set_session_history(
                                                transport.session_history(session.id.clone()).await,
                                            );
                                        } else {
                                            let workspace = deleted
                                                .as_ref()
                                                .map(|session| session.workspace.clone())
                                                .unwrap_or_else(|| ".".into());
                                            let session = transport
                                                .create_session(riga_server::CreateSessionRequest {
                                                    title: "New session".into(),
                                                    workspace,
                                                })
                                                .await?;
                                            app.state.selected_session = Some(session.id.clone());
                                            app.state.sessions.push(session);
                                            app.history_archived_view = false;
                                            app.set_session_history(Vec::new());
                                        }
                                        app.state.active_run = None;
                                        app.current_prompt = None;
                                        events = None;
                                    }
                                    app.notice = Some("Session permanently deleted.".into());
                                }
                                Err(error) => {
                                    app.notice = Some(format!("Could not delete session: {error}"));
                                }
                            }
                        }
                        UiCommand::SaveProvider { config } => {
                            let selected_model = config.model.clone();
                            match transport.configure_provider(config).await {
                                Ok(_) => {
                                    app.provider.error = None;
                                    app.model_picker_open = false;
                                    app.panel = UiPanel::Transcript;
                                    app.notice = Some(format!(
                                        "Provider settings saved · model {selected_model}."
                                    ));
                                }
                                Err(error) => {
                                    app.provider.error = Some(error.clone());
                                    app.notice = Some(format!("Could not save provider: {error}"));
                                }
                            }
                        }
                        UiCommand::SaveProviderProfile { input } => {
                            match transport.save_provider_profile(input).await {
                                Ok(profile) => {
                                    app.provider_profiles.retain(|saved| saved.id != profile.id);
                                    app.provider_profiles.push(profile.clone());
                                    app.provider.profile_name = profile.name.clone();
                                    app.provider.provider_id = Some(profile.id.clone());
                                    app.provider.provider_type = profile.provider_type;
                                    app.provider.endpoint = profile.endpoint.clone();
                                    app.provider.model = profile.model.clone();
                                    app.provider.reasoning_effort =
                                        profile.reasoning_effort.clone();
                                    app.provider.api = profile.api;
                                    app.provider.subagent_model =
                                        profile.subagent_model.clone().unwrap_or_default();
                                    app.provider.api_key.clear();
                                    app.provider.kind = riga_server::ws::ProviderKind::Remote;
                                    match transport
                                        .configure_provider(app.provider.to_config())
                                        .await
                                    {
                                        Ok(_) => {
                                            app.provider.error = None;
                                            app.notice = Some(format!(
                                                "Saved and activated provider {} · {}.",
                                                profile.name, profile.model
                                            ));
                                        }
                                        Err(error) => {
                                            app.provider.error = Some(error.clone());
                                            app.notice = Some(format!(
                                                "Profile saved, but activation failed: {error}"
                                            ));
                                        }
                                    }
                                }
                                Err(error) => {
                                    app.provider.error = Some(error.clone());
                                    app.notice =
                                        Some(format!("Could not save provider profile: {error}"));
                                }
                            }
                        }
                        UiCommand::DeleteProviderProfile {
                            provider_id,
                            confirmation_name,
                        } => {
                            let expected = app
                                .provider_profiles
                                .iter()
                                .find(|profile| profile.id == provider_id)
                                .map(|profile| profile.name.clone());
                            if expected.as_deref() != Some(confirmation_name.as_str()) {
                                app.notice = Some(
                                    "Provider name did not match; nothing was deleted.".into(),
                                );
                                continue;
                            }
                            match transport.delete_provider_profile(provider_id.clone()).await {
                                Ok(()) => {
                                    app.provider_profiles
                                        .retain(|profile| profile.id != provider_id);
                                    if app.provider.provider_id.as_deref()
                                        == Some(provider_id.as_str())
                                    {
                                        app.provider = ProviderForm::default();
                                        app.provider_field = 0;
                                        app.sync_provider_input();
                                    }
                                    app.notice = Some("Provider profile deleted.".into());
                                }
                                Err(error) => {
                                    app.notice =
                                        Some(format!("Could not delete provider profile: {error}"))
                                }
                            }
                        }
                        UiCommand::LoadRemoteModels => {
                            app.remote_models_loading = true;
                            app.remote_models_error = None;
                            let tx = completion_tx.clone();
                            let transport = transport.clone();
                            tokio::spawn(async move {
                                let result = async {
                                    let profiles = transport.provider_profiles().await?;
                                    let mut choices = Vec::new();
                                    let mut errors = Vec::new();
                                    for profile in &profiles {
                                        match transport.remote_models(Some(profile.id.clone())).await {
                                            Ok(models) => {
                                                for model in models.into_iter().chain(std::iter::once(profile.model.clone())) {
                                                    if !model.is_empty() && !choices.iter().any(|choice: &RemoteModelChoice| choice.provider_id == profile.id && choice.model == model) {
                                                        choices.push(RemoteModelChoice { provider_id: profile.id.clone(), provider_name: profile.name.clone(), model });
                                                    }
                                                }
                                            }
                                            Err(error) => errors.push(format!("{}: {error}", profile.name)),
                                        }
                                    }
                                    if profiles.is_empty() {
                                        return Err("No saved provider profiles; use Ctrl+N in Settings to add one.".to_owned());
                                    }
                                    Ok((profiles, choices, errors))
                                }.await;
                                let _ = tx.send(BackgroundResult::RemoteModels(result));
                            });
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
                                Err(error) => {
                                    app.notice = Some(format!("Unable to resume run: {error}"))
                                }
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
                        UiCommand::SendShellInput { call_id, text } => {
                            app.shell_input.clear();
                            if riga_server::catalog::send_interactive_shell_input(&call_id, text) {
                                app.notice = Some("Input sent to the active shell command.".into());
                            } else {
                                app.notice =
                                    Some("The shell command is no longer accepting input.".into());
                                app.shell_input_mode = false;
                            }
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
                                    app.local_model_error =
                                        Some(format!("attachment read failed: {error}"))
                                }
                            }
                        }
                        UiCommand::ExportConversation {
                            path,
                            jsonl,
                            record_count,
                        } => match write_jsonl_export(&path, &jsonl).await {
                            Ok(()) => {
                                app.notice = Some(format!(
                                    "Exported {record_count} conversation records to {}.",
                                    path.display()
                                ));
                            }
                            Err(error) => {
                                app.notice = Some(format!(
                                    "Could not export conversation to {}: {error}",
                                    path.display()
                                ));
                            }
                        },
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

    fn submit_text(app: &mut UiState, text: &str) -> Option<UiCommand> {
        for character in text.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    #[test]
    fn slash_model_opens_remote_picker_and_direct_model_selection_saves_provider() {
        let mut app = UiState::default();
        app.provider.endpoint = "https://provider.example/v1".into();
        app.set_completion_candidates(vec![CompletionItem {
            trigger: '/',
            category: "Commands".into(),
            label: "/model".into(),
            detail: "Choose a remote model".into(),
            insert_text: "/model".into(),
        }]);
        assert_eq!(
            submit_text(&mut app, "/model"),
            Some(UiCommand::LoadRemoteModels)
        );
        assert!(app.model_picker_open);
        assert_eq!(app.panel, UiPanel::Settings);

        let mut direct = UiState::default();
        direct.provider.endpoint = "https://provider.example/v1".into();
        let command = submit_text(&mut direct, "/model set org/model-v2");
        assert!(matches!(
            command,
            Some(UiCommand::SaveProvider { config })
                if config.model == "org/model-v2"
                    && config.kind == riga_server::ws::ProviderKind::Remote
        ));
    }

    #[test]
    fn export_jsonl_contains_only_the_open_conversation() {
        let mut app = UiState::default();
        app.state.selected_session = Some("open-session".into());
        app.state.sessions.extend([
            riga_kernel::state::Session {
                id: "open-session".into(),
                title: "Open conversation".into(),
                workspace: ".".into(),
                archived: false,
                created_at: "now".into(),
                updated_at: "now".into(),
            },
            riga_kernel::state::Session {
                id: "other-session".into(),
                title: "Do not export".into(),
                workspace: ".".into(),
                archived: false,
                created_at: "now".into(),
                updated_at: "now".into(),
            },
        ]);
        app.set_session_history(vec![
            riga_server::ws::ConversationTurn {
                role: "user".into(),
                content: "question from current session".into(),
            },
            riga_server::ws::ConversationTurn {
                role: "assistant".into(),
                content: "answer from current session".into(),
            },
        ]);
        app.set_completion_candidates(vec![CompletionItem {
            trigger: '/',
            category: "Commands".into(),
            label: "/export".into(),
            detail: "Export this conversation as JSONL".into(),
            insert_text: "/export".into(),
        }]);

        let command = submit_text(&mut app, "/export current.jsonl");
        let Some(UiCommand::ExportConversation {
            path,
            jsonl,
            record_count,
        }) = command
        else {
            panic!("/export should produce a JSONL export command");
        };
        assert_eq!(path, PathBuf::from("current.jsonl"));
        assert_eq!(record_count, 2);
        let records = jsonl
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["type"], "conversation");
        assert_eq!(records[0]["session_id"], "open-session");
        assert_eq!(records[0]["session_title"], "Open conversation");
        assert_eq!(records[1]["content"], "question from current session");
        assert_eq!(records[2]["content"], "answer from current session");
        assert!(!jsonl.contains("Do not export"));
    }

    #[tokio::test]
    async fn jsonl_export_creates_new_file_without_overwriting_existing_data() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "riga-cli-export-{}-{stamp}.jsonl",
            std::process::id()
        ));
        write_jsonl_export(&path, "{\"type\":\"conversation\"}\n")
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "{\"type\":\"conversation\"}\n"
        );
        assert!(write_jsonl_export(&path, "replacement\n").await.is_err());
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "{\"type\":\"conversation\"}\n"
        );
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[test]
    fn remote_model_picker_filters_and_selects_the_active_provider_model() {
        let alpha = RemoteModelChoice {
            provider_id: "alpha-id".into(),
            provider_name: "Alpha".into(),
            model: "model-x".into(),
        };
        let beta_chat = RemoteModelChoice {
            provider_id: "beta-id".into(),
            provider_name: "Beta".into(),
            model: "chat".into(),
        };
        let beta_code = RemoteModelChoice {
            provider_id: "beta-id".into(),
            provider_name: "Beta".into(),
            model: "code".into(),
        };
        let mut app = UiState {
            panel: UiPanel::Settings,
            model_picker_open: true,
            remote_models: vec![alpha, beta_chat, beta_code],
            provider_profiles: vec![
                riga_server::ipc::ProviderProfileSummary {
                    id: "alpha-id".into(),
                    name: "Alpha".into(),
                    provider_type: riga_server::ws::ProviderType::OpenAiCompatible,
                    endpoint: "https://alpha.test/v1".into(),
                    model: "model-x".into(),
                    reasoning_effort: "low".into(),
                    api: riga_server::ws::ProviderApi::Chat,
                    subagent_model: None,
                    api_key_configured: true,
                },
                riga_server::ipc::ProviderProfileSummary {
                    id: "beta-id".into(),
                    name: "Beta".into(),
                    provider_type: riga_server::ws::ProviderType::OpencodeOpenAiCompatible,
                    endpoint: "https://beta.test/v1".into(),
                    model: "code".into(),
                    reasoning_effort: "medium".into(),
                    api: riga_server::ws::ProviderApi::Responses,
                    subagent_model: Some("cheap".into()),
                    api_key_configured: true,
                },
            ],
            ..UiState::default()
        };
        app.handle_paste("beta");
        assert_eq!(
            app.filtered_remote_models()
                .iter()
                .map(|choice| choice.model.as_str())
                .collect::<Vec<_>>(),
            vec!["chat", "code"]
        );
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::SaveProvider { config }) if config.model == "code" && config.provider_id.as_deref() == Some("beta-id") && config.provider_type == riga_server::ws::ProviderType::OpencodeOpenAiCompatible
        ));
        assert!(!app.model_picker_open);
        assert_eq!(app.panel, UiPanel::Transcript);
    }

    #[test]
    fn slash_new_and_resume_use_existing_session_state() {
        let mut app = UiState::default();
        app.state.sessions.push(riga_kernel::state::Session {
            id: "session-1".into(),
            title: "Current".into(),
            workspace: "/project".into(),
            archived: false,
            created_at: "now".into(),
            updated_at: "now".into(),
        });
        app.state.selected_session = Some("session-1".into());
        assert_eq!(
            submit_text(&mut app, "/new scratch"),
            Some(UiCommand::CreateSession {
                title: "scratch".into(),
                workspace: "/project".into(),
            })
        );
        assert!(submit_text(&mut app, "/resume").is_none());
        assert_eq!(app.panel, UiPanel::History);
        assert_eq!(app.panel_cursor, 0);
    }

    #[test]
    fn slash_navigation_commands_open_scoped_sidebar_panels() {
        let mut app = UiState::default();
        app.set_catalog(vec![
            CatalogEntry {
                id: "read".into(),
                kind: "tools".into(),
                description: "Read files".into(),
                insert_text: String::new(),
                requires_approval: false,
            },
            CatalogEntry {
                id: "docs".into(),
                kind: "mcp_servers".into(),
                description: "Documentation server".into(),
                insert_text: String::new(),
                requires_approval: false,
            },
            CatalogEntry {
                id: "rust-review".into(),
                kind: "skills".into(),
                description: "Review Rust code".into(),
                insert_text: String::new(),
                requires_approval: false,
            },
            CatalogEntry {
                id: "src/main.rs".into(),
                kind: "files".into(),
                description: String::new(),
                insert_text: String::new(),
                requires_approval: false,
            },
        ]);

        submit_text(&mut app, "/tools read");
        assert_eq!(app.panel, UiPanel::Catalog);
        assert_eq!(app.catalog_scope.as_deref(), Some("tools"));
        assert_eq!(app.catalog_query.text(), "read");
        assert_eq!(
            app.filtered_catalog()
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["read"]
        );

        app.panel = UiPanel::Transcript;
        submit_text(&mut app, "/mcp");
        assert_eq!(app.catalog_scope.as_deref(), Some("mcp"));
        assert_eq!(app.filtered_catalog()[0].id, "docs");

        app.panel = UiPanel::Transcript;
        submit_text(&mut app, "/skills");
        assert_eq!(app.catalog_scope.as_deref(), Some("skills"));
        assert_eq!(app.filtered_catalog()[0].id, "rust-review");

        app.panel = UiPanel::Transcript;
        submit_text(&mut app, "/settings");
        assert_eq!(app.panel, UiPanel::Settings);

        app.panel = UiPanel::Transcript;
        submit_text(&mut app, "/local-models");
        assert_eq!(app.panel, UiPanel::LocalModels);
    }

    #[test]
    fn slash_and_at_completions_require_a_line_or_space_boundary() {
        let mut app = UiState::default();
        app.set_completion_candidates(vec![
            CompletionItem {
                trigger: '/',
                category: "Commands".into(),
                label: "/model".into(),
                detail: "Choose a model".into(),
                insert_text: "/model".into(),
            },
            CompletionItem {
                trigger: '@',
                category: "Files".into(),
                label: "@src/main.rs".into(),
                detail: "Workspace file".into(),
                insert_text: "@src/main.rs".into(),
            },
        ]);
        for character in "word|/".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(app.completion_trigger, None);

        app.draft.clear();
        for character in "word /".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(app.completion_trigger, Some('/'));

        app.draft.clear();
        for character in "first line\n@".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(app.completion_trigger, Some('@'));
    }

    #[test]
    fn backspace_at_the_start_of_the_composer_never_opens_history() {
        let mut app = UiState::default();
        for key in [
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('\u{8}'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        ] {
            app.handle_key(key);
            assert_eq!(app.panel, UiPanel::Transcript);
            assert!(app.draft.is_empty());
        }
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::History);
    }

    #[test]
    fn ctrl_c_stops_the_active_run_and_preserves_draft_when_idle() {
        let mut running = UiState::default();
        running.state.active_run = Some("run-1".into());
        running.busy = Some("Running".into());
        running.shell_input_mode = true;
        assert_eq!(
            running.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL,)),
            Some(UiCommand::CancelRun {
                run_id: "run-1".into(),
            })
        );
        assert!(!running.shell_input_mode);

        let mut idle = UiState::default();
        idle.draft.insert_str("unsent draft");
        assert!(
            idle.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL,))
                .is_none()
        );
        assert_eq!(idle.draft.text(), "unsent draft");
        assert_eq!(idle.panel, UiPanel::Transcript);
    }

    #[test]
    fn unknown_slash_text_remains_a_normal_chat_prompt() {
        let mut app = UiState::default();
        app.state.selected_session = Some("session-1".into());
        assert_eq!(
            submit_text(&mut app, "/explain this syntax"),
            Some(UiCommand::StartRun {
                session_id: "session-1".into(),
                prompt: "/explain this syntax".into(),
            })
        );
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
        assert!(
            app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
                .is_none()
        );
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
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
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
    fn history_panel_can_rename_a_session() {
        let mut app = UiState::default();
        app.state.sessions.push(riga_kernel::state::Session {
            id: "session-1".into(),
            title: "Old".into(),
            workspace: ".".into(),
            archived: false,
            created_at: "now".into(),
            updated_at: "now".into(),
        });
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        for _ in 0..3 {
            app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        for character in "Renamed".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::RenameSession {
                session_id: "session-1".into(),
                title: "Renamed".into(),
            })
        );
    }

    #[test]
    fn history_archives_restores_and_filters_sessions() {
        let mut app = UiState {
            panel: UiPanel::History,
            ..UiState::default()
        };
        app.state.sessions.extend([
            riga_kernel::state::Session {
                id: "active-session".into(),
                title: "Active".into(),
                workspace: ".".into(),
                archived: false,
                created_at: "now".into(),
                updated_at: "now".into(),
            },
            riga_kernel::state::Session {
                id: "archived-session".into(),
                title: "Archived".into(),
                workspace: ".".into(),
                archived: true,
                created_at: "now".into(),
                updated_at: "now".into(),
            },
        ]);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(UiCommand::SetSessionArchived {
                session_id: "active-session".into(),
                archived: true,
            })
        );

        // Simulate the server's successful archive response before changing views.
        app.state.sessions[0].archived = true;
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert!(app.history_archived_view);
        assert_eq!(
            app.visible_history_sessions()
                .iter()
                .map(|session| session.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Active", "Archived"]
        );
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)),
            Some(UiCommand::SetSessionArchived {
                session_id: "active-session".into(),
                archived: false,
            })
        );
    }

    #[test]
    fn session_deletion_requires_an_exact_name_match() {
        let mut app = UiState {
            panel: UiPanel::History,
            ..UiState::default()
        };
        app.state.sessions.push(riga_kernel::state::Session {
            id: "protected-session".into(),
            title: "Exact Session Name".into(),
            workspace: ".".into(),
            archived: false,
            created_at: "now".into(),
            updated_at: "now".into(),
        });
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        for character in "wrong name".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .is_none()
        );
        assert_eq!(app.state.sessions.len(), 1);
        assert!(app.deleting_session.is_some());

        for _ in 0.."wrong name".chars().count() {
            app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        for character in "Exact Session Name".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::DeleteSession {
                session_id: "protected-session".into(),
                confirmation_name: "Exact Session Name".into(),
            })
        );
    }

    #[test]
    fn first_response_title_uses_a_concise_sentence_and_respects_limit() {
        assert_eq!(
            session_title_from_response("## Build complete. More detail follows."),
            Some("Build complete.".into())
        );
        assert_eq!(session_title_from_response("   \n"), None);
        assert!(
            session_title_from_response(&"a".repeat(120))
                .unwrap()
                .chars()
                .count()
                <= 80
        );
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
    fn multiline_composer_arrows_move_the_caret_without_replacing_the_draft() {
        let mut app = UiState::default();
        app.draft.replace("first\nsecond");
        app.draft.move_end();
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.draft.cursor_position(), (5, 0));
        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.draft.cursor_position(), (0, 0));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.draft.cursor_position(), (0, 1));
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(app.draft.cursor_position(), (6, 1));
        assert_eq!(app.draft.text(), "first\nsecond");
    }

    #[test]
    fn ctrl_o_sends_newline_terminated_input_to_active_shell_call() {
        let mut app = UiState::default();
        app.draft.replace("keep this chat draft");
        app.state.select_run(Some("run-1".into()));
        app.apply_event(RigaEventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            event_id: "tool-call".into(),
            session_id: "session-1".into(),
            run_id: "run-1".into(),
            sequence: 1,
            timestamp: "now".into(),
            event: RigaEvent::ToolCallStarted {
                call: serde_json::json!({"call_id": "shell-1", "tool": "bash"}),
            },
        });

        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert!(app.shell_input_mode);
        for character in "npm install".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }

        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiCommand::SendShellInput {
                call_id: "shell-1".into(),
                text: "npm install\n".into(),
            })
        );
        assert_eq!(app.draft.text(), "keep this chat draft");
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
    fn tab_focus_preserves_the_selected_sidebar_panel_and_rundeck_toggle() {
        let mut app = UiState::default();
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(app.panel, UiPanel::RunDeck);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::Transcript);
        assert_eq!(app.last_sidebar_panel, Some(UiPanel::RunDeck));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::RunDeck);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::Transcript);
        assert_eq!(app.last_sidebar_panel, Some(UiPanel::RunDeck));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(app.panel, UiPanel::RunDeck);
    }

    #[test]
    fn focused_sidebar_can_be_resized_with_plus_and_minus() {
        let mut app = UiState {
            panel: UiPanel::History,
            ..UiState::default()
        };
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(app.sidebar_width, 38);
        app.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        assert_eq!(app.sidebar_width, 34);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "+");
    }

    #[test]
    fn mouse_selects_sidebar_tabs_and_returns_focus_to_the_composer() {
        let mut app = UiState::default();
        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 90,
                row: 4,
                modifiers: KeyModifiers::NONE,
            },
            120,
            40,
        );
        assert_eq!(app.panel, UiPanel::Help);
        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: 38,
                modifiers: KeyModifiers::NONE,
            },
            120,
            40,
        );
        assert_eq!(app.panel, UiPanel::Transcript);
        assert_eq!(app.last_sidebar_panel, Some(UiPanel::Help));
    }

    #[test]
    fn mouse_clicking_a_history_session_opens_it() {
        let mut app = UiState::default();
        app.state.sessions.push(riga_kernel::state::Session {
            id: "clicked-session".into(),
            title: "Clicked session".into(),
            workspace: ".".into(),
            archived: false,
            created_at: "now".into(),
            updated_at: "now".into(),
        });

        assert_eq!(
            app.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 90,
                    row: 9,
                    modifiers: KeyModifiers::NONE,
                },
                120,
                40,
            ),
            Some(UiCommand::SelectSession {
                session_id: "clicked-session".into(),
            })
        );
        assert_eq!(app.panel, UiPanel::Transcript);
        assert_eq!(app.last_sidebar_panel, Some(UiPanel::History));
    }

    #[test]
    fn bracketed_paste_inserts_text_at_the_current_composer_cursor() {
        let mut app = UiState::default();
        app.draft.replace("ab");
        app.draft.move_left();
        app.handle_paste("你\ncd");
        assert_eq!(app.draft.text(), "a你\ncdb");
    }

    #[test]
    fn settings_save_command_preserves_the_secret_for_provider_profile_persistence() {
        let mut app = UiState::default();
        app.provider.profile_name = "Test profile".into();
        app.provider.endpoint = "https://model.test/v1".into();
        app.provider.api_key = "secret".into();
        app.provider.model = "model".into();
        app.handle_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::CONTROL));
        let Some(UiCommand::SaveProviderProfile { input }) =
            app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        else {
            panic!("settings should emit an explicit save command")
        };
        assert_eq!(input.name, "Test profile");
        assert_eq!(input.api_key, "secret");
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
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::Transcript);
        assert_eq!(app.last_sidebar_panel, Some(UiPanel::RunDeck));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::RunDeck);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
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
    fn sidebar_detail_toggles_preserve_the_selected_panel_and_lens() {
        let mut rundeck = UiState {
            panel: UiPanel::RunDeck,
            deck_lens: DeckLens::Evidence,
            ..UiState::default()
        };
        rundeck.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert!(rundeck.deck_details_expanded);
        assert_eq!(rundeck.panel, UiPanel::RunDeck);
        assert_eq!(rundeck.deck_lens, DeckLens::Evidence);

        let mut models = UiState {
            panel: UiPanel::LocalModels,
            ..UiState::default()
        };
        models.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert!(models.local_models_expanded);
        assert_eq!(models.panel, UiPanel::LocalModels);
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
    fn raw_shift_enter_adds_newlines_and_backspace_does_not_open_history() {
        let mut app = UiState::default();
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('\n'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('\r'), KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "a\nb\n");

        app.handle_key(KeyEvent::new(KeyCode::Char('\u{4}'), KeyModifiers::NONE));
        assert!(app.rundeck_open);
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(app.draft.text(), "a\nb\n\n");
        app.panel = UiPanel::Transcript;
        app.draft.clear();
        app.handle_key(KeyEvent::new(KeyCode::Char('\u{8}'), KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::Transcript);
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::History);
    }

    #[test]
    fn slash_and_at_commands_filter_and_insert_candidates() {
        let mut app = UiState {
            completion_candidates: vec![
                CompletionItem {
                    trigger: '/',
                    category: "Tools".into(),
                    label: "/tools/read".into(),
                    detail: "Read a file".into(),
                    insert_text: "Use the read tool: ".into(),
                },
                CompletionItem {
                    trigger: '/',
                    category: "Tools".into(),
                    label: "/tools/write".into(),
                    detail: "Write a file".into(),
                    insert_text: "/tools/write".into(),
                },
                CompletionItem {
                    trigger: '@',
                    category: "Files".into(),
                    label: "@README.md".into(),
                    detail: "Workspace file".into(),
                    insert_text: "@README.md".into(),
                },
            ],
            ..UiState::default()
        };
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert_eq!(
            app.completion_items()
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["/tools/read", "/tools/write"]
        );
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "Use the read tool: ");
        app.handle_key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE));
        assert_eq!(
            app.completion_items()
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["@README.md"]
        );
    }

    #[test]
    fn ctrl_i_opens_info_and_question_mark_remains_composer_text() {
        let mut app = UiState::default();
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.draft.text(), "?");
        assert_eq!(app.panel, UiPanel::Transcript);
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL));
        assert_eq!(app.panel, UiPanel::Help);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.panel, UiPanel::Transcript);
        assert_eq!(app.draft.text(), "?");
    }

    #[test]
    fn completion_replaces_only_the_active_token_and_preserves_trailing_text() {
        let mut app = UiState {
            completion_candidates: vec![CompletionItem {
                trigger: '@',
                category: "Files".into(),
                label: "@src/main.rs".into(),
                detail: "Workspace file".into(),
                insert_text: "@src/main.rs".into(),
            }],
            ..UiState::default()
        };
        app.draft.replace("ask @sr later");
        app.draft.move_home();
        for _ in 0..7 {
            app.draft.move_right();
        }
        app.refresh_completion();
        assert_eq!(app.completion_trigger, Some('@'));
        app.accept_completion();
        assert_eq!(app.draft.text(), "ask @src/main.rs later");
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
