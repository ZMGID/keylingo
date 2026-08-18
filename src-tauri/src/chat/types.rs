use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::chat::model::ModelMessage;
use crate::mcp::types::ChatToolArtifact;

fn default_context_usage_status() -> String {
    "unknown".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextUsageSegment {
    pub id: String,
    pub label: String,
    pub estimated_tokens: usize,
    #[serde(default)]
    pub color: Option<String>,
}

/// Timeline marker for a context compaction event (user/auto compress or agent-loop L2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionBoundaryRecord {
    pub id: String,
    /// Last UI message fully covered by the summary (context split point; replay
    /// truth lives in `ConversationContextSummary.source_until_message_id`).
    pub source_until_message_id: String,
    /// Timeline anchor: the divider renders after this message — the last message
    /// at the moment compaction was triggered — so the marker shows *when* the
    /// compaction happened, not where the token split landed. Older records
    /// without it fall back to `source_until_message_id`.
    #[serde(default)]
    pub display_after_message_id: Option<String>,
    pub token_estimate_before: usize,
    pub token_estimate_after: usize,
    pub summary_content: String,
    /// `manual` | `auto` | `agent_loop`
    pub trigger: String,
    pub created_at: i64,
}

/// Deterministic, machine-tracked record of files read/modified by tool calls
/// in the summarized history. Rendered under the LLM summary as a factual floor
/// so compaction can't silently forget which files an agent touched.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileLedger {
    /// Read-only file paths, oldest→newest, deduped.
    #[serde(default)]
    pub read_files: Vec<String>,
    /// Modified file paths (sticky: once modified, stays modified).
    #[serde(default)]
    pub modified_files: Vec<String>,
    /// Diagnostic count of entries evicted by the render budget.
    #[serde(default)]
    pub omitted_count: usize,
}

