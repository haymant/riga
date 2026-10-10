use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
};

use crate::app::UiState;
use crate::model::{ConnectionState, RunStatus};

pub fn render(frame: &mut Frame<'_>, app: &UiState) {
    if app.panel != crate::model::UiPanel::Transcript {
        render_panel(frame, app);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(composer_height(app, frame.area())),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_topbar(frame, chunks[0], app);
    render_transcript(frame, chunks[1], app);
    render_composer(frame, chunks[2], app);
    render_footer(frame, chunks[3], app);
}

fn render_panel(frame: &mut Frame<'_>, app: &UiState) {
    let area = frame.area();
    let (title, lines) = match app.panel {
        crate::model::UiPanel::History => {
            let mut lines = vec![Line::from(
                "↑/↓ select · Enter switch · n new session · Esc close",
            )];
            if app.creating_session {
                lines.push(Line::from(format!(
                    "New session title: {}",
                    app.panel_input.text()
                )));
                lines.push(Line::from(format!(
                    "Workspace (Tab switches): {}",
                    app.workspace_input.text()
                )));
                lines.push(Line::from("Enter create · Esc cancel"));
            } else if app.state.sessions.is_empty() {
                lines.push(Line::from("No sessions. Press n to create one."));
            } else {
                for (index, session) in app.state.sessions.iter().enumerate() {
                    lines.push(Line::from(format!(
                        "{} {} · {} · {} · updated {}",
                        if index == app.panel_cursor {
                            "▶"
                        } else {
                            " "
                        },
                        session.title,
                        session.id,
                        session.workspace,
                        session.updated_at
                    )));
                }
            }
            ("Session history", lines)
        }
        crate::model::UiPanel::Settings => {
            let p = &app.provider;
            let masked = if p.api_key.is_empty() {
                "(unchanged/empty)".into()
            } else {
                "•".repeat(p.api_key.chars().count().min(32))
            };
            let values = [
                format!("endpoint: {}", p.endpoint),
                format!("api key: {masked}"),
                format!("model: {}", p.model),
                format!("reasoning effort: {}", p.reasoning_effort),
                format!("provider kind: {:?}", p.kind),
                format!("provider API: {:?}", p.api),
                format!("subagent model: {}", p.subagent_model),
            ];
            let mut lines = vec![Line::from("Tab/Shift+Tab fields · Ctrl+S save · Esc close")];
            for (index, value) in values.into_iter().enumerate() {
                lines.push(Line::from(format!(
                    "{} {}",
                    if index == app.provider_field {
                        "▶"
                    } else {
                        " "
                    },
                    value
                )));
            }
            if let Some(error) = &p.error {
                lines.push(Line::from(format!("ERROR: {error}")));
            }
            ("Provider settings", lines)
        }
        crate::model::UiPanel::Catalog => {
            let entries = app.filtered_catalog();
            let mut lines = vec![Line::from(format!(
                "Filter: {} · ↑/↓ select · Enter insert · Esc close",
                app.catalog_query.text()
            ))];
            if entries.is_empty() {
                lines.push(Line::from("No server catalog entries match the filter."));
            }
            for (index, entry) in entries
                .iter()
                .enumerate()
                .take(area.height.saturating_sub(4) as usize)
            {
                lines.push(Line::from(format!(
                    "{} [{}] {} — {}{}",
                    if index == app.panel_cursor {
                        "▶"
                    } else {
                        " "
                    },
                    entry.kind,
                    entry.id,
                    entry.description,
                    if entry.requires_approval {
                        " · approval"
                    } else {
                        ""
                    }
                )));
            }
            ("Command palette (server catalog)", lines)
        }
        crate::model::UiPanel::LocalModels => {
            ("Local models and attachments", render_local_models(app))
        }
        crate::model::UiPanel::RunDeck => ("RunDeck", render_rundeck(app)),
        crate::model::UiPanel::Help => (
            "Help",
            vec![Line::from(
                "Ctrl+H history · Ctrl+,/F2 settings/model · Ctrl+K catalog · Ctrl+D RunDeck · Ctrl+L local models · ? help · Ctrl+R reasoning · Ctrl+T tools · y/a/n approvals · Esc close/cancel · q quit",
            )],
        ),
        crate::model::UiPanel::Transcript => unreachable!(),
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
    if app.panel == crate::model::UiPanel::LocalModels {
        let gauge_area = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(area.height.saturating_sub(2)),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        if let Some(progress) = app.local_model_progress {
            frame.render_widget(
                Gauge::default()
                    .gauge_style(Style::default().fg(Color::Cyan))
                    .ratio(progress.clamp(0.0, 1.0))
                    .label(format!("Downloading {:.1}%", progress * 100.0)),
                gauge_area,
            );
        } else if app.busy.is_some() {
            let ratio = ((app.busy_tick % 20) as f64 + 1.0) / 20.0;
            frame.render_widget(
                Gauge::default()
                    .gauge_style(Style::default().fg(Color::Yellow))
                    .ratio(ratio)
                    .label(app.busy.as_deref().unwrap_or("Working...")),
                gauge_area,
            );
        }
    }
}

fn render_local_models(app: &UiState) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(
        "↑/↓ select · Enter/d download or load · x cancel download · u unload · a attach file · Esc close",
    )];
    if app.local_attachment_input {
        lines.push(Line::from(format!(
            "Attachment path: {} · Enter upload",
            app.panel_input.text()
        )));
        return lines;
    }
    let Some(overview) = &app.local_models else {
        lines.push(Line::from(
            "Local-model protocol is unavailable in this adapter.",
        ));
        return lines;
    };
    lines.push(Line::from(format!(
        "Accelerator: {} · loaded: {}",
        overview.accelerator,
        overview.loaded.as_deref().unwrap_or("none")
    )));
    if let Some(status) = &app.local_model_status {
        lines.push(Line::from(format!("Status: {status}")));
    }
    if overview.installed.is_empty() {
        lines.push(Line::from(
            "No installed models. Select a catalog entry to download it.",
        ));
    }
    for (index, model) in overview.installed.iter().enumerate() {
        lines.push(Line::from(format!(
            "{} [installed] {} · {} · {}",
            if index == app.panel_cursor {
                "▶"
            } else {
                " "
            },
            model.id,
            model.name,
            model.path
        )));
    }
    for (offset, model) in overview.catalog.iter().enumerate() {
        let index = overview.installed.len() + offset;
        lines.push(Line::from(format!(
            "{} [catalog] {} · {} · {:.1} GiB · context {}",
            if index == app.panel_cursor {
                "▶"
            } else {
                " "
            },
            model.id,
            model.name,
            model.size_bytes as f64 / 1_073_741_824.0,
            model.recommended_context
        )));
    }
    if let Some(error) = &app.local_model_error {
        lines.push(Line::from(format!("ERROR: {error}")));
    }
    lines
}

