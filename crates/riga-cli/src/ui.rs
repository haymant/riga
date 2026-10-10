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

fn push_reasoning(lines: &mut Vec<Line<'static>>, text: &str, collapsed: bool, width: usize) {
    let surface = Color::Rgb(34, 34, 36);
    let visual_rows = text_visual_rows(text, width, 2).saturating_add(1);
    if collapsed && visual_rows > 3 {
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
                    "Thinking · {} chars · {preview} · Ctrl+R expand",
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

fn text_visual_rows(text: &str, width: usize, prefix_width: usize) -> usize {
    let available = width.saturating_sub(prefix_width).max(1);
    text.lines()
        .map(|line| Line::from(line).width().max(1).div_ceil(available))
        .sum()
}

fn push_tool_call(
    lines: &mut Vec<Line<'static>>,
    tool: &str,
    call_id: &str,
    call: &serde_json::Value,
    collapsed: bool,
    width: usize,
) {
    let pretty = serde_json::to_string_pretty(call).unwrap_or_else(|_| call.to_string());
    let rows = text_visual_rows(&pretty, width, 2).saturating_add(1);
    if collapsed && rows > 3 {
        let arguments = call.get("arguments").unwrap_or(call);
        let summary = serde_json::to_string(arguments).unwrap_or_default();
        let preview: String = summary.chars().take(68).collect();
        push_transcript_note(
            lines,
            &format!("↳ {tool} · {call_id} · {rows} rows · {preview} · Ctrl+T expand"),
            Color::DarkGray,
        );
        return;
    }
    push_transcript_note(lines, &format!("↳ {tool} · {call_id}"), Color::DarkGray);
    for line in pretty.lines() {
        push_transcript_note(lines, &format!("  {line}"), Color::Gray);
    }
}

fn push_tool_detail(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    text: &str,
    color: Color,
    collapsed: bool,
    width: usize,
) {
    let rows = text_visual_rows(text, width, 2).saturating_add(1);
    if collapsed && rows > 3 {
        let preview: String = text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("(empty)")
            .chars()
            .take(68)
            .collect();
        push_transcript_note(
            lines,
            &format!("{label} · {rows} rows · {preview} · Ctrl+T expand"),
            Color::DarkGray,
        );
        return;
    }
    push_transcript_note(lines, &format!("{label} ·"), color);
    for line in text.lines() {
        push_transcript_note(lines, line, color);
    }
}

pub fn render(frame: &mut Frame<'_>, app: &UiState) {
    let area = frame.area();
    if has_wide_sidebar(area) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(40),
                Constraint::Length(1),
                Constraint::Length(if app.sidebar_width == 0 {
                    34
                } else {
                    app.sidebar_width.clamp(24, 56)
                }),
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
    } else {
        app.last_sidebar_panel.unwrap_or(UiPanel::History)
    }
}

fn sidebar_panel_title(panel: UiPanel, model_picker_open: bool) -> &'static str {
    if panel == UiPanel::Settings && model_picker_open {
        return "Models";
    }
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

fn fit_sidebar_text(text: &str, max_width: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if max_width == 0 {
        return String::new();
    }
    if Line::from(compact.clone()).width() as usize <= max_width {
        return compact;
    }
    let mut output = String::new();
    for character in compact.chars() {
        let candidate = format!("{output}{character}…");
        if Line::from(candidate).width() as usize > max_width {
            break;
        }
        output.push(character);
    }
    output.push('…');
    output
}

fn sidebar_section(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        title.to_uppercase(),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

fn sidebar_field(label: &str, value: &str, max_width: usize) -> Line<'static> {
    sidebar_field_color(label, value, max_width, Color::Gray)
}

fn sidebar_field_color(
    label: &str,
    value: &str,
    max_width: usize,
    value_color: Color,
) -> Line<'static> {
    let prefix = format!("{label} · ");
    let value_width = max_width.saturating_sub(Line::from(prefix.as_str()).width() as usize);
    Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::DarkGray)),
        Span::styled(
            fit_sidebar_text(value, value_width),
            Style::default().fg(value_color),
        ),
    ])
}

fn sidebar_setting_line(
    index: usize,
    selected: usize,
    label: &str,
    value: &str,
    max_width: usize,
) -> Line<'static> {
    let marker = if index == selected { "›" } else { " " };
    sidebar_field(&format!("{marker} {label}"), value, max_width)
}