impl FileLedger {
    pub fn is_empty(&self) -> bool {
        self.read_files.is_empty() && self.modified_files.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContextSummary {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub source_message_ids: Vec<String>,
    pub source_until_message_id: String,
    pub token_estimate_before: usize,
    pub token_estimate_after: usize,
    pub created_at: i64,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub stale: bool,
    /// Files touched by tool calls in the summarized region (see [`FileLedger`]).
    #[serde(default)]
    pub file_ledger: Option<FileLedger>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversationContextState {
    #[serde(default)]
    pub estimated_input_tokens: usize,
    #[serde(default)]
    pub context_window_tokens: Option<usize>,
    #[serde(default)]
    pub context_window_estimated: bool,
    #[serde(default)]
    pub usage_ratio: Option<f32>,
    #[serde(default = "default_context_usage_status")]
    pub status: String,
    #[serde(default)]
    pub segments: Vec<ContextUsageSegment>,
    #[serde(default)]
    pub last_measured_at: i64,
    #[serde(default)]
    pub last_compressed_at: Option<i64>,
    #[serde(default)]
    pub compressed_message_count: usize,
    /// 本会话累计执行压缩的次数（含自动与手动）。
    #[serde(default)]
    pub compression_count: usize,
    #[serde(default)]
    pub summary: Option<ConversationContextSummary>,
    #[serde(default)]
    pub compaction_boundaries: Vec<CompactionBoundaryRecord>,
    #[serde(default)]
    pub warning: Option<String>,
    /// `kivio_builtin` or `external_cli`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_source: Option<String>,
    /// `cli_reported` or `estimated` (external CLI only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_count_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_input_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_output_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl Default for AgentTodoStatus {
    fn default() -> Self {
        Self::Pending
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AgentTodoItem {
    pub id: String,
    /// 一行 subject（保留字段名 `content` 向后兼容；概念上是任务标题）。
    pub content: String,
    /// 可选的详细描述（subject/description 分离）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub status: AgentTodoStatus,
    /// 本条完成后才能开始的任务 id（正向依赖边）。写侧自动同步对端 blocked_by。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
    /// 必须先完成才能开始本条的任务 id（反向依赖边）。写侧自动同步对端 blocks。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// 认领者（P3 subagent 预留，本期不接消费方）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AgentTodoState {
    #[serde(default)]
    pub items: Vec<AgentTodoItem>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPlanMode {
    Act,
    Plan,
    Orchestrate,
}

impl Default for AgentPlanMode {
    fn default() -> Self {
        Self::Act
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPlanStatus {
    Empty,
    Draft,
    Approved,
}

impl Default for AgentPlanStatus {
    fn default() -> Self {
        Self::Empty
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AgentPlanState {
    #[serde(default)]
    pub mode: AgentPlanMode,
    #[serde(default)]
    pub status: AgentPlanStatus,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

/// 工具调用状态（保存在 assistant message metadata 中）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    Running,
    Success,
    Error,
    Cancelled,
    Skipped,
}

/// Chat 工具调用记录。字段保持 snake_case 存储，前端可同时兼容 camelCase 事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub arguments: String,
    pub status: ToolCallStatus,
    #[serde(default)]
    pub result_preview: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub round: u32,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default)]
    pub artifacts: Vec<ChatToolArtifact>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub span_id: Option<String>,
    #[serde(default)]
    pub structured_content: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageSegmentKind {
    Text,
    Reasoning,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageSegmentPhase {
    Auxiliary,
    Plain,
    ToolLoop,
    Synthesis,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessageSegment {
    pub id: String,
    pub kind: ChatMessageSegmentKind,
    pub phase: ChatMessageSegmentPhase,
    pub order: u32,
    #[serde(default)]
    pub step_number: Option<u8>,
    #[serde(default)]
    pub round: Option<u32>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

/// 对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String, // "user" | "assistant"
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<ChatToolArtifact>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRecord>,
    #[serde(default)]
    pub segments: Vec<ChatMessageSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_plan: Option<AgentPlanState>,
    /// Hidden OpenAI-compatible transcript messages produced while answering this UI message.
    ///
    /// Tool calls stay rendered as metadata in `tool_calls`, but strict tool-calling
    /// providers such as DeepSeek need the original assistant `tool_calls` messages
    /// and matching `role: tool` results replayed in later requests.
    #[serde(default)]
    pub api_messages: Vec<Value>,
    /// Canonical provider-agnostic transcript messages produced while answering this UI message.
    ///
    /// New replay paths prefer this field. `api_messages` stays readable for legacy
    /// conversations and OpenAI-compatible debugging.
    #[serde(default)]
    pub model_messages: Vec<ModelMessage>,
    #[serde(default)]
    pub active_skill_id: Option<String>,
    /// How this assistant message was produced: `send` or `regenerate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_entry: Option<String>,
    /// Final stream outcome for this assistant message: `completed`, `cancelled`, or `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_outcome: Option<String>,
    /// 降级兜底的结构化描述：模型调用失败、但本轮已有工具结果时填充。
    /// 前端渲染成独立错误卡片；正文 `content` 不再混入故障文案。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<crate::chat::agent::recovery::DegradedAnswer>,
    /// Provider-reported usage accumulated across all model calls of this reply
    /// (planning/synthesis/compaction). None when the provider reports no usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::chat::model::ModelUsage>,
    /// Provider-reported usage of the **last** model call of this reply (真实用量锚点)。
    /// 与累计 `usage` 区分：累计是多步 prompt 之和、会虚高数倍，不能当锚点；锚点必须是单次调用
    /// 的 usage，代表「本轮结束时整个对话 prompt 的真实大小」。下一轮 / footer 据此把上下文占用
    /// 锚定到 provider 实报值（见 `chat/agent/context_estimate.rs`）。旧会话无字段 → None → 回落估算。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_usage: Option<crate::chat::model::ModelUsage>,
    /// 多模型一问多答（任务 06-30）：同一条 user 消息 fan-out 出的 N 条 assistant 共享同一个
    /// group_id；单模型回答为 None（旧会话缺字段反序列化为 None，向后兼容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// 该 assistant 实际所用 provider id（多模型时每条各记自己的；单模型为 None，回退会话级
    /// `Conversation.provider_id`）。供前端列头「model | provider」展示。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// 该 assistant 实际所用 model（多模型时每条各记自己的；单模型为 None，回退会话级
    /// `Conversation.model`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub timestamp: i64,
}

/// 附件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    #[serde(rename = "type")]
    pub attachment_type: String, // "image" | "file"
    pub name: String,
    pub path: String, // 相对于对话附件目录的路径；`memory://` 前缀 = 内存虚拟文本附件
    /// 内存虚拟文本附件（粘贴长文本生成的虚拟 txt）正文：随对话消息持久化，不生成独立磁盘文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Agent 运行时种类：内置 loop 或外部 CLI。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntimeKind {
    #[default]
    Builtin,
    /// Built-in conversational runtime: research tools only (search/fetch/KB/read-only MCP).
    Chat,
    External,
}

/// 对话级 Agent 运行时配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentRuntimeConfig {
    #[serde(default)]
    pub kind: AgentRuntimeKind,
    #[serde(default)]
    pub external_agent_id: Option<String>,
    #[serde(default)]
    pub external_model: Option<String>,
    #[serde(default)]
    pub external_reasoning: Option<String>,
    /// External-CLI sandbox/permission level (claude --permission-mode / codex --sandbox).
    #[serde(default)]
    pub external_sandbox: Option<String>,
    /// dsh Agent preset：`standard` / `code` / `minimal` / `cordis`。其它 CLI 忽略。
    #[serde(default)]
    pub external_agent_preset: Option<String>,
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            kind: AgentRuntimeKind::Builtin,
            external_agent_id: None,
            external_model: None,
            external_reasoning: None,
            external_sandbox: None,
            external_agent_preset: None,
        }
    }
}

impl AgentRuntimeConfig {
    pub fn is_external(&self) -> bool {
        self.kind == AgentRuntimeKind::External
            && self
                .external_agent_id
                .as_ref()
                .is_some_and(|id| !id.trim().is_empty())
    }

    pub fn is_chat(&self) -> bool {
        self.kind == AgentRuntimeKind::Chat
    }
}

/// 会话级联网搜索模式（任务 07-23）。
/// - `Off`：不联网。
/// - `Builtin`：模型原生内置搜索（仅部分 provider 支持，按 `api_format` 判定）。
/// - `ThirdParty`：既有 `search_web` 工具（复用 Lens 第三方配置 Tavily/Exa/…）。
///
/// `Conversation.web_search_mode` 为 `None` 时运行时回退全局 `nativeTools.webSearch`
/// （on ⇒ ThirdParty，off ⇒ Off），保证旧对话行为逐字节不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchMode {
    Off,
    Builtin,
    ThirdParty,
}

impl WebSearchMode {
    /// 解析会话的**有效**联网搜索模式（任务 07-23）。
    /// 会话未显式设置（`None`）时回退全局 `native_tools.web_search`：
    /// on ⇒ `ThirdParty`，off ⇒ `Off`，保证旧对话行为逐字节不变。
    pub fn resolve(
        conv_mode: Option<WebSearchMode>,
        settings: &crate::settings::Settings,
    ) -> WebSearchMode {
        conv_mode.unwrap_or({
            if settings.chat_tools.native_tools.web_search {
                WebSearchMode::ThirdParty
            } else {
                WebSearchMode::Off
            }
        })
    }
}

/// 完整对话数据（存储在 conversations/{id}.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    /// Monotonic on-disk revision maintained by `ConversationRepository`.
    /// Legacy conversation files deserialize as revision 0.
    #[serde(default)]
    pub revision: u64,
    pub title: String,
    pub provider_id: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub agent_runtime: AgentRuntimeConfig,
    #[serde(default)]
    pub active_skill_id: Option<String>,
    #[serde(default)]
    pub assistant_id: Option<String>,
    #[serde(default)]
    pub assistant_snapshot: Option<ChatAssistantSnapshot>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub pinned: bool,
    /// 对话库归档：侧栏默认隐藏，对话库「归档」书架可见。旧文件缺字段 = false。
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    /// 所属「集」(人设分组) id。与 `project_id` 互斥：至多一个有值。
    #[serde(default)]
    pub set_id: Option<String>,
    #[serde(default)]
    pub context_state: ConversationContextState,
    #[serde(default)]
    pub agent_todo_state: AgentTodoState,
    #[serde(default)]
    pub agent_plan_state: AgentPlanState,
    /// 本会话挂载的知识库 id 列表；`knowledge_search` 缺省检索这些库。
    #[serde(default)]
    pub knowledge_base_ids: Vec<String>,
    /// 强制检索：开启后系统提示要求模型回答前必须先调用 `knowledge_search`。默认关。
    #[serde(default)]
    pub force_knowledge_search: bool,
    /// 每对话「思考等级」：`"off"|"low"|"medium"|"high"`，`None` = 跟随全局思考开关。
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// 会话级联网搜索模式（任务 07-23）。`None` = 未设置，运行时回退全局 `nativeTools.webSearch`。
    #[serde(default)]
    pub web_search_mode: Option<WebSearchMode>,
    /// 多模型一问多答（任务 06-30，决策 D2）：会话级持久化的多答模型集合（上限 4）。
    /// 空或单元素 = 单模型现状。一条 user 消息会 fan-out 给这些模型并发回答。
    #[serde(default)]
    pub reply_models: Vec<ModelRef>,
    /// 多答组的「选中条」（决策 D5）：group_id → 被采纳进下一轮历史的 assistant message_id。
    /// 无记录时取该组顺序第一条。serde default 为空（旧会话兼容）。
    #[serde(default)]
    pub group_selections: HashMap<String, String>,
    /// 对话分支来源（方案 B）：非 None 表示本对话是从某对话某消息处分叉而来。
    /// serde default ⇒ 旧对话 JSON 缺字段正常反序列化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkOrigin>,
}

/// 一次回答所用的 (provider, model) 引用。多模型一问多答的会话级模型集元素。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRef {
    pub provider_id: String,
    pub model: String,
}

/// 对话分支来源（方案 B）：本对话由某条对话在某消息处分叉而来。
/// 全部为分叉时的快照——源对话后续改名/删除都不影响此处显示与回跳。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForkOrigin {
    /// 源对话 id（回跳目标）。
    pub conversation_id: String,
    /// 分叉锚点消息 id（源对话中被「建分支」的那条）。
    pub message_id: String,
    /// 分叉时的源对话标题快照（面包屑显示用）。
    pub title: String,
}

/// 对话列表项（index.json 中的元数据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationListItem {
    pub id: String,
    /// `None` identifies a legacy index entry that must be reconciled from the
    /// conversation file before the index can be trusted again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub title: String,
    pub preview: String, // 最后一条消息的前 100 字符
    pub provider_id: String,
    pub model: String,
    pub message_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub pinned: bool,
    /// 对话库归档（与 Conversation.archived 同步进 index）。
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    /// 所属「集」id（与 project_id 互斥）。
    #[serde(default)]
    pub set_id: Option<String>,
    #[serde(default)]
    pub assistant_id: Option<String>,
    #[serde(default)]
    pub assistant_name: Option<String>,
    #[serde(default)]
    pub agent_runtime: AgentRuntimeConfig,
    /// 对话分支来源（方案 B）。旧索引缺字段正常反序列化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkOrigin>,
}

/// 侧栏全局搜索命中：在列表元数据上附带首个匹配位置，供高亮片段与跳转消息。
/// `#[serde(flatten)]` 保持旧字段扁平，前端可当 ConversationListItem 用。
#[derive(Debug, Clone, Serialize)]
pub struct ConversationSearchHit {
    #[serde(flatten)]
    pub item: ConversationListItem,
    /// title | preview | folder | content | reasoning
    pub match_field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_message_id: Option<String>,
    /// 围绕关键词的上下文片段（已折叠空白）；标题/文件夹命中时可能为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_snippet: Option<String>,
}

/// 对话索引文件结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversationIndex {
    pub conversations: Vec<ConversationListItem>,
}

