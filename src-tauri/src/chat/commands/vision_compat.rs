use std::path::Path;

use tauri::AppHandle;

use crate::chat::vision;
use crate::mcp;
use crate::settings::Settings;

// 历史拼装的唯一入口：send 与 regenerate 都最终走这里。
/// 失败/无视觉模型时逐级降级，始终返回一个可读的文本结果。
pub(crate) async fn read_image_as_tool_result(
    app: &AppHandle,
    settings: &Settings,
    conversation_id: &str,
    message_id: &str,
    path: &Path,
) -> Result<mcp::types::McpToolCallResult, String> {
    vision::read_image_as_tool_result(app, settings, conversation_id, message_id, path).await
}

pub(crate) async fn read_images_as_tool_result(
    app: &AppHandle,
    settings: &Settings,
    conversation_id: &str,
    message_id: &str,
    paths: &[std::path::PathBuf],
    overview: bool,
) -> Result<mcp::types::McpToolCallResult, String> {
    vision::read_images_as_tool_result(
        app,
        settings,
        conversation_id,
        message_id,
        paths,
        overview,
    )
    .await
}

/// R1：MCP 工具结果里的图片 artifact「直达模型」。通用于所有 MCP server（非
/// officecli 专属），复用 `read_image_as_tool_result` 已验证的两级策略：
/// ① 主模型支持视觉 → 把图片作为 follow-up user 消息直喂（`data_url_image_part`，
/// 不落盘）；② 纯文本主模型 → 落临时文件 `kivio-mcpimg-<uuid>.<ext>` 走辅助视觉
/// 模型做审查向分析（R2），把分析文字追加进 tool 结果的 content，随后删除临时
/// 文件。全程尽力而为：拿不到会话上下文、无可用视觉模型、分析失败等任何一步
/// 出错都原样保留 `[image: <mime>]` 占位符，不影响 MCP 工具调用本身的成败。
/// 仅对当前这一轮工具结果生效，不回填历史轮（调用方每轮都会重新执行）。
pub(crate) async fn attach_image_artifacts_for_model(
    app: &AppHandle,
    settings: &Settings,
    conversation_id: &str,
    message_id: &str,
    result: &mut mcp::types::McpToolCallResult,
) {
    vision::attach_image_artifacts_for_model(app, settings, conversation_id, message_id, result)
        .await;
}
