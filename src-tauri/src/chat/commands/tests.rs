use std::{collections::HashMap, path::Path};

use crate::chat::agent::prepare as agent_prepare;
use crate::chat::attachments::compose_user_content_for_api;
use crate::chat::model::{
    openai_messages_from_model_messages, MessagePart, ModelMessage, ModelRole,
};
use crate::chat::vision::{user_content_with_auxiliary_vision_result, AuxiliaryVisionResult};
use crate::chat::{
    AgentTodoState, Attachment, CompactionBoundaryRecord, ConversationContextState,
    ConversationContextSummary, ModelRef,
};
use crate::mcp::ChatToolDefinition;
use crate::settings::{ModelProvider, SessionModel, Settings};
use crate::skills;

use super::catalog::{assistant_from_builder_args, strip_transcripts_for_frontend};
use super::context::{
    build_chat_api_messages, count_tokens_in_value, estimate_image_tokens_for_dimensions,
    group_answer_excluded_from_context, mark_summary_stale_if_needed, resolve_usage_anchor,
    should_auto_compress_context,
};
use super::interaction::{
    approve_agent_plan_for_execution, format_tool_approval_summary, stream_delta_event_kinds,
};
use super::messages::{
    assistant_model_messages_for_storage, build_assistant_message, build_error_arm_message,
    content_from_segments, normalize_assistant_segments, reasoning_from_segments,
    reconcile_orphan_tool_segments, replace_final_text_segments_for_edit,
};
use super::mutations::{
    apply_regenerate_truncation, build_fork_messages, build_fork_messages_before_anchor,
};
use super::reply_runtime::resolve_reply_arms;
use super::sanitization::sanitize_image_payloads_for_model;
use super::title::{build_title_summary_prompt, generate_title, sanitize_generated_title};
use super::tooling::{
    apply_chat_mode_tool_filter, should_answer_inline_without_file_write,
    try_apply_skill_slash_trigger,
};
use super::*;

#[test]
fn resolve_thinking_maps_levels_and_defaults_to_high() {
    // 有 effort 旋钮的模型（gpt-5.6 支持 low/medium/high/xhigh/max）。模型库里查得到，
    // 不需要 provider（provider 只用于读 model_overrides + Anthropic 家族兜底）。
    let r = |level: Option<&str>, global: bool| resolve_thinking(level, global, None, "gpt-5.6");
    // 未设置 → 默认档 high，不再跟随全局（全局只服务 lens / 翻译）。
    assert_eq!(r(None, true), (true, Some("high".to_string())));
    assert_eq!(r(None, false), (true, Some("high".to_string())));
    // off → 强制关。
    assert_eq!(r(Some("off"), true), (false, None));
    // 具体等级 → 开 + 带等级。
    assert_eq!(r(Some("low"), false), (true, Some("low".to_string())));
    assert_eq!(r(Some("high"), false), (true, Some("high".to_string())));
    // xhigh / max 原样放行（该模型认不认由模型库门控，不在这里收敛）。
    assert_eq!(r(Some("xhigh"), false), (true, Some("xhigh".to_string())));
    assert_eq!(r(Some("max"), false), (true, Some("max".to_string())));
    // 未知值 → 当作未设置，落默认档 high。
    assert_eq!(r(Some("ultra"), true), (true, Some("high".to_string())));
}

#[test]
fn resolve_thinking_drops_level_for_models_without_effort_knob() {
    // 模型库里 `reasoningEfforts: []` = 没有 effort 旋钮。Claude 4.5 及更早不认
    // `output_config.effort`（传了 400），这里就得把等级抹成 None，适配器自然不发。
    for model in ["claude-sonnet-4.5", "claude-opus-4", "claude-haiku-4.5"] {
        assert_eq!(
            resolve_thinking(Some("high"), true, None, model),
            (true, None),
            "{model} 不该带 effort"
        );
    }
    // 4.6+ 反过来必须带上。
    assert_eq!(
        resolve_thinking(Some("max"), true, None, "claude-opus-4.7"),
        (true, Some("max".to_string()))
    );
    // off 仍然优先：关思考就是关思考。
    assert_eq!(
        resolve_thinking(Some("off"), true, None, "claude-opus-4.7"),
        (false, None)
    );
}

#[test]
fn builder_args_produce_valid_assistant() {
    let args = serde_json::json!({
        "name": "  写作助手 ",
        "system_prompt": "你是写作助手。",
        "description": "写文案",
        "mcp_server_ids": ["mcp-1", "  ", "mcp-2"],
        "skill_ids": ["doc"]
    });
    let a = assistant_from_builder_args(&args).expect("should parse");
    assert!(a.id.starts_with("asst_"));
    assert_eq!(a.name, "写作助手");
    assert_eq!(a.system_prompt, "你是写作助手。");
    assert_eq!(a.source, "user");
    assert!(!a.built_in);
    assert_eq!(a.mcp_server_ids, vec!["mcp-1", "mcp-2"]); // 空串被过滤
    assert_eq!(a.skill_ids, vec!["doc"]);
}

#[test]
fn builder_args_reject_missing_required() {
    assert!(assistant_from_builder_args(&serde_json::json!({ "system_prompt": "x" })).is_err());
    assert!(assistant_from_builder_args(&serde_json::json!({ "name": "x" })).is_err());
    assert!(assistant_from_builder_args(
        &serde_json::json!({ "name": "x", "system_prompt": "  " })
    )
    .is_err());
}
fn slash_skill_record(id: &str, name: &str, triggers: Vec<&str>) -> skills::SkillRecord {
    skills::SkillRecord {
        meta: skills::SkillMeta {
            id: id.to_string(),
            name: name.to_string(),
            description: "desc".to_string(),
            source: "user".to_string(),
            path: None,
            recommended_tools: vec![],
            disable_model_invocation: false,
            files: vec![],
            triggers: triggers.into_iter().map(str::to_string).collect(),
            argument_hint: Some("<message>".to_string()),
            arguments: vec!["message".to_string()],
        },
        location: std::path::PathBuf::from(format!("/skills/{id}/SKILL.md")),
        base_dir: std::path::PathBuf::from(format!("/skills/{id}")),
        body: "Write a commit for: $ARGUMENTS (subject $MESSAGE)".to_string(),
    }
}

fn slash_skill_registry(record: skills::SkillRecord) -> skills::SkillRegistry {
    skills::SkillRegistry {
        records: vec![record],
        warnings: vec![],
    }
}

#[test]
fn slash_trigger_rewrites_body_and_pins_skill() {
    let registry = slash_skill_registry(slash_skill_record("commit", "Commit", vec!["/commit"]));
    let chat_tools = crate::settings::ChatToolsConfig::default();

    let (skill_id, rewritten) = try_apply_skill_slash_trigger(
        &registry,
        &chat_tools,
        None,
        "/commit fix login",
        false,
    )
    .expect("slash trigger should match");

    assert_eq!(skill_id, "commit");
    assert!(rewritten.starts_with("[Skill: Commit]\n\n"));
    assert!(rewritten.contains("Write a commit for: fix login"));
    // first positional arg ($MESSAGE) → "fix"
    assert!(rewritten.contains("subject fix"));
}

#[test]
fn slash_trigger_ignores_non_slash_and_unknown() {
    let registry = slash_skill_registry(slash_skill_record("commit", "Commit", vec!["/commit"]));
    let chat_tools = crate::settings::ChatToolsConfig::default();

    assert!(
        try_apply_skill_slash_trigger(&registry, &chat_tools, None, "commit fix", false)
            .is_none()
    );
    assert!(
        try_apply_skill_slash_trigger(&registry, &chat_tools, None, "/unknown x", false)
            .is_none()
    );
}

#[test]
fn slash_trigger_skips_disabled_skill() {
    let registry = slash_skill_registry(slash_skill_record("commit", "Commit", vec!["/commit"]));
    let mut chat_tools = crate::settings::ChatToolsConfig::default();
    chat_tools.disabled_skill_ids = vec!["commit".to_string()];

    assert!(
        try_apply_skill_slash_trigger(&registry, &chat_tools, None, "/commit fix", false)
            .is_none()
    );
}

fn test_provider(id: &str, name: &str, enabled_models: Vec<&str>) -> ModelProvider {
    ModelProvider {
        id: id.to_string(),
        name: name.to_string(),
        api_keys: vec!["sk-test".to_string()],
        api_key_legacy: None,
        base_url: "https://api.example.com/v1".to_string(),
        available_models: Vec::new(),
        enabled_models: enabled_models.into_iter().map(str::to_string).collect(),
        enabled: true,
        api_format: "openai_chat".to_string(),
        model_overrides: HashMap::new(),
        compress_request_body: false,
        request: Default::default(),
    }
}

#[test]
fn inline_code_request_filter_removes_file_creation_tools_for_fenced_code() {
    let mut tools = vec![
        crate::mcp::types::native_read_file_tool(),
        crate::mcp::types::native_write_file_tool(),
        crate::mcp::types::native_edit_file_tool(),
    ];

    apply_inline_code_request_tool_filter(
        &mut tools,
        Some("生成一个完整的 HTML demo，用 ```html 代码块包起来。"),
    );

    assert!(tools.iter().any(|tool| tool.name == "read"));
    assert!(!tools.iter().any(|tool| tool.name == "write"));
    assert!(tools.iter().any(|tool| tool.name == "edit"));
}

#[test]
fn inline_code_request_filter_does_not_hide_file_tools_for_generic_demo_words() {
    let mut tools = vec![
        crate::mcp::types::native_read_file_tool(),
        crate::mcp::types::native_write_file_tool(),
    ];

    apply_inline_code_request_tool_filter(&mut tools, Some("生成一个完整的 HTML demo"));

    assert!(tools.iter().any(|tool| tool.name == "write"));
}

#[test]
fn inline_code_request_filter_treats_put_into_code_block_as_inline() {
    let mut tools = vec![
        crate::mcp::types::native_read_file_tool(),
        crate::mcp::types::native_write_file_tool(),
    ];

    apply_inline_code_request_tool_filter(&mut tools, Some("把完整 HTML 放到代码块里给我"));

    assert!(!tools.iter().any(|tool| tool.name == "write"));
}

#[test]
fn inline_code_request_filter_keeps_write_tools_for_save_intent() {
    let mut tools = vec![
        crate::mcp::types::native_read_file_tool(),
        crate::mcp::types::native_write_file_tool(),
        crate::mcp::types::native_edit_file_tool(),
    ];

    apply_inline_code_request_tool_filter(
        &mut tools,
        Some("生成一个完整的 HTML demo，保存为 ~/news-demo.html。"),
    );

    assert!(tools.iter().any(|tool| tool.name == "write"));
    assert!(tools.iter().any(|tool| tool.name == "edit"));
}

