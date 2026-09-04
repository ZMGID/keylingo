use serde_json::Value;

use crate::api::extract_status_code;
use crate::chat::model::{pending_tool_calls_from_openai_message, PendingToolCall};

pub fn step_limit_system_message() -> Value {
    serde_json::json!({
        "role": "system",
        "content": "已达到本轮工具调用轮次上限。你现在必须根据对话中已有的工具返回结果直接给用户一个普通文本回答。不要再调用任何工具，不要输出 tool_calls、function_call、DSML、XML 或 JSON 工具调用标记。"
    })
}

pub fn extract_reasoning_content(message: &Value) -> Option<String> {
    message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn assistant_content_from_api_message(message: &Value) -> String {
    message
        .get("content")
        .and_then(|content| content.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn merge_reasoning(
    planning_parts: &[String],
    final_reasoning: Option<String>,
) -> Option<String> {
    let mut parts = planning_parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(reasoning) = final_reasoning
        .as_deref()
        .map(str::trim)
        .filter(|reasoning| !reasoning.is_empty())
    {
        parts.push(reasoning.to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

pub fn final_response_from_planning_message(
    message: &Value,
    planning_reasoning_parts: &[String],
) -> Result<(String, Option<String>), String> {
    final_response_from_planning_message_with_images(message, planning_reasoning_parts, false)
}

/// 同上，但允许「本轮出了图」时正文为空：hosted image generation（Responses
/// `image_generation_call`）的正文本就是空串——图即答案，此时空正文不是失败。
pub fn final_response_from_planning_message_with_images(
    message: &Value,
    planning_reasoning_parts: &[String],
    has_images: bool,
) -> Result<(String, Option<String>), String> {
    let response = sanitize_assistant_text_response(&assistant_content_from_api_message(message));
    if response.trim().is_empty() && !has_images {
        return Err(empty_assistant_response_error("Chat tools planning"));
    }
    let reasoning = merge_reasoning(planning_reasoning_parts, extract_reasoning_content(message));
    Ok((response, reasoning))
}

pub fn final_assistant_api_message(content: &str, reasoning: Option<&str>) -> Value {
    let mut message = serde_json::json!({
        "role": "assistant",
        "content": content,
    });
    if let Some(reasoning) = reasoning.map(str::trim).filter(|value| !value.is_empty()) {
        message["reasoning_content"] = Value::String(reasoning.to_string());
    }
    message
}

pub fn extract_tool_calls(message: &Value) -> Vec<PendingToolCall> {
    let from_api = pending_tool_calls_from_openai_message(message);
    if !from_api.is_empty() {
        return from_api;
    }
    let content = assistant_content_from_api_message(message);
    pending_tool_calls_from_dsml(&content)
}

pub fn pending_tool_calls_from_dsml(content: &str) -> Vec<PendingToolCall> {
    crate::chat::dsml_tools::extract_dsml_tool_calls(content)
        .into_iter()
        .map(|call| {
            let arguments = Value::Object(call.arguments);
            let arguments_raw =
                serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string());
            PendingToolCall {
                id: format!("tool_{}", uuid::Uuid::new_v4()),
                function_name: call.name,
                arguments,
                arguments_raw,
                arguments_parse_error: None,
                signature: None,
            }
        })
        .collect()
}

pub fn assistant_api_message_for_tool_calls(
    message: &Value,
    tool_calls: &[PendingToolCall],
) -> Value {
    if message
        .get("tool_calls")
        .and_then(|value| value.as_array())
        .is_some_and(|calls| !calls.is_empty())
    {
        return message.clone();
    }
    let mut rebuilt = serde_json::json!({
        "role": "assistant",
        "content": Value::Null,
        "tool_calls": tool_calls.iter().map(|call| {
            serde_json::json!({
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.function_name,
                    "arguments": call.arguments_raw,
                }
            })
        }).collect::<Vec<_>>(),
    });
    // DSML 重建会丢掉原生 `tool_calls` 之外的回放键。Responses 下一轮靠
    // `reasoning_items` 里的 encrypted_content 接上思考链，必须原样带过去。
    if let Some(items) = message.get("reasoning_items") {
        rebuilt["reasoning_items"] = items.clone();
    }
    rebuilt
}

pub fn sanitize_assistant_text_response(content: &str) -> String {
    let stripped = crate::chat::dsml_tools::strip_dsml_tool_markup(content);
    if stripped.is_empty() && crate::chat::dsml_tools::contains_dsml_tool_markup(content) {
        return String::new();
    }
    stripped
}

pub fn empty_assistant_response_error(scope: &str) -> String {
    format!("{scope} returned an empty assistant response")
}

pub fn is_tools_unsupported_error(err: &str) -> bool {
    let Some(code) = extract_status_code(err) else {
        return false;
    };
    if !matches!(code, 400 | 404 | 422 | 501) {
        return false;
    }
    let lower = err.to_ascii_lowercase();
    lower.contains("tools")
        || lower.contains("tool_choice")
        || lower.contains("tool_calls")
        || lower.contains("function calling")
        || lower.contains("function_call")
        || lower.contains("function call")
        || lower.contains("not support")
        || (code == 400 && lower.contains("tool"))
}

pub fn patch_system_message(messages: &mut [Value], prompt: &str) {
    if let Some(first) = messages.first_mut() {
        if first.get("role").and_then(|role| role.as_str()) == Some("system") {
            first["content"] = Value::String(prompt.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dsml_tool_calls() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": "<|DSML|tool_calls><|DSML|invoke name=\"skill\"><|DSML|parameter name=\"name\">doc</|DSML|parameter></|DSML|invoke></|DSML|tool_calls>"
        });

        assert_eq!(extract_tool_calls(&message).len(), 1);
    }

    #[test]
    fn step_limit_system_message_matches_existing_contract() {
        let message = step_limit_system_message();
        assert_eq!(
            message.get("role").and_then(|role| role.as_str()),
            Some("system")
        );
        assert!(message
            .get("content")
            .and_then(|content| content.as_str())
            .unwrap_or_default()
            .contains("工具调用轮次上限"));
    }

    #[test]
    fn merge_reasoning_keeps_planning_and_final_sections() {
        let reasoning = merge_reasoning(
            &[
                "  planning round one  ".to_string(),
                String::new(),
                "planning round two".to_string(),
            ],
            Some(" final answer reasoning ".to_string()),
        )
        .expect("reasoning should be merged");

        assert_eq!(
            reasoning,
            "planning round one\n\nplanning round two\n\nfinal answer reasoning"
        );
    }

    #[test]
    fn is_tools_unsupported_error_detects_provider_rejection_messages() {
        assert!(is_tools_unsupported_error(
            "Chat tools planning Error: 400 Bad Request - tools not supported (attempt 1/3)"
        ));
        assert!(is_tools_unsupported_error(
            "Chat tools planning Error: 422 Unprocessable Entity - invalid tool_choice (attempt 1/1)"
        ));
        assert!(is_tools_unsupported_error(
            "Chat tools planning Error: 400 Bad Request - function call is not supported (attempt 1/3)"
        ));
        assert!(is_tools_unsupported_error(
            "Chat tools planning Error: 400 Bad Request - tools[0]: unknown variant `function`, expected `web_search_20250305` or `web_search_20260209` (attempt 1/1)"
        ));
        assert!(!is_tools_unsupported_error(
            "Chat tools planning Error: 429 Too Many Requests - rate limited (attempt 1/3)"
        ));
        assert!(!is_tools_unsupported_error("network timeout"));
    }

    #[test]
    fn extract_tool_calls_parses_dsml_when_api_tool_calls_missing() {
        const SAMPLE: &str = concat!(
            "<|DSML|tool_calls><|DSML|invoke name=\"skill\">",
            "<|DSML|parameter name=\"name\" string=\"true\">pdf</|DSML|parameter>",
            "</|DSML|invoke></|DSML|tool_calls>",
        );
        let message = serde_json::json!({
            "role": "assistant",
            "content": SAMPLE,
        });

        let calls = extract_tool_calls(&message);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function_name, "skill");
        assert_eq!(
            calls[0]
                .arguments
                .get("name")
                .and_then(|value| value.as_str()),
            Some("pdf")
        );
    }

    #[test]
    fn extract_openai_tool_calls_preserves_invalid_arguments_error() {
        let message = serde_json::json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "call_write",
                "type": "function",
                "function": {
                    "name": "write_file",
                    "arguments": "{\"path\":\"/tmp/out.html\",\"content\":\"unterminated"
                }
            }]
        });

        let calls = extract_tool_calls(&message);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function_name, "write_file");
        assert!(calls[0].arguments.is_null());
        assert!(calls[0]
            .arguments_parse_error
            .as_deref()
            .unwrap_or_default()
            .contains("invalid or incomplete"));
    }

    #[test]
    fn empty_assistant_response_error_exposes_flow_failure() {
        let response = empty_assistant_response_error("Chat stream");

        assert_eq!(response, "Chat stream returned an empty assistant response");
    }

    #[test]
    fn planning_final_message_becomes_final_reply_without_second_request() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": "直接回答",
            "reasoning_content": "final thought"
        });

        let (response, reasoning) =
            final_response_from_planning_message(&message, &["plan thought".to_string()])
                .expect("planning final message should become final reply");

        assert_eq!(response, "直接回答");
        assert_eq!(reasoning.as_deref(), Some("plan thought\n\nfinal thought"));
    }

    #[test]
    fn planning_final_message_rejects_empty_text() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": "<|DSML|tool_calls></|DSML|tool_calls>"
        });

        let err = final_response_from_planning_message(&message, &[])
            .expect_err("empty planning final text should fail");

        assert_eq!(
            err,
            "Chat tools planning returned an empty assistant response"
        );
    }

    #[test]
    fn patch_system_message_replaces_first_system_entry() {
        let mut messages = vec![
            serde_json::json!({ "role": "system", "content": "old" }),
            serde_json::json!({ "role": "user", "content": "hi" }),
        ];

        patch_system_message(&mut messages, "new prompt");

        assert_eq!(
            messages[0].get("content").and_then(|value| value.as_str()),
            Some("new prompt")
        );
    }

    #[test]
    fn final_assistant_api_message_omits_reasoning_without_tool_calls() {
        let message = final_assistant_api_message("done", None);

        assert_eq!(
            message.get("role").and_then(|value| value.as_str()),
            Some("assistant")
        );
        assert_eq!(
            message.get("content").and_then(|value| value.as_str()),
            Some("done")
        );
        assert!(message.get("reasoning_content").is_none());
    }

    #[test]
    fn final_assistant_api_message_keeps_reasoning_when_requested() {
        let message = final_assistant_api_message("done", Some(" thinking "));

        assert_eq!(
            message
                .get("reasoning_content")
                .and_then(|value| value.as_str()),
            Some("thinking")
        );
    }

    #[test]
    fn dsml_rebuild_keeps_reasoning_items() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": "<|DSML|tool_calls></|DSML|tool_calls>",
            "reasoning_items": [{
                "model": "gpt-5.6",
                "item": { "id": "rs_1", "encrypted_content": "gAAA" }
            }]
        });
        let calls = vec![PendingToolCall {
            id: "call_1".to_string(),
            function_name: "skill".to_string(),
            arguments: serde_json::json!({ "name": "pdf" }),
            arguments_raw: "{\"name\":\"pdf\"}".to_string(),
            arguments_parse_error: None,
            signature: None,
        }];
        let rebuilt = assistant_api_message_for_tool_calls(&message, &calls);
        assert_eq!(rebuilt["tool_calls"][0]["id"], "call_1");
        assert_eq!(rebuilt["reasoning_items"][0]["item"]["id"], "rs_1");
        assert_eq!(
            rebuilt["reasoning_items"][0]["item"]["encrypted_content"],
            "gAAA"
        );
    }

    #[test]
    fn assistant_content_from_api_message_trims_missing_or_null_content() {
        assert_eq!(
            assistant_content_from_api_message(&serde_json::json!({
                "role": "assistant",
                "content": " answer "
            })),
            "answer"
        );
        assert_eq!(
            assistant_content_from_api_message(&serde_json::json!({
                "role": "assistant",
                "content": null
            })),
            ""
        );
    }
}
