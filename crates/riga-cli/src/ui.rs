use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
};

use crate::app::UiState;
use crate::{
    markdown::render_markdown,
    model::{ConnectionState, RunStatus, TranscriptItem, UiPanel},
};

fn push_rich_message(lines: &mut Vec<Line<'static>>, rail_color: Color, text: &str) {
    for line in render_markdown(text) {
        let mut spans = vec![Span::styled("┃ ", Style::default().fg(rail_color))];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }
    if text.is_empty() {
        lines.push(Line::from(Span::styled(
            "┃",
            Style::default().fg(rail_color),
        )));
    }
}

fn push_transcript_note(lines: &mut Vec<Line<'static>>, text: &str, color: Color) {
    for line in text.lines() {
        lines.push(Line::from(vec![
            Span::styled("┃ ", Style::default().fg(Color::DarkGray)),
            Span::styled(line.to_owned(), Style::default().fg(color)),
        ]));
    }
}

fn push_reasoning(lines: &mut Vec<Line<'static>>, text: &str, collapsed: bool) {
    let surface = Color::Rgb(34, 34, 36);
    if collapsed {
        let preview: String = text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Reasoning details hidden")
            .chars()
            .take(72)
            .collect();
        lines.push(Line::from(vec![
            Span::styled("╎ ", Style::default().fg(Color::DarkGray).bg(surface)),
            Span::styled(
                format!(
                    "Thinking · {} chars · {preview} · Ctrl+R to expand",
                    text.chars().count()
                ),
                Style::default()
                    .fg(Color::Gray)
                    .bg(surface)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
        return;
    }
    lines.push(Line::from(vec![
        Span::styled("╎ ", Style::default().fg(Color::DarkGray).bg(surface)),
        Span::styled(
            "Thinking",
            Style::default()
                .fg(Color::Gray)
                .bg(surface)
                .add_modifier(Modifier::ITALIC),
        ),
    ]));
    for line in text.lines() {
        lines.push(Line::from(vec![
            Span::styled("╎ ", Style::default().fg(Color::DarkGray).bg(surface)),
            Span::styled(
                line.to_owned(),
                Style::default().fg(Color::Gray).bg(surface),
            ),
        ]));
    }
}

pub fn render(frame: &mut Frame<'_>, app: &UiState) {
    let area = frame.area();
    if has_wide_sidebar(area) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(72),
                Constraint::Length(1),
                Constraint::Length(34),
            ])
            .split(area);
        render_transcript_column(frame, columns[0], app);
        frame.render_widget(
            Block::default().style(
                Style::default()
                    .fg(Color::DarkGray)
                    .bg(Color::Rgb(25, 25, 27)),
            ),
            columns[1],
        );
        render_sidebar(frame, columns[2], app);
        return;
    }
    render_transcript_column(frame, area, app);
    if app.panel != UiPanel::Transcript || app.rundeck_open {
        if area.height < 7 {
            render_panel(frame, app);
        } else {
            render_compact_panel_overlay(frame, area, app);
        }
        return;
    }
}

fn has_wide_sidebar(area: Rect) -> bool {
    area.width >= 112 && area.height >= 12
}

fn active_sidebar_panel(app: &UiState) -> UiPanel {
    if app.panel != UiPanel::Transcript {
        app.panel
    } else if app.rundeck_open {
        UiPanel::RunDeck
    } else {
        UiPanel::History
    }
}

fn sidebar_panel_title(panel: UiPanel) -> &'static str {
    match panel {
        UiPanel::History => "History",
        UiPanel::Settings => "Settings",
        UiPanel::Catalog => "Catalog",
        UiPanel::RunDeck => "RunDeck",
        UiPanel::LocalModels => "Local models",
        UiPanel::Help => "Info",
        UiPanel::Transcript => "History",
    }
}