#[test]
fn agent_plan_tool_filter_keeps_only_read_only_and_agent_state_tools() {
    let readonly_mcp_tool = ChatToolDefinition {
        id: "mcp__docs__search".to_string(),
        name: "search".to_string(),
        description: "Search docs".to_string(),
        source: "mcp".to_string(),
        server_id: Some("docs".to_string()),
        server_name: Some("Docs".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
        sensitive: false,
        annotations: Some(serde_json::json!({ "readOnlyHint": true })),
        output_schema: None,
    };
    let write_mcp_tool = ChatToolDefinition {
        id: "mcp__fs__write".to_string(),
        name: "write".to_string(),
        description: "Write file".to_string(),
        source: "mcp".to_string(),
        server_id: Some("fs".to_string()),
        server_name: Some("FS".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
        sensitive: true,
        annotations: Some(serde_json::json!({ "readOnlyHint": false })),
        output_schema: None,
    };
    let mut tools = vec![
        crate::mcp::types::native_read_file_tool(),
        crate::mcp::types::native_write_file_tool(),
        crate::mcp::types::native_run_command_tool(),
        crate::mcp::types::native_run_python_tool(),
        crate::mcp::types::native_memory_read_tool(),
        crate::mcp::types::native_memory_modify_tool(),
        crate::mcp::types::mixer_generate_image_tool(),
        crate::mcp::types::native_skill_activate_tool(),
        crate::chat::ask_user::ask_user_tool(),
        crate::chat::todo::todo_write_tool(),
        readonly_mcp_tool,
        write_mcp_tool,
    ];

    let blocked = apply_agent_plan_tool_filter(&mut tools, true);

    let names = tools
        .iter()
        .map(|tool| tool.openai_tool_name())
        .collect::<Vec<_>>();
    let blocked_names = blocked
        .iter()
        .map(|tool| tool.openai_tool_name())
        .collect::<Vec<_>>();
    assert!(names.contains(&"read".to_string()));
    assert!(names.contains(&"memory_read".to_string()));
    assert!(names.contains(&"skill".to_string()));
    assert!(names.contains(&"ask_user".to_string()));
    assert!(names.contains(&"todo_write".to_string()));
    assert!(names.contains(&"mcp__docs__search".to_string()));
    assert!(!names.contains(&"write".to_string()));
    assert!(!names.contains(&"bash".to_string()));
    assert!(!names.contains(&"run_python".to_string()));
    assert!(!names.contains(&"memory_modify".to_string()));
    assert!(!names.contains(&"mixer_generate_image".to_string()));
    assert!(!names.contains(&"mcp__fs__write".to_string()));
    assert!(blocked_names.contains(&"write".to_string()));
    assert!(blocked_names.contains(&"bash".to_string()));
    assert!(blocked_names.contains(&"run_python".to_string()));
    assert!(blocked_names.contains(&"memory_modify".to_string()));
    assert!(blocked_names.contains(&"mixer_generate_image".to_string()));
    assert!(blocked_names.contains(&"mcp__fs__write".to_string()));
}

#[test]
fn agent_plan_tool_filter_is_noop_outside_plan_mode() {
    let mut tools = vec![
        crate::mcp::types::native_read_file_tool(),
        crate::mcp::types::native_write_file_tool(),
        crate::mcp::types::native_run_command_tool(),
    ];

    let blocked = apply_agent_plan_tool_filter(&mut tools, false);

    assert!(tools.iter().any(|tool| tool.name == "read"));
    assert!(tools.iter().any(|tool| tool.name == "write"));
    assert!(tools.iter().any(|tool| tool.name == "bash"));
    assert!(blocked.is_empty());
}

#[test]
fn chat_mode_tool_filter_keeps_research_tools_only() {
    let readonly_mcp_tool = ChatToolDefinition {
        id: "mcp__docs__search".to_string(),
        name: "search".to_string(),
        description: "Search docs".to_string(),
        source: "mcp".to_string(),
        server_id: Some("docs".to_string()),
        server_name: Some("Docs".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
        sensitive: false,
        annotations: Some(serde_json::json!({ "readOnlyHint": true })),
        output_schema: None,
    };
    let write_mcp_tool = ChatToolDefinition {
        id: "mcp__fs__write".to_string(),
        name: "write".to_string(),
        description: "Write file".to_string(),
        source: "mcp".to_string(),
        server_id: Some("fs".to_string()),
        server_name: Some("FS".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
        sensitive: true,
        annotations: Some(serde_json::json!({ "readOnlyHint": false })),
        output_schema: None,
    };
    let mut tools = vec![
        crate::mcp::types::native_web_search_tool(),
        crate::mcp::types::native_web_fetch_tool(),
        crate::mcp::types::native_knowledge_search_tool(),
        crate::mcp::types::native_read_file_tool(),
        crate::mcp::types::native_write_file_tool(),
        crate::mcp::types::native_run_command_tool(),
        crate::mcp::types::native_memory_read_tool(),
        crate::mcp::types::native_memory_modify_tool(),
        crate::mcp::types::native_skill_activate_tool(),
        crate::chat::ask_user::ask_user_tool(),
        crate::chat::todo::todo_write_tool(),
        readonly_mcp_tool,
        write_mcp_tool,
    ];

    let blocked = apply_chat_mode_tool_filter(
        &mut tools,
        true,
        &crate::settings::ChatModeConfig::default(),
    );
    let names = tools
        .iter()
        .map(|tool| tool.openai_tool_name())
        .collect::<Vec<_>>();
    let blocked_names = blocked
        .iter()
        .map(|tool| tool.openai_tool_name())
        .collect::<Vec<_>>();

    assert!(names.contains(&"search_web".to_string()) || names.contains(&"web_search".to_string()));
    assert!(names.contains(&"web_fetch".to_string()));
    assert!(names.contains(&"knowledge_search".to_string()));
    assert!(names.contains(&"memory_read".to_string()));
    assert!(names.contains(&"ask_user".to_string()));
    assert!(names.contains(&"mcp__docs__search".to_string()));
    assert!(!names.contains(&"read".to_string()));
    assert!(!names.contains(&"write".to_string()));
    assert!(!names.contains(&"bash".to_string()));
    assert!(!names.contains(&"todo_write".to_string()));
    assert!(!names.contains(&"skill".to_string()));
    assert!(!names.contains(&"memory_modify".to_string()));
    assert!(!names.contains(&"mcp__fs__write".to_string()));
    assert!(blocked_names.contains(&"read".to_string()));
    assert!(blocked_names.contains(&"bash".to_string()));
}

#[test]
fn chat_mode_tool_filter_is_noop_when_disabled() {
    let mut tools = vec![
        crate::mcp::types::native_read_file_tool(),
        crate::mcp::types::native_write_file_tool(),
    ];
    let blocked = apply_chat_mode_tool_filter(
        &mut tools,
        false,
        &crate::settings::ChatModeConfig::default(),
    );
    assert_eq!(tools.len(), 2);
    assert!(blocked.is_empty());
}

#[test]
fn chat_mode_tool_filter_respects_config_toggles() {
    let mut config = crate::settings::ChatModeConfig::default();
    config.web_search = false;
    config.web_fetch = true;
    config.knowledge_search = false;
    config.memory_tools = false;
    config.mcp_read_only = false;
    let mut tools = vec![
        crate::mcp::types::native_web_search_tool(),
        crate::mcp::types::native_web_fetch_tool(),
        crate::mcp::types::native_knowledge_search_tool(),
        crate::mcp::types::native_memory_read_tool(),
        crate::chat::ask_user::ask_user_tool(),
    ];
    let readonly_mcp = ChatToolDefinition {
        id: "mcp__docs__search".to_string(),
        name: "search".to_string(),
        description: "Search".to_string(),
        source: "mcp".to_string(),
        server_id: Some("docs".to_string()),
        server_name: Some("Docs".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
        sensitive: false,
        annotations: Some(serde_json::json!({ "readOnlyHint": true })),
        output_schema: None,
    };
    tools.push(readonly_mcp);
    apply_chat_mode_tool_filter(&mut tools, true, &config);
    let names: Vec<_> = tools.iter().map(|t| t.openai_tool_name()).collect();
    assert!(names.contains(&"web_fetch".to_string()));
    assert!(names.contains(&"ask_user".to_string()));
    assert!(!names.iter().any(|n| n == "search_web" || n == "web_search"));
    assert!(!names.contains(&"knowledge_search".to_string()));
    assert!(!names.contains(&"memory_read".to_string()));
    assert!(!names.contains(&"mcp__docs__search".to_string()));
}

#[test]
fn orchestrate_budget_bump_raises_rounds_but_keeps_unlimited() {
    use crate::settings::ORCHESTRATE_MIN_TOOL_ROUNDS;
    let bump =
        |configured: Option<u32>| configured.map(|rounds| rounds.max(ORCHESTRATE_MIN_TOOL_ROUNDS));
    // Configured below the floor -> raised to the floor.
    assert_eq!(bump(Some(20)), Some(ORCHESTRATE_MIN_TOOL_ROUNDS));
    // Configured above the floor -> preserved.
    assert_eq!(bump(Some(80)), Some(80));
    // Unlimited (None) stays unlimited.
    assert_eq!(bump(None), None);
}

#[test]
fn inline_code_request_ignores_attachment_safe_copy_paths() {
    let content = compose_user_content_for_api(
            "用 ```html 包起来给我",
            &[Attachment {
                id: "att_1".to_string(),
                attachment_type: "file".to_string(),
                name: "report.pdf".to_string(),
                path: "att_1-report.pdf".to_string(),
                content: None,
            }],
            Some(Path::new("/Users/test/Library/Application Support/com.zmair.kivio/conversations/conv_1_attachments")),
        );

    assert!(should_answer_inline_without_file_write(Some(&content)));
}

#[test]
fn generate_title_truncates_unicode_safely() {
    let title = generate_title("附件: 这是一张非常非常非常非常非常非常非常长的图片文件名.png");

    assert!(title.ends_with("..."));
    assert!(title.chars().count() <= 33);
}

#[test]
fn agent_run_entry_label_distinguishes_regenerate() {
    assert_eq!(
        agent_run_entry_label(crate::chat::agent::AgentRunEntry::Send),
        "send"
    );
    assert_eq!(
        agent_run_entry_label(crate::chat::agent::AgentRunEntry::Regenerate),
        "regenerate"
    );
}

#[test]
fn build_title_summary_prompt_uses_first_turn_context() {
    let prompt = build_title_summary_prompt(
        "今天下雨吗，吉林市。天气怎么样？",
        "吉林市今天有小雨，建议带伞。",
        "zh-CN",
    );

    assert!(prompt.contains("首轮对话"));
    assert!(prompt.contains("用户：今天下雨吗"));
    assert!(prompt.contains("助手：吉林市今天有小雨"));
    assert!(prompt.contains("只输出标题本身"));
}

#[test]
fn sanitize_generated_title_removes_model_formatting() {
    assert_eq!(
        sanitize_generated_title("- 标题：\"吉林天气查询。\""),
        Some("吉林天气查询".to_string())
    );
    assert_eq!(
        sanitize_generated_title("Title: `Jilin Weather Forecast.`"),
        Some("Jilin Weather Forecast".to_string())
    );
}

#[test]
fn sanitize_generated_title_rejects_empty_output() {
    assert_eq!(sanitize_generated_title("\n\n  "), None);
    assert_eq!(sanitize_generated_title("标题：..."), None);
}

/// 计划批准卡上必须是**计划正文**：用户批的是这份计划，看到一坨截断的 JSON 就没法判断
/// 该不该点批准。
#[test]
fn format_tool_approval_summary_shows_the_plan_text() {
    let record = ToolCallRecord {
        id: "call_1".to_string(),
        name: "ExitPlanMode".to_string(),
        source: "external_cli".to_string(),
        server_id: None,
        arguments: r#"{"plan":"1. 改 run.rs\n2. 加测试"}"#.to_string(),
        status: ToolCallStatus::Pending,
        result_preview: None,
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round: 1,
        sensitive: true,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    };

    let summary = format_tool_approval_summary(&record);
    assert!(summary.detail.contains("1. 改 run.rs"));
    assert!(summary.detail.contains("2. 加测试"));
    // 认出计划之后不再拖一坨原始 JSON。
    assert!(!summary.detail.contains("\"plan\""));
}

/// `EnterPlanMode` 没有入参：卡片不能因为「认不出操作对象」退到裸 JSON 分支、蹲一个 `{}`。
/// 标题（前端的 `toolApprovalTitle`）已经把事情说完了。
#[test]
fn format_tool_approval_summary_leaves_enter_plan_mode_without_detail() {
    let record = ToolCallRecord {
        id: "call_1".to_string(),
        name: "EnterPlanMode".to_string(),
        source: "external_cli".to_string(),
        server_id: None,
        arguments: "{}".to_string(),
        status: ToolCallStatus::Pending,
        result_preview: None,
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round: 1,
        sensitive: true,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    };

    let summary = format_tool_approval_summary(&record);
    assert!(summary.target.is_none());
    assert!(
        summary.detail.is_empty(),
        "不该显示 `{{}}`：{}",
        summary.detail
    );
}

#[test]
fn format_tool_approval_summary_highlights_run_command() {
    let record = ToolCallRecord {
        id: "call_1".to_string(),
        name: "bash".to_string(),
        source: "native".to_string(),
        server_id: None,
        arguments: r#"{"command":"npm test","cwd":"/tmp/project"}"#.to_string(),
        status: ToolCallStatus::Pending,
        result_preview: None,
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round: 1,
        sensitive: true,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    };

    let summary = format_tool_approval_summary(&record);
    assert_eq!(summary.target.as_deref(), Some("npm test"));
    assert!(summary.detail.contains("npm test"));
    assert!(summary.detail.contains("Working directory: /tmp/project"));
    // 认出操作对象后不再拖一坨原始 JSON。
    assert!(!summary.detail.contains("Raw arguments"));
    assert!(!summary.detail.contains("\"command\""));
}

#[test]
fn format_tool_approval_summary_highlights_file_path() {
    let record = ToolCallRecord {
        id: "call_1".to_string(),
        name: "write".to_string(),
        source: "native".to_string(),
        server_id: None,
        arguments: r#"{"path":"/tmp/project/out.txt","content":"hello"}"#.to_string(),
        status: ToolCallStatus::Pending,
        result_preview: None,
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round: 1,
        sensitive: true,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    };

    let summary = format_tool_approval_summary(&record);
    assert_eq!(summary.target.as_deref(), Some("/tmp/project/out.txt"));
    assert_eq!(summary.detail, "/tmp/project/out.txt");
    // 正文里不该再出现 `content` 那一大坨。
    assert!(!summary.detail.contains("hello"));
}

#[test]
fn format_tool_approval_summary_falls_back_to_raw_arguments() {
    let record = ToolCallRecord {
        id: "call_1".to_string(),
        name: "some_mcp_tool".to_string(),
        source: "mcp".to_string(),
        server_id: Some("srv".to_string()),
        arguments: r#"{"foo":"bar"}"#.to_string(),
        status: ToolCallStatus::Pending,
        result_preview: None,
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round: 1,
        sensitive: true,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    };

    let summary = format_tool_approval_summary(&record);
    assert!(summary.target.is_none());
    assert_eq!(summary.detail, r#"{"foo":"bar"}"#);
}

#[test]
fn assistant_model_messages_marks_failed_tool_results_as_error() {
    let api_messages = vec![
        serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call_error",
                "type": "function",
                "function": {
                    "name": "run_python",
                    "arguments": "{\"code\":\"print(1/0)\"}"
                }
            }]
        }),
        serde_json::json!({
            "role": "tool",
            "tool_call_id": "call_error",
            "content": "Python 执行失败：ZeroDivisionError: division by zero"
        }),
        serde_json::json!({
            "role": "assistant",
            "content": "ZeroDivisionError"
        }),
    ];
    let tool_calls = vec![ToolCallRecord {
        id: "call_error".to_string(),
        name: "run_python".to_string(),
        source: "native".to_string(),
        server_id: None,
        arguments: "{\"code\":\"print(1/0)\"}".to_string(),
        status: ToolCallStatus::Error,
        result_preview: None,
        error: Some("Python 执行失败：ZeroDivisionError: division by zero".to_string()),
        duration_ms: Some(31),
        started_at: Some(1),
        completed_at: Some(2),
        round: 1,
        sensitive: false,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    }];

    let model_messages =
        assistant_model_messages_for_storage("ZeroDivisionError", None, &api_messages, &tool_calls);
    let tool_result_is_error = model_messages
        .iter()
        .flat_map(|message| message.content.iter())
        .find_map(|part| match part {
            MessagePart::ToolResult {
                tool_call_id,
                is_error,
                ..
            } if tool_call_id == "call_error" => Some(*is_error),
            _ => None,
        });

    assert_eq!(tool_result_is_error, Some(true));
}

fn test_tool_record(id: &str, source: &str, round: u32, status: ToolCallStatus) -> ToolCallRecord {
    ToolCallRecord {
        id: id.to_string(),
        name: if source == "mixer" {
            "mixer_vision".to_string()
        } else {
            "run_python".to_string()
        },
        source: source.to_string(),
        server_id: None,
        arguments: "{}".to_string(),
        status,
        result_preview: None,
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round,
        sensitive: false,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    }
}

fn tool_segment(order: u32, tool_call_id: &str, round: u32) -> ChatMessageSegment {
    ChatMessageSegment {
        id: format!("seg_{order}_tool_{tool_call_id}"),
        kind: ChatMessageSegmentKind::Tool,
        phase: ChatMessageSegmentPhase::ToolLoop,
        order,
        step_number: Some(1),
        round: Some(round),
        text: None,
        tool_call_id: Some(tool_call_id.to_string()),
    }
}

#[test]
fn reconcile_orphan_tool_segments_synthesizes_cancelled_record_with_recovered_meta() {
    let mut tool_calls = vec![test_tool_record(
        "call_ok",
        "native",
        1,
        ToolCallStatus::Success,
    )];
    let segments = vec![
        tool_segment(1, "call_ok", 1),
        tool_segment(2, "fc_call_function_4agzr50pp9go_1", 2),
    ];
    let api_messages = vec![serde_json::json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "fc_call_function_4agzr50pp9go_1",
            "type": "function",
            "function": { "name": "run_python", "arguments": "{\"code\":\"1\"}" }
        }]
    })];

    reconcile_orphan_tool_segments(&mut tool_calls, &segments, &api_messages);

    assert_eq!(
        tool_calls.len(),
        2,
        "orphan segment should get a synthesized record"
    );
    let synthesized = tool_calls
        .iter()
        .find(|r| r.id == "fc_call_function_4agzr50pp9go_1")
        .expect("synthesized record present");
    assert!(matches!(synthesized.status, ToolCallStatus::Cancelled));
    assert_eq!(
        synthesized.name, "run_python",
        "name recovered from api_messages"
    );
    assert_eq!(synthesized.arguments, "{\"code\":\"1\"}");
    assert_eq!(synthesized.round, 2);
    assert!(synthesized.error.is_some());
}