fn sidebar_panel_lines(
    app: &UiState,
    panel: UiPanel,
    max_lines: usize,
    max_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match panel {
        UiPanel::History | UiPanel::Transcript => {
            if app.creating_session {
                lines.push(Line::from("New session"));
                lines.push(Line::from(format!("Title: {}", app.panel_input.text())));
                lines.push(Line::from(format!(
                    "Workspace: {}",
                    fit_sidebar_text(app.workspace_input.text(), max_width.saturating_sub(11))
                )));
            } else if app.renaming_session.is_some() {
                lines.push(Line::from("Rename session"));
                lines.push(Line::from(app.panel_input.text().to_owned()));
            } else {
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
                    lines.push(Line::from(format!(
                        "{marker} {}",
                        fit_sidebar_text(&session.title, max_width.saturating_sub(2))
                    )));
                }
            }
        }
        UiPanel::Settings if app.model_picker_open => {
            lines.extend(render_remote_models_sidebar(app, max_lines, max_width));
        }
        UiPanel::Settings => {
            let provider = &app.provider;
            lines.push(sidebar_section("Connection"));
            lines.push(sidebar_setting_line(
                0,
                app.provider_field,
                "Endpoint",
                if provider.endpoint.is_empty() {
                    "Not configured"
                } else {
                    &provider.endpoint
                },
                max_width,
            ));
            lines.push(sidebar_setting_line(
                1,
                app.provider_field,
                "API key",
                if provider.api_key.is_empty() {
                    "Not configured"
                } else {
                    "Configured"
                },
                max_width,
            ));
            lines.push(sidebar_setting_line(
                2,
                app.provider_field,
                "Model",
                if provider.model.is_empty() {
                    "Not configured"
                } else {
                    &provider.model
                },
                max_width,
            ));
            lines.push(sidebar_section("Behavior"));
            lines.push(sidebar_setting_line(
                3,
                app.provider_field,
                "Reasoning",
                &provider.reasoning_effort,
                max_width,
            ));
            lines.push(sidebar_setting_line(
                4,
                app.provider_field,
                "Provider",
                &format!("{:?}", provider.kind),
                max_width,
            ));
            lines.push(sidebar_setting_line(
                5,
                app.provider_field,
                "API",
                &format!("{:?}", provider.api),
                max_width,
            ));
            lines.push(sidebar_setting_line(
                6,
                app.provider_field,
                "Subagent",
                if provider.subagent_model.is_empty() {
                    "Default"
                } else {
                    &provider.subagent_model
                },
                max_width,
            ));
            if let Some(error) = &provider.error {
                lines.push(Line::from(vec![
                    Span::styled("Error · ", Style::default().fg(Color::Red)),
                    Span::styled(
                        fit_sidebar_text(error, max_width.saturating_sub(9)),
                        Style::default().fg(Color::Red),
                    ),
                ]));
            }
        }
        UiPanel::LocalModels => {
            lines.extend(render_local_models_sidebar(app, max_lines, max_width));
        }
        UiPanel::RunDeck => {
            lines.extend(render_rundeck_sidebar(app, max_lines, max_width));
        }
        UiPanel::Catalog => {
            let entries = app.filtered_catalog();
            lines.push(sidebar_field("Filter", app.catalog_query.text(), max_width));
            lines.push(sidebar_field(
                "Matches",
                &format!("{} entries", entries.len()),
                max_width,
            ));
            if let Some(entry) = entries.get(app.panel_cursor) {
                lines.push(sidebar_section("Selected"));
                lines.push(sidebar_field("Type", &entry.kind, max_width));
                lines.push(Line::from(fit_sidebar_text(&entry.id, max_width)));
                if !entry.description.is_empty() {
                    lines.push(sidebar_field("About", &entry.description, max_width));
                }
            } else if entries.is_empty() {
                lines.push(Line::from("No matching entries"));
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
                Line::from("/model · choose remote model"),
                Line::from("/new · /resume · /rename · /status"),
                Line::from("v · model choices / RunDeck details"),
                Line::from("Ctrl+R / Ctrl+T · collapse output"),
                Line::from("Tab / mouse · focus composer, chat, sidebar"),
                Line::from("Shift+drag select · Ctrl+C copy · Ctrl+Shift+V paste"),
                Line::from("+/- · resize sidebar · Esc close · q quit"),
            ]);
        }
    }
    lines.truncate(max_lines.max(1));
    lines
}

fn sidebar_footer_hints(app: &UiState, panel: UiPanel) -> Vec<String> {
    if app.panel != panel {
        return vec![
            "Tab focus · +/- resize".into(),
            "Ctrl+H history · Ctrl+I info".into(),
        ];
    }
    match panel {
        UiPanel::History if app.creating_session || app.renaming_session.is_some() => [
            "Enter save · Esc cancel".into(),
            "Tab chat · Ctrl+H history".into(),
        ]
        .into(),
        UiPanel::History | UiPanel::Transcript => [
            "↑/↓ select · Enter open".into(),
            "n new · r rename · Esc close".into(),
        ]
        .into(),
        UiPanel::Settings if app.model_picker_open => [
            "Type to filter · ↑/↓ choose".into(),
            "Enter select · Esc close · /model ID direct".into(),
        ]
        .into(),
        UiPanel::Settings => [
            "Tab fields · Ctrl+S save".into(),
            "↑/↓ change · Esc close".into(),
        ]
        .into(),
        UiPanel::LocalModels if app.local_attachment_input => [
            "Enter upload · Esc cancel".into(),
            "Tab chat · Ctrl+L models".into(),
        ]
        .into(),
        UiPanel::LocalModels => [
            "↑/↓ choose · Enter load/get".into(),
            "v list · a attach · u unload".into(),
            "x cancel download · Esc close".into(),
        ]
        .into(),
        UiPanel::RunDeck => [
            "←/→ lens · ↑/↓ runs".into(),
            format!(
                "v {} · Enter resume",
                if app.deck_details_expanded {
                    "summary"
                } else {
                    "details"
                }
            ),
            "x stop · Esc close".into(),
        ]
        .into(),
        UiPanel::Catalog => [
            "Type filter · ↑/↓ select".into(),
            "Enter insert · Esc close".into(),
        ]
        .into(),
        UiPanel::Help => vec![
            "Tab focus · Esc close".into(),
            "q quit · Ctrl+I info".into(),
        ],
    }
}

fn sidebar_footer_height(app: &UiState, panel: UiPanel, available_height: u16) -> u16 {
    if available_height < 5 {
        0
    } else {
        (sidebar_footer_hints(app, panel).len() as u16).min(available_height.saturating_sub(1))
    }
}

