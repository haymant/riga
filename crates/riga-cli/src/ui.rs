use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::UiState;
use crate::model::{ConnectionState, RunStatus};

pub fn render(frame: &mut Frame<'_>, app: &UiState) {
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
                    crate::model::TranscriptItem::AssistantText(text) => Line::from(vec![
                        Span::styled("You ", Style::default().fg(Color::Green)),
                        Span::raw(text),
                    ]),
                    crate::model::TranscriptItem::AssistantReasoning(text) => {
                        Line::from(Span::styled(
                            format!("thinking · {text}"),
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
    } else {
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
            Span::raw(" · r reasoning · t tools · q quit"),
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
        model::{AppState, ConnectionState},
    };
    use ratatui::{Terminal, backend::TestBackend};

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
}