#[test]
fn reconcile_orphan_tool_segments_falls_back_to_empty_name_without_api_meta() {
    let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
    let segments = vec![tool_segment(1, "orphan_no_meta", 1)];

    reconcile_orphan_tool_segments(&mut tool_calls, &segments, &[]);

    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "orphan_no_meta");
    assert!(
        tool_calls[0].name.is_empty(),
        "no api meta → empty name fallback"
    );
    assert!(matches!(tool_calls[0].status, ToolCallStatus::Cancelled));
}

#[test]
fn reconcile_orphan_tool_segments_noop_when_all_segments_have_records() {
    let mut tool_calls = vec![test_tool_record(
        "call_ok",
        "native",
        1,
        ToolCallStatus::Success,
    )];
    let segments = vec![tool_segment(1, "call_ok", 1)];

    reconcile_orphan_tool_segments(&mut tool_calls, &segments, &[]);

    assert_eq!(tool_calls.len(), 1, "no orphan → tool_calls unchanged");
}

#[test]
fn old_assistant_message_without_segments_deserializes() {
    let message: ChatMessage = serde_json::from_value(serde_json::json!({
        "id": "msg_legacy",
        "role": "assistant",
        "content": "legacy answer",
        "timestamp": 42
    }))
    .expect("legacy message should deserialize");

    assert_eq!(message.content, "legacy answer");
    assert!(message.segments.is_empty());
    assert!(message.tool_calls.is_empty());
}

#[test]
fn segment_legacy_fields_join_only_their_owned_segment_kinds() {
    let segments = vec![
        ChatMessageSegment {
            id: "seg_tool_loop_text".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order: 20,
            step_number: Some(1),
            round: Some(1),
            text: Some("planning text".to_string()),
            tool_call_id: None,
        },
        ChatMessageSegment {
            id: "seg_plain".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::Plain,
            order: 10,
            step_number: None,
            round: None,
            text: Some("plain answer".to_string()),
            tool_call_id: None,
        },
        ChatMessageSegment {
            id: "seg_reasoning".to_string(),
            kind: ChatMessageSegmentKind::Reasoning,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order: 30,
            step_number: Some(1),
            round: Some(1),
            text: Some("reasoning block".to_string()),
            tool_call_id: None,
        },
        ChatMessageSegment {
            id: "seg_synthesis".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::Synthesis,
            order: 40,
            step_number: Some(2),
            round: None,
            text: Some("final answer".to_string()),
            tool_call_id: None,
        },
    ];

    assert_eq!(
        content_from_segments(&segments).as_deref(),
        Some("plain answer\n\nfinal answer")
    );
    assert_eq!(
        reasoning_from_segments(&segments).as_deref(),
        Some("reasoning block")
    );
}

#[test]
fn normalize_segments_adds_auxiliary_and_skipped_tool_segments() {
    let tool_calls = vec![
        test_tool_record("call_aux", "mixer", 0, ToolCallStatus::Success),
        test_tool_record("call_blocked", "native", 1, ToolCallStatus::Skipped),
    ];
    let segments = normalize_assistant_segments(
        "final",
        None,
        &tool_calls,
        vec![ChatMessageSegment {
            id: "seg_final".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::Synthesis,
            order: 1000,
            step_number: Some(2),
            round: None,
            text: Some("final".to_string()),
            tool_call_id: None,
        }],
    );

    let auxiliary = segments
        .iter()
        .find(|segment| segment.tool_call_id.as_deref() == Some("call_aux"))
        .expect("auxiliary tool should have a segment");
    let skipped = segments
        .iter()
        .find(|segment| segment.tool_call_id.as_deref() == Some("call_blocked"))
        .expect("skipped tool should have a segment");

    assert_eq!(auxiliary.kind, ChatMessageSegmentKind::Tool);
    assert_eq!(auxiliary.phase, ChatMessageSegmentPhase::Auxiliary);
    assert_eq!(skipped.kind, ChatMessageSegmentKind::Tool);
    assert_eq!(skipped.phase, ChatMessageSegmentPhase::ToolLoop);
}

#[test]
fn normalize_segments_does_not_duplicate_toolloop_only_answer_text() {
    // 回归：正文段必须是 Plain|Synthesis —— content_from_segments 只认这两个 phase，
    // 正文若只落在别的 phase 上，这里就会再补一条同文案的 Synthesis 段，前端遂把同一段
    // 文案渲染两遍。本用例守住「正文段 = Synthesis 时不重复」这一半，另一半（生产者不许
    // 把正文标成 ToolLoop）由 external_cli_answer_text_after_tools_is_not_duplicated 守。
    let recovered = "⚠️ 模型调用失败。请重试。".to_string();
    let segments = normalize_assistant_segments(
        &recovered,
        None,
        &[],
        vec![ChatMessageSegment {
            id: "seg_1002_step_2_text".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::Synthesis,
            order: 1002,
            step_number: Some(2),
            round: Some(2),
            text: Some(recovered.clone()),
            tool_call_id: None,
        }],
    );
    assert_eq!(
        segments
            .iter()
            .filter(|segment| segment.text.as_deref() == Some(recovered.as_str()))
            .count(),
        1,
        "answer text must not be duplicated into a second segment"
    );
    assert_eq!(
        content_from_segments(&segments).as_deref(),
        Some(&*recovered)
    );
}

/// 回归（用户实测）：本地 CLI 会话里「只要这一轮调用过工具，回答就在气泡里显示两遍」。
///
/// 外部 CLI 路径的 `content` 是所有 TextDelta 的累加，所以它产出的**每一条**文本分段都必须
/// 被 `content_from_segments` 认可（Plain|Synthesis）。工具之后的正文一度标成 ToolLoop，
/// 于是这里判定「有正文但没有任何分段覆盖」→ 再补一条同文案的 Synthesis 段 → 渲染两遍。
///
/// 本用例直接问 `segment_phase_for_tool_count`（生产者本人）要 phase，而不是硬编码一个
/// phase 常量——这样生产者哪天再标回 ToolLoop，这条就会红。
#[test]
fn external_cli_answer_text_after_tools_is_not_duplicated() {
    use crate::external_agents::run::segment_phase_for_tool_count;

    let answer = "已创建 `dup.md`，内容为 `hello`。".to_string();
    let tool_calls = vec![test_tool_record(
        "call_write",
        "external_cli",
        1,
        ToolCallStatus::Success,
    )];
    // 外部 CLI 落库前的分段形态：推理 → 工具 → 工具之后的正文。
    let segments = vec![
        ChatMessageSegment {
            id: "seg_reasoning".to_string(),
            kind: ChatMessageSegmentKind::Reasoning,
            phase: segment_phase_for_tool_count(&ChatMessageSegmentKind::Reasoning, 0),
            order: 1,
            step_number: None,
            round: None,
            text: Some("我先建文件".to_string()),
            tool_call_id: None,
        },
        ChatMessageSegment {
            id: "seg_tool".to_string(),
            kind: ChatMessageSegmentKind::Tool,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order: 2,
            step_number: None,
            round: Some(1),
            text: None,
            tool_call_id: Some("call_write".to_string()),
        },
        ChatMessageSegment {
            id: "seg_answer".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: segment_phase_for_tool_count(&ChatMessageSegmentKind::Text, tool_calls.len()),
            order: 3,
            step_number: None,
            round: Some(1),
            text: Some(answer.clone()),
            tool_call_id: None,
        },
    ];

    let segments = normalize_assistant_segments(&answer, None, &tool_calls, segments);

    assert_eq!(
        segments
            .iter()
            .filter(|segment| segment.kind == ChatMessageSegmentKind::Text
                && segment.text.as_deref() == Some(answer.as_str()))
            .count(),
        1,
        "工具之后的正文只能有一条分段，否则气泡里会显示两遍"
    );
    assert_eq!(content_from_segments(&segments).as_deref(), Some(&*answer));
}

