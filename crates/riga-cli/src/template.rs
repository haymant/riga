// Pre-filled trade/market templates and the quick-adjust path for the TUI and
// headless runs.
//
// A template is the engine's own `PriceRequest` (the shape `price_trade` takes
// and every other riga-flow fixture tool takes as its `request`). The engine is
// the authority on validity: the CLI only patches fields and reports whatever
// the tool returns.

use serde_json::Value;

/// The shipped worst-of Phoenix baseline, authored from the termsheet1
/// reconciliation record (30k paths, seed 1729). `price_trade` accepts it
/// unchanged.
pub const DEFAULT_TEMPLATE: &str = include_str!("../templates/termsheet1.request.json");

/// The baseline template as a JSON value.
pub fn default_template() -> Value {
    serde_json::from_str(DEFAULT_TEMPLATE).expect("the embedded template is valid JSON")
}

/// Merge a composed request into a tool input. **Every** riga-flow tool —
/// including `price_trade` — takes the request as a `request` field
/// (`PriceTradeParams`/`PricePutParams` both wrap it), so the request is always
/// injected rather than replacing the input.
pub fn compose_input(_tool: &str, base: Option<Value>, request: Option<Value>) -> Value {
    let base = base.unwrap_or_else(|| serde_json::json!({}));
    match request {
        None => base,
        Some(request) => match base {
            Value::Object(mut map) => {
                map.insert("request".to_string(), request);
                Value::Object(map)
            }
            other => other,
        },
    }
}

/// Apply `--set path=value` overrides. Paths are dot-separated with optional
/// `[index]` steps, e.g. `trade.strike=0.6`, `market.vols[0]=0.48`,
/// `market.correlation[0][1]=0.3`.
///
/// The value is parsed as JSON when it looks like JSON (`0.6`, `true`, `"x"`,
/// `[1,2]`); otherwise it is taken as a bare string.
pub fn apply_overrides(root: &mut Value, overrides: &[String]) -> Result<(), String> {
    for override_ in overrides {
        let (path, raw) = override_
            .split_once('=')
            .ok_or_else(|| format!("--set expects path=value, got '{override_}'"))?;
        let value = parse_value(raw.trim());
        set_path(root, path.trim(), value)?;
    }
    Ok(())
}

fn parse_value(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(value) => value,
        Err(_) => Value::String(raw.to_string()),
    }
}

fn set_path(root: &mut Value, path: &str, value: Value) -> Result<(), String> {
    let steps = parse_path(path)?;
    set_at(root, &steps, path, value)
}

fn set_at(node: &mut Value, steps: &[Step], path: &str, value: Value) -> Result<(), String> {
    let (step, rest) = steps
        .split_first()
        .ok_or_else(|| format!("'{path}' is empty"))?;
    let last = rest.is_empty();
    match step {
        Step::Key(key) => {
            let map = node
                .as_object_mut()
                .ok_or_else(|| format!("'{path}': '{key}' is not inside an object"))?;
            if last {
                map.insert(key.clone(), value);
                return Ok(());
            }
            let child = map
                .entry(key.clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            set_at(child, rest, path, value)
        }
        Step::Index(index) => {
            let items = node
                .as_array_mut()
                .ok_or_else(|| format!("'{path}': [{index}] is not inside an array"))?;
            let len = items.len();
            if *index >= len {
                return Err(format!("'{path}': index {index} is out of range (len {len})"));
            }
            if last {
                items[*index] = value;
                return Ok(());
            }
            set_at(&mut items[*index], rest, path, value)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(usize),
}

fn parse_path(path: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for segment in path.split('.') {
        if segment.is_empty() {
            return Err(format!("'{path}': empty path segment"));
        }
        let mut rest = segment;
        let mut first = true;
        loop {
            let (head, tail) = match rest.find('[') {
                Some(index) => (&rest[..index], &rest[index..]),
                None => (rest, ""),
            };
            if !head.is_empty() {
                if !first {
                    return Err(format!("'{path}': expected '[' after '{head}'"));
                }
                steps.push(Step::Key(head.to_string()));
            }
            if tail.is_empty() {
                break;
            }
            let close = tail
                .find(']')
                .ok_or_else(|| format!("'{path}': unclosed '[' in '{segment}'"))?;
            let index: usize = tail[1..close]
                .parse()
                .map_err(|_| format!("'{path}': '{tail}' is not a valid index"))?;
            steps.push(Step::Index(index));
            rest = &tail[close + 1..];
            first = false;
            if rest.is_empty() {
                break;
            }
        }
    }
    if steps.is_empty() {
        return Err(format!("'{path}' is empty"));
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_embedded_template_is_a_price_request() {
        let template = default_template();
        assert!(template.get("trade").and_then(|t| t.get("strike")).is_some());
        assert!(template.get("market").and_then(|m| m.get("correlation")).is_some());
        assert_eq!(template["trade"]["strike"], json!(0.78));
    }

    #[test]
    fn numeric_boolean_string_and_array_values_parse_as_json() {
        let mut root = default_template();
        apply_overrides(
            &mut root,
            &[
                "trade.strike=0.6".to_string(),
                "trade.memory_coupon=false".to_string(),
                "trade.trade_id=demo".to_string(),
                "market.spots[0]=250".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(root["trade"]["strike"], json!(0.6));
        assert_eq!(root["trade"]["memory_coupon"], json!(false));
        assert_eq!(root["trade"]["trade_id"], json!("demo"));
        assert_eq!(root["market"]["spots"][0], json!(250));
    }

    #[test]
    fn an_unparseable_value_becomes_a_string() {
        let mut root = default_template();
        apply_overrides(&mut root, &["trade.trade_id=not json".to_string()]).unwrap();
        assert_eq!(root["trade"]["trade_id"], json!("not json"));
    }

    #[test]
    fn nested_indices_patch_the_correlation_matrix() {
        let mut root = default_template();
        apply_overrides(&mut root, &["market.correlation[0][1]=0.5".to_string()]).unwrap();
        assert_eq!(root["market"]["correlation"][0][1], json!(0.5));
    }

    #[test]
    fn a_path_into_a_new_object_creates_it() {
        let mut root = json!({});
        apply_overrides(&mut root, &["trade.strike=0.5".to_string()]).unwrap();
        assert_eq!(root["trade"]["strike"], json!(0.5));
    }

    #[test]
    fn bad_overrides_are_reported() {
        let mut root = default_template();
        assert!(apply_overrides(&mut root, &["trade.strike".to_string()]).is_err());
        assert!(apply_overrides(&mut root, &["trade.strike[0]=1".to_string()]).is_err());
        assert!(apply_overrides(&mut root, &["market.spots[9]=1".to_string()]).is_err());
    }

    #[test]
    fn every_tool_takes_the_request_as_a_request_field() {
        let request = json!({"trade": {"strike": 0.6}});
        // `price_trade` takes `{request: …}` too (PriceTradeParams), not a bare request.
        assert_eq!(
            compose_input("price_trade", Some(json!({})), Some(request.clone())),
            json!({"request": request})
        );
        assert_eq!(
            compose_input("query_execution_net", Some(json!({})), Some(request.clone())),
            json!({"request": request})
        );
        // Other params survive alongside the injected request.
        assert_eq!(
            compose_input("query_path_cube", Some(json!({"percentile": 0.05})), Some(request.clone())),
            json!({"percentile": 0.05, "request": request})
        );
        // No request → the base input is untouched (baseline fixture behaviour).
        assert_eq!(compose_input("price_put", Some(json!({})), None), json!({}));
    }
}
