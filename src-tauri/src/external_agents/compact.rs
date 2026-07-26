use tauri::{AppHandle, State};

use crate::chat::types::Conversation;
use crate::external_agents::context::compute_external_context_state_with_probe;
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::run::run_external_cli_slash_command;
use crate::external_agents::workspace::resolve_effective_cwd;
use crate::state::AppState;

pub fn compact_prompt_for_agent(agent_id: &str) -> Option<&'static str> {
    match agent_id {
        // codex: "/compact" is intercepted in CodexAppServerSession::run_turn and sent as the
        // real `thread/compact/start` RPC (as prompt text the model would just role-play it).
        "pi" | "claude" | "opencode" | "grok" | "codex" => Some("/compact"),
        _ => None,
    }
}

pub async fn request_external_compaction(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation: &mut Conversation,
) -> Result<(), String> {
    let agent_id = conversation
        .agent_runtime
        .external_agent_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "未选择外部 Agent".to_string())?;
    let compact_prompt = compact_prompt_for_agent(&agent_id).ok_or_else(|| {
        format!(
            "{} 不支持从 Kivio 手动触发压缩，请在该 CLI 内使用其自带的 context 命令。",
            get_agent_def(&agent_id)
                .map(|def| def.name)
                .unwrap_or(agent_id.as_str())
        )
    })?;

    run_external_cli_slash_command(app, state, conversation, compact_prompt).await?;

    conversation.context_state.summary = None;
    conversation.context_state.compression_count = conversation
        .context_state
        .compression_count
        .saturating_add(1);
    conversation.context_state.last_compressed_at = Some(chrono::Local::now().timestamp());
    conversation.context_state.warning = None;

    let cwd = resolve_effective_cwd(app, &conversation.id, conversation.project_id.as_deref())?;
    conversation.context_state = compute_external_context_state_with_probe(
        conversation,
        true,
        None,
        None,
        Some(&cwd),
        Some(&cwd),
    )
    .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_prompt_supported_for_pi_and_claude() {
        assert_eq!(compact_prompt_for_agent("pi"), Some("/compact"));
        assert_eq!(compact_prompt_for_agent("claude"), Some("/compact"));
        assert_eq!(compact_prompt_for_agent("grok"), Some("/compact"));
        assert_eq!(compact_prompt_for_agent("codex"), Some("/compact"));
        assert!(compact_prompt_for_agent("kimi").is_none());
    }
}