/// 「说一句 → 调工具 → 再说一句」：外部 CLI 的 `content` 是两段拼起来的，所以两条文本分段
/// 都必须被 `content_from_segments` 认可——只认最后一条的话，落库正文会丢掉工具之前那句。
#[test]
fn external_cli_text_around_tool_call_all_counts_as_content() {
    use crate::external_agents::run::segment_phase_for_tool_count;

    let tool_calls = vec![test_tool_record(
        "call_write",
        "external_cli",
        1,
        ToolCallStatus::Success,
    )];
    let before = "我来建这个文件。";
    let after = "已创建 `dup.md`。";
    let segments = vec![
        ChatMessageSegment {
            id: "seg_before".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: segment_phase_for_tool_count(&ChatMessageSegmentKind::Text, 0),
            order: 1,
            step_number: None,
            round: None,
            text: Some(before.to_string()),
            tool_call_id: None,
        },
        ChatMessageSegment {
            id: "seg_tool".to_string(),
            kind: ChatMessageSegmentKind::Tool,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order: 2,
            step_number: None,
            round: Some(1),
            text: None,
            tool_call_id: Some("call_write".to_string()),
        },
        ChatMessageSegment {
            id: "seg_after".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: segment_phase_for_tool_count(&ChatMessageSegmentKind::Text, tool_calls.len()),
            order: 3,
            step_number: None,
            round: Some(1),
            text: Some(after.to_string()),
            tool_call_id: None,
        },
    ];
    let content = format!("{before}{after}");

    let segments = normalize_assistant_segments(&content, None, &tool_calls, segments);

    assert_eq!(
        segments
            .iter()
            .filter(|segment| segment.kind == ChatMessageSegmentKind::Text)
            .count(),
        2,
        "不许为「正文没落段」再补一条全文分段"
    );
    let stored = content_from_segments(&segments).expect("content must be covered by segments");
    assert!(stored.contains(before), "工具之前那句不能从正文里掉出去");
    assert!(stored.contains(after), "工具之后那句不能从正文里掉出去");
}

/// 内置路径的护栏：内置 loop 的 `content` **只**是最终答案（工具循环期间的过程文字留在
/// ToolLoop 分段里、不进 content，见 `chat/agent/planning.rs`）。所以
/// `content_from_segments` 必须继续把 ToolLoop 文本**排除**在正文之外——谁要是为了修
/// 外部 CLI 的重复而放宽这把共享的尺，这条用例就会红。
#[test]
fn builtin_toolloop_narration_stays_out_of_content() {
    let final_answer = "结论：改好了。";
    let tool_calls = vec![test_tool_record(
        "call_read",
        "native",
        1,
        ToolCallStatus::Success,
    )];
    let segments = vec![
        ChatMessageSegment {
            id: "seg_1000_step_1_text".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order: 1000,
            step_number: Some(1),
            round: Some(1),
            text: Some("我先看一下这个文件。".to_string()),
            tool_call_id: None,
        },
        ChatMessageSegment {
            id: "seg_1001_tool_call_read".to_string(),
            kind: ChatMessageSegmentKind::Tool,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order: 1001,
            step_number: Some(1),
            round: Some(1),
            text: None,
            tool_call_id: Some("call_read".to_string()),
        },
        ChatMessageSegment {
            id: "seg_1002_step_2_text".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::Synthesis,
            order: 1002,
            step_number: Some(2),
            round: None,
            text: Some(final_answer.to_string()),
            tool_call_id: None,
        },
    ];

    let normalized = normalize_assistant_segments(final_answer, None, &tool_calls, segments);

    // 一条都不许补：正文已经有 Synthesis 段覆盖。
    assert_eq!(normalized.len(), 3);
    // 过程文字不进正文，正文就是最终答案本身。
    assert_eq!(
        content_from_segments(&normalized).as_deref(),
        Some(final_answer)
    );
    // 过程文字仍以 ToolLoop 分段留在时间线上（前端按 phase 渲染成灰色过程文本）。
    assert_eq!(
        normalized
            .iter()
            .filter(|segment| segment.kind == ChatMessageSegmentKind::Text
                && segment.phase == ChatMessageSegmentPhase::ToolLoop)
            .count(),
        1
    );
}

#[test]
fn normalize_segments_inserts_tool_segments_before_synthesis_text() {
    let tool_calls = vec![test_tool_record(
        "call_read",
        "external_cli",
        1,
        ToolCallStatus::Success,
    )];
    let segments = normalize_assistant_segments(
        "final answer",
        Some("reasoning"),
        &tool_calls,
        vec![
            ChatMessageSegment {
                id: "seg_reasoning".to_string(),
                kind: ChatMessageSegmentKind::Reasoning,
                phase: ChatMessageSegmentPhase::Plain,
                order: 1,
                step_number: None,
                round: None,
                text: Some("reasoning".to_string()),
                tool_call_id: None,
            },
            ChatMessageSegment {
                id: "seg_before".to_string(),
                kind: ChatMessageSegmentKind::Text,
                phase: ChatMessageSegmentPhase::ToolLoop,
                order: 2,
                step_number: None,
                round: Some(1),
                text: Some("working".to_string()),
                tool_call_id: None,
            },
            ChatMessageSegment {
                id: "seg_final".to_string(),
                kind: ChatMessageSegmentKind::Text,
                phase: ChatMessageSegmentPhase::Synthesis,
                order: 3,
                step_number: None,
                round: None,
                text: Some("final answer".to_string()),
                tool_call_id: None,
            },
        ],
    );

    let tool_segment = segments
        .iter()
        .find(|segment| segment.tool_call_id.as_deref() == Some("call_read"))
        .expect("tool segment should exist");
    let final_segment = segments
        .iter()
        .find(|segment| segment.id == "seg_final")
        .expect("final segment should exist");
    assert_eq!(tool_segment.kind, ChatMessageSegmentKind::Tool);
    assert!(tool_segment.order < final_segment.order);
}

#[test]
fn editing_assistant_reply_replaces_final_text_segments_only() {
    let tool_call = test_tool_record("call_blocked", "native", 1, ToolCallStatus::Skipped);
    let mut message = ChatMessage {
        id: "msg_assistant".to_string(),
        role: "assistant".to_string(),
        content: "old final".to_string(),
        attachments: Vec::new(),
        reasoning: Some("reasoning block".to_string()),
        artifacts: Vec::new(),
        tool_calls: vec![tool_call],
        segments: vec![
            ChatMessageSegment {
                id: "seg_plan".to_string(),
                kind: ChatMessageSegmentKind::Text,
                phase: ChatMessageSegmentPhase::ToolLoop,
                order: 1000,
                step_number: Some(1),
                round: Some(1),
                text: Some("planning text".to_string()),
                tool_call_id: None,
            },
            ChatMessageSegment {
                id: "seg_tool".to_string(),
                kind: ChatMessageSegmentKind::Tool,
                phase: ChatMessageSegmentPhase::ToolLoop,
                order: 1001,
                step_number: Some(1),
                round: Some(1),
                text: None,
                tool_call_id: Some("call_blocked".to_string()),
            },
            ChatMessageSegment {
                id: "seg_reasoning".to_string(),
                kind: ChatMessageSegmentKind::Reasoning,
                phase: ChatMessageSegmentPhase::ToolLoop,
                order: 1002,
                step_number: Some(1),
                round: Some(1),
                text: Some("reasoning block".to_string()),
                tool_call_id: None,
            },
            ChatMessageSegment {
                id: "seg_old".to_string(),
                kind: ChatMessageSegmentKind::Text,
                phase: ChatMessageSegmentPhase::Synthesis,
                order: 1003,
                step_number: Some(2),
                round: None,
                text: Some("old final".to_string()),
                tool_call_id: None,
            },
        ],
        agent_plan: None,
        api_messages: Vec::new(),
        model_messages: Vec::new(),
        active_skill_id: None,
        run_entry: None,
        stream_outcome: None,
        usage: None,
        anchor_usage: None,
        group_id: None,
        provider_id: None,
        model: None,
        timestamp: 1,
        degraded: None,
    };

    replace_final_text_segments_for_edit(&mut message, "new final");

    assert_eq!(message.content, "new final");
    assert_eq!(message.reasoning.as_deref(), Some("reasoning block"));
    assert!(message.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Tool
            && segment.tool_call_id.as_deref() == Some("call_blocked")
    }));
    assert!(message.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && segment.phase == ChatMessageSegmentPhase::ToolLoop
            && segment.text.as_deref() == Some("planning text")
    }));
    assert!(!message.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && matches!(
                segment.phase,
                ChatMessageSegmentPhase::Plain | ChatMessageSegmentPhase::Synthesis
            )
            && segment.text.as_deref() == Some("old final")
    }));
    assert!(message.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && segment.phase == ChatMessageSegmentPhase::Synthesis
            && segment.text.as_deref() == Some("new final")
    }));
}

#[test]
fn editing_assistant_reply_rewrites_replay_to_edited_final_answer() {
    let mut message = ChatMessage {
        id: "msg_assistant".to_string(),
        role: "assistant".to_string(),
        content: "old final".to_string(),
        attachments: Vec::new(),
        reasoning: Some("old visible reasoning".to_string()),
        artifacts: Vec::new(),
        tool_calls: vec![test_tool_record(
            "call_1",
            "native",
            1,
            ToolCallStatus::Success,
        )],
        segments: vec![
            ChatMessageSegment {
                id: "seg_reasoning".to_string(),
                kind: ChatMessageSegmentKind::Reasoning,
                phase: ChatMessageSegmentPhase::Synthesis,
                order: 999,
                step_number: Some(2),
                round: None,
                text: Some("old visible reasoning".to_string()),
                tool_call_id: None,
            },
            ChatMessageSegment {
                id: "seg_old".to_string(),
                kind: ChatMessageSegmentKind::Text,
                phase: ChatMessageSegmentPhase::Synthesis,
                order: 1000,
                step_number: Some(2),
                round: None,
                text: Some("old final".to_string()),
                tool_call_id: None,
            },
        ],
        agent_plan: None,
        api_messages: vec![
            serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"/tmp/old.txt\"}"
                    }
                }]
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "tool output"
            }),
            serde_json::json!({
                "role": "assistant",
                "content": "old final",
                "reasoning_content": "old final reasoning"
            }),
        ],
        model_messages: Vec::new(),
        active_skill_id: None,
        run_entry: None,
        stream_outcome: None,
        usage: None,
        anchor_usage: None,
        group_id: None,
        provider_id: None,
        model: None,
        timestamp: 1,
        degraded: None,
    };

    replace_final_text_segments_for_edit(&mut message, "new final");

    assert!(message.api_messages.is_empty());
    let replay = openai_messages_from_model_messages(&message.model_messages);
    let serialized = serde_json::to_string(&replay).expect("replay serializes");
    assert!(serialized.contains("tool output"));
    assert!(serialized.contains("new final"));
    assert!(serialized.contains("old visible reasoning"));
    assert!(!serialized.contains("old final"));
    assert!(!serialized.contains("old final reasoning"));
}

fn test_chat_message(id: &str, role: &str, content: &str, timestamp: i64) -> ChatMessage {
    ChatMessage {
        id: id.to_string(),
        role: role.to_string(),
        content: content.to_string(),
        attachments: Vec::new(),
        reasoning: None,
        artifacts: Vec::new(),
        tool_calls: Vec::new(),
        segments: Vec::new(),
        agent_plan: None,
        api_messages: Vec::new(),
        model_messages: Vec::new(),
        active_skill_id: None,
        run_entry: None,
        stream_outcome: None,
        usage: None,
        anchor_usage: None,
        group_id: None,
        provider_id: None,
        model: None,
        timestamp,
        degraded: None,
    }
}