fn sidebar_panel_lines(app: &UiState, panel: UiPanel, max_lines: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match panel {
        UiPanel::History | UiPanel::Transcript => {
            if app.creating_session {
                lines.push(Line::from("New session"));
                lines.push(Line::from(format!("Title: {}", app.panel_input.text())));
                lines.push(Line::from(format!(
                    "Workspace: {}",
                    app.workspace_input.text()
                )));
                lines.push(Line::from("Enter save · Esc cancel"));
            } else if app.renaming_session.is_some() {
                lines.push(Line::from("Rename session"));
                lines.push(Line::from(app.panel_input.text().to_owned()));
                lines.push(Line::from("Enter save · Esc cancel"));
            } else {
                lines.push(Line::from("Ctrl+H · n new · r rename"));
                if app.state.sessions.is_empty() {
                    lines.push(Line::from("No sessions yet"));
                }
                for (index, session) in app.state.sessions.iter().enumerate() {
                    let selected = if app.panel == UiPanel::History {
                        index == app.panel_cursor
                    } else {
                        app.state.selected_session.as_deref() == Some(&session.id)
                    };
                    let marker = if selected { "●" } else { "○" };
                    lines.push(Line::from(format!("{marker} {}", session.title)));
                }
            }
        }
        UiPanel::Settings => {
            let provider = &app.provider;
            let api_key = if provider.api_key.is_empty() {
                "(empty)".to_owned()
            } else {
                "•".repeat(provider.api_key.chars().count().min(24))
            };
            lines.extend([
                Line::from("Ctrl+, · Tab fields · Ctrl+S save"),
                Line::from(format!("Endpoint: {}", provider.endpoint)),
                Line::from(format!("API key: {api_key}")),
                Line::from(format!("Model: {}", provider.model)),
                Line::from(format!("Reasoning: {}", provider.reasoning_effort)),
                Line::from(format!("Provider: {:?}", provider.kind)),
                Line::from(format!("API: {:?}", provider.api)),
                Line::from(format!("Subagent: {}", provider.subagent_model)),
            ]);
            if let Some(error) = &provider.error {
                lines.push(Line::from(format!("Error: {error}")));
            }
        }
        UiPanel::LocalModels => {
            lines.extend(render_local_models(app));
            if let Some(progress) = app.local_model_progress {
                lines.push(Line::from(format!("Downloading {:.1}%", progress * 100.0)));
            } else if let Some(busy) = &app.busy {
                lines.push(Line::from(format!("{busy} …")));
            }
        }
        UiPanel::RunDeck => {
            lines.extend(render_rundeck(app));
            lines.push(Line::from("Tab / ← → change view · Esc close"));
        }
        UiPanel::Catalog => {
            lines.push(Line::from(format!("Filter: {}", app.catalog_query.text())));
            for entry in app
                .filtered_catalog()
                .into_iter()
                .take(max_lines.saturating_sub(2))
            {
                lines.push(Line::from(format!("[{}] {}", entry.kind, entry.id)));
                if !entry.description.is_empty() {
                    lines.push(Line::from(format!("  {}", entry.description)));
                }
            }
        }
        UiPanel::Help => {
            lines.extend([
                Line::from("Ctrl+I / F1 · Info"),
                Line::from("Ctrl+H · session history"),
                Line::from("Ctrl+, / F2 · provider settings"),
                Line::from("Ctrl+L · local models"),
                Line::from("Ctrl+K · server catalog"),
                Line::from("Ctrl+D · RunDeck"),
                Line::from("/ · tools, skills, MCP"),
                Line::from("@ · files, subagents"),
                Line::from("Ctrl+R / Ctrl+T · collapse output"),
                Line::from("Esc · close · q · quit"),
            ]);
        }
    }
    lines.truncate(max_lines.max(1));
    lines
}

fn active_panel_editor(app: &UiState, panel: UiPanel) -> Option<(&'static str, String, usize)> {
    let (label, input, masked) = match panel {
        UiPanel::History if app.renaming_session.is_some() => ("Title", &app.panel_input, false),
        UiPanel::History if app.creating_session && app.session_field == 0 => {
            ("Title", &app.panel_input, false)
        }
        UiPanel::History if app.creating_session => ("Workspace", &app.workspace_input, false),
        UiPanel::Settings => match app.provider_field {
            0 => ("Endpoint", &app.panel_input, false),
            1 => ("API key", &app.panel_input, true),
            2 => ("Model", &app.panel_input, false),
            3 => ("Reasoning", &app.panel_input, false),
            6 => ("Subagent", &app.panel_input, false),
            _ => return None,
        },
        UiPanel::Catalog => ("Filter", &app.catalog_query, false),
        UiPanel::LocalModels if app.local_attachment_input => {
            ("Model path", &app.panel_input, false)
        }
        _ => return None,
    };
    let cursor = input.cursor_position().0 as usize;
    let value = if masked {
        "•".repeat(input.text().chars().count())
    } else {
        input.text().to_owned()
    };
    Some((label, value, cursor))
}