fn render_rundeck(app: &UiState) -> Vec<Line<'static>> {
    let lens = match app.deck_lens {
        crate::model::DeckLens::Execution => "Execution",
        crate::model::DeckLens::Evidence => "Evidence",
        crate::model::DeckLens::Knowledge => "Knowledge",
    };
    let mut lines = vec![Line::from(format!(
        "{lens} · Tab/←/→ lens · ↑/↓ runs · Enter resume · x stop · Esc close"
    ))];
    if app.state.active_runs.is_empty() {
        lines.push(Line::from(
            "No active runs. Finished runs remain inspectable in the selected session.",
        ));
    } else {
        for (index, active) in app.state.active_runs.iter().enumerate() {
            lines.push(Line::from(format!(
                "{} {} · session {} · {} {}",
                if index == app.panel_cursor {
                    "▶"
                } else {
                    " "
                },
                active.run_id,
                active.session_id,
                if active.local { "local" } else { "remote" },
                if app.state.active_run.as_deref() == Some(active.run_id.as_str()) {
                    "· selected"
                } else {
                    ""
                }
            )));
        }
    }
    let Some(run_id) = &app.state.active_run else {
        return lines;
    };
    let Some(run) = app.state.run(run_id) else {
        return lines;
    };
    let done = run
        .tasks
        .values()
        .filter(|task| task.state.is_terminal())
        .count();
    let total = run
        .graph
        .as_ref()
        .map(|graph| graph.nodes.len())
        .unwrap_or(run.tasks.len());
    lines.push(Line::from(format!(
        "Run {run_id} · {:?} · progress {done}/{total} · sequence {}",
        run.status, run.last_sequence
    )));
    match app.deck_lens {
        crate::model::DeckLens::Execution => {
            if let Some(plan) = &run.plan {
                lines.push(Line::from(format!("Plan: {}", plan.title)));
                for (index, step) in plan.steps.iter().enumerate() {
                    lines.push(Line::from(format!(
                        "  {} {} {}",
                        if index == plan.active_index {
                            "▶"
                        } else {
                            "·"
                        },
                        step.id,
                        step.label
                    )));
                }
            }
            if let Some(todos) = &run.todos {
                let (done, total) = todos.progress();
                lines.push(Line::from(format!("Todos: {done}/{total}")));
                for item in &todos.items {
                    lines.push(Line::from(format!("  [{:?}] {}", item.status, item.text)));
                }
            }
            for task in run.tasks.values() {
                lines.push(Line::from(format!(
                    "Task {} · {} · {:?} · {}",
                    task.record
                        .as_ref()
                        .map(|record| record.agent.as_str())
                        .unwrap_or("task"),
                    task.record
                        .as_ref()
                        .map(|record| record.description.as_str())
                        .unwrap_or(""),
                    task.state,
                    task.result.as_deref().unwrap_or("")
                )));
            }
            if let Some(graph) = &run.graph {
                lines.push(Line::from(format!("Graph: {}", graph.title)));
                for node in &graph.nodes {
                    lines.push(Line::from(format!(
                        "  {} ← {}",
                        node.id,
                        if node.depends_on.is_empty() {
                            "ready".into()
                        } else {
                            node.depends_on.join(", ")
                        }
                    )));
                }
            }
        }
        crate::model::DeckLens::Evidence => {
            for evidence in &run.evidence {
                lines.push(Line::from(format!(
                    "{} · {} · confidence {} · {}",
                    evidence.id, evidence.claim, evidence.confidence, evidence.source_ref
                )));
            }
            if run.evidence.is_empty() {
                lines.push(Line::from("No evidence recorded for this run."));
            }
        }
        crate::model::DeckLens::Knowledge => {
            for knowledge in &run.knowledge {
                lines.push(Line::from(format!(
                    "{} · {} · confidence {} · source {}",
                    knowledge.id, knowledge.fact, knowledge.confidence, knowledge.source_run_id
                )));
            }
            if run.knowledge.is_empty() {
                lines.push(Line::from("No reusable knowledge recorded for this run."));
            }
        }
    }
    lines
}