fn test_conversation_with_summary(stale: bool) -> Conversation {
    Conversation {
        id: "conv_test".to_string(),
        revision: 0,
        title: "test".to_string(),
        provider_id: "provider".to_string(),
        model: "model".to_string(),
        messages: vec![
            test_chat_message("msg_user_1", "user", "old user content", 1),
            test_chat_message("msg_assistant_1", "assistant", "old assistant content", 2),
            test_chat_message("msg_user_2", "user", "recent user content", 3),
            test_chat_message(
                "msg_assistant_2",
                "assistant",
                "recent assistant content",
                4,
            ),
        ],
        active_skill_id: None,
        assistant_id: None,
        assistant_snapshot: None,
        created_at: 1,
        updated_at: 4,
        pinned: false,
        archived: false,
        folder: None,
        project_id: None,
        set_id: None,
        context_state: ConversationContextState {
            summary: Some(ConversationContextSummary {
                id: "ctxsum_test".to_string(),
                content: "summary of older messages".to_string(),
                source_message_ids: vec!["msg_user_1".to_string(), "msg_assistant_1".to_string()],
                source_until_message_id: "msg_assistant_1".to_string(),
                token_estimate_before: 100,
                token_estimate_after: 10,
                created_at: 5,
                provider_id: "provider".to_string(),
                model: "model".to_string(),
                stale,
                file_ledger: None,
            }),
            ..ConversationContextState::default()
        },
        agent_todo_state: AgentTodoState::default(),
        agent_plan_state: AgentPlanState::default(),
        knowledge_base_ids: Vec::new(),
        force_knowledge_search: false,
        thinking_level: None,
        web_search_mode: None,
        reply_models: Vec::new(),
        group_selections: std::collections::HashMap::new(),
        forked_from: None,
        agent_runtime: crate::chat::AgentRuntimeConfig::default(),
    }
}

#[test]
fn approve_agent_plan_targets_selected_message_plan() {
    let mut conversation = test_conversation_with_summary(false);
    let old_plan = "1. Inspect current code\n2. Draft older fix";
    let new_plan = "1. Inspect plan mode\n2. Implement inline execution";
    let mut older = test_chat_message("msg_plan_old", "assistant", old_plan, 10);
    older.agent_plan = Some(AgentPlanState {
        mode: crate::chat::AgentPlanMode::Plan,
        status: crate::chat::AgentPlanStatus::Draft,
        plan: Some(old_plan.to_string()),
        updated_at: 10,
    });
    let mut newer = test_chat_message("msg_plan_new", "assistant", new_plan, 11);
    newer.agent_plan = Some(AgentPlanState {
        mode: crate::chat::AgentPlanMode::Plan,
        status: crate::chat::AgentPlanStatus::Draft,
        plan: Some(new_plan.to_string()),
        updated_at: 11,
    });
    conversation.agent_plan_state = older.agent_plan.clone().unwrap();
    conversation.messages.push(older);
    conversation.messages.push(newer);

    approve_agent_plan_for_execution(&mut conversation, Some("msg_plan_new")).unwrap();

    assert_eq!(
        conversation.agent_plan_state.plan.as_deref(),
        Some(new_plan)
    );
    assert_eq!(
        conversation.agent_plan_state.status,
        crate::chat::AgentPlanStatus::Approved
    );
    let older = conversation
        .messages
        .iter()
        .find(|message| message.id == "msg_plan_old")
        .unwrap();
    assert_eq!(
        older.agent_plan.as_ref().unwrap().status,
        crate::chat::AgentPlanStatus::Draft
    );
    let newer = conversation
        .messages
        .iter()
        .find(|message| message.id == "msg_plan_new")
        .unwrap();
    assert_eq!(
        newer.agent_plan.as_ref().unwrap().status,
        crate::chat::AgentPlanStatus::Approved
    );
}

#[test]
fn approve_agent_plan_rejects_non_plan_message_target() {
    let mut conversation = test_conversation_with_summary(false);
    conversation.messages.push(test_chat_message(
        "msg_plain",
        "assistant",
        "plain answer",
        10,
    ));

    let error = approve_agent_plan_for_execution(&mut conversation, Some("msg_plain")).unwrap_err();

    assert_eq!(error, "该消息不是可执行计划");
}

#[test]
fn approve_agent_plan_rejects_empty_message_plan_target() {
    let mut conversation = test_conversation_with_summary(false);
    let mut message = test_chat_message("msg_empty_plan", "assistant", "plain answer", 10);
    message.agent_plan = Some(AgentPlanState {
        mode: crate::chat::AgentPlanMode::Plan,
        status: crate::chat::AgentPlanStatus::Draft,
        plan: Some("   ".to_string()),
        updated_at: 10,
    });
    conversation.messages.push(message);

    let error =
        approve_agent_plan_for_execution(&mut conversation, Some("msg_empty_plan")).unwrap_err();

    assert_eq!(error, "该消息不是可执行计划");
}

#[test]
fn approve_agent_plan_rejects_non_executable_fragment_target() {
    let mut conversation = test_conversation_with_summary(false);
    let mut message = test_chat_message("msg_fragment_plan", "assistant", "没问题！积萌,", 10);
    message.agent_plan = Some(AgentPlanState {
        mode: crate::chat::AgentPlanMode::Plan,
        status: crate::chat::AgentPlanStatus::Draft,
        plan: Some("没问题！积萌,".to_string()),
        updated_at: 10,
    });
    conversation.messages.push(message);

    let error =
        approve_agent_plan_for_execution(&mut conversation, Some("msg_fragment_plan")).unwrap_err();

    assert_eq!(error, "该消息不是可执行计划");
}

#[test]
fn strip_transcripts_for_frontend_keeps_interrupted_draft_drops_completed() {
    let mut completed = test_chat_message("msg_done", "assistant", "final answer", 2);
    completed.api_messages = vec![serde_json::json!({
        "role": "assistant",
        "content": "final answer"
    })];
    completed.model_messages = vec![ModelMessage::text(ModelRole::Assistant, "final answer")];
    completed.stream_outcome = Some("completed".to_string());

    let mut draft = test_chat_message("msg_draft", "assistant", "partial answer", 4);
    draft.api_messages = vec![serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": "call_1",
            "type": "function",
            "function": { "name": "read_file", "arguments": "{}" }
        }]
    })];
    draft.model_messages = vec![ModelMessage::text(ModelRole::Assistant, "partial answer")];
    draft.stream_outcome = Some("interrupted".to_string());

    // 旧对话：完成但没有 model_messages，回放需回落 api_messages，DTO 不应剥。
    let mut legacy = test_chat_message("msg_legacy", "assistant", "legacy answer", 6);
    legacy.api_messages = vec![serde_json::json!({
        "role": "assistant",
        "content": "legacy answer"
    })];
    legacy.stream_outcome = Some("completed".to_string());

    let mut user = test_chat_message("msg_user", "user", "hi", 1);
    user.api_messages = vec![serde_json::json!({ "role": "user", "content": "hi" })];

    let mut conversation = test_conversation_with_summary(false);
    conversation.messages = vec![user, completed, draft, legacy];

    strip_transcripts_for_frontend(&mut conversation);

    // 已完成 + 有 model_messages：两份转录都剥光。
    assert!(conversation.messages[1].api_messages.is_empty());
    assert!(conversation.messages[1].model_messages.is_empty());
    // 中断草稿：两份都保住，「继续」要靠它恢复工具上下文。
    assert!(!conversation.messages[2].api_messages.is_empty());
    assert!(!conversation.messages[2].model_messages.is_empty());
    // legacy（无 model_messages）：api_messages 也剥——前端不读，后端回放读盘上完整副本。
    assert!(conversation.messages[3].api_messages.is_empty());
    // user 消息不动。
    assert!(!conversation.messages[0].api_messages.is_empty());
}

#[test]
fn effective_side_models_auto_use_session_main_model() {
    let mut settings = Settings::default();
    settings.providers.push(test_provider(
        "global",
        "Global",
        vec!["gemini-3.1-flash-lite"],
    ));
    settings
        .providers
        .push(test_provider("session", "Session", vec!["gpt-4.1"]));
    settings.default_models.chat.provider_id = "global".to_string();
    settings.default_models.chat.model = "gemini-3.1-flash-lite".to_string();

    let session = SessionModel {
        provider_id: "session",
        model: "gpt-4.1",
    };

    assert_eq!(
        settings.effective_compression_model_for_session(Some(session)),
        ("session".to_string(), "gpt-4.1".to_string())
    );
    assert_eq!(
        settings.effective_title_summary_model_for_session(Some(session)),
        ("session".to_string(), "gpt-4.1".to_string())
    );
    assert_eq!(
        settings.effective_vision_model_for_session(Some(session)),
        ("session".to_string(), "gpt-4.1".to_string())
    );
}

#[test]
fn effective_side_models_honor_explicit_mixer_selection() {
    let mut settings = Settings::default();
    settings.providers.push(test_provider(
        "global",
        "Global",
        vec!["gemini-3.1-flash-lite"],
    ));
    settings.providers.push(test_provider(
        "cheap",
        "Cheap",
        vec!["gemini-3.1-flash-lite"],
    ));
    settings.default_models.compression.provider_id = "cheap".to_string();
    settings.default_models.compression.model = "gemini-3.1-flash-lite".to_string();

    let session = SessionModel {
        provider_id: "global",
        model: "gpt-4.1",
    };

    assert_eq!(
        settings.effective_compression_model_for_session(Some(session)),
        ("cheap".to_string(), "gemini-3.1-flash-lite".to_string())
    );
}

#[test]
fn should_auto_compress_allows_recompression_when_summary_exists() {
    let mut conversation = test_conversation_with_summary(false);
    for i in 0..12 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let content = format!("extra content {i} ").repeat(1_000);
        conversation.messages.push(test_chat_message(
            &format!("msg_extra_{i}"),
            role,
            &content,
            10 + i,
        ));
    }
    let context_state = ConversationContextState {
        usage_ratio: Some(0.9),
        ..ConversationContextState::default()
    };
    assert!(should_auto_compress_context(&context_state, &conversation));
}

#[test]
fn should_auto_compress_false_when_no_new_compressible_range() {
    let mut conversation = test_conversation_with_summary(false);
    conversation
        .context_state
        .summary
        .as_mut()
        .expect("summary")
        .source_until_message_id = "msg_assistant_2".to_string();
    let context_state = ConversationContextState {
        usage_ratio: Some(0.9),
        ..ConversationContextState::default()
    };
    assert!(!should_auto_compress_context(&context_state, &conversation));
}

#[test]
fn token_split_starts_after_existing_summary() {
    let mut conversation = test_conversation_with_summary(false);
    // summary source_until = msg_assistant_1（index 1）→ summary_start = 2。
    // 推 3 条大消息（每条 ~20000 tokens，ASCII 4 chars/token），recent 尾窗 20000 只够最后 1 条，
    // 其余进 old_segment；boundary 落在倒数第 2 条（index = len-2）。
    for i in 0..3 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        conversation.messages.push(test_chat_message(
            &format!("msg_extra_{i}"),
            role,
            &"a".repeat(80_000),
            10 + i as i64,
        ));
    }
    let summary_start = 2;
    let boundary = crate::chat::agent::compaction::token_split_chat_messages(
        &conversation.messages,
        summary_start,
        crate::chat::agent::compaction::RECENT_KEEP_TOKENS,
    )
    .expect("boundary");
    assert_eq!(boundary, conversation.messages.len() - 2);
    assert!(boundary > summary_start);
}

#[test]
fn token_split_returns_none_when_recent_window_covers_all() {
    // 全是小消息，远不到 20k 尾窗 → 没有可摘要旧段。
    let mut conversation = test_conversation_with_summary(false);
    for i in 0..5 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        conversation.messages.push(test_chat_message(
            &format!("msg_small_{i}"),
            role,
            "x",
            10 + i as i64,
        ));
    }
    let split = crate::chat::agent::compaction::token_split_chat_messages(
        &conversation.messages,
        2,
        crate::chat::agent::compaction::RECENT_KEEP_TOKENS,
    );
    assert!(split.is_none());
}