fn render_panel_surface_body(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &UiState,
    panel: UiPanel,
    background: Color,
    show_title: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let editor = active_panel_editor(app, panel);
    let editor_height = u16::from(editor.is_some() && area.height > 1);
    let body_area = Rect {
        height: area.height.saturating_sub(editor_height),
        ..area
    };
    let mut lines = Vec::new();
    if show_title {
        lines.push(Line::from(Span::styled(
            format!("  {}", sidebar_panel_title(panel).to_uppercase()),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.extend(sidebar_panel_lines(
        app,
        panel,
        body_area.height.saturating_sub(u16::from(show_title)) as usize,
    ));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().fg(Color::Gray).bg(background))
            .wrap(Wrap { trim: true }),
        body_area,
    );

    if let Some((label, value, cursor)) = editor {
        if editor_height == 0 {
            return;
        }
        let editor_area = Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        };
        let prefix = format!("› {label}: ");
        let prefix_width = prefix.chars().count().min(editor_area.width as usize);
        let available = (editor_area.width as usize).saturating_sub(prefix_width);
        let start = cursor.saturating_sub(available);
        let visible: String = value.chars().skip(start).take(available).collect();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(prefix.clone(), Style::default().fg(Color::Cyan)),
                Span::styled(visible, Style::default().fg(Color::White)),
            ]))
            .style(Style::default().bg(Color::Rgb(47, 47, 50))),
            editor_area,
        );
        let cursor_x = editor_area
            .x
            .saturating_add(prefix_width as u16)
            .saturating_add(cursor.saturating_sub(start).min(available) as u16)
            .min(editor_area.x + editor_area.width.saturating_sub(1));
        frame.set_cursor_position((cursor_x, editor_area.y));
    }
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let background = Color::Rgb(29, 29, 31);
    frame.render_widget(
        Block::default().style(Style::default().bg(background)),
        area,
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Min(1),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  WORKSPACE",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        sections[0],
    );
    let selected = active_sidebar_panel(app);
    let entries = [
        (UiPanel::History, "History     ^H"),
        (UiPanel::Settings, "Settings    ^,"),
        (UiPanel::LocalModels, "Local models ^L"),
        (UiPanel::Help, "Info        ^I"),
        (UiPanel::RunDeck, "RunDeck     ^D"),
    ];
    let nav_lines = entries
        .into_iter()
        .map(|(panel, label)| {
            let active = panel == selected;
            Line::from(Span::styled(
                format!(" {}", label),
                if active {
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(53, 53, 57))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(nav_lines)), sections[1]);

    render_panel_surface_body(frame, sections[2], app, selected, background, true);
}

fn render_compact_panel_overlay(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let panel = active_sidebar_panel(app);
    let composer = composer_height(app, area);
    let height = area
        .height
        .saturating_sub(composer.saturating_add(3))
        .min(14);
    if height < 3 || area.width < 8 {
        return;
    }
    let popup = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height,
    };
    let background = Color::Rgb(31, 31, 34);
    frame.render_widget(
        Block::default()
            .title(format!(" {} · Esc close ", sidebar_panel_title(panel)))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .style(Style::default().fg(Color::Gray).bg(background)),
        popup,
    );
    let content = Rect {
        x: popup.x.saturating_add(1),
        y: popup.y.saturating_add(1),
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    render_panel_surface_body(frame, content, app, panel, background, false);
}

fn render_transcript_column(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(composer_height(app, area)),
            Constraint::Length(1),
        ])
        .split(area);

    render_topbar(frame, chunks[0], app);
    render_transcript(frame, chunks[1], app);
    render_composer(frame, chunks[2], app);
    render_completion_popup(frame, chunks[2], app);
    render_footer(frame, chunks[3], app);
}

