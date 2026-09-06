use serde_json::Value;

use super::types::NodeOutput;

pub fn interpolate(template: &str, prev: &NodeOutput) -> String {
    // Expand {{output}} to a sentinel first so JSON replacements that happen
    // to contain "{{output}}" are not scanned as templates, then restore text
    // last so prev.text is never scanned for {{json.*}}.
    const SENTINEL: &str = "\u{0}OUTPUT\u{0}";
    let mut expanded = String::new();
    let mut rest = template;
    // Resolve node references once, without interpreting templates inside data.
    let mut values = Vec::new();
    while let Some(start) = rest.find("{{nodes.") {
        let Some(end) = rest[start..].find("}}") else { break };
        expanded.push_str(&rest[..start]);
        let reference = &rest[start + 8..start + end];
        let value = reference.split_once('#').and_then(|(id, pointer)|
            prev.sources.get(id).and_then(|value| value.pointer(pointer)));
        let text = match value {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(value) => value.to_string(),
        };
        expanded.push_str(&format!("\u{0}NODE{}\u{0}", values.len()));
        values.push(text);
        rest = &rest[start + end + 2..];
    }
    expanded.push_str(rest);
    let mut out = expanded.replace("{{output}}", SENTINEL);
    let mut search_from = 0;
    while let Some(start) = out[search_from..].find("{{json.") {
        let abs = search_from + start;
        let Some(end) = out[abs + 7..].find("}}") else {
            break;
        };
        let path = &out[abs + 7..abs + 7 + end];
        if path.is_empty() || !path.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            search_from = abs + 7;
            continue;
        }
        let replacement = lookup_json(&prev.json, path).unwrap_or_default();
        let replace_end = abs + 7 + end + 2;
        out.replace_range(abs..replace_end, &replacement);
        search_from = abs + replacement.len();
    }
    out = out.replace(SENTINEL, &prev.text);
    for (index, value) in values.iter().enumerate() {
        out = out.replace(&format!("\u{0}NODE{index}\u{0}"), value);
    }
    out
}

/// Explicit node references must resolve; never silently send empty fields to actions.
pub fn check_references(value: &Value, prev: &NodeOutput) -> Result<(), String> {
    match value {
        Value::String(template) => {
            let mut rest = template.as_str();
            while let Some(start) = rest.find("{{nodes.") {
                let end = rest[start..].find("}}").ok_or("unclosed node reference")?;
                let reference = &rest[start + 8..start + end];
                let (id, pointer) = reference.split_once('#').ok_or("invalid node reference")?;
                if prev.sources.get(id).and_then(|value| value.pointer(pointer)).is_none() {
                    return Err(format!("node reference is unavailable: {reference}; run the upstream path or use recorded input"));
                }
                rest = &rest[start + end + 2..];
            }
        }
        Value::Array(items) => for item in items { check_references(item, prev)?; },
        Value::Object(fields) => for (key, value) in fields {
            if key != "label" { check_references(value, prev)?; }
        },
        _ => {}
    }
    Ok(())
}

fn lookup_json(value: &Value, path: &str) -> Option<String> {
    let mut current = value;
    for part in path.split('.') {
        current = if current.is_array() { current.get(part.parse::<usize>().ok()?)? } else { current.get(part)? };
    }
    match current {
        Value::Null => Some(String::new()),
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

pub fn eval_if(op: &str, expected: &str, actual: &str) -> bool {
    match op {
        "equals" => actual.trim() == expected.trim(),
        "notEmpty" => !actual.trim().is_empty(),
        _ => actual.contains(expected),
    }
}

pub fn node_disabled(data: &Value) -> bool {
    data.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replaces_output_and_json_fields() {
        let prev = NodeOutput::with_json(
            "hello world",
            json!({ "text": "hello world", "status": 200, "nested": { "ok": true } }),
        );
        assert_eq!(interpolate("got {{output}}", &prev), "got hello world");
        assert_eq!(interpolate("{{json.status}}", &prev), "200");
        assert_eq!(interpolate("flag={{json.nested.ok}}", &prev), "flag=true");
    }

    #[test]
    fn does_not_rescan_placeholders_inside_output_text() {
        let prev = NodeOutput::with_json(
            "see {{json.status}}",
            json!({ "status": 200 }),
        );
        assert_eq!(interpolate("{{output}}", &prev), "see {{json.status}}");
        assert_eq!(
            interpolate("{{output}} / {{json.status}}", &prev),
            "see {{json.status}} / 200"
        );
    }

    #[test]
    fn does_not_expand_output_placeholder_inside_json_values() {
        let prev = NodeOutput::with_json(
            "HELLO",
            json!({ "note": "see {{output}}" }),
        );
        assert_eq!(interpolate("{{json.note}}", &prev), "see {{output}}");
    }

    #[test]
    fn if_ops() {
        assert!(eval_if("contains", "ok", "looks ok to me"));
        assert!(eval_if("equals", "yes", "yes"));
        assert!(!eval_if("equals", "yes", "no"));
        assert!(eval_if("notEmpty", "", "x"));
        assert!(!eval_if("notEmpty", "", "  "));
    }

    #[test]
    fn node_references_support_arrays_escaped_keys_and_never_expand_data() {
        let mut input = NodeOutput::from_text("previous");
        input.sources.insert("node-id".into(), serde_json::json!({
            "text": "{{output}}", "json": { "商品/规格~": [{ "name": "蓝色" }] }
        }));
        let template = "{{nodes.node-id#/text}} / {{nodes.node-id#/json/商品~1规格~0/0/name}}";
        assert!(check_references(&serde_json::json!(template), &input).is_ok());
        assert_eq!(interpolate(template, &input), "{{output}} / 蓝色");
        assert!(check_references(&serde_json::json!("{{nodes.other#/text}}"), &input).is_err());
        assert!(check_references(&serde_json::json!("{{nodes.node-id#/json/missing}}"), &input).is_err());
    }

    #[test]
    fn incomplete_reference_is_not_duplicated_and_is_rejected() {
        let input = NodeOutput::from_text("previous");
        let template = "prefix {{nodes.missing";
        assert_eq!(interpolate(template, &input), template);
        assert!(check_references(&serde_json::json!(template), &input).is_err());
    }
}