#[test]
fn build_chat_api_messages_injects_summary_and_skips_old_raw_messages() {
    let conversation = test_conversation_with_summary(false);
    let messages = build_chat_api_messages(None, "system", &conversation, None, None, &[])
        .expect("messages should build");
    let serialized = serde_json::to_string(&messages).expect("messages serialize");

    assert_eq!(messages.len(), 4);
    assert!(serialized.contains("Previous conversation summary"));
    assert!(serialized.contains("summary of older messages"));
    assert!(!serialized.contains("old user content"));
    assert!(!serialized.contains("old assistant content"));
    assert!(serialized.contains("recent user content"));
    assert!(serialized.contains("recent assistant content"));
}

#[test]
fn stale_summary_is_ignored_by_message_builder() {
    let conversation = test_conversation_with_summary(true);
    let messages = build_chat_api_messages(None, "system", &conversation, None, None, &[])
        .expect("messages should build");
    let serialized = serde_json::to_string(&messages).expect("messages serialize");

    assert!(!serialized.contains("Previous conversation summary"));
    assert!(serialized.contains("old user content"));
    assert!(serialized.contains("recent assistant content"));
}

#[test]
fn auxiliary_vision_result_becomes_text_for_main_chat_model() {
    let conversation = Conversation {
        id: "conv_test".to_string(),
        revision: 0,
        title: "test".to_string(),
        provider_id: "provider".to_string(),
        model: "text-model".to_string(),
        messages: vec![test_chat_message("msg_user_1", "user", "这是什么？", 1)],
        active_skill_id: None,
        assistant_id: None,
        assistant_snapshot: None,
        created_at: 1,
        updated_at: 1,
        pinned: false,
        archived: false,
        folder: None,
        project_id: None,
        set_id: None,
        context_state: ConversationContextState::default(),
        agent_todo_state: AgentTodoState::default(),
        agent_plan_state: AgentPlanState::default(),
        knowledge_base_ids: Vec::new(),
        force_knowledge_search: false,
        thinking_level: None,
        web_search_mode: None,
        reply_models: Vec::new(),
        group_selections: std::collections::HashMap::new(),
        forked_from: None,
        agent_runtime: crate::chat::AgentRuntimeConfig::default(),
    };
    let result = AuxiliaryVisionResult {
        provider_name: "Vision Provider".to_string(),
        model: "vision-model".to_string(),
        content: "图片里是一张 Kivio 设置页截图。".to_string(),
    };
    let augmented = user_content_with_auxiliary_vision_result(Some("这是什么？"), &result, "zh");

    let messages = build_chat_api_messages(
        None,
        "system",
        &conversation,
        Some(0),
        Some(&augmented),
        &[],
    )
    .expect("messages should build");
    let content = &messages[1]["content"];

    assert!(content.is_string());
    assert!(content.as_str().unwrap().contains("[混音器视觉副任务结果]"));
    assert!(content.as_str().unwrap().contains("Kivio 设置页截图"));
    assert!(!serde_json::to_string(&messages)
        .expect("messages serialize")
        .contains("image_url"));
}

#[test]
fn mark_summary_stale_if_boundary_or_older_message_changes() {
    let mut after_boundary = test_conversation_with_summary(false);
    mark_summary_stale_if_needed(&mut after_boundary, 2);
    assert_eq!(
        after_boundary
            .context_state
            .summary
            .as_ref()
            .map(|summary| summary.stale),
        Some(false)
    );

    let mut at_boundary = test_conversation_with_summary(false);
    mark_summary_stale_if_needed(&mut at_boundary, 1);
    assert_eq!(
        at_boundary
            .context_state
            .summary
            .as_ref()
            .map(|summary| summary.stale),
        Some(true)
    );
}

#[test]
fn regenerate_truncation_edits_user_content_and_truncates_after() {
    // 编辑 msg_user_2（index 2）：内容替换、其后 assistant 被截、摘要保持未过期
    // （msg_user_2 在摘要 boundary msg_assistant_1 之后，不触发 stale）。
    let mut conversation = test_conversation_with_summary(false);
    apply_regenerate_truncation(&mut conversation, 2, Some("edited question".to_string())).unwrap();
    assert_eq!(conversation.messages.len(), 3);
    assert_eq!(conversation.messages[2].id, "msg_user_2");
    assert_eq!(conversation.messages[2].content, "edited question");
    assert_eq!(
        conversation.context_state.summary.as_ref().map(|s| s.stale),
        Some(false)
    );

    // 编辑被摘要覆盖的 msg_user_1（index 0）：摘要必须标 stale（内容变了摘要即过期）。
    let mut covered = test_conversation_with_summary(false);
    apply_regenerate_truncation(
        &mut covered,
        0,
        Some("rewritten first question".to_string()),
    )
    .unwrap();
    assert_eq!(covered.messages.len(), 1);
    assert_eq!(covered.messages[0].content, "rewritten first question");
    assert_eq!(
        covered.context_state.summary.as_ref().map(|s| s.stale),
        Some(true)
    );
}

#[test]
fn regenerate_truncation_rejects_bad_edit_targets() {
    // 空内容 → 报错且对话未被改动。
    let mut conversation = test_conversation_with_summary(false);
    let err =
        apply_regenerate_truncation(&mut conversation, 2, Some("   ".to_string())).unwrap_err();
    assert_eq!(err, "消息内容不能为空");
    assert_eq!(conversation.messages.len(), 4);

    // new_content 指向 assistant → 明确报错（不静默忽略）。
    let err =
        apply_regenerate_truncation(&mut conversation, 3, Some("nope".to_string())).unwrap_err();
    assert_eq!(err, "编辑内容仅支持用户消息");
    assert_eq!(conversation.messages.len(), 4);

    // 无 new_content 的既有行为不回归：assistant 截到它之前；user 孤儿保留自身。
    let mut plain = test_conversation_with_summary(false);
    apply_regenerate_truncation(&mut plain, 3, None).unwrap();
    assert_eq!(plain.messages.len(), 3);
    assert_eq!(plain.messages.last().unwrap().id, "msg_user_2");
}

#[test]
fn build_chat_api_messages_replays_hidden_tool_transcript() {
    let conversation = Conversation {
        id: "conv_test".to_string(),
        revision: 0,
        title: "test".to_string(),
        provider_id: "provider".to_string(),
        model: "model".to_string(),
        messages: vec![
            ChatMessage {
                id: "msg_user_1".to_string(),
                role: "user".to_string(),
                content: "use a skill".to_string(),
                attachments: Vec::new(),
                reasoning: None,
                artifacts: Vec::new(),
                tool_calls: Vec::new(),
                segments: Vec::new(),
                agent_plan: None,
                api_messages: Vec::new(),
                model_messages: Vec::new(),
                active_skill_id: None,
                run_entry: None,
                stream_outcome: None,
                usage: None,
                anchor_usage: None,
                group_id: None,
                provider_id: None,
                model: None,
                timestamp: 1,
                degraded: None,
            },
            ChatMessage {
                id: "msg_assistant_1".to_string(),
                role: "assistant".to_string(),
                content: "visible answer".to_string(),
                attachments: Vec::new(),
                reasoning: Some("hidden thinking".to_string()),
                artifacts: Vec::new(),
                tool_calls: Vec::new(),
                segments: Vec::new(),
                agent_plan: None,
                api_messages: vec![
                    serde_json::json!({
                        "role": "assistant",
                        "content": null,
                        "reasoning_content": "plan",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "skill_activate",
                                "arguments": "{\"name\":\"doc\"}"
                            }
                        }]
                    }),
                    serde_json::json!({
                        "role": "tool",
                        "tool_call_id": "call_1",
                        "content": "Skill body"
                    }),
                    serde_json::json!({
                        "role": "assistant",
                        "content": "visible answer",
                        "reasoning_content": "final"
                    }),
                ],
                model_messages: Vec::new(),
                active_skill_id: Some("doc".to_string()),
                run_entry: None,
                stream_outcome: None,
                usage: None,
                anchor_usage: None,
                group_id: None,
                provider_id: None,
                model: None,
                timestamp: 2,
                degraded: None,
            },
        ],
        active_skill_id: Some("doc".to_string()),
        assistant_id: None,
        assistant_snapshot: None,
        created_at: 1,
        updated_at: 2,
        pinned: false,
        archived: false,
        folder: None,
        project_id: None,
        set_id: None,
        context_state: ConversationContextState::default(),
        agent_todo_state: AgentTodoState::default(),
        agent_plan_state: AgentPlanState::default(),
        knowledge_base_ids: Vec::new(),
        force_knowledge_search: false,
        thinking_level: None,
        web_search_mode: None,
        reply_models: Vec::new(),
        group_selections: std::collections::HashMap::new(),
        forked_from: None,
        agent_runtime: crate::chat::AgentRuntimeConfig::default(),
    };

    let messages = build_chat_api_messages(None, "system", &conversation, None, None, &[])
        .expect("messages should build");

    assert_eq!(messages.len(), 5);
    assert_eq!(
        messages[0].get("role").and_then(|value| value.as_str()),
        Some("system")
    );
    assert_eq!(
        messages[1].get("role").and_then(|value| value.as_str()),
        Some("user")
    );
    assert_eq!(
        messages[2]
            .get("tool_calls")
            .and_then(|value| value.as_array())
            .and_then(|calls| calls.first())
            .and_then(|call| call.get("function"))
            .and_then(|function| function.get("name"))
            .and_then(|value| value.as_str()),
        Some("skill_activate")
    );
    assert_eq!(
        messages[3].get("role").and_then(|value| value.as_str()),
        Some("tool")
    );
    assert_eq!(
        messages[4]
            .get("reasoning_content")
            .and_then(|value| value.as_str()),
        Some("final")
    );
}

#[test]
fn sanitize_image_payloads_replaces_data_urls() {
    let content = "before ![img](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA) after";

    let sanitized = sanitize_image_payloads_for_model(content);

    assert!(sanitized.contains("[image data URL omitted; image is available as a tool artifact]"));
    assert!(!sanitized.contains("data:image/png;base64"));
    assert!(!sanitized.contains("iVBORw0KGgo"));
}

#[test]
fn sanitize_image_payloads_replaces_raw_base64_lines() {
    let content = concat!(
            "stdout:\n",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n",
            "done\n"
        );

    let sanitized = sanitize_image_payloads_for_model(content);

    assert!(sanitized.contains("[image base64 omitted; image is available as a tool artifact]"));
    assert!(!sanitized.contains("iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB"));
    assert!(sanitized.contains("done"));
}

#[test]
fn build_chat_api_messages_sanitizes_image_payloads_in_replayed_history() {
    let conversation = Conversation {
            id: "conv_test".to_string(),
            revision: 0,
            title: "test".to_string(),
            provider_id: "provider".to_string(),
            model: "model".to_string(),
            messages: vec![
                test_chat_message("msg_user_1", "user", "make an image", 1),
                ChatMessage {
                    id: "msg_assistant_1".to_string(),
                    role: "assistant".to_string(),
                    content: "![img](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA)".to_string(),
                    attachments: Vec::new(),
                    reasoning: None,
                    artifacts: Vec::new(),
                    tool_calls: Vec::new(),
                    segments: Vec::new(),
                    agent_plan: None,
                    api_messages: vec![
                        serde_json::json!({
                            "role": "assistant",
                            "content": "![img](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA)"
                        }),
                        serde_json::json!({
                            "role": "tool",
                            "content": concat!(
                                "stdout:\n",
                                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n"
                            )
                        }),
                    ],
                    model_messages: Vec::new(),
                    active_skill_id: None,
                    run_entry: None,
                    stream_outcome: None,
                    usage: None,
                    anchor_usage: None,
                    group_id: None,
                    provider_id: None,
                    model: None,
                    timestamp: 2,
                    degraded: None,
                },
            ],
            active_skill_id: None,
            assistant_id: None,
            assistant_snapshot: None,
            created_at: 1,
            updated_at: 2,
            pinned: false,
            archived: false,
            folder: None,
            project_id: None,
            set_id: None,
            context_state: ConversationContextState::default(),
            agent_todo_state: AgentTodoState::default(),
            agent_plan_state: AgentPlanState::default(),
            knowledge_base_ids: Vec::new(),
            force_knowledge_search: false,
        thinking_level: None,
        web_search_mode: None,
            reply_models: Vec::new(),
            group_selections: std::collections::HashMap::new(),
            forked_from: None,
            agent_runtime: crate::chat::AgentRuntimeConfig::default(),
        };

    let messages = build_chat_api_messages(None, "system", &conversation, None, None, &[])
        .expect("messages should build");
    let serialized = serde_json::to_string(&messages).expect("messages serialize");

    assert!(serialized.contains("[image data URL omitted; image is available as a tool artifact]"));
    assert!(serialized.contains("[image base64 omitted; image is available as a tool artifact]"));
    assert!(!serialized.contains("data:image/png;base64"));
    assert!(!serialized.contains("iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB"));
}