fn render_completion_popup(frame: &mut Frame<'_>, composer: Rect, app: &UiState) {
    if app.completion_trigger.is_none() {
        return;
    }
    let items = app.completion_items();
    if items.is_empty() {
        return;
    }
    let height = items.len().min(7) as u16 + 1;
    let area = Rect {
        x: composer.x,
        y: composer.y.saturating_sub(height),
        width: composer.width,
        height,
    };
    let lines = items
        .iter()
        .take(7)
        .enumerate()
        .map(|(index, item)| {
            let selected = index == app.completion_cursor;
            let surface = if selected {
                Color::Rgb(58, 58, 62)
            } else {
                Color::Rgb(34, 34, 36)
            };
            let mut line = Line::from(vec![
                Span::styled(
                    format!("{} {} ", if selected { "›" } else { " " }, item.label),
                    Style::default()
                        .fg(if selected { Color::White } else { Color::Gray })
                        .bg(surface),
                ),
                Span::styled(
                    format!("{} · {}", item.category, item.detail),
                    Style::default().fg(Color::DarkGray).bg(surface),
                ),
            ]);
            line.style = Style::default().bg(surface);
            line
        })
        .collect::<Vec<_>>();
    let title = if app.completion_trigger == Some('@') {
        "Mentions · files / subagents"
    } else {
        "Slash · commands / skills / tools / MCP"
    };
    let surface = Color::Rgb(34, 34, 36);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().fg(Color::Gray).bg(surface))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .style(Style::default().bg(surface))
                    .title(title),
            ),
        area,
    );
}

