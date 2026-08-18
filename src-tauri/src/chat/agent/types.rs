use serde_json::Value;

use crate::chat::types::{
    ChatAssistantSnapshot, ChatMessageSegment, CompactionBoundaryRecord, ToolCallRecord,
};
use crate::mcp::ChatToolDefinition;
use crate::settings::{ChatToolsConfig, ModelProvider, Settings};
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRunEntry {
    Send,
    Regenerate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPhase {
    ToolLoop,
    Synthesis,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStreamPolicy {
    PlanningNoDoneUntilNoTools,
    SynthesisAlwaysDone,
    SynthesisDeferEmpty,
}

pub struct AgentRunConfig<'a> {
    pub state: &'a AppState,
    pub conversation_id: String,
    /// Conversation that conversation-scoped tools (todo / native workspace)
    /// target. Equals `conversation_id` for a normal chat run; for a sub-agent
    /// run it is the parent conversation (see `ToolExecutionContext`).
    pub tool_conversation_id: String,
    /// Sub-agent nesting depth (0 = top-level chat run).
    pub depth: u8,
    pub run_id: String,
    pub message_id: String,
    pub generation: u64,
    pub provider: ModelProvider,
    pub model: String,
    pub runtime_messages: Vec<Value>,
    pub tools: Vec<ChatToolDefinition>,
    pub blocked_tool_calls: Vec<ChatToolDefinition>,
    pub settings: Settings,
    pub effective_chat_tools: ChatToolsConfig,
    pub language: String,
    pub thinking_enabled: bool,
    /// 每对话「思考等级」(`Some("low"|"medium"|"high")`)。`None` = 未设置，维持现状。
    /// 仅作用于答案生成（planning/synthesis），不作用于压缩摘要。
    pub thinking_level: Option<String>,
    /// 会话级联网搜索有效模式（任务 07-23）。由 `effective_web_search_mode` 解析后传入。
    /// 决定本 run 是否暴露 `search_web`（ThirdParty）或请求内置搜索（Builtin，且模型支持）。
    pub web_search_mode: crate::chat::types::WebSearchMode,
    pub max_output_tokens: u32,
    pub retry_attempts: usize,
    pub assistant_snapshot: Option<ChatAssistantSnapshot>,
    pub provider_tools_fallback_system_prompt: String,
    /// 真实用量锚点（来自会话最后一条带 `anchor_usage` 的 assistant 消息，由 commands.rs 解析）：
    /// run 首次压缩检查前用它把上下文占用锚定到 provider 实报值。值为「上次 prompt + 该次响应」的
    /// token **总数**（含 output，见 `context_estimate::anchor_total_tokens`）。`None` = 无可用锚点
    /// （新对话 / 旧数据无 usage / 切换过 provider / 压缩边界之后），回落纯字符估算。
    pub initial_anchor_total_tokens: Option<u64>,
    /// 锚点响应**之后**（不含响应本身，响应用 output 计入锚点）新增消息的字符估算，由 commands.rs
    /// 组装 runtime_messages 时算好。与 `initial_anchor_total_tokens` 配对：`effective = 锚点 + 该 trailing`。
    pub initial_anchor_trailing_estimate: usize,
    /// 对话工作目录，用于扫描项目 `.kivio/skills` 与 `.agents/skills`。
    pub skill_project_cwd: Option<std::path::PathBuf>,
}

impl AgentRunConfig<'_> {
    /// 本 run 是否应请求模型**原生内置联网搜索**（任务 07-23）：
    /// 会话为 `Builtin` 模式且当前 provider 支持（按 `api_format`）。
    /// 用于答案生成（planning/synthesis）设置 `GenerateOptions.builtin_web_search`。
    pub(crate) fn builtin_web_search_active(&self) -> bool {
        self.web_search_mode == crate::chat::types::WebSearchMode::Builtin
            && crate::chat::model_metadata::builtin_web_search_supported(&self.provider)
    }
}

#[derive(Debug, Clone)]
pub struct AgentRunResult {
    pub content: String,
    pub reasoning: Option<String>,
    pub tool_records: Vec<ToolCallRecord>,
    pub segments: Vec<ChatMessageSegment>,
    pub api_messages: Vec<Value>,
    pub stream_outcome: String,
    /// 本轮全部模型调用（规划/合成/压缩摘要）累计的 provider 真实 usage；
    /// provider 不报告时为 None（前端回落到 chars 估算）。
    pub usage: Option<crate::chat::model::ModelUsage>,
    /// 本轮**最后一次**模型调用的 usage（真实用量锚点，落盘到 `ChatMessage.anchor_usage`）。
    /// 与累计 `usage` 区分——累计是多步之和会虚高，锚点须是单次调用值。None = provider 未报。
    pub last_step_usage: Option<crate::chat::model::ModelUsage>,
    /// 本轮发生了上下文压缩（L2 摘要）时，这里携带压缩后的**完整历史**
    /// （system + 摘要 + 受保护尾段 + 本轮后续消息 + 最终 assistant 回答）。
    /// 跨轮调用方（kivio-code 交互模式）据此**替换**自己累积的 runtime_messages，
    /// 让压缩真正跨轮生效；为 None 时维持"追加 api_messages"的旧行为。
    pub compacted_history: Option<Vec<Value>>,
    /// Agent-loop L2 compaction boundary for timeline UI persistence.
    pub compaction_boundary: Option<CompactionBoundaryRecord>,
    /// Agent-loop L2 compaction summary for `context_state.summary` persistence
    /// (L2 不再只 push boundary，run 结束时由 commands.rs 写回 summary + compression_count）。
    pub compaction_summary: Option<crate::chat::types::ConversationContextSummary>,
    /// 本轮降级兜底的结构化描述（模型失败但已产出工具结果）。None = 正常回答。
    /// 落到 `ChatMessage.degraded`，前端据此渲染错误卡片。
    pub degraded: Option<crate::chat::agent::recovery::DegradedAnswer>,
    /// 模型原生生成的图片（Gemini native image gen，任务 07-24）：跨轮累积的
    /// `GenerateOutput.images`，reply 侧落成 assistant 消息级 artifacts。空 = 未出图。
    pub images: Vec<crate::chat::model::GeneratedImageData>,
}