fn render_sidebar_footer(frame: &mut Frame<'_>, area: Rect, app: &UiState, panel: UiPanel) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let background = Color::Rgb(25, 25, 27);
    frame.render_widget(
        Block::default().style(Style::default().bg(background)),
        area,
    );
    let hints = sidebar_footer_hints(app, panel);
    let lines = hints
        .into_iter()
        .take(area.height as usize)
        .map(|hint| {
            Line::from(Span::styled(
                fit_sidebar_text(&hint, area.width.saturating_sub(1) as usize),
                Style::default().fg(Color::DarkGray).bg(background),
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(background)),
        area,
    );
}

fn render_local_models_sidebar(
    app: &UiState,
    max_lines: usize,
    max_width: usize,
) -> Vec<Line<'static>> {
    if app.local_attachment_input {
        return vec![
            sidebar_section("Attach file"),
            sidebar_field("Path", app.panel_input.text(), max_width),
        ];
    }
    let Some(overview) = &app.local_models else {
        let mut lines = vec![sidebar_section("Runtime"), Line::from("No model inventory")];
        if let Some(progress) = app.local_model_progress {
            lines.push(sidebar_field(
                "Downloading",
                &format!("{:.1}%", progress * 100.0),
                max_width,
            ));
        } else if let Some(activity) = app.local_model_status.as_deref().or(app.busy.as_deref()) {
            lines.push(sidebar_field("Activity", activity, max_width));
        }
        if let Some(error) = &app.local_model_error {
            lines.push(sidebar_field("Error", error, max_width));
        }
        lines.truncate(max_lines.max(1));
        return lines;
    };

    let mut lines = vec![sidebar_section("Runtime")];
    lines.push(sidebar_field("Device", overview.accelerator, max_width));
    lines.push(sidebar_field(
        "Loaded",
        overview.loaded.as_deref().unwrap_or("None"),
        max_width,
    ));

    let total = overview.installed.len() + overview.catalog.len();
    if let Some(progress) = app.local_model_progress {
        let downloading =
            local_model_sidebar_item(overview, app.panel_cursor.min(total.saturating_sub(1)))
                .map(|(name, _, _)| name)
                .unwrap_or_else(|| "model".into());
        lines.push(sidebar_field(
            "Downloading",
            &format!("{downloading} · {:.1}%", progress * 100.0),
            max_width,
        ));
    } else if let Some(status) = &app.local_model_status {
        lines.push(sidebar_field("Activity", status, max_width));
    } else if let Some(busy) = &app.busy {
        lines.push(sidebar_field("Activity", busy, max_width));
    }

    lines.push(sidebar_section("Library"));
    lines.push(sidebar_field(
        "Models",
        &format!(
            "{} installed · {} catalog",
            overview.installed.len(),
            overview.catalog.len()
        ),
        max_width,
    ));

    if total == 0 {
        lines.push(Line::from("No models available"));
    } else if app.local_models_expanded {
        lines.push(sidebar_section("Choices"));
        let list_capacity = max_lines.saturating_sub(lines.len()).max(1);
        let selected = app.panel_cursor.min(total - 1);
        let start = selected
            .saturating_sub(list_capacity / 2)
            .min(total.saturating_sub(list_capacity));
        for index in start..(start + list_capacity).min(total) {
            if let Some((name, detail, kind)) = local_model_sidebar_item(overview, index) {
                let marker = if index == selected { "›" } else { "·" };
                lines.push(Line::from(fit_sidebar_text(
                    &format!("{marker} {name} · {kind} · {detail}"),
                    max_width,
                )));
            }
        }
    } else {
        let selected = app.panel_cursor.min(total - 1);
        if let Some((name, detail, kind)) = local_model_sidebar_item(overview, selected) {
            lines.push(sidebar_field(
                "Selected",
                &format!("{}/{} · {kind}", selected + 1, total),
                max_width,
            ));
            lines.push(Line::from(Span::styled(
                fit_sidebar_text(&name, max_width),
                Style::default().fg(Color::White),
            )));
            lines.push(sidebar_field("Details", &detail, max_width));
        }
    }

    if let Some(error) = &app.local_model_error {
        lines.push(Line::from(vec![
            Span::styled("Error · ", Style::default().fg(Color::Red)),
            Span::styled(
                fit_sidebar_text(error, max_width.saturating_sub(9)),
                Style::default().fg(Color::Red),
            ),
        ]));
    }
    lines.truncate(max_lines.max(1));
    lines
}

fn local_model_sidebar_item(
    overview: &riga_server::LocalModelOverview,
    index: usize,
) -> Option<(String, String, &'static str)> {
    if let Some(model) = overview.installed.get(index) {
        return Some((
            model.name.clone(),
            format!(
                "{} · {:.1} GiB",
                model.id,
                model.size_bytes as f64 / 1_073_741_824.0
            ),
            "installed",
        ));
    }
    overview
        .catalog
        .get(index.saturating_sub(overview.installed.len()))
        .map(|model| {
            (
                model.name.clone(),
                format!(
                    "{} · {:.1} GiB · ctx {}",
                    model.id,
                    model.size_bytes as f64 / 1_073_741_824.0,
                    model.recommended_context
                ),
                "catalog",
            )
        })
}

fn render_remote_models_sidebar(
    app: &UiState,
    max_lines: usize,
    max_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![sidebar_section("Remote models")];
    let current = if app.provider.kind == riga_server::ws::ProviderKind::Remote {
        app.provider.model.as_str()
    } else {
        "Local model active"
    };
    lines.push(sidebar_field("Current", current, max_width));
    lines.push(sidebar_field(
        "Filter",
        app.remote_model_query.text(),
        max_width,
    ));
    if app.remote_models_loading {
        lines.push(Line::from("Loading provider catalog…"));
    }
    if let Some(error) = &app.remote_models_error {
        lines.push(Line::from(vec![
            Span::styled("Unavailable · ", Style::default().fg(Color::Red)),
            Span::styled(
                fit_sidebar_text(error, max_width.saturating_sub(13)),
                Style::default().fg(Color::Red),
            ),
        ]));
    }
    let models = app.filtered_remote_models();
    if models.is_empty() && !app.remote_models_loading {
        lines.push(Line::from(if app.remote_models.is_empty() {
            "No listed models · type an ID to set it"
        } else {
            "No models match this filter"
        }));
    }
    for (index, model) in models.iter().enumerate() {
        let selected = index == app.panel_cursor;
        let active = app.provider.kind == riga_server::ws::ProviderKind::Remote
            && *model == app.provider.model;
        let marker = if selected {
            "›"
        } else if active {
            "●"
        } else {
            "·"
        };
        let surface = if selected {
            Color::Rgb(53, 53, 57)
        } else {
            Color::Rgb(29, 29, 31)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} "),
                Style::default()
                    .fg(if active { Color::Green } else { Color::Cyan })
                    .bg(surface),
            ),
            Span::styled(
                fit_sidebar_text(model, max_width.saturating_sub(2)),
                Style::default()
                    .fg(if active { Color::White } else { Color::Gray })
                    .bg(surface),
            ),
        ]));
    }
    lines.truncate(max_lines.max(1));
    lines
}