/// Chat 项目。`folder` 保留用于旧对话兼容，真实项目归属使用 `project_id`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatProject {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub root_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 项目索引文件结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatProjectIndex {
    pub projects: Vec<ChatProject>,
}

/// Chat 集(Set)：助手之上的人设分组。不带工作目录（区别于项目）；持有自己的系统提示词
/// 和默认助手。集名下的对话通过 `Conversation.set_id` 关联（与 project_id 互斥）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSet {
    pub id: String,
    pub name: String,
    /// 集级系统提示词，运行时实时注入集内对话（不冻结）。
    #[serde(default)]
    pub system_prompt: String,
    /// 集的默认助手 id；在集下新建对话且未显式指定助手时使用。
    #[serde(default)]
    pub default_assistant_id: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 集索引文件结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatSetIndex {
    pub sets: Vec<ChatSet>,
}

/// 一条对话被拖到集/项目里的固定行号。见 storage::set_conversation_pins。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationPin {
    pub id: String,
    pub row: u32,
}

/// 可复用 Chat 助手配置。存储字段保持 snake_case，与 Conversation JSON 一致。
/// 重建后只保留：身份(name/desc/icon/color) + 系统提示词 + 模型 + 勾选的 MCP/技能白名单。
/// 旧文件里的 author/tags/quick_commands/data_connectors/knowledge_skills 等字段由 serde 忽略未知字段容错。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAssistant {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    /// 该助手允许使用的 MCP 服务器 id 白名单。空 = 不可用任何 MCP。
    #[serde(default)]
    pub mcp_server_ids: Vec<String>,
    /// 该助手允许激活的技能 id 白名单。空 = 不可用任何技能。
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub installed: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub built_in: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 对话创建时冻结的助手配置，避免后续编辑助手静默改变旧对话行为。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAssistantSnapshot {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub mcp_server_ids: Vec<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
}