#[test]
fn context_token_count_ignores_image_data_url_payloads() {
    let image_part = serde_json::json!({
        "type": "image_url",
        "image_url": {
            "url": format!(
                "data:image/png;base64,{}",
                "A".repeat(200_000)
            )
        }
    });
    let text_part = serde_json::json!({
        "type": "text",
        "text": "describe this image"
    });

    assert_eq!(count_tokens_in_value(&image_part), 0);
    assert_eq!(
        count_tokens_in_value(&text_part),
        agent_prepare::estimate_tokens("describe this image")
    );
}

#[test]
fn image_token_estimates_follow_provider_dimension_rules() {
    assert_eq!(
        estimate_image_tokens_for_dimensions(None, "gpt-4o", 1024, 1024),
        765
    );
    assert_eq!(
        estimate_image_tokens_for_dimensions(None, "gpt-4o", 2048, 4096),
        1105
    );
    assert_eq!(
        estimate_image_tokens_for_dimensions(None, "gpt-4.1-mini", 1024, 1024),
        1659
    );
    assert_eq!(
        estimate_image_tokens_for_dimensions(None, "claude-sonnet-4", 1000, 1000),
        1334
    );
    assert_eq!(
        estimate_image_tokens_for_dimensions(None, "gemini-2.0-flash", 384, 384),
        258
    );
    assert_eq!(
        estimate_image_tokens_for_dimensions(None, "gemini-2.0-flash", 1024, 1024),
        1032
    );
}

// ===== 任务 06-30 多模型一问多答（步骤 3 + 步骤 4）=====

fn test_conversation_with_messages(messages: Vec<ChatMessage>) -> Conversation {
    Conversation {
        id: "conv_multi".to_string(),
        revision: 0,
        title: "test".to_string(),
        provider_id: "openai".to_string(),
        model: "gpt-4o".to_string(),
        messages,
        active_skill_id: None,
        assistant_id: None,
        assistant_snapshot: None,
        created_at: 1,
        updated_at: 1,
        pinned: false,
        archived: false,
        folder: None,
        project_id: None,
        set_id: None,
        context_state: ConversationContextState::default(),
        agent_todo_state: AgentTodoState::default(),
        agent_plan_state: AgentPlanState::default(),
        knowledge_base_ids: Vec::new(),
        force_knowledge_search: false,
        thinking_level: None,
        web_search_mode: None,
        reply_models: Vec::new(),
        group_selections: std::collections::HashMap::new(),
        forked_from: None,
        agent_runtime: crate::chat::AgentRuntimeConfig::default(),
    }
}

fn grouped_assistant(id: &str, content: &str, group_id: &str, ts: i64) -> ChatMessage {
    let mut m = test_chat_message(id, "assistant", content, ts);
    m.group_id = Some(group_id.to_string());
    m.provider_id = Some("openai".to_string());
    m.model = Some("gpt-4o".to_string());
    m
}

fn test_settings_with_providers(provider_ids: &[&str]) -> Settings {
    let mut settings = Settings::default();
    settings.providers = provider_ids
        .iter()
        .map(|id| {
            serde_json::from_value::<ModelProvider>(serde_json::json!({
                "id": id,
                "name": id,
                "baseUrl": "https://example.com/v1",
                "apiKeys": ["k"],
            }))
            .expect("provider deserialize")
        })
        .collect();
    settings
}

/// 带 anchor_usage 的 assistant（openai_chat 口径：anchor_prompt = input_tokens）。
fn assistant_with_anchor(id: &str, ts: i64, input_tokens: u64) -> ChatMessage {
    let mut m = test_chat_message(id, "assistant", "reply", ts);
    m.provider_id = Some("openai".to_string());
    m.anchor_usage = Some(crate::chat::model::ModelUsage {
        input_tokens: Some(input_tokens),
        output_tokens: Some(100),
        ..Default::default()
    });
    m
}

fn boundary_at(created_at: i64) -> CompactionBoundaryRecord {
    CompactionBoundaryRecord {
        id: "ctxbd_test".to_string(),
        source_until_message_id: "u1".to_string(),
        display_after_message_id: None,
        token_estimate_before: 0,
        token_estimate_after: 0,
        summary_content: String::new(),
        trigger: "manual".to_string(),
        created_at,
    }
}

#[test]
fn resolve_usage_anchor_reports_prompt_and_trailing() {
    let conv = test_conversation_with_messages(vec![
        test_chat_message("u1", "user", "hi", 1),
        assistant_with_anchor("a1", 2, 100_000),
        test_chat_message("u2", "user", "follow-up question here", 3),
    ]);
    let provider = test_provider("openai", "OpenAI", vec!["gpt-4o"]);
    let (total, trailing) = resolve_usage_anchor(&conv, Some(&provider));
    // openai 无 total_tokens → input(100000) + output(100)。
    assert_eq!(total, Some(100_100), "openai anchor = input + output");
    // trailing = 锚点 assistant **之后** 的消息（新 user），> 0；锚点响应本身不算进 trailing。
    assert!(trailing > 0);
}

#[test]
fn resolve_usage_anchor_none_without_usage() {
    let conv = test_conversation_with_messages(vec![
        test_chat_message("u1", "user", "hi", 1),
        test_chat_message("a1", "assistant", "reply", 2),
    ]);
    let provider = test_provider("openai", "OpenAI", vec!["gpt-4o"]);
    assert_eq!(resolve_usage_anchor(&conv, Some(&provider)), (None, 0));
}

#[test]
fn resolve_usage_anchor_invalidated_on_provider_switch() {
    let conv = test_conversation_with_messages(vec![
        test_chat_message("u1", "user", "hi", 1),
        assistant_with_anchor("a1", 2, 100_000),
    ]);
    // 会话切换到 anthropic：旧 openai 锚点计数口径不可比 → 作废。
    let provider = test_provider("anthropic", "Anthropic", vec!["claude"]);
    assert_eq!(resolve_usage_anchor(&conv, Some(&provider)), (None, 0));
}

#[test]
fn resolve_usage_anchor_invalidated_after_compaction() {
    // 手动压缩发生在锚点消息之后（boundary.created_at=10 > anchor.ts=2）→ 锚点失真 → 作废（R4）。
    let mut conv = test_conversation_with_messages(vec![
        test_chat_message("u1", "user", "hi", 1),
        assistant_with_anchor("a1", 2, 100_000),
    ]);
    conv.context_state.compaction_boundaries = vec![boundary_at(10)];
    let provider = test_provider("openai", "OpenAI", vec!["gpt-4o"]);
    assert_eq!(resolve_usage_anchor(&conv, Some(&provider)), (None, 0));
}

#[test]
fn resolve_usage_anchor_kept_when_compaction_precedes_anchor() {
    // run 内自动压缩：boundary.created_at=2 <= 压缩后生成的 assistant.ts=5 → 锚点仍有效。
    let mut conv = test_conversation_with_messages(vec![
        test_chat_message("u1", "user", "hi", 1),
        assistant_with_anchor("a1", 5, 100_000),
    ]);
    conv.context_state.compaction_boundaries = vec![boundary_at(2)];
    let provider = test_provider("openai", "OpenAI", vec!["gpt-4o"]);
    let (total, _) = resolve_usage_anchor(&conv, Some(&provider));
    assert_eq!(total, Some(100_100)); // input(100000) + output(100)
}

#[test]
fn resolve_reply_arms_dedups_filters_and_caps() {
    let settings = test_settings_with_providers(&["openai", "anthropic"]);

    // 单模型 / 空 → ≤1（调用方走单模型路径）。
    assert!(resolve_reply_arms(&settings, &[]).unwrap().is_empty());
    let one = vec![ModelRef {
        provider_id: "openai".to_string(),
        model: "gpt-4o".to_string(),
    }];
    assert_eq!(resolve_reply_arms(&settings, &one).unwrap().len(), 1);

    // 去重（相同 provider+model）、保序、丢空、丢未知 provider。
    let many = vec![
        ModelRef {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        },
        ModelRef {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        }, // dup
        ModelRef {
            provider_id: "anthropic".to_string(),
            model: "claude-3".to_string(),
        },
        ModelRef {
            provider_id: "ghost".to_string(),
            model: "y".to_string(),
        }, // unknown provider
    ];
    let arms = resolve_reply_arms(&settings, &many).unwrap();
    assert_eq!(
        arms,
        vec![
            ("openai".to_string(), "gpt-4o".to_string()),
            ("anthropic".to_string(), "claude-3".to_string()),
        ]
    );

    // 空 provider 也被丢弃（单独验证，避免与上面的 4 条上限冲突）。
    let with_empty = vec![
        ModelRef {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        },
        ModelRef {
            provider_id: "".to_string(),
            model: "x".to_string(),
        },
    ];
    assert_eq!(resolve_reply_arms(&settings, &with_empty).unwrap().len(), 1);

    // 超上限 → Err。
    let over: Vec<ModelRef> = (0..(MAX_REPLY_MODELS + 1))
        .map(|i| ModelRef {
            provider_id: "openai".to_string(),
            model: format!("m{i}"),
        })
        .collect();
    assert!(resolve_reply_arms(&settings, &over).is_err());
}

#[test]
fn build_assistant_message_records_group_meta_only_when_provided() {
    let single = build_assistant_message(
        "msg_single".to_string(),
        "hi".to_string(),
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        Some("send"),
        Some("completed"),
        None,
        None,
        None,
        None,
    );
    assert!(single.group_id.is_none());
    assert!(single.provider_id.is_none());
    assert!(single.model.is_none());

    let arm = build_assistant_message(
        "msg_arm".to_string(),
        "hi".to_string(),
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        Some("send"),
        Some("completed"),
        None,
        None,
        None,
        Some((
            "grp_1".to_string(),
            "anthropic".to_string(),
            "claude-3".to_string(),
        )),
    );
    assert_eq!(arm.group_id.as_deref(), Some("grp_1"));
    assert_eq!(arm.provider_id.as_deref(), Some("anthropic"));
    assert_eq!(arm.model.as_deref(), Some("claude-3"));
}

#[test]
fn build_error_arm_message_keeps_column_identity_and_marks_error() {
    // 报错臂合成的「错误列」：保留 group_id/provider/model，错误信息进 content，
    // stream_outcome 标 error —— 这样前端仍按 group_id 聚合出该列，不再被吞掉。
    let msg = build_error_arm_message(
        "grp_err",
        "provider-x".to_string(),
        "model-y".to_string(),
        "上游返回 429：额度不足".to_string(),
        "send",
        None,
    );
    assert_eq!(msg.role, "assistant");
    assert_eq!(msg.group_id.as_deref(), Some("grp_err"));
    assert_eq!(msg.provider_id.as_deref(), Some("provider-x"));
    assert_eq!(msg.model.as_deref(), Some("model-y"));
    assert_eq!(msg.stream_outcome.as_deref(), Some("error"));
    assert!(msg.content.contains("429"));
    assert!(msg.id.starts_with("msg_"));
}

#[test]
fn build_chat_api_messages_keeps_only_selected_group_answer() {
    // user + 3 答（grp_1）。默认无 group_selections → 取顺序第一条 a1。
    let messages = vec![
        test_chat_message("msg_user", "user", "compare these", 1),
        grouped_assistant("msg_a1", "answer one", "grp_1", 2),
        grouped_assistant("msg_a2", "answer two", "grp_1", 3),
        grouped_assistant("msg_a3", "answer three", "grp_1", 4),
    ];
    let mut conversation = test_conversation_with_messages(messages);

    let built =
        build_chat_api_messages(None, "system", &conversation, Some(0), None, &[]).expect("build");
    let serialized = serde_json::to_string(&built).unwrap();
    assert!(serialized.contains("answer one"));
    assert!(!serialized.contains("answer two"));
    assert!(!serialized.contains("answer three"));

    // 用户点选第二条 → 历史改为只含 a2。
    conversation
        .group_selections
        .insert("grp_1".to_string(), "msg_a2".to_string());
    let built =
        build_chat_api_messages(None, "system", &conversation, Some(0), None, &[]).expect("build");
    let serialized = serde_json::to_string(&built).unwrap();
    assert!(!serialized.contains("answer one"));
    assert!(serialized.contains("answer two"));
    assert!(!serialized.contains("answer three"));
}

