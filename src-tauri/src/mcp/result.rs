//! MCP 工具结果 → Kivio 内部结构的映射。
//!
//! 这里的逻辑与 wire 实现无关：输入是 MCP `tools/call` 结果的 JSON（wire 形状），
//! 输出是 `McpToolCallResult`。rmcp 的 `CallToolResult` 序列化出来就是同一个形状
//! （`isError` / `structuredContent` / `content[].type|text|data|mimeType`），
//! 所以换 SDK 时这一层一行都不用改 —— 调用方传 `serde_json::to_value(result)` 进来即可。

use base64::{engine::general_purpose, Engine as _};
use serde_json::Value;

use super::types::{ChatToolArtifact, McpToolCallResult};

pub(crate) fn parse_tool_result(value: Value) -> McpToolCallResult {
    let is_error = value
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let structured_content = value.get("structuredContent").cloned();

    let mut artifacts: Vec<ChatToolArtifact> = Vec::new();
    let content = value
        .get("content")
        .and_then(|content| content.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| content_block_text(item, &mut artifacts))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| {
            if artifacts.is_empty() {
                compact_json(&value, 4000)
            } else {
                String::new()
            }
        });

    McpToolCallResult {
        content,
        is_error,
        raw: value,
        artifacts,
        structured_content,
        follow_up_user_messages: Vec::new(),
    }
}

/// Maps a single MCP content block to its model-facing text. Image blocks are
/// pushed onto `artifacts` and represented in text by a `[image: <mime>]`
/// placeholder so the model knows an image was produced without inlining bytes.
fn content_block_text(item: &Value, artifacts: &mut Vec<ChatToolArtifact>) -> Option<String> {
    let block_type = item.get("type").and_then(|value| value.as_str());
    if block_type == Some("image") {
        if let Some(artifact) = image_block_to_artifact(item, artifacts.len()) {
            let placeholder = format!("[image: {}]", artifact.mime_type);
            artifacts.push(artifact);
            return Some(placeholder);
        }
        return None;
    }
    item.get("text")
        .and_then(|text| text.as_str())
        .map(|text| text.to_string())
        .or_else(|| {
            item.get("resource")
                .map(|resource| compact_json(resource, 4000))
        })
}

/// Builds a `ChatToolArtifact` from an MCP `image` content block
/// (`{ "type": "image", "data": "<base64>", "mimeType": "image/png" }`).
fn image_block_to_artifact(item: &Value, index: usize) -> Option<ChatToolArtifact> {
    let data = item.get("data").and_then(|value| value.as_str())?;
    if data.trim().is_empty() {
        return None;
    }
    let mime_type = item
        .get("mimeType")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("image/png")
        .to_string();
    let size_bytes = general_purpose::STANDARD
        .decode(data.trim())
        .ok()
        .map(|bytes| bytes.len() as u64);
    let extension = mime_type
        .rsplit('/')
        .next()
        .filter(|ext| !ext.is_empty())
        .unwrap_or("png");
    // 文件名必须全局唯一：同一消息里多次 MCP 截图若都叫 mcp-image-1.png，
    // 重载后按 basename 解析会互相覆盖，且外置缩略图只显示 256px 小图。
    let unique = uuid::Uuid::new_v4().to_string();
    let short = unique.get(..8).unwrap_or(unique.as_str());
    Some(ChatToolArtifact {
        id: None,
        name: format!("mcp-image-{}-{}.{}", index + 1, short, extension),
        mime_type: mime_type.clone(),
        data_url: format!("data:{};base64,{}", mime_type, data.trim()),
        size_bytes,
        path: None,
    })
}