impl From<&ChatAssistant> for ChatAssistantSnapshot {
    fn from(assistant: &ChatAssistant) -> Self {
        Self {
            id: assistant.id.clone(),
            name: assistant.name.clone(),
            description: assistant.description.clone(),
            source: assistant.source.clone(),
            system_prompt: assistant.system_prompt.clone(),
            provider_id: assistant.provider_id.clone(),
            model: assistant.model.clone(),
            mcp_server_ids: assistant.mcp_server_ids.clone(),
            skill_ids: assistant.skill_ids.clone(),
        }
    }
}

/// 助手索引文件结构。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatAssistantIndex {
    pub assistants: Vec<ChatAssistant>,
}

fn default_true() -> bool {
    true
}

impl From<&Conversation> for ConversationListItem {
    fn from(conv: &Conversation) -> Self {
        let preview = conv
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user" || m.role == "assistant")
            .map(|m| {
                let text = m.content.trim();
                crate::chat::agent::execute::truncate_chars(text, 100)
            })
            .unwrap_or_default();

        ConversationListItem {
            id: conv.id.clone(),
            revision: Some(conv.revision),
            title: conv.title.clone(),
            preview,
            provider_id: conv.provider_id.clone(),
            model: conv.model.clone(),
            message_count: conv.messages.len(),
            created_at: conv.created_at,
            updated_at: conv.updated_at,
            pinned: conv.pinned,
            archived: conv.archived,
            folder: conv.folder.clone(),
            project_id: conv.project_id.clone(),
            set_id: conv.set_id.clone(),
            assistant_id: conv.assistant_id.clone(),
            assistant_name: conv
                .assistant_snapshot
                .as_ref()
                .map(|snapshot| snapshot.name.clone()),
            agent_runtime: conv.agent_runtime.clone(),
            forked_from: conv.forked_from.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 多模型一问多答（任务 06-30 步骤 2）：旧会话 JSON 缺新字段时反序列化不崩（R9/AC6）。

    #[test]
    fn chat_message_deserializes_without_multi_model_fields() {
        // 旧版 assistant 消息，没有 group_id / provider_id / model。
        let json = r#"{
            "id": "msg_old",
            "role": "assistant",
            "content": "hello",
            "timestamp": 123
        }"#;
        let msg: ChatMessage = serde_json::from_str(json).expect("legacy ChatMessage parses");
        assert_eq!(msg.id, "msg_old");
        assert!(msg.group_id.is_none());
        assert!(msg.provider_id.is_none());
        assert!(msg.model.is_none());
    }

    // 会话级三态联网搜索（任务 07-23，R1）：老会话 web_search_mode 缺字段反序列化为 None，
    // 且有效模式回退全局 nativeTools.webSearch，保证旧对话行为逐字节不变。
    #[test]
    fn conversation_deserializes_without_web_search_mode_field() {
        // 只给必需字段的最小老会话 JSON（无 web_search_mode）。
        let json = r#"{
            "id": "conv_old",
            "title": "old",
            "provider_id": "openai",
            "model": "gpt-4o",
            "messages": [],
            "created_at": 1,
            "updated_at": 1
        }"#;
        let conv: Conversation =
            serde_json::from_str(json).expect("legacy Conversation parses without web_search_mode");
        assert!(conv.web_search_mode.is_none());
    }