#[test]
fn build_chat_api_messages_default_first_follows_deletion() {
    // 删除第一条后，默认「顺序第一条」自动变成原第二条。
    let messages = vec![
        test_chat_message("msg_user", "user", "compare these", 1),
        grouped_assistant("msg_a2", "answer two", "grp_1", 3),
        grouped_assistant("msg_a3", "answer three", "grp_1", 4),
    ];
    let conversation = test_conversation_with_messages(messages);
    let built =
        build_chat_api_messages(None, "system", &conversation, Some(0), None, &[]).expect("build");
    let serialized = serde_json::to_string(&built).unwrap();
    assert!(serialized.contains("answer two"));
    assert!(!serialized.contains("answer three"));
}

#[test]
fn build_chat_api_messages_default_skips_errored_arm() {
    // 首臂报错（stream_outcome=error）+ 次臂正常，且无显式 group_selections：
    // 默认应保留首个「非错误」臂、跳过错误臂文案，避免把错误回灌给模型（F2 修复）。
    let mut a1 = grouped_assistant("msg_a1", "arm one failed", "grp_1", 2);
    a1.stream_outcome = Some("error".to_string());
    let a2 = grouped_assistant("msg_a2", "arm two ok", "grp_1", 3);
    let messages = vec![
        test_chat_message("msg_user", "user", "compare these", 1),
        a1,
        a2,
    ];
    let conversation = test_conversation_with_messages(messages);
    let built =
        build_chat_api_messages(None, "system", &conversation, Some(0), None, &[]).expect("build");
    let serialized = serde_json::to_string(&built).unwrap();
    assert!(
        !serialized.contains("arm one failed"),
        "errored arm must be excluded from context"
    );
    assert!(
        serialized.contains("arm two ok"),
        "first non-errored arm is retained"
    );
}

#[test]
fn build_chat_api_messages_single_answer_unaffected() {
    // 无 group_id 的常规历史完全不受过滤影响（防回归 AC5/AC6）。
    let messages = vec![
        test_chat_message("msg_user", "user", "hello", 1),
        test_chat_message("msg_a", "assistant", "world", 2),
    ];
    let conversation = test_conversation_with_messages(messages);
    let built =
        build_chat_api_messages(None, "system", &conversation, Some(0), None, &[]).expect("build");
    let serialized = serde_json::to_string(&built).unwrap();
    assert!(serialized.contains("hello"));
    assert!(serialized.contains("world"));
}

#[test]
fn group_excludes_only_non_selected_assistants() {
    let messages = vec![
        test_chat_message("msg_user", "user", "q", 1),
        grouped_assistant("msg_a1", "a1", "grp_1", 2),
        grouped_assistant("msg_a2", "a2", "grp_1", 3),
    ];
    let conversation = test_conversation_with_messages(messages);
    // 默认选第一条：a1 保留、a2 排除。
    assert!(!group_answer_excluded_from_context(
        &conversation,
        &conversation.messages[1]
    ));
    assert!(group_answer_excluded_from_context(
        &conversation,
        &conversation.messages[2]
    ));
    // user 消息（即便带 group_id）永不被该过滤排除。
    let mut user_in_group = test_chat_message("msg_u2", "user", "uq", 4);
    user_in_group.group_id = Some("grp_1".to_string());
    assert!(!group_answer_excluded_from_context(
        &conversation,
        &user_in_group
    ));
}

#[test]
fn stale_group_selection_falls_back_to_first_remaining() {
    // D5/AC4：删除显式选中条后，清掉指向已删消息的 group_selections，选中条回退到组内
    // 顺序第一条（这里模拟 chat_delete_message / chat_regenerate_message 的清理后状态）。
    let messages = vec![
        test_chat_message("msg_user", "user", "q", 1),
        grouped_assistant("msg_a1", "answer one", "grp_1", 2),
        grouped_assistant("msg_a2", "answer two", "grp_1", 3),
    ];
    let mut conversation = test_conversation_with_messages(messages);
    // 用户显式选了第二条。
    conversation
        .group_selections
        .insert("grp_1".to_string(), "msg_a2".to_string());

    // 模拟删除被选中的 msg_a2：移除消息 + 删除命令对 group_selections 的清理。
    conversation.messages.retain(|m| m.id != "msg_a2");
    if conversation
        .group_selections
        .get("grp_1")
        .map(String::as_str)
        == Some("msg_a2")
    {
        conversation.group_selections.remove("grp_1");
    }

    // 残余的 msg_a1 必须仍进上下文（回退到组内第一条），而非被整组排除。
    assert!(!group_answer_excluded_from_context(
        &conversation,
        &conversation.messages[1]
    ));
    let built =
        build_chat_api_messages(None, "system", &conversation, Some(0), None, &[]).expect("build");
    let serialized = serde_json::to_string(&built).unwrap();
    assert!(serialized.contains("answer one"));
}

// ===== 对话分支（方案 B）=====

#[test]
fn build_fork_messages_keeps_prefix_including_anchor() {
    let messages = vec![
        test_chat_message("m0", "user", "q1", 1),
        test_chat_message("m1", "assistant", "a1", 2),
        test_chat_message("m2", "user", "q2", 3),
        test_chat_message("m3", "assistant", "a2", 4),
    ];
    // 在 m2（user）建分支：保留 m0..=m2，丢弃其后。
    let forked = build_fork_messages(&messages, 2);
    let ids: Vec<&str> = forked.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["m0", "m1", "m2"]);
    // 源不变。
    assert_eq!(messages.len(), 4);
}

#[test]
fn pi_fork_prefix_stops_before_the_selected_user_message() {
    let messages = vec![
        test_chat_message("m0", "user", "q1", 1),
        test_chat_message("m1", "assistant", "a1", 2),
        test_chat_message("m2", "user", "q2", 3),
    ];
    let forked = build_fork_messages_before_anchor(&messages, 2);
    assert_eq!(
        forked
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["m0", "m1"]
    );
    assert!(build_fork_messages_before_anchor(&messages, 0).is_empty());
}

#[test]
fn build_fork_messages_collapses_group_to_selected_column() {
    // 一轮多模型多答：m0 user，m1/m2/m3 同组三列答案。锚点选中 m2。
    let messages = vec![
        test_chat_message("m0", "user", "q", 1),
        grouped_assistant("m1", "col1", "grp", 2),
        grouped_assistant("m2", "col2", "grp", 3),
        grouped_assistant("m3", "col3", "grp", 4),
    ];
    let forked = build_fork_messages(&messages, 2);
    let ids: Vec<&str> = forked.iter().map(|m| m.id.as_str()).collect();
    // 只留 user + 选中列 m2，丢弃 m1（前序兄弟列）与 m3（切片外）。
    assert_eq!(ids, vec!["m0", "m2"]);
    // 锚点转普通单答（去 group_id）。
    assert_eq!(forked.last().unwrap().group_id, None);
}

#[test]
fn build_fork_messages_non_group_anchor_leaves_group_id_untouched() {
    let messages = vec![
        test_chat_message("m0", "user", "q", 1),
        test_chat_message("m1", "assistant", "a", 2),
    ];
    let forked = build_fork_messages(&messages, 1);
    assert_eq!(forked.len(), 2);
    assert_eq!(forked[1].group_id, None);
}

#[test]
fn fork_group_selection_cleanup_drops_dangling_and_collapsed() {
    // 模拟 chat_fork_conversation 内的 group_selections 清理逻辑。
    // 新前缀：g1 组完整保留（选中 s1）；g2 组被折叠成单答（锚点去 group_id）。
    let messages = vec![
        test_chat_message("u1", "user", "q1", 1),
        grouped_assistant("s1", "g1a", "g1", 2),
        grouped_assistant("s2", "g1b", "g1", 3),
        test_chat_message("u2", "user", "q2", 4),
        // g2 折叠后：这条已去 group_id（模拟 build_fork_messages 结果）。
        test_chat_message("s3", "assistant", "g2sel", 5),
    ];
    let existing_groups: std::collections::HashMap<&str, &str> = messages
        .iter()
        .filter_map(|m| m.group_id.as_deref().map(|g| (m.id.as_str(), g)))
        .collect();
    let mut selections: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    selections.insert("g1".to_string(), "s1".to_string()); // 有效：s1 仍在且仍属 g1
    selections.insert("g2".to_string(), "s3".to_string()); // 失效：s3 已去 group_id（组被折叠）
    selections.insert("g3".to_string(), "gone".to_string()); // 失效：消息已不存在
    selections
        .retain(|group_id, sel| existing_groups.get(sel.as_str()) == Some(&group_id.as_str()));

    assert_eq!(selections.len(), 1);
    assert_eq!(selections.get("g1").map(String::as_str), Some("s1"));
}

/// End-to-end channel: a file-op tool call in the summarized region →
/// build_for_boundary → summary.file_ledger → build_chat_api_messages injects a
/// system summary message carrying the `### Files touched` block.
#[test]
fn file_ledger_flows_into_replayed_summary_message() {
    fn file_call(id: &str, name: &str, path: &str) -> ToolCallRecord {
        ToolCallRecord {
            id: id.to_string(),
            name: name.to_string(),
            source: "agent".to_string(),
            server_id: None,
            arguments: serde_json::json!({ "path": path }).to_string(),
            status: crate::chat::types::ToolCallStatus::Success,
            result_preview: None,
            error: None,
            duration_ms: None,
            started_at: None,
            completed_at: None,
            round: 1,
            sensitive: false,
            artifacts: Vec::new(),
            trace_id: None,
            span_id: None,
            structured_content: None,
        }
    }

    let mut conversation = test_conversation_with_summary(false);
    // Attach file ops to the boundary assistant message (covered by the summary).
    let boundary = conversation
        .messages
        .iter_mut()
        .find(|m| m.id == "msg_assistant_1")
        .unwrap();
    boundary.tool_calls = vec![
        file_call("t1", "read", "src/lib.rs"),
        file_call("t2", "write", "src/main.rs"),
    ];

    // Recompute the ledger over the covered history and store it (mirrors both
    // compaction persist sites).
    let ledger =
        crate::chat::agent::file_ledger::build_for_boundary(&conversation, "msg_assistant_1");
    assert!(!ledger.is_empty(), "ledger should capture the two ops");
    conversation
        .context_state
        .summary
        .as_mut()
        .unwrap()
        .file_ledger = Some(ledger);

    let messages =
        build_chat_api_messages(None, "system prompt", &conversation, Some(2), None, &[]).unwrap();

    // The injected summary system message (index 1) must carry the ledger block.
    let summary_sys = messages
        .iter()
        .find(|m| {
            m.get("role").and_then(|r| r.as_str()) == Some("system")
                && m.get("content")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains("### Files touched"))
                    .unwrap_or(false)
        })
        .expect("a system message carries the Files touched block");
    let content = summary_sys["content"].as_str().unwrap();
    assert!(
        content.contains(r#"Modified: "src/main.rs""#),
        "modified path: {content}"
    );
    assert!(
        content.contains(r#"Read: "src/lib.rs""#),
        "read path: {content}"
    );
    // The summary text itself is still present above the ledger.
    assert!(content.contains("summary of older messages"));
}

/// 回归护栏：空 delta + 有 segment 是工具卡 / 内置搜索卡的「占位」调用，必须照发一条
/// TextDelta，否则工具卡整场流式期间都不渲染（协议里工具事件不带 segment/order）。
#[test]
fn empty_delta_with_segment_still_emits_a_placeholder_event() {
    // 工具槽位宣告：只发 text。
    assert_eq!(stream_delta_event_kinds("", None, true), (false, true));
    // reasoning 占位：只发 reasoning，不重复发 text。
    assert_eq!(stream_delta_event_kinds("", Some(""), true), (true, false));
    // 有 reasoning 正文时同样不重复发 text。
    assert_eq!(
        stream_delta_event_kinds("", Some("think"), true),
        (true, false)
    );
    // 正常文本增量。
    assert_eq!(stream_delta_event_kinds("hi", None, true), (false, true));
    // 什么都没有：一条都不发。
    assert_eq!(stream_delta_event_kinds("", None, false), (false, false));
    assert_eq!(
        stream_delta_event_kinds("", Some(""), false),
        (false, false)
    );
}
