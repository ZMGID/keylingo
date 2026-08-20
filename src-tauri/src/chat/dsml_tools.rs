//! Parse DeepSeek-style DSML tool markup that some providers emit in `content`
//! instead of OpenAI `tool_calls`.

use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDsmlCall {
    pub name: String,
    pub arguments: Map<String, Value>,
}

pub fn contains_dsml_tool_markup(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("dsml") && (lower.contains("invoke") || lower.contains("tool_calls"))
}

pub fn strip_dsml_tool_markup(content: &str) -> String {
    if !contains_dsml_tool_markup(content) {
        return content.to_string();
    }
    let lower = content.to_ascii_lowercase();
    let Some(dsml_idx) = lower.find("dsml") else {
        return content.to_string();
    };
    let start = content[..dsml_idx].rfind('<').unwrap_or(0);
    let end = lower[dsml_idx..]
        .rfind('>')
        .map(|rel| dsml_idx + rel + 1)
        .unwrap_or(content.len());
    let mut out = String::new();
    let head = content[..start].trim();
    let tail = content[end.min(content.len())..].trim();
    if !head.is_empty() {
        out.push_str(head);
    }
    if !head.is_empty() && !tail.is_empty() {
        out.push_str("\n\n");
    }
    if !tail.is_empty() {
        out.push_str(tail);
    }
    out.trim().to_string()
}

pub fn extract_dsml_tool_calls(content: &str) -> Vec<ParsedDsmlCall> {
    if !contains_dsml_tool_markup(content) {
        return Vec::new();
    }
    let lower = content.to_ascii_lowercase();
    let mut calls = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = lower[search_from..].find("invoke") {
        let invoke_start = search_from + rel;
        if invoke_start > 0 && content.as_bytes().get(invoke_start - 1) == Some(&b'/') {
            search_from = invoke_start + 6;
            continue;
        }
        let slice = &content[invoke_start..];
        let after_invoke = slice.get(6..).unwrap_or("");
        let Some(tool_name) = parse_quoted_attr(after_invoke, "name") else {
            search_from = invoke_start + 6;
            continue;
        };
        let block_end = find_invoke_block_end(&content[invoke_start..], &lower[invoke_start..])
            .map(|len| invoke_start + len)
            .unwrap_or(content.len());
        let block = &content[invoke_start..block_end];
        let arguments = parse_parameters(block);
        calls.push(ParsedDsmlCall {
            name: tool_name,
            arguments,
        });
        search_from = block_end;
    }
    calls
}

fn find_invoke_block_end(content: &str, lower: &str) -> Option<usize> {
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find("</") {
        let close_start = search + rel;
        let tail = &lower[close_start..];
        let close_tag_end = tail.find('>').unwrap_or(tail.len());
        let close_tag = &tail[..close_tag_end];
        if close_tag.contains("invoke") {
            if let Some(gt) = content[close_start..].find('>') {
                return Some(close_start + gt + 1);
            }
        }
        search = close_start + 2;
    }
    Some(content.len())
}

fn is_closing_parameter_tag(content: &str, param_start: usize) -> bool {
    let Some(lt) = content[..param_start].rfind('<') else {
        return false;
    };
    content.as_bytes().get(lt + 1) == Some(&b'/')
}

fn parse_quoted_attr(slice: &str, attr: &str) -> Option<String> {
    let lower = slice.to_ascii_lowercase();
    let needle = format!("{attr}=\"");
    let start = lower.find(&needle)? + needle.len();
    let rest = &slice[start..];
    let end = rest.find('"')?;
    Some(rest[..end].trim().to_string())
}

fn find_next_open_parameter(block: &str, lower: &str, from: usize) -> Option<usize> {
    let mut search = from;
    while let Some(rel) = lower[search..].find("parameter") {
        let idx = search + rel;
        if !is_closing_parameter_tag(block, idx) {
            return Some(idx);
        }
        search = idx + 1;
    }
    None
}

fn parse_parameters(block: &str) -> Map<String, Value> {
    let lower = block.to_ascii_lowercase();
    let mut args = Map::new();
    let mut from = 0usize;
    while let Some(param_start) = find_next_open_parameter(block, &lower, from) {
        let slice = &block[param_start..];
        let after_kw = slice.get("parameter".len()..).unwrap_or("");
        let Some(param_name) = parse_quoted_attr(after_kw, "name") else {
            from = param_start + 1;
            continue;
        };
        let value = parse_parameter_value(slice);
        if param_name == "args" {
            if let Ok(parsed) = serde_json::from_str::<Value>(&value) {
                args.insert(param_name, parsed);
            } else {
                args.insert(param_name, Value::String(value));
            }
        } else if param_name.ends_with("_json") || value.starts_with('{') || value.starts_with('[')
        {
            if let Ok(parsed) = serde_json::from_str::<Value>(&value) {
                args.insert(param_name.trim_end_matches("_json").to_string(), parsed);
            } else {
                args.insert(param_name, Value::String(value));
            }
        } else {
            args.insert(param_name, Value::String(value));
        }
        from = param_start + "parameter".len();
    }
    args
}

fn parse_parameter_value(slice: &str) -> String {
    let Some(gt) = slice.find('>') else {
        return String::new();
    };
    let rest = &slice[gt + 1..];
    let end = rest.find("</").unwrap_or(rest.len());
    rest[..end].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        "<|DSML|tool_calls><|DSML|invoke name=\"bash\">",
        "<|DSML|parameter name=\"cwd\" string=\"false\">",
        r#""/tmp""#,
        "</|DSML|parameter>",
        "<|DSML|parameter name=\"command\" string=\"true\">echo 1</|DSML|parameter>",
        "</|DSML|invoke></|DSML|tool_calls>",
    );

    #[test]
    fn extracts_multi_param_tool_from_dsml() {
        let calls = extract_dsml_tool_calls(SAMPLE);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert_eq!(
            calls[0].arguments.get("command").and_then(|v| v.as_str()),
            Some("echo 1")
        );
        assert!(calls[0]
            .arguments
            .get("cwd")
            .and_then(|v| v.as_str())
            .is_some());
    }

    #[test]
    fn strip_removes_dsml_markup() {
        let stripped = strip_dsml_tool_markup(&format!("hello {SAMPLE}"));
        assert!(stripped.starts_with("hello"));
        assert!(!stripped.contains("DSML"));
    }

    #[test]
    fn extracts_fullwidth_dsml_delimiters() {
        const FULLWIDTH: &str = concat!(
            "<｜｜DSML｜｜tool_calls><｜｜DSML｜｜invoke name=\"skill\">",
            "<｜｜DSML｜｜parameter name=\"name\" string=\"true\">pdf</｜｜DSML｜｜parameter>",
            "</｜｜DSML｜｜invoke></｜｜DSML｜｜tool_calls>",
        );
        let calls = extract_dsml_tool_calls(FULLWIDTH);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "skill");
    }
}