fn composer_height(app: &UiState, area: Rect) -> u16 {
    let lines = app.draft.text().lines().count().max(1) as u16;
    lines
        .saturating_add(2)
        .min(area.height.saturating_sub(5).max(3))
}

fn render_topbar(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let connection = match app.state.connection {
        ConnectionState::Connected => "connected",
        ConnectionState::Reconnecting => "reconnecting",
        ConnectionState::Disconnected => "disconnected",
    };
    let session = app
        .state
        .selected_session
        .as_deref()
        .unwrap_or("no session");
    let line = Line::from(vec![
        Span::styled(
            " RIGA ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("· "),
        Span::styled(
            connection,
            Style::default().fg(connection_color(app.state.connection)),
        ),
        Span::raw(" · session: "),
        Span::styled(session, Style::default().fg(Color::White)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let mut lines = Vec::new();
    for item in &app.state.session_history {
        match item {
            crate::model::TranscriptItem::UserText(text) => lines.push(Line::from(vec![
                Span::styled("You ", Style::default().fg(Color::Green)),
                Span::raw(text),
            ])),
            crate::model::TranscriptItem::AssistantText(text) => lines.push(Line::from(vec![
                Span::styled("Assistant ", Style::default().fg(Color::Cyan)),
                Span::raw(text),
            ])),
            crate::model::TranscriptItem::AssistantReasoning(text) => {
                lines.push(Line::from(Span::styled(
                    format!("Thinking · {text}"),
                    Style::default().fg(Color::DarkGray),
                )))
            }
            crate::model::TranscriptItem::System(text) => lines.push(Line::from(Span::styled(
                text,
                Style::default().fg(Color::Magenta),
            ))),
            _ => lines.push(Line::from(Span::styled(
                "[tool activity available in RunDeck]",
                Style::default().fg(Color::Yellow),
            ))),
        }
    }
    if let Some(prompt) = &app.current_prompt {
        lines.push(Line::from(vec![
            Span::styled("You ", Style::default().fg(Color::Green)),
            Span::raw(prompt),
        ]));
    }
    if let Some(run_id) = &app.state.active_run {
        if let Some(run) = app.state.run(run_id) {
            let capacity = area.height.saturating_sub(2) as usize * 3;
            let start = run
                .transcript
                .len()
                .saturating_sub(capacity)
                .saturating_sub(app.transcript_scroll);
            let end = (start + capacity).min(run.transcript.len());
            for item in &run.transcript[start..end] {
                if app.reasoning_collapsed
                    && matches!(item, crate::model::TranscriptItem::AssistantReasoning(_))
                {
                    continue;
                }
                if app.tools_collapsed
                    && matches!(
                        item,
                        crate::model::TranscriptItem::ToolCall { .. }
                            | crate::model::TranscriptItem::ToolOutput { .. }
                            | crate::model::TranscriptItem::ToolResult { .. }
                    )
                {
                    continue;
                }
                lines.push(match item {
                    crate::model::TranscriptItem::UserText(text) => Line::from(vec![
                        Span::styled("You ", Style::default().fg(Color::Green)),
                        Span::raw(text),
                    ]),
                    crate::model::TranscriptItem::AssistantText(text) => Line::from(vec![
                        Span::styled("Assistant ", Style::default().fg(Color::Cyan)),
                        Span::raw(text),
                    ]),
                    crate::model::TranscriptItem::AssistantReasoning(text) => {
                        Line::from(Span::styled(
                            format!("Thinking · {text}"),
                            Style::default().fg(Color::DarkGray),
                        ))
                    }
                    crate::model::TranscriptItem::ToolCall {
                        call_id,
                        tool,
                        call,
                    } => Line::from(Span::styled(
                        format!("tool {tool} [{call_id}] · {call}"),
                        Style::default().fg(Color::Yellow),
                    )),
                    crate::model::TranscriptItem::ToolOutput { call_id, output } => {
                        Line::from(Span::styled(
                            format!("{call_id} · {output}"),
                            Style::default().fg(Color::Yellow),
                        ))
                    }
                    crate::model::TranscriptItem::ToolResult { call_id, result } => {
                        Line::from(Span::styled(
                            format!(
                                "tool result{} · {result}",
                                call_id
                                    .as_deref()
                                    .map(|id| format!(" [{id}]"))
                                    .unwrap_or_default()
                            ),
                            Style::default().fg(Color::Yellow),
                        ))
                    }
                    crate::model::TranscriptItem::TaskResult {
                        task_id,
                        ok,
                        result,
                    } => Line::from(Span::styled(
                        format!(
                            "task {task_id} {} · {result}",
                            if *ok { "done" } else { "failed" }
                        ),
                        Style::default().fg(if *ok { Color::Green } else { Color::Red }),
                    )),
                    crate::model::TranscriptItem::Approval {
                        approval_id,
                        tool,
                        summary,
                    } => Line::from(Span::styled(
                        format!("approval {approval_id} · {tool} · {summary}"),
                        Style::default().fg(Color::Red),
                    )),
                    crate::model::TranscriptItem::System(text) => {
                        Line::from(Span::styled(text, Style::default().fg(Color::Magenta)))
                    }
                });
            }
            if lines.is_empty() {
                lines.push(Line::from(Span::styled(
                    "No events yet. Type a prompt below.",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    } else if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "IPC ready. Select or create a session to begin.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title("Transcript"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_composer(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let title = if let Some(approval) = app.state.pending_approval() {
        format!(
            "Approval · {} / {} · y allow · a always · n deny",
            approval.tool, approval.approval_id
        )
    } else if app.state.active_run.is_some() {
        "Composer · Esc stop · Enter send".to_owned()
    } else {
        "Composer · Shift+Enter newline · Enter send".to_owned()
    };
    frame.render_widget(
        Paragraph::new(app.draft.text())
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
    let (column, line) = app.draft.cursor_position();
    frame.set_cursor_position((
        area.x.saturating_add(1).saturating_add(column),
        area.y
            .saturating_add(1)
            .saturating_add(line.min(area.height.saturating_sub(3))),
    ));
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let status = app
        .state
        .active_run
        .as_deref()
        .and_then(|id| app.state.run(id))
        .map(|run| match run.status {
            RunStatus::Running => "thinking",
            RunStatus::WaitingForApproval => "waiting for approval",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Unknown => "idle",
        })
        .unwrap_or("idle");
    let notice = app.notice.as_deref().unwrap_or("");
    if !notice.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("ERROR: {notice}"),
                Style::default().fg(Color::Yellow),
            )),
            area,
        );
        return;
    }
    if let Some(busy) = &app.busy {
        const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
        frame.render_widget(
            Paragraph::new(format!(
                "{} {busy}",
                SPINNER[(app.busy_tick as usize) % SPINNER.len()]
            ))
            .style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓ history ", Style::default().fg(Color::DarkGray)),
            Span::raw("· "),
            Span::styled(
                if app.new_events > 0 {
                    format!("{} new events", app.new_events)
                } else if app.follow_output {
                    "following".into()
                } else {
                    "paused · End to follow".into()
                },
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" · "),
            Span::styled(status, Style::default().fg(Color::Cyan)),
            Span::raw(" · Ctrl+,/F2 settings/model · Ctrl+L local models · ? help · Ctrl+R reasoning · Ctrl+T tools · q quit"),
        ])),
        area,
    );
}

fn connection_color(connection: ConnectionState) -> Color {
    match connection {
        ConnectionState::Connected => Color::Green,
        ConnectionState::Reconnecting => Color::Yellow,
        ConnectionState::Disconnected => Color::Red,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::UiState,
        input::TextBuffer,
        model::{AppState, ConnectionState, ProviderForm, UiPanel},
    };
    use ratatui::{
        Terminal,
        backend::{Backend, TestBackend},
    };
    use riga_kernel::{
        PROTOCOL_VERSION,
        events::{RigaEvent, RigaEventEnvelope},
    };

    #[test]
    fn idle_view_renders_at_small_and_large_sizes() {
        for (width, height) in [(80, 24), (120, 40)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let app = UiState {
                state: AppState::default(),
                draft: TextBuffer::default(),
                last_submitted: None,
                should_quit: false,
                ..UiState::default()
            };
            terminal.draw(|frame| render(frame, &app)).unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer.area().width, width);
            assert_eq!(buffer.area().height, height);
            assert!(buffer.content().iter().any(|cell| cell.symbol() == "R"));
        }
    }

    #[test]
    fn disconnected_state_is_visible() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = UiState {
            state: AppState {
                connection: ConnectionState::Disconnected,
                ..AppState::default()
            },
            draft: TextBuffer::default(),
            last_submitted: None,
            should_quit: false,
            ..UiState::default()
        };
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("disconnected"));
    }

    #[test]
    fn composer_cursor_and_notice_are_rendered() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState {
            notice: Some("ERR".into()),
            ..UiState::default()
        };
        app.draft.insert('x');
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("ERR"));
        assert_eq!(
            terminal.backend_mut().get_cursor_position().unwrap(),
            (2, 21).into()
        );
    }

    #[test]
    fn local_model_progress_gauge_is_rendered() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = UiState {
            panel: crate::model::UiPanel::LocalModels,
            local_model_progress: Some(0.5),
            busy: Some("Downloading model...".into()),
            ..UiState::default()
        };
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Downloading 50.0%"));
    }

    #[test]
    fn assistant_stream_is_labeled_assistant_not_you() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState::default();
        app.state.selected_session = Some("session-1".into());
        app.state.select_run(Some("run-1".into()));
        for (sequence, event) in [
            RigaEvent::RunStarted,
            RigaEvent::TextDelta {
                delta: "Hel".into(),
            },
            RigaEvent::TextDelta { delta: "lo".into() },
        ]
        .into_iter()
        .enumerate()
        {
            app.state.apply_event(RigaEventEnvelope {
                protocol_version: PROTOCOL_VERSION,
                event_id: format!("event-{sequence}"),
                session_id: "session-1".into(),
                run_id: "run-1".into(),
                sequence: sequence as u64 + 1,
                timestamp: "now".into(),
                event,
            });
        }
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Assistant Hello"));
        assert!(!text.contains("You Hello"));
    }

    #[test]
    fn provider_settings_mask_api_keys() {
        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState {
            panel: UiPanel::Settings,
            ..UiState::default()
        };
        app.set_provider(ProviderForm {
            api_key: "do-not-display".into(),
            ..ProviderForm::default()
        });
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.contains("do-not-display"));
        assert!(text.contains("••••"));
    }
}
