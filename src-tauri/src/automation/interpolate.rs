use serde_json::Value;

use super::types::NodeOutput;

pub fn interpolate(template: &str, prev: &NodeOutput) -> String {
    // Replace {{json.*}} on the template first so text from {{output}} is not
    // scanned for more placeholders.
    let mut out = template.to_string();
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
    out.replace("{{output}}", &prev.text)
}

fn lookup_json(value: &Value, path: &str) -> Option<String> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
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
    fn if_ops() {
        assert!(eval_if("contains", "ok", "looks ok to me"));
        assert!(eval_if("equals", "yes", "yes"));
        assert!(!eval_if("equals", "yes", "no"));
        assert!(eval_if("notEmpty", "", "x"));
        assert!(!eval_if("notEmpty", "", "  "));
    }
}
