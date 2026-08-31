use std::{path::PathBuf, time::Instant};

use tauri::{AppHandle, State};

use crate::chat::agent::execute::truncate_chars;
use crate::mcp::types::ChatToolArtifact;
use crate::settings::{ModelProvider, Settings};
use crate::state::AppState;

use super::interaction::{emit_chat_stream_delta, wait_for_chat_cancel};
use super::messages::{plain_text_segment, push_assistant_message};
use super::{agent_run_entry_label, Conversation};

const DIRECT_IMAGE_GENERATION_PENDING: &str = "[[KIVIO_DIRECT_IMAGE_GENERATION_PENDING]]";

pub(super) async fn complete_direct_image_generation_reply(
    app: &AppHandle,
    state: &State<'_, AppState>,
    settings: &Settings,
    provider: &ModelProvider,
    conversation: &mut Conversation,
    title_from_first_user: Option<&str>,
    last_user_api_content: Option<&str>,
    last_user_image_paths: &[PathBuf],
    active_skill_id: Option<&str>,
    run_id: &str,
    assistant_message_id: String,
    run_generation: u64,
    retry_attempts: usize,
    entry: crate::chat::agent::AgentRunEntry,
) -> Result<(), String> {
    let prompt = direct_image_generation_prompt(conversation, last_user_api_content)?;
    let arguments = serde_json::json!({
        "prompt": prompt,
        "size": "auto",
        "quality": "auto",
        "n": 1,
    });
    let input_images =
        crate::chat::image_generation::load_input_images_from_paths(last_user_image_paths)?;
    let started = Instant::now();
    emit_chat_stream_delta(
        app,
        run_id,
        DIRECT_IMAGE_GENERATION_PENDING,
        None,
        Some(&plain_text_segment(1000, DIRECT_IMAGE_GENERATION_PENDING)),
    );

    let model = conversation.model.clone();
    let result = tokio::select! {
        result = crate::chat::image_generation::generate_image_with_provider(
            state.inner(),
            provider,
            &model,
            &arguments,
            &input_images,
            retry_attempts,
            "Chat image generation",
        ) => result,
        _ = wait_for_chat_cancel(state.inner(), &conversation.id, run_generation) => {
            crate::chat::protocol::finish_run(
                app,
                run_id,
                "cancelled",
                "",
                conversation.revision,
            );
            return Err("cancelled".to_string());
        }
    };

    match result {
        Ok(output) if !output.is_error => {
            // 有图 → 渲染图片；模型只回文字（澄清/拒绝）无图 → 展示那段文字。
            let content = if output.artifacts.is_empty() {
                output.content.clone()
            } else {
                direct_image_generation_content(&output.artifacts)
            };
            let active_skill = active_skill_id
                .map(str::to_string)
                .or_else(|| conversation.active_skill_id.clone());
            push_assistant_message(
                app,
                state,
                settings,
                conversation,
                assistant_message_id,
                content.clone(),
                None,
                output.artifacts,
                Vec::new(),
                Vec::new(),
                vec![plain_text_segment(1000, content.as_str())],
                active_skill.as_deref(),
                title_from_first_user,
                Some(agent_run_entry_label(entry)),
                Some("completed"),
                None,
                None,
                None,
                // 直出图路径不经 agent 循环，没有降级兜底。
                None,
            )
            .await?;
            crate::chat::protocol::finish_run(
                app,
                run_id,
                "completed",
                &content,
                conversation.revision,
            );
            Ok(())
        }
        Ok(output) => {
            let err = output.content;
            eprintln!(
                "Direct image generation failed after {}ms: {err}",
                started.elapsed().as_millis()
            );
            crate::chat::protocol::finish_run(app, run_id, "error", "", conversation.revision);
            Err(err)
        }
        Err(err) => {
            eprintln!(
                "Direct image generation failed after {}ms: {err}",
                started.elapsed().as_millis()
            );
            crate::chat::protocol::finish_run(app, run_id, "error", "", conversation.revision);
            Err(err)
        }
    }
}

fn direct_image_generation_content(artifacts: &[ChatToolArtifact]) -> String {
    artifacts
        .iter()
        .map(|artifact| format!("![{}]({})", artifact.name, artifact.name))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn direct_image_generation_prompt(
    conversation: &Conversation,
    last_user_api_content: Option<&str>,
) -> Result<String, String> {
    let prompt = conversation
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.trim())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            last_user_api_content
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| "请输入要生成的图片描述。".to_string())?;
    Ok(truncate_chars(prompt, 8000))
}
