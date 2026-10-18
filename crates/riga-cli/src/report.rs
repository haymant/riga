// Deterministic JSON → markdown, the CLI's presentation layer.
//
// A tool result is JSON: flat objects and arrays of records become markdown
// tables so a tile's content reads the same in the TUI as it does in the
// dashboard. Anything deeper is kept verbatim in a fenced block rather than
// summarised, so nothing is hidden.

use serde_json::Value;

/// Render a JSON value as markdown.
///
/// - object with scalar fields → a two-column `Field | Value` table
/// - array of objects sharing keys → a table with those keys as columns
/// - nested objects/arrays → a bold key heading + a recursive render
/// - anything unrepresentable → a fenced ```json block (never silently dropped)
pub fn json_to_markdown(value: &Value) -> String {
    match value {
        Value::Null => "_(null)_".to_string(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => render_string(text),
        Value::Array(items) => render_array(items),
        Value::Object(map) => render_object(map),
    }
}

fn render_string(text: &str) -> String {
    if text.is_empty() {
        return "_(empty)_".to_string();
    }
    if text.contains('\n') || text.len() > 120 {
        return format!("```\n{text}\n```");
    }
    text.to_string()
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("—".to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(render_string(text).replace('\n', " ")),
        Value::Array(items) if items.iter().all(is_scalar) => Some(
            items
                .iter()
                .map(|item| scalar(item).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        _ => None,
    }
}

fn is_scalar(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

fn escape(cell: &str) -> String {
    cell.replace('|', "\\|").replace('\n', " ")
}

/// A markdown table from a header row and body rows.
fn table(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    out.push('|');
    for header in headers {
        out.push_str(&format!(" {} |", escape(header)));
    }
    out.push('\n');
    out.push('|');
    for _ in headers {
        out.push_str(" --- |");
    }
    out.push('\n');
    for row in rows {
        out.push('|');
        for cell in row {
            out.push_str(&format!(" {} |", escape(cell)));
        }
        out.push('\n');
    }
    out
}

fn render_object(map: &serde_json::Map<String, Value>) -> String {
    if map.is_empty() {
        return "_(empty)_".to_string();
    }
    let mut flat: Vec<(String, String)> = Vec::new();
    let mut nested: Vec<(&String, &Value)> = Vec::new();
    for (key, value) in map {
        match scalar(value) {
            Some(text) => flat.push((key.clone(), text)),
            None => nested.push((key, value)),
        }
    }

    let mut out = String::new();
    if !flat.is_empty() {
        let headers = vec!["Field".to_string(), "Value".to_string()];
        let rows = flat
            .iter()
            .map(|(key, text)| vec![key.clone(), text.clone()])
            .collect::<Vec<_>>();
        out.push_str(&table(&headers, &rows));
    }
    for (key, value) in nested {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("**{key}**\n\n"));
        out.push_str(&json_to_markdown(value));
    }
    out
}

fn render_array(items: &[Value]) -> String {
    if items.is_empty() {
        return "_(empty)_".to_string();
    }
    if items.iter().all(is_scalar) {
        return items
            .iter()
            .map(|item| scalar(item).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(", ");
    }
    // An array of records → one table. Columns are the union of the keys, in
    // first-seen order, so heterogeneous records still render.
    if items.iter().all(|item| item.is_object()) {
        let mut headers: Vec<String> = Vec::new();
        for item in items {
            if let Some(map) = item.as_object() {
                for key in map.keys() {
                    if !headers.contains(key) {
                        headers.push(key.clone());
                    }
                }
            }
        }
        let rows = items
            .iter()
            .map(|item| {
                headers
                    .iter()
                    .map(|header| match item.get(header) {
                        Some(value) => {
                            scalar(value).unwrap_or_else(|| json_to_markdown(value))
                        }
                        None => "—".to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        return table(&headers, &rows);
    }
    // Mixed array → a bullet per item.
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let body = json_to_markdown(item);
            if body.starts_with('|') || body.contains('\n') {
                format!("{}. \n{}", index + 1, body)
            } else {
                format!("{}. {}", index + 1, body)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Best-effort: render a tool's textual output. When the text is JSON (which is
/// how the riga/tac servers emit `structuredContent`) it becomes tables;
/// otherwise it is shown verbatim in a fence.
pub fn tool_output_to_markdown(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "_(empty)_".to_string();
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => json_to_markdown(&value),
        Err(_) => format!("```\n{trimmed}\n```"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalars_render_plainly() {
        assert_eq!(json_to_markdown(&json!(null)), "_(null)_");
        assert_eq!(json_to_markdown(&json!(true)), "true");
        assert_eq!(json_to_markdown(&json!(42)), "42");
        assert_eq!(json_to_markdown(&json!("hi")), "hi");
        assert_eq!(json_to_markdown(&json!("")), "_(empty)_");
    }

    #[test]
    fn flat_object_becomes_a_field_value_table() {
        let markdown = json_to_markdown(&json!({"pv": 1.000265, "ki_probability": 0.1127}));
        assert!(markdown.starts_with("| Field | Value |"), "{markdown}");
        assert!(markdown.contains("| pv | 1.000265 |"), "{markdown}");
        assert!(markdown.contains("| ki_probability | 0.1127 |"), "{markdown}");
    }

    #[test]
    fn array_of_records_becomes_one_table() {
        let markdown = json_to_markdown(&json!([
            {"id": "daily_global_ko", "probability": 0.1},
            {"id": "final_fixing_eki", "probability": 0.1127},
        ]));
        assert!(markdown.starts_with("| id | probability |"), "{markdown}");
        assert!(markdown.contains("| final_fixing_eki | 0.1127 |"), "{markdown}");
    }

    #[test]
    fn heterogeneous_records_union_their_columns() {
        let markdown = json_to_markdown(&json!([{"a": 1}, {"b": 2}]));
        assert!(markdown.starts_with("| a | b |"), "{markdown}");
        assert!(markdown.contains("| 1 | — |"), "{markdown}");
    }

    #[test]
    fn nested_objects_are_headed_and_recursed() {
        let markdown = json_to_markdown(&json!({
            "run_id": "sha256:abc",
            "legs": {"put": 0.02, "coupon": 0.3},
        }));
        assert!(markdown.contains("| run_id | sha256:abc |"), "{markdown}");
        assert!(markdown.contains("**legs**"), "{markdown}");
        assert!(markdown.contains("| put | 0.02 |"), "{markdown}");
    }

    #[test]
    fn pipes_in_values_are_escaped() {
        let markdown = json_to_markdown(&json!({"cond": "a || b"}));
        assert!(markdown.contains("a \\|\\| b"), "{markdown}");
    }

    #[test]
    fn tool_output_prefers_json_then_falls_back_to_a_fence() {
        assert!(tool_output_to_markdown("{\"pv\": 1}").contains("| pv | 1 |"));
        assert!(tool_output_to_markdown("not json").starts_with("```"));
        assert_eq!(tool_output_to_markdown("   "), "_(empty)_");
    }
}