    #[test]
    fn web_search_mode_resolve_falls_back_to_global() {
        let mut settings = crate::settings::Settings::default();
        // None（老会话/未设置）→ 依全局开关：on ⇒ ThirdParty，off ⇒ Off。
        settings.chat_tools.native_tools.web_search = true;
        assert_eq!(
            WebSearchMode::resolve(None, &settings),
            WebSearchMode::ThirdParty
        );
        settings.chat_tools.native_tools.web_search = false;
        assert_eq!(WebSearchMode::resolve(None, &settings), WebSearchMode::Off);
        // 显式设过 ⇒ 无视全局，原样返回（即便全局 off 也保留会话选择）。
        assert_eq!(
            WebSearchMode::resolve(Some(WebSearchMode::Builtin), &settings),
            WebSearchMode::Builtin
        );
        assert_eq!(
            WebSearchMode::resolve(Some(WebSearchMode::Off), &settings),
            WebSearchMode::Off
        );
        settings.chat_tools.native_tools.web_search = true;
        assert_eq!(
            WebSearchMode::resolve(Some(WebSearchMode::Off), &settings),
            WebSearchMode::Off
        );
    }

    #[test]
    fn chat_message_roundtrips_multi_model_fields() {
        let json = r#"{
            "id": "msg_new",
            "role": "assistant",
            "content": "hi",
            "group_id": "grp_1",
            "provider_id": "openai",
            "model": "gpt-4o",
            "timestamp": 9
        }"#;
        let msg: ChatMessage = serde_json::from_str(json).expect("new ChatMessage parses");
        assert_eq!(msg.group_id.as_deref(), Some("grp_1"));
        assert_eq!(msg.provider_id.as_deref(), Some("openai"));
        assert_eq!(msg.model.as_deref(), Some("gpt-4o"));
        // 重新序列化再反序列化保持一致。
        let again: ChatMessage =
            serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(again.group_id, msg.group_id);
        assert_eq!(again.model, msg.model);
    }

    #[test]
    fn conversation_deserializes_without_multi_model_fields() {
        // 旧版会话，没有 reply_models / group_selections。
        let json = r#"{
            "id": "conv_old",
            "title": "t",
            "provider_id": "p",
            "model": "m",
            "messages": [],
            "created_at": 1,
            "updated_at": 2
        }"#;
        let conv: Conversation = serde_json::from_str(json).expect("legacy Conversation parses");
        assert_eq!(conv.id, "conv_old");
        assert!(conv.reply_models.is_empty());
        assert!(conv.group_selections.is_empty());
    }

    #[test]
    fn conversation_roundtrips_multi_model_fields() {
        let json = r#"{
            "id": "conv_new",
            "title": "t",
            "provider_id": "p",
            "model": "m",
            "messages": [],
            "reply_models": [
                {"provider_id": "openai", "model": "gpt-4o"},
                {"provider_id": "anthropic", "model": "claude-3"}
            ],
            "group_selections": {"grp_1": "msg_a"},
            "created_at": 1,
            "updated_at": 2
        }"#;
        let conv: Conversation = serde_json::from_str(json).expect("new Conversation parses");
        assert_eq!(conv.reply_models.len(), 2);
        assert_eq!(conv.reply_models[0].provider_id, "openai");
        assert_eq!(conv.reply_models[0].model, "gpt-4o");
        assert_eq!(
            conv.group_selections.get("grp_1").map(String::as_str),
            Some("msg_a")
        );
    }
}
