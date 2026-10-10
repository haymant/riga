use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

fn current_style(stack: &[Style]) -> Style {
    stack
        .last()
        .copied()
        .unwrap_or_else(|| Style::default().fg(Color::White))
}

fn flush_line(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>) {
    lines.push(Line::from(std::mem::take(spans)));
}

fn is_blank_line(line: &Line<'_>) -> bool {
    line.spans.is_empty() || line.spans.iter().all(|span| span.content.is_empty())
}

fn ensure_prefix(spans: &mut Vec<Span<'static>>, quote_depth: usize) {
    if spans.is_empty() && quote_depth > 0 {
        spans.push(Span::styled(
            format!("{}", "│ ".repeat(quote_depth.min(4))),
            Style::default().fg(Color::DarkGray),
        ));
    }
}

fn append_text(
    lines: &mut Vec<Line<'static>>,
    spans: &mut Vec<Span<'static>>,
    text: &str,
    style: Style,
    quote_depth: usize,
) {
    let mut pieces = text.split_inclusive('\n').peekable();
    while let Some(piece) = pieces.next() {
        let has_newline = piece.ends_with('\n');
        let content = piece.strip_suffix('\n').unwrap_or(piece);
        if !content.is_empty() {
            ensure_prefix(spans, quote_depth);
            spans.push(Span::styled(content.to_owned(), style));
        }
        if has_newline {
            flush_line(lines, spans);
            if quote_depth > 0 && pieces.peek().is_some() {
                ensure_prefix(spans, quote_depth);
            }
        }
    }
}

fn code_line_style(line: &str) -> Style {
    let trimmed = line.trim_start();
    let color = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        Color::LightBlue
    } else if trimmed.starts_with('$')
        || trimmed.starts_with("npm ")
        || trimmed.starts_with("cargo ")
    {
        Color::LightYellow
    } else if trimmed.starts_with("enum ") || trimmed.contains("::") {
        Color::LightMagenta
    } else if trimmed.starts_with('-') || trimmed.starts_with('*') {
        Color::LightGreen
    } else {
        Color::Gray
    };
    Style::default().fg(color).bg(Color::Rgb(29, 29, 31))
}

fn push_code_block(lines: &mut Vec<Line<'static>>, language: &str, body: &str) {
    let language = if language.trim().is_empty() {
        "code"
    } else {
        language.trim()
    };
    lines.push(Line::from(Span::styled(
        format!("┌─ {language}"),
        Style::default().fg(Color::DarkGray),
    )));
    let body = body.strip_suffix('\n').unwrap_or(body);
    if !body.is_empty() {
        for code_line in body.split('\n') {
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                Span::styled(code_line.to_owned(), code_line_style(code_line)),
            ]));
        }
    }
    lines.push(Line::from(Span::styled(
        "└─",
        Style::default().fg(Color::DarkGray),
    )));
}