fn run_status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Unknown => "Unknown",
        RunStatus::Running => "Running",
        RunStatus::WaitingForApproval => "Needs approval",
        RunStatus::Completed => "Completed",
        RunStatus::Failed => "Failed",
        RunStatus::Cancelled => "Cancelled",
    }
}

fn run_status_color(status: RunStatus) -> Color {
    match status {
        RunStatus::Running => Color::Cyan,
        RunStatus::WaitingForApproval => Color::Yellow,
        RunStatus::Completed => Color::Green,
        RunStatus::Failed => Color::Red,
        RunStatus::Cancelled | RunStatus::Unknown => Color::DarkGray,
    }
}

fn render_rundeck_sidebar(app: &UiState, max_lines: usize, max_width: usize) -> Vec<Line<'static>> {
    let lens = match app.deck_lens {
        crate::model::DeckLens::Execution => "Execution",
        crate::model::DeckLens::Evidence => "Evidence",
        crate::model::DeckLens::Knowledge => "Knowledge",
    };
    let mut lines = vec![sidebar_section(lens)];

    let run_count = app.state.active_runs.len();
    let selected_index = app.panel_cursor.min(run_count.saturating_sub(1));
    let selected_active = app.state.active_runs.get(selected_index);
    let run_id = selected_active
        .map(|active| active.run_id.as_str())
        .or(app.state.active_run.as_deref());

    if run_count > 0 {
        lines.push(sidebar_field(
            "Run",
            &format!("{} of {} active", selected_index + 1, run_count),
            max_width,
        ));
    }
    let Some(run_id) = run_id else {
        lines.push(Line::from("No active runs"));
        lines.truncate(max_lines.max(1));
        return lines;
    };
    let Some(run) = app.state.runs.get(run_id) else {
        lines.push(sidebar_field("Run", run_id, max_width));
        lines.push(Line::from("Waiting for run details"));
        lines.truncate(max_lines.max(1));
        return lines;
    };

    lines.push(sidebar_field_color(
        "Status",
        run_status_label(run.status),
        max_width,
        run_status_color(run.status),
    ));
    let session_id = selected_active
        .map(|active| active.session_id.as_str())
        .unwrap_or(run.session_id.as_str());
    let session_name = app
        .state
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .map(|session| session.title.as_str())
        .unwrap_or(session_id);
    lines.push(sidebar_field("Session", session_name, max_width));

    if let Some(todos) = &run.todos {
        let (done, total) = todos.progress();
        lines.push(sidebar_field(
            "Progress",
            &format!("{done}/{total} todos"),
            max_width,
        ));
    } else if !run.tasks.is_empty() {
        let completed = run
            .tasks
            .values()
            .filter(|task| task.state == riga_kernel::task::TaskState::Completed)
            .count();
        let running = run
            .tasks
            .values()
            .filter(|task| task.state == riga_kernel::task::TaskState::Running)
            .count();
        lines.push(sidebar_field(
            "Tasks",
            &format!("{completed}/{} · {running} running", run.tasks.len()),
            max_width,
        ));
    }
    let waiting_approvals = run
        .approvals
        .values()
        .filter(|approval| approval.resolved.is_none())
        .count();
    if waiting_approvals > 0 {
        lines.push(sidebar_field_color(
            "Approvals",
            &format!("{waiting_approvals} waiting"),
            max_width,
            Color::Yellow,
        ));
    }

    match app.deck_lens {
        crate::model::DeckLens::Execution => {
            if let Some(plan) = &run.plan {
                lines.push(sidebar_field("Plan", &plan.title, max_width));
                if let Some(step) = plan.steps.get(plan.active_index) {
                    lines.push(sidebar_field("Next", &step.label, max_width));
                } else if !plan.steps.is_empty() {
                    lines.push(sidebar_field("Next", "Plan complete", max_width));
                }
            } else if let Some(todo) = run.todos.as_ref().and_then(|todos| {
                todos
                    .items
                    .iter()
                    .find(|item| item.status == riga_kernel::task::TodoStatus::Active)
                    .or_else(|| {
                        todos
                            .items
                            .iter()
                            .find(|item| item.status == riga_kernel::task::TodoStatus::Pending)
                    })
            }) {
                lines.push(sidebar_field("Next", &todo.text, max_width));
            } else if let Some(graph) = &run.graph {
                lines.push(sidebar_field("Graph", &graph.title, max_width));
            } else {
                lines.push(Line::from("Waiting for execution updates"));
            }

            if app.deck_details_expanded {
                if let Some(plan) = &run.plan {
                    lines.push(sidebar_section("Plan steps"));
                    for (index, step) in plan.steps.iter().enumerate() {
                        let marker = if index == plan.active_index {
                            "›"
                        } else {
                            "·"
                        };
                        lines.push(Line::from(fit_sidebar_text(
                            &format!("{marker} {}", step.label),
                            max_width,
                        )));
                    }
                }
                if let Some(todos) = &run.todos {
                    lines.push(sidebar_section("Todos"));
                    for item in &todos.items {
                        lines.push(Line::from(fit_sidebar_text(
                            &format!("{:?} · {}", item.status, item.text),
                            max_width,
                        )));
                    }
                }
                if !run.tasks.is_empty() {
                    lines.push(sidebar_section("Subtasks"));
                    for task in run.tasks.values() {
                        let detail = task
                            .record
                            .as_ref()
                            .map(|record| format!("{} · {}", record.agent, record.description))
                            .or_else(|| task.result.clone())
                            .unwrap_or_else(|| "Task details pending".into());
                        lines.push(Line::from(fit_sidebar_text(
                            &format!("{:?} · {detail}", task.state),
                            max_width,
                        )));
                    }
                }
                if let Some(graph) = &run.graph {
                    lines.push(sidebar_section("Graph"));
                    for node in &graph.nodes {
                        lines.push(Line::from(fit_sidebar_text(
                            &format!("{} · {}", node.profile, node.description),
                            max_width,
                        )));
                    }
                }
            }
        }
        crate::model::DeckLens::Evidence => {
            lines.push(sidebar_field(
                "Records",
                &run.evidence.len().to_string(),
                max_width,
            ));
            if let Some(latest) = run.evidence.last() {
                lines.push(sidebar_field("Latest", &latest.claim, max_width));
                lines.push(sidebar_field("Source", &latest.source_ref, max_width));
                lines.push(sidebar_field(
                    "Confidence",
                    &format!("{}%", latest.confidence),
                    max_width,
                ));
            } else {
                lines.push(Line::from("No evidence captured yet"));
            }
            if app.deck_details_expanded {
                lines.push(sidebar_section("Recent evidence"));
                for evidence in run.evidence.iter().rev() {
                    lines.push(Line::from(fit_sidebar_text(
                        &format!("{} · {}%", evidence.claim, evidence.confidence),
                        max_width,
                    )));
                    lines.push(sidebar_field("Source", &evidence.source_ref, max_width));
                }
            }
        }
        crate::model::DeckLens::Knowledge => {
            lines.push(sidebar_field(
                "Facts",
                &run.knowledge.len().to_string(),
                max_width,
            ));
            if let Some(latest) = run.knowledge.last() {
                lines.push(sidebar_field("Latest", &latest.fact, max_width));
                lines.push(sidebar_field(
                    "Source run",
                    &latest.source_run_id,
                    max_width,
                ));
                lines.push(sidebar_field(
                    "Confidence",
                    &format!("{}%", latest.confidence),
                    max_width,
                ));
            } else {
                lines.push(Line::from("No reusable knowledge yet"));
            }
            if app.deck_details_expanded {
                lines.push(sidebar_section("Recent facts"));
                for knowledge in run.knowledge.iter().rev() {
                    lines.push(Line::from(fit_sidebar_text(
                        &format!("{} · {}%", knowledge.fact, knowledge.confidence),
                        max_width,
                    )));
                    lines.push(sidebar_field(
                        "Source run",
                        &knowledge.source_run_id,
                        max_width,
                    ));
                }
            }
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
        UiPanel::Settings if app.model_picker_open => return None,
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
    let footer_height = sidebar_footer_height(app, panel, area.height);
    let editor_height =
        u16::from(editor.is_some() && area.height > footer_height.saturating_add(1));
    let body_area = Rect {
        height: area
            .height
            .saturating_sub(editor_height)
            .saturating_sub(footer_height),
        ..area
    };
    let mut lines = Vec::new();
    if show_title {
        lines.push(Line::from(Span::styled(
            format!(
                "  {}",
                sidebar_panel_title(panel, app.model_picker_open).to_uppercase()
            ),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.extend(sidebar_panel_lines(
        app,
        panel,
        body_area.height.saturating_sub(u16::from(show_title)) as usize,
        body_area.width as usize,
    ));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().fg(Color::Gray).bg(background))
            .wrap(Wrap { trim: true }),
        body_area,
    );

    if let Some((label, value, cursor)) = editor {
        if editor_height == 0 {
            if footer_height > 0 {
                render_sidebar_footer(
                    frame,
                    Rect {
                        y: area.y + area.height - footer_height,
                        height: footer_height,
                        ..area
                    },
                    app,
                    panel,
                );
            }
            return;
        }
        let editor_area = Rect {
            y: area.y + body_area.height,
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
        if app.panel == panel {
            frame.set_cursor_position((cursor_x, editor_area.y));
        }
    }
    if footer_height > 0 {
        render_sidebar_footer(
            frame,
            Rect {
                y: area.y + area.height - footer_height,
                height: footer_height,
                ..area
            },
            app,
            panel,
        );
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
                        .bg(if app.panel == panel {
                            Color::Rgb(53, 53, 57)
                        } else {
                            Color::Rgb(39, 39, 42)
                        })
                        .add_modifier(if app.panel == panel {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        })
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
            .title(format!(
                " {} · Esc close ",
                sidebar_panel_title(panel, app.model_picker_open)
            ))
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
                "Ctrl+I info · Ctrl+H history · Ctrl+,/F2 settings · Ctrl+K catalog · Ctrl+J newline · Ctrl+D RunDeck · Ctrl+L models · Ctrl+O shell stdin · Ctrl+R reasoning · Ctrl+T tools · Tab focus · drag select · Ctrl+C copy · Ctrl+Shift+V paste · +/- sidebar · y/a/n approvals · Esc close/cancel · q quit",
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

fn transcript_lines(app: &UiState, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let user_rail = Color::LightBlue;
    let assistant_rail = Color::LightYellow;
    for item in &app.state.session_history {
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
                push_reasoning(&mut lines, text, app.reasoning_collapsed, width);
            }
            crate::model::TranscriptItem::System(text) => {
                push_transcript_note(&mut lines, text, Color::Magenta);
            }
            TranscriptItem::ToolCall {
                call_id,
                tool,
                call,
            } => {
                push_tool_call(&mut lines, tool, call_id, call, app.tools_collapsed, width);
            }
            TranscriptItem::ToolOutput { output, .. } => {
                push_tool_detail(
                    &mut lines,
                    "Tool output",
                    output,
                    Color::Gray,
                    app.tools_collapsed,
                    width,
                );
            }
            TranscriptItem::ToolResult { result, .. } => {
                let result =
                    serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string());
                push_tool_detail(
                    &mut lines,
                    "Tool result",
                    &result,
                    Color::DarkGray,
                    app.tools_collapsed,
                    width,
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
                        push_reasoning(&mut lines, text, app.reasoning_collapsed, width);
                    }
                    TranscriptItem::ToolCall {
                        call_id,
                        tool,
                        call,
                    } => {
                        push_tool_call(&mut lines, tool, call_id, call, app.tools_collapsed, width);
                    }
                    TranscriptItem::ToolOutput { output, .. } => {
                        push_tool_detail(
                            &mut lines,
                            "Tool output",
                            output,
                            Color::Gray,
                            app.tools_collapsed,
                            width,
                        );
                    }
                    TranscriptItem::ToolResult { result, .. } => {
                        let result = serde_json::to_string_pretty(result)
                            .unwrap_or_else(|_| result.to_string());
                        push_tool_detail(
                            &mut lines,
                            "Tool result",
                            &result,
                            Color::DarkGray,
                            app.tools_collapsed,
                            width,
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
    lines
}

fn transcript_area_width(terminal_width: u16, terminal_height: u16, app: &UiState) -> usize {
    if terminal_width >= 112 && terminal_height >= 12 {
        let sidebar = if app.sidebar_width == 0 {
            34
        } else {
            app.sidebar_width.clamp(24, 56)
        };
        terminal_width.saturating_sub(sidebar.saturating_add(1)) as usize
    } else {
        terminal_width as usize
    }
}

fn transcript_viewport_height(terminal_height: u16, app: &UiState) -> u16 {
    let composer = app
        .draft
        .text()
        .lines()
        .count()
        .max(1)
        .saturating_add(2)
        .min(terminal_height.saturating_sub(5).max(3) as usize) as u16;
    terminal_height.saturating_sub(composer.saturating_add(2))
}

fn transcript_offset(
    lines: &[Line<'_>],
    width: usize,
    viewport_height: usize,
    app: &UiState,
) -> usize {
    let total_lines = wrapped_line_count(lines, width);
    let bottom_offset = total_lines.saturating_sub(viewport_height);
    if app.follow_output {
        bottom_offset
    } else {
        bottom_offset.saturating_sub(app.transcript_scroll)
    }
}

fn line_plain_text(line: &Line<'_>) -> String {
    let text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    text.trim_start_matches("┃ ")
        .trim_start_matches("╎ ")
        .to_owned()
}

fn selected_line_indices(
    lines: &[Line<'_>],
    width: usize,
    start_visual_row: usize,
    end_visual_row: usize,
) -> Vec<usize> {
    let mut selected = Vec::new();
    let mut visual_row = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let row_count = line.width().max(1).div_ceil(width.max(1));
        let end = visual_row.saturating_add(row_count);
        if visual_row <= end_visual_row && end > start_visual_row {
            selected.push(index);
        }
        visual_row = end;
    }
    selected
}

pub fn selected_transcript_text(
    app: &UiState,
    terminal_width: u16,
    terminal_height: u16,
) -> String {
    let (Some(anchor), Some(cursor)) = (app.selection_anchor, app.selection_cursor) else {
        return String::new();
    };
    let width = transcript_area_width(terminal_width, terminal_height, app).max(1);
    let viewport_height = transcript_viewport_height(terminal_height, app) as usize;
    let lines = transcript_lines(app, width);
    let offset = transcript_offset(&lines, width, viewport_height, app);
    let first_screen_row = anchor.min(cursor).max(1) as usize;
    let last_screen_row = anchor.max(cursor).min(viewport_height as u16) as usize;
    if first_screen_row > last_screen_row {
        return String::new();
    }
    let start = offset.saturating_add(first_screen_row.saturating_sub(1));
    let end = offset.saturating_add(last_screen_row.saturating_sub(1));
    selected_line_indices(&lines, width, start, end)
        .into_iter()
        .map(|index| line_plain_text(&lines[index]))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, app: &UiState) {
    let width = area.width.max(1) as usize;
    let mut lines = transcript_lines(app, width);
    let viewport_lines = area.height as usize;
    let offset = transcript_offset(&lines, width, viewport_lines, app);
    if let (Some(anchor), Some(cursor)) = (app.selection_anchor, app.selection_cursor) {
        let first = anchor.min(cursor).max(area.y) as usize;
        let last = cursor
            .max(anchor)
            .min(area.y.saturating_add(area.height.saturating_sub(1))) as usize;
        if first <= last {
            let selected = selected_line_indices(
                &lines,
                width,
                offset.saturating_add(first.saturating_sub(area.y as usize)),
                offset.saturating_add(last.saturating_sub(area.y as usize)),
            );
            for index in selected {
                lines[index].style = lines[index].style.bg(Color::Rgb(54, 67, 82));
            }
        }
    }
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
    let composer_focused =
        app.shell_input_mode || (app.panel == UiPanel::Transcript && !app.transcript_focused);
    let title = if app.shell_input_mode {
        "Shell stdin · Enter send · Esc return to chat".to_owned()
    } else if let Some(approval) = app.state.pending_approval() {
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
        format!(
            "Composer{} · Tab sidebar · Esc stop · Enter send",
            if composer_focused { " [focused]" } else { "" }
        )
    } else {
        format!(
            "Composer{} · Tab sidebar · Ctrl+J newline · Enter send",
            if composer_focused { " [focused]" } else { "" }
        )
    };
    let surface = Color::Rgb(34, 34, 36);
    frame.render_widget(
        Paragraph::new(if app.shell_input_mode {
            format!("$ {}", app.shell_input.text())
        } else {
            format!("> {}", app.draft.text())
        })
        .style(Style::default().fg(Color::White).bg(surface))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(if composer_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }))
                .style(Style::default().bg(surface))
                .title(title),
        )
        .wrap(Wrap { trim: false }),
        area,
    );
    if composer_focused {
        let (column, line) = if app.shell_input_mode {
            app.shell_input.cursor_position()
        } else {
            app.draft.cursor_position()
        };
        frame.set_cursor_position((
            area.x.saturating_add(2).saturating_add(column),
            area.y
                .saturating_add(1)
                .saturating_add(line.min(area.height.saturating_sub(2))),
        ));
    }
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
        "Tab focus · Ctrl+I info · Ctrl+H history · / tools · @ files"
    } else {
        "Tab focus · Ctrl+I info · Ctrl+H history · Ctrl+, settings · Ctrl+L models · / tools · @ files"
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
                " · {navigation} · Ctrl+O shell stdin · Ctrl+R reasoning · Ctrl+T tools · drag select · Ctrl+C copy · Ctrl+Shift+V paste · +/- sidebar · q quit"
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
        model::{
            AppState, ConnectionState, DeckLens, ProviderForm, RunStatus, RunView, TranscriptItem,
            UiPanel,
        },
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{
        Terminal,
        backend::{Backend, TestBackend},
    };
    use riga_kernel::{
        PROTOCOL_VERSION,
        events::{EvidenceNode, KnowledgeNode, RigaEvent, RigaEventEnvelope},
        task::{Plan, PlanStep},
    };
    use std::collections::BTreeMap;

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
    fn shell_input_mode_renders_its_prompt_and_visible_caret() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = UiState {
            shell_input_mode: true,
            ..UiState::default()
        };
        app.shell_input.replace("y");
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Shell stdin"));
        assert!(text.contains("$ y"));
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
        assert!(text.contains("Downloading"));
        assert!(text.contains("50.0%"));
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
    fn reasoning_auto_collapses_only_when_it_exceeds_three_rows() {
        let mut short = Vec::new();
        push_reasoning(&mut short, "one\ntwo", true, 80);
        assert!(!line_plain_text(&short[0]).contains("chars"));

        let mut long = Vec::new();
        push_reasoning(&mut long, "one\ntwo\nthree", true, 80);
        assert!(line_plain_text(&long[0]).contains("Ctrl+R expand"));

        let mut expanded = Vec::new();
        push_reasoning(&mut expanded, "one\ntwo\nthree", false, 80);
        assert!(line_plain_text(&expanded[0]).contains("Thinking"));
        assert!(line_plain_text(&expanded[1]).contains("one"));
    }

    #[test]
    fn long_tool_details_collapse_and_selected_rows_copy_as_plain_text() {
        let mut app = UiState {
            tools_collapsed: true,
            ..UiState::default()
        };
        app.state.session_history = vec![
            TranscriptItem::ToolOutput {
                call_id: "call-1".into(),
                output: "first\nsecond\nthird".into(),
            },
            TranscriptItem::UserText("copy this line".into()),
        ];
        let lines = transcript_lines(&app, 80);
        assert!(
            lines
                .iter()
                .any(|line| line_plain_text(line).contains("Ctrl+T expand"))
        );

        app.tools_collapsed = false;
        let expanded = transcript_lines(&app, 80);
        assert!(
            expanded
                .iter()
                .any(|line| line_plain_text(line).contains("second"))
        );

        app.state.session_history = vec![TranscriptItem::UserText("copy this line".into())];
        app.selection_anchor = Some(1);
        app.selection_cursor = Some(1);
        assert_eq!(selected_transcript_text(&app, 80, 24), "copy this line");
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
        assert!(text.contains("Configured"));
    }

    #[test]
    fn remote_model_picker_is_readable_in_sidebar_and_narrow_overlay() {
        for (width, height) in [(120, 40), (80, 24)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let app = UiState {
                panel: UiPanel::Settings,
                model_picker_open: true,
                remote_models: vec!["alpha-model".into(), "beta-model".into()],
                provider: ProviderForm {
                    endpoint: "https://provider.example/v1".into(),
                    model: "beta-model".into(),
                    ..ProviderForm::default()
                },
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
            assert!(
                text.to_lowercase().contains("models"),
                "width {width}: {text}"
            );
            assert!(text.contains("beta-model"), "width {width}: {text}");
            assert!(text.contains("Enter select"), "width {width}: {text}");
        }
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
    fn sidebar_shortcuts_are_in_the_footer_not_repeated_in_panel_content() {
        for panel in [
            UiPanel::History,
            UiPanel::Settings,
            UiPanel::LocalModels,
            UiPanel::RunDeck,
        ] {
            let app = UiState {
                panel,
                ..UiState::default()
            };
            let body = sidebar_panel_lines(&app, panel, 24, 34)
                .iter()
                .map(line_plain_text)
                .collect::<Vec<_>>()
                .join("\n");
            let footer = sidebar_footer_hints(&app, panel).join("\n");
            assert!(
                !body.contains("Esc close"),
                "panel body for {panel:?}: {body}"
            );
            assert!(
                !body.contains("Tab chat"),
                "panel body for {panel:?}: {body}"
            );
            assert!(!footer.is_empty());
        }
        let history = UiState {
            panel: UiPanel::History,
            ..UiState::default()
        };
        assert!(sidebar_footer_hints(&history, UiPanel::History)[1].contains("n new"));
    }

    #[test]
    fn panel_shortcut_footers_render_in_wide_and_narrow_layouts() {
        for (width, height) in [(120, 40), (80, 24), (48, 18)] {
            for (panel, expected) in [
                (UiPanel::LocalModels, "x cancel"),
                (UiPanel::RunDeck, "x stop"),
            ] {
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                let app = UiState {
                    panel,
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
                assert!(
                    text.contains(expected),
                    "footer hint `{expected}` missing at {width}x{height}: {text}"
                );
            }
        }
    }

    #[test]
    fn local_model_sidebar_is_compact_and_expands_a_scroll_window_of_choices() {
        let mut app = UiState {
            panel: UiPanel::LocalModels,
            local_models: Some(riga_server::LocalModelOverview {
                accelerator: "CPU (OpenMP)",
                catalog: Vec::new(),
                installed: vec![riga_server::local_model::InstalledModel {
                    id: "friendly-model-id".into(),
                    name: "Friendly local model".into(),
                    file_name: "model.gguf".into(),
                    path: "/private/model/path.gguf".into(),
                    size_bytes: 2_000_000_000,
                    curated: true,
                    recommended_context: Some(8192),
                    license_url: None,
                }],
                loaded: None,
            }),
            ..UiState::default()
        };
        let compact = sidebar_panel_lines(&app, UiPanel::LocalModels, 20, 34);
        let compact_text = compact
            .iter()
            .map(line_plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(compact_text.contains("Friendly local model"));
        assert!(compact_text.contains("1 installed"));
        assert!(!compact_text.contains("/private/model/path.gguf"));
        assert!(!compact_text.contains("Enter load/get"));
        assert!(compact.iter().all(|line| line.width() <= 34));
        assert!(sidebar_footer_hints(&app, UiPanel::LocalModels)[0].contains("Enter load/get"));

        app.local_models_expanded = true;
        let expanded = sidebar_panel_lines(&app, UiPanel::LocalModels, 20, 34);
        let expanded_text = expanded
            .iter()
            .map(line_plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(expanded_text.contains("CHOICES"));
        assert!(expanded_text.contains("Friendly local model"));
    }

    #[test]
    fn rundeck_lenses_show_compact_summaries_and_expand_details_on_demand() {
        let mut app = UiState {
            panel: UiPanel::RunDeck,
            deck_lens: DeckLens::Execution,
            ..UiState::default()
        };
        app.state.sessions.push(riga_kernel::state::Session {
            id: "session-1".into(),
            title: "Build session".into(),
            workspace: ".".into(),
            created_at: "now".into(),
            updated_at: "now".into(),
        });
        app.state.active_run = Some("run-1".into());
        app.state.active_runs.push(riga_server::ws::ActiveRun {
            run_id: "run-1".into(),
            session_id: "session-1".into(),
            local: true,
        });
        app.state.runs.insert(
            "run-1".into(),
            RunView {
                run_id: "run-1".into(),
                session_id: "session-1".into(),
                status: RunStatus::Running,
                last_sequence: 1,
                needs_replay: false,
                output: String::new(),
                reasoning: String::new(),
                transcript: Vec::new(),
                plan: Some(Plan {
                    title: "Tidy the RunDeck sidebar".into(),
                    steps: vec![PlanStep {
                        id: "step-1".into(),
                        label: "Summarize the selected run".into(),
                        description: None,
                    }],
                    active_index: 0,
                }),
                todos: None,
                graph: None,
                tasks: BTreeMap::new(),
                evidence: vec![
                    EvidenceNode {
                        id: "evidence-1".into(),
                        claim: "Earlier verified result".into(),
                        source_ref: "test:earlier".into(),
                        confidence: 80,
                        task_id: None,
                    },
                    EvidenceNode {
                        id: "evidence-2".into(),
                        claim: "Latest compact summary is clear".into(),
                        source_ref: "test:latest".into(),
                        confidence: 96,
                        task_id: None,
                    },
                ],
                knowledge: vec![KnowledgeNode {
                    id: "knowledge-1".into(),
                    fact: "Keyboard hints live in the footer".into(),
                    source_run_id: "run-previous".into(),
                    confidence: 91,
                }],
                approvals: BTreeMap::new(),
                tool_outputs: BTreeMap::new(),
            },
        );

        for (lens, expected) in [
            (DeckLens::Execution, "Summarize the selected run"),
            (DeckLens::Evidence, "Latest compact summary"),
            (DeckLens::Knowledge, "Keyboard hints live"),
        ] {
            app.deck_lens = lens;
            let lines = sidebar_panel_lines(&app, UiPanel::RunDeck, 20, 34);
            let text = lines
                .iter()
                .map(line_plain_text)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains(expected), "{lens:?} summary: {text}");
            assert!(text.contains("Build session"));
            assert!(lines.iter().all(|line| line.width() <= 34));
        }

        app.deck_lens = DeckLens::Evidence;
        app.deck_details_expanded = true;
        let detailed = sidebar_panel_lines(&app, UiPanel::RunDeck, 20, 34)
            .iter()
            .map(line_plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(detailed.contains("RECENT EVIDENCE"));
        assert!(detailed.contains("Earlier verified result"));
        assert!(sidebar_footer_hints(&app, UiPanel::RunDeck)[1].contains("summary"));
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
        assert!(text.contains("private chain preview"));
        assert!(!text.contains("Ctrl+R expand"));
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