pub(crate) fn compact_json(value: &Value, max_chars: usize) -> String {
    let raw = serde_json::to_string(value).unwrap_or_else(|_| String::new());
    raw.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_result_preserves_structured_content() {
        let result = parse_tool_result(serde_json::json!({
            "content": [{ "type": "text", "text": "summary" }],
            "structuredContent": {
                "items": [{ "title": "A" }]
            },
            "isError": false
        }));

        assert_eq!(result.content, "summary");
        assert_eq!(
            result.structured_content.as_ref(),
            Some(&serde_json::json!({ "items": [{ "title": "A" }] }))
        );
        assert!(!result.is_error);
    }

    #[test]
    fn parse_tool_result_maps_image_to_artifact() {
        // "hello" base64 → aGVsbG8=
        let result = parse_tool_result(serde_json::json!({
            "content": [
                { "type": "text", "text": "here is a chart" },
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" }
            ],
            "isError": false
        }));

        assert_eq!(result.artifacts.len(), 1);
        let artifact = &result.artifacts[0];
        assert_eq!(artifact.mime_type, "image/png");
        assert_eq!(artifact.data_url, "data:image/png;base64,aGVsbG8=");
        assert_eq!(artifact.size_bytes, Some(5));
        assert!(artifact.name.ends_with(".png"));
        // Text content keeps the prose and inserts a placeholder for the image.
        assert_eq!(result.content, "here is a chart\n[image: image/png]");
        assert!(!result.is_error);
    }

    #[test]
    fn parse_tool_result_image_only_has_empty_content() {
        let result = parse_tool_result(serde_json::json!({
            "content": [
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/jpeg" }
            ]
        }));

        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.artifacts[0].mime_type, "image/jpeg");
        assert!(result.artifacts[0].name.ends_with(".jpeg"));
        assert_eq!(result.content, "[image: image/jpeg]");
    }

    // ---------------------------------------------------------------------
    // rmcp 形状契约：以下测试用真的 rmcp 类型构造，序列化后喂进上面的解析器。
    // rmcp 若改了 serde 命名（比如 isError → is_error），这几个测试会红，
    // 而不是让工具结果在运行时静默退化成一坨 JSON。
    // ---------------------------------------------------------------------

    #[test]
    fn rmcp_call_tool_result_text_matches_wire_shape() {
        let result = rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text(
            "summary",
        )]);
        let parsed = parse_tool_result(serde_json::to_value(&result).expect("serialize"));

        assert_eq!(parsed.content, "summary");
        assert!(!parsed.is_error);
    }

    #[test]
    fn rmcp_call_tool_result_error_flag_survives() {
        let result = rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::text(
            "boom",
        )]);
        let parsed = parse_tool_result(serde_json::to_value(&result).expect("serialize"));

        assert_eq!(parsed.content, "boom");
        assert!(parsed.is_error, "isError 必须是 camelCase 才认得出来");
    }

    #[test]
    fn rmcp_call_tool_result_image_becomes_artifact() {
        let result = rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::image(
            "aGVsbG8=",
            "image/png",
        )]);
        let parsed = parse_tool_result(serde_json::to_value(&result).expect("serialize"));

        assert_eq!(parsed.artifacts.len(), 1, "mimeType 必须是 camelCase");
        assert_eq!(parsed.artifacts[0].data_url, "data:image/png;base64,aGVsbG8=");
        assert_eq!(parsed.content, "[image: image/png]");
    }

    #[test]
    fn rmcp_call_tool_result_structured_content_survives() {
        let result = rmcp::model::CallToolResult::structured(serde_json::json!({ "count": 2 }));
        let parsed = parse_tool_result(serde_json::to_value(&result).expect("serialize"));

        assert_eq!(
            parsed.structured_content.as_ref(),
            Some(&serde_json::json!({ "count": 2 })),
            "structuredContent 必须是 camelCase"
        );
    }

    #[test]
    fn rmcp_tool_converts_into_mcp_tool() {
        // 工具 schema 也走同一个「序列化再反序列化」的路子，所以 inputSchema /
        // outputSchema 的命名同样是契约。
        let tool = rmcp::model::Tool::new(
            "echo",
            "Echo text back",
            std::sync::Arc::new(
                serde_json::json!({ "type": "object", "properties": { "text": { "type": "string" } } })
                    .as_object()
                    .cloned()
                    .expect("object"),
            ),
        );
        let ours: super::super::types::McpTool =
            serde_json::from_value(serde_json::to_value(&tool).expect("serialize"))
                .expect("rmcp Tool 必须能直接读成 McpTool");

        assert_eq!(ours.name, "echo");
        assert_eq!(ours.description, "Echo text back");
        assert_eq!(ours.input_schema["properties"]["text"]["type"], "string");
    }
}