/// Render Markdown into semantic terminal lines. Width wrapping is deliberately
/// left to Ratatui's Paragraph so the caller can use the exact transcript rect.
pub fn render_markdown(markdown: &str) -> Vec<Line<'static>> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES;
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    let mut styles = vec![Style::default().fg(Color::White)];
    let mut lists: Vec<(bool, u64)> = Vec::new();
    let mut quote_depth = 0usize;
    let mut code_block: Option<(String, String)> = None;
    let mut table_cells = 0usize;

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    if !lines.is_empty() && !lines.last().is_some_and(is_blank_line) {
                        lines.push(Line::default());
                    }
                }
                Tag::Heading { level, .. } => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    if !lines.is_empty() && !lines.last().is_some_and(is_blank_line) {
                        lines.push(Line::default());
                    }
                    let heading_style = Style::default()
                        .fg(match level {
                            pulldown_cmark::HeadingLevel::H1 => Color::LightCyan,
                            pulldown_cmark::HeadingLevel::H2 => Color::Cyan,
                            _ => Color::LightBlue,
                        })
                        .add_modifier(Modifier::BOLD);
                    styles.push(heading_style);
                }
                Tag::BlockQuote(_) => {
                    quote_depth = quote_depth.saturating_add(1);
                    ensure_prefix(&mut spans, quote_depth);
                    styles.push(
                        current_style(&styles)
                            .fg(Color::Gray)
                            .add_modifier(Modifier::ITALIC),
                    );
                }
                Tag::CodeBlock(kind) => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    if !lines.is_empty() && !lines.last().is_some_and(is_blank_line) {
                        lines.push(Line::default());
                    }
                    let language = match kind {
                        CodeBlockKind::Fenced(info) => info.into_string(),
                        CodeBlockKind::Indented => "code".into(),
                    };
                    code_block = Some((language, String::new()));
                }
                Tag::List(start) => lists.push((start.is_some(), start.unwrap_or(1))),
                Tag::Item => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    let depth = lists.len().saturating_sub(1).min(8);
                    spans.push(Span::styled(
                        format!("{}", "  ".repeat(depth)),
                        Style::default().fg(Color::DarkGray),
                    ));
                    let marker = if let Some((true, next)) = lists.last_mut() {
                        let marker = format!("{next}. ");
                        *next = next.saturating_add(1);
                        marker
                    } else {
                        "• ".into()
                    };
                    spans.push(Span::styled(marker, Style::default().fg(Color::LightCyan)));
                }
                Tag::Emphasis => styles.push(current_style(&styles).add_modifier(Modifier::ITALIC)),
                Tag::Strong => styles.push(current_style(&styles).add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => {
                    styles.push(current_style(&styles).add_modifier(Modifier::CROSSED_OUT))
                }
                Tag::Link { .. } => styles.push(
                    current_style(&styles)
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Tag::Image { .. } => {
                    spans.push(Span::styled(
                        "[image: ",
                        Style::default().fg(Color::DarkGray),
                    ));
                    styles.push(
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::ITALIC),
                    );
                }
                Tag::Table(_) => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    table_cells = 0;
                }
                Tag::TableHead | Tag::TableRow => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    table_cells = 0;
                    styles.push(Style::default().fg(Color::Gray));
                }
                Tag::TableCell => {
                    if table_cells > 0 {
                        spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                    }
                    table_cells += 1;
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    if matches!(tag, TagEnd::Paragraph | TagEnd::Heading(_))
                        && !lines.last().is_some_and(is_blank_line)
                    {
                        lines.push(Line::default());
                    }
                    if matches!(tag, TagEnd::Heading(_)) && styles.len() > 1 {
                        styles.pop();
                    }
                }
                TagEnd::BlockQuote(_) => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    quote_depth = quote_depth.saturating_sub(1);
                    if styles.len() > 1 {
                        styles.pop();
                    }
                }
                TagEnd::CodeBlock => {
                    let (language, body) = code_block.take().unwrap_or_default();
                    push_code_block(&mut lines, &language, &body);
                    lines.push(Line::default());
                }
                TagEnd::List(_) => {
                    lists.pop();
                    if !lines.last().is_some_and(is_blank_line) {
                        lines.push(Line::default());
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    if styles.len() > 1 {
                        styles.pop();
                    }
                }
                TagEnd::Image => {
                    if styles.len() > 1 {
                        styles.pop();
                    }
                    spans.push(Span::styled("]", Style::default().fg(Color::DarkGray)));
                }
                TagEnd::TableCell => {}
                TagEnd::TableHead | TagEnd::TableRow => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                    if styles.len() > 1 {
                        styles.pop();
                    }
                }
                TagEnd::Table => {
                    if !spans.is_empty() {
                        flush_line(&mut lines, &mut spans);
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some((_, body)) = code_block.as_mut() {
                    body.push_str(&text);
                } else {
                    append_text(
                        &mut lines,
                        &mut spans,
                        &text,
                        current_style(&styles),
                        quote_depth,
                    );
                }
            }
            Event::Code(code) => {
                append_text(
                    &mut lines,
                    &mut spans,
                    &code,
                    current_style(&styles)
                        .fg(Color::LightYellow)
                        .bg(Color::Rgb(43, 43, 45)),
                    quote_depth,
                );
            }
            Event::SoftBreak => append_text(
                &mut lines,
                &mut spans,
                " ",
                current_style(&styles),
                quote_depth,
            ),
            Event::HardBreak => flush_line(&mut lines, &mut spans),
            Event::Rule => {
                if !spans.is_empty() {
                    flush_line(&mut lines, &mut spans);
                }
                lines.push(Line::from(Span::styled(
                    "────────────────────────",
                    Style::default().fg(Color::DarkGray),
                )));
                lines.push(Line::default());
            }
            Event::TaskListMarker(checked) => spans.push(Span::styled(
                if checked { "☑ " } else { "☐ " },
                Style::default().fg(if checked {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
            )),
            Event::Html(html) | Event::InlineHtml(html) => append_text(
                &mut lines,
                &mut spans,
                &html,
                Style::default().fg(Color::DarkGray),
                quote_depth,
            ),
            Event::FootnoteReference(name) => append_text(
                &mut lines,
                &mut spans,
                &format!("[{name}]"),
                Style::default().fg(Color::Cyan),
                quote_depth,
            ),
            Event::InlineMath(math) | Event::DisplayMath(math) => append_text(
                &mut lines,
                &mut spans,
                &math,
                current_style(&styles).add_modifier(Modifier::ITALIC),
                quote_depth,
            ),
        }
    }
    if let Some((language, body)) = code_block {
        push_code_block(&mut lines, &language, &body);
    }
    if !spans.is_empty() {
        flush_line(&mut lines, &mut spans);
    }
    while lines.last().is_some_and(is_blank_line) {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_common_markdown_blocks_and_inline_markup() {
        let lines = render_markdown(
            "# Title\n\n**bold** and *italic* with `inline` and [link](https://example.test).\n\n- one\n- two\n\n> quote\n\n- [x] done\n- [ ] todo\n\n| A | B |\n|---|---|\n| 1 | 2 |",
        );
        let text = plain(&lines);
        for expected in [
            "Title",
            "bold",
            "italic",
            "inline",
            "link",
            "• one",
            "│ quote",
            "☑",
            "☐",
            "1",
            "2",
        ] {
            assert!(
                text.contains(expected),
                "missing {expected:?} from {text:?}"
            );
        }
    }

    #[test]
    fn renders_fenced_code_with_language_label_and_bounded_style() {
        let lines = render_markdown("```json\n{\"ok\":true}\n```");
        let text = plain(&lines);
        assert!(text.contains("┌─ json"));
        assert!(text.contains("│ {\"ok\":true}"));
        assert!(text.contains("└─"));
    }

    #[test]
    fn malformed_or_unclosed_fence_remains_readable() {
        let lines = render_markdown("```rust\nfn main() {}\nstill code");
        let text = plain(&lines);
        assert!(text.contains("fn main() {}"));
        assert!(text.contains("still code"));
    }
}