fn render_panel(frame: &mut Frame<'_>, app: &UiState) {
    let area = frame.area();
    let (title, lines) = match app.panel {
        crate::model::UiPanel::History => {
            let mut lines = vec![Line::from(
                "↑/↓ select · Enter switch · n new session · r rename · Esc close",
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
            } else if app.renaming_session.is_some() {
                lines.push(Line::from(format!(
                    "Rename session: {}",
                    app.panel_input.text()
                )));
                lines.push(Line::from("Enter save · Esc cancel"));
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
                "Ctrl+H history · Ctrl+,/F2 settings/model · Ctrl+K catalog · Ctrl+J newline · Ctrl+D RunDeck side panel · Ctrl+L local models · ? help · Ctrl+R reasoning · Ctrl+T tools · y/a/n approvals · Esc close/cancel · q quit",
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
        .as_ref()
        .and_then(|id| app.state.sessions.iter().find(|session| &session.id == id))
        .map(|session| session.title.as_str())
        .or_else(|| app.state.selected_session.as_deref())
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
    let user_rail = Color::LightBlue;
    let assistant_rail = Color::LightYellow;
    for item in &app.state.session_history {
        if app.tools_collapsed
            && matches!(
                item,
                TranscriptItem::ToolCall { .. }
                    | TranscriptItem::ToolOutput { .. }
                    | TranscriptItem::ToolResult { .. }
            )
        {
            continue;
        }
        match item {
            crate::model::TranscriptItem::UserText(text) => {
                push_rich_message(&mut lines, user_rail, text);
                lines.push(Line::default());
            }
            crate::model::TranscriptItem::AssistantText(text) => {
                push_rich_message(&mut lines, assistant_rail, text);
                lines.push(Line::default());
            }
            TranscriptItem::AssistantReasoning(text) => {
                push_reasoning(&mut lines, text, app.reasoning_collapsed);
            }
            crate::model::TranscriptItem::System(text) => {
                push_transcript_note(&mut lines, text, Color::Magenta);
            }
            TranscriptItem::ToolCall { tool, call, .. } => {
                push_transcript_note(&mut lines, &format!("↳ {tool} · {call}"), Color::DarkGray);
            }
            TranscriptItem::ToolOutput { output, .. } => {
                push_transcript_note(&mut lines, &format!("  {output}"), Color::Gray);
            }
            TranscriptItem::ToolResult { result, .. } => {
                push_transcript_note(&mut lines, &format!("  Result · {result}"), Color::DarkGray);
            }
            crate::model::TranscriptItem::TaskResult {
                task_id,
                ok,
                result,
            } => push_transcript_note(
                &mut lines,
                &format!(
                    "Task {task_id} {} · {result}",
                    if *ok { "done" } else { "failed" }
                ),
                if *ok { Color::Green } else { Color::Red },
            ),
            crate::model::TranscriptItem::Approval {
                approval_id,
                tool,
                summary,
            } => push_transcript_note(
                &mut lines,
                &format!("Approval · {tool} · {summary} [{approval_id}]"),
                Color::LightYellow,
            ),
        }
    }
    if let Some(prompt) = &app.current_prompt {
        push_rich_message(&mut lines, user_rail, prompt);
        lines.push(Line::default());
    }
    if let Some(run_id) = &app.state.active_run {
        if let Some(run) = app.state.run(run_id) {
            for item in &run.transcript {
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
                match item {
                    crate::model::TranscriptItem::UserText(text) => {
                        push_rich_message(&mut lines, user_rail, text);
                        lines.push(Line::default());
                    }
                    crate::model::TranscriptItem::AssistantText(text) => {
                        push_rich_message(&mut lines, assistant_rail, text);
                        lines.push(Line::default());
                    }
                    TranscriptItem::AssistantReasoning(text) => {
                        push_reasoning(&mut lines, text, app.reasoning_collapsed);
                    }
                    TranscriptItem::ToolCall { tool, call, .. } => {
                        lines.push(Line::from(Span::styled(
                            format!("┃ ↳ {tool} · {call}"),
                            Style::default().fg(Color::DarkGray),
                        )))
                    }
                    TranscriptItem::ToolOutput { output, .. } => {
                        push_transcript_note(&mut lines, &format!("  {output}"), Color::Gray);
                    }
                    TranscriptItem::ToolResult { result, .. } => {
                        push_transcript_note(
                            &mut lines,
                            &format!("Result · {result}"),
                            Color::DarkGray,
                        );
                    }
                    crate::model::TranscriptItem::TaskResult {
                        task_id,
                        ok,
                        result,
                    } => push_transcript_note(
                        &mut lines,
                        &format!(
                            "Task {task_id} {} · {result}",
                            if *ok { "done" } else { "failed" }
                        ),
                        if *ok { Color::Green } else { Color::Red },
                    ),
                    crate::model::TranscriptItem::Approval {
                        approval_id,
                        tool,
                        summary,
                    } => push_transcript_note(
                        &mut lines,
                        &format!("Approval · {tool} · {summary} [{approval_id}]"),
                        Color::Red,
                    ),
                    crate::model::TranscriptItem::System(text) => {
                        push_transcript_note(&mut lines, text, Color::Magenta);
                    }
                }
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
    let total_lines = wrapped_line_count(&lines, area.width as usize);
    let viewport_lines = area.height as usize;
    let bottom_offset = total_lines.saturating_sub(viewport_lines);
    let offset = if app.follow_output {
        bottom_offset
    } else {
        bottom_offset.saturating_sub(app.transcript_scroll)
    };
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    frame.render_widget(
        paragraph.scroll((offset.min(u16::MAX as usize) as u16, 0)),
        area,
    );
}

fn wrapped_line_count(lines: &[Line<'_>], width: usize) -> usize {
    let width = width.max(1);
    lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum()
}

fn render_composer(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let title = if let Some(approval) = app.state.pending_approval() {
        format!(
            "Approval{} · {} / {} · Tab focus · y allow · a always · n deny",
            if app.approval_focused {
                " [focused]"
            } else {
                ""
            },
            approval.tool,
            approval.approval_id
        )
    } else if app.state.active_run.is_some() {
        "Composer · Esc stop · Enter send".to_owned()
    } else {
        "Composer · Ctrl+J newline · Enter send".to_owned()
    };
    let surface = Color::Rgb(34, 34, 36);
    frame.render_widget(
        Paragraph::new(format!("> {}", app.draft.text()))
            .style(Style::default().fg(Color::White).bg(surface))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .style(Style::default().bg(surface))
                    .title(title),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
    let (column, line) = app.draft.cursor_position();
    frame.set_cursor_position((
        area.x.saturating_add(2).saturating_add(column),
        area.y
            .saturating_add(1)
            .saturating_add(line.min(area.height.saturating_sub(2))),
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
    let navigation = if area.width < 90 {
        "Ctrl+I info · Ctrl+H history · / tools · @ files"
    } else {
        "Ctrl+I info · Ctrl+H history · Ctrl+, settings · Ctrl+L models · / tools · @ files"
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓ prompts ", Style::default().fg(Color::DarkGray)),
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
            Span::raw(format!(
                " · {navigation} · Ctrl+R reasoning · Ctrl+T tools · q quit"
            )),
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
            (3, 21).into()
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
    fn assistant_stream_uses_its_own_transcript_rail() {
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
        assert!(text.contains("┃ Hello"));
        assert!(!text.contains("You Hello"));
    }

    #[test]
    fn long_loaded_history_follows_the_last_message() {
        let backend = TestBackend::new(100, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState::default();
        app.state.selected_session = Some("session-1".into());
        let mut turns = Vec::new();
        for index in 0..20 {
            turns.push(riga_server::ws::ConversationTurn {
                role: "user".into(),
                content: format!("old question {index}"),
            });
            turns.push(riga_server::ws::ConversationTurn {
                role: "assistant".into(),
                content: if index == 19 {
                    "final answer".into()
                } else {
                    format!("old answer {index}")
                },
            });
        }
        app.set_session_history(turns);
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("final answer"));
    }

    #[test]
    fn fenced_code_is_split_into_distinct_transcript_lines() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState::default();
        app.state.selected_session = Some("session-1".into());
        app.state.select_run(Some("run-1".into()));
        for (sequence, event) in [
            RigaEvent::RunStarted,
            RigaEvent::TextDelta {
                delta: "Example:\n```json\n{\"ok\":true}\n```".into(),
            },
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
        assert!(text.contains("┌─ json"));
        assert!(text.contains("┃ │ {\"ok\":true}"));
        assert!(text.contains("┃ └─"));
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

    #[test]
    fn wide_layout_keeps_named_history_and_navigation_in_a_sidebar() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState::default();
        app.state.sessions.push(riga_kernel::state::Session {
            id: "session-1".into(),
            title: "Design review".into(),
            workspace: ".".into(),
            created_at: "now".into(),
            updated_at: "now".into(),
        });
        app.state.selected_session = Some("session-1".into());
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("WORKSPACE"));
        assert!(text.contains("Settings"));
        assert!(text.contains("Local models"));
        assert!(text.contains("Design review"));
        assert!(text.contains("Ctrl+I"));
    }

    #[test]
    fn narrow_settings_sheet_preserves_the_chat_composer() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState {
            panel: UiPanel::Settings,
            ..UiState::default()
        };
        app.draft.replace("draft remains here");
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Settings"));
        assert!(text.contains("draft remains here"));
        assert!(!text.contains("WORKSPACE"));
    }

    #[test]
    fn reasoning_is_muted_on_a_darker_background_and_can_be_expanded() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState {
            reasoning_collapsed: true,
            ..UiState::default()
        };
        app.state
            .session_history
            .push(TranscriptItem::AssistantReasoning(
                "private chain preview".into(),
            ));
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer.content().iter().any(|cell| {
            cell.symbol() == "╎" && cell.fg == Color::DarkGray && cell.bg == Color::Rgb(34, 34, 36)
        }));
        let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("Ctrl+R to expand"));
    }

    #[test]
    fn panel_switching_keeps_chat_visible_at_desktop_and_mobile_widths() {
        for (width, height) in [(120, 40), (80, 24), (48, 18)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut app = UiState::default();
            app.draft.replace("draft remains visible");

            app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
            assert_eq!(app.panel, UiPanel::History);
            terminal.draw(|frame| render(frame, &app)).unwrap();
            let history: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                history.contains("History"),
                "History missing at {width} columns"
            );
            assert!(
                history.contains("draft remains visible"),
                "composer missing at {width} columns"
            );

            app.handle_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::CONTROL));
            assert_eq!(app.panel, UiPanel::Settings);
            terminal.draw(|frame| render(frame, &app)).unwrap();
            let settings: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                settings.contains("Settings"),
                "Settings missing at {width} columns"
            );
            assert!(
                settings.contains("draft remains visible"),
                "composer missing at {width} columns"
            );

            app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL));
            assert_eq!(app.panel, UiPanel::Help);
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert_eq!(app.panel, UiPanel::Transcript);
        }
    }
}
