// Chat 前端类型定义

export type ToolCallStatus =
  | 'pending'
  | 'running'
  | 'success'
  | 'completed'
  | 'error'
  | 'skipped'
  | 'cancelled'

export interface SkillMeta {
  id: string
  name: string
  description: string
  source?: 'builtin' | 'user' | 'external' | string
  path?: string
  recommended_tools?: string[]
  recommendedTools?: string[]
  disable_model_invocation?: boolean
  disableModelInvocation?: boolean
  files?: SkillFileEntry[]
  enabled?: boolean
  triggers?: string[]
  argument_hint?: string | null
  argumentHint?: string | null
  arguments?: string[]
}

export interface SkillFileEntry {
  relative_path?: string
  relativePath?: string
  kind?: string
  size_bytes?: number
  sizeBytes?: number
}

export interface SkillDetail extends SkillMeta {
  content?: string
  body?: string
  frontmatter?: Record<string, unknown>
  updated_at?: number
  updatedAt?: number
}

export interface ToolCallRecord {
  id: string
  conversationId?: string
  runId?: string
  messageId?: string
  toolCallId?: string
  call_id?: string
  callId?: string
  tool_name?: string
  toolName?: string
  name?: string
  server_id?: string
  serverId?: string
  server_name?: string
  serverName?: string
  source?: string
  status?: ToolCallStatus
  started_at?: number
  startedAt?: number
  completed_at?: number
  completedAt?: number
  duration_ms?: number
  durationMs?: number
  arguments?: unknown
  args?: unknown
  input?: unknown
  argument_preview?: string
  argumentPreview?: string
  argumentsPreview?: string
  result?: unknown
  output?: unknown
  result_preview?: string
  resultPreview?: string
  error?: string
  round?: number
  sensitive?: boolean
  requires_confirmation?: boolean
  requiresConfirmation?: boolean
  artifacts?: ChatToolArtifact[]
  trace_id?: string | null
  traceId?: string | null
  span_id?: string | null
  spanId?: string | null
  structured_content?: unknown
  structuredContent?: unknown
}

export type AskUserPhase = 'awaiting' | 'answered' | 'skipped' | 'timeout' | 'cancelled'

export interface AskUserOption {
  id: string
  label: string
  description?: string | null
}

export interface AskUserQuestion {
  id: string
  prompt: string
  options: AskUserOption[]
  allow_multiple?: boolean
  allowMultiple?: boolean
  allow_custom?: boolean
  allowCustom?: boolean
}

export interface AskUserPromptPayload {
  title?: string | null
  questions: AskUserQuestion[]
}

export interface AskUserAnswer {
  selected_option_ids?: string[]
  selectedOptionIds?: string[]
  custom_text?: string | null
  customText?: string | null
}

export interface AskUserStructuredContent {
  askUser?: {
    phase?: AskUserPhase | string
    title?: string | null
    questions?: AskUserQuestion[]
    answers?: Record<string, AskUserAnswer>
  }
}

export interface ChatToolArtifact {
  id?: string | null
  name: string
  mime_type?: string
  mimeType?: string
  data_url?: string
  dataUrl?: string
  size_bytes?: number | null
  sizeBytes?: number | null
  path?: string | null
  filePath?: string | null
  localPath?: string | null
}

export type ChatMessageSegmentKind = 'text' | 'reasoning' | 'tool'

export type ChatMessageSegmentPhase = 'auxiliary' | 'plain' | 'tool_loop' | 'synthesis'

/** 降级兜底的结构化描述（后端 recovery.rs 产出）。 */
export interface DegradedAnswer {
  /** 稳定标识，前端据此选图标/措辞，不解析文案。 */
  kind:
    | 'rate_limited'
    | 'context_overflow'
    | 'timeout'
    | 'moderation'
    | 'empty_response'
    | 'unknown'
    | string
  /** 一行人读的失败原因。 */
  reason: string
  /** 供应商返回的原始报错（已剥壳裁剪）——真正能排查的那一句。 */
  detail?: string | null
  /** 本轮已完成的工具调用摘要。 */
  toolSummaries?: { name: string; preview: string }[]
  /** 纯文本版本，供不渲染卡片的场景回落。 */
  text: string
}

export interface ChatMessageSegment {
  id: string
  kind: ChatMessageSegmentKind
  phase: ChatMessageSegmentPhase
  order: number
  step_number?: number | null
  stepNumber?: number | null
  round?: number | null
  text?: string | null
  tool_call_id?: string | null
  toolCallId?: string | null
}

export interface ChatMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  attachments?: Attachment[]
  reasoning?: string
  artifacts?: ChatToolArtifact[]
  tool_calls?: ToolCallRecord[]
  toolCalls?: ToolCallRecord[]
  segments?: ChatMessageSegment[]
  agent_plan?: AgentPlanState | null
  agentPlan?: AgentPlanState | null
  api_messages?: unknown[]
  apiMessages?: unknown[]
  model_messages?: unknown[]
  modelMessages?: unknown[]
  active_skill_id?: string | null
  activeSkillId?: string | null
  run_entry?: 'send' | 'regenerate' | string | null
  runEntry?: 'send' | 'regenerate' | string | null
  stream_outcome?: 'completed' | 'cancelled' | 'error' | 'interrupted' | string | null
  streamOutcome?: 'completed' | 'cancelled' | 'error' | 'interrupted' | string | null
  /**
   * 降级兜底：模型调用失败、但本轮已有工具结果时由后端填充。
   * 渲染成独立错误卡片 —— 正文 content 不再混入故障文案。
   */
  degraded?: DegradedAnswer | null
  /** Provider 报告的本条回复真实 token 用量（规划/合成/压缩累计）；不报告时缺省。 */
  usage?: MessageUsage | null
  /** 多模型一问多答：同一条 user 消息 fan-out 出的 N 条 assistant 共享同一个 group id；单模型为空。 */
  group_id?: string | null
  groupId?: string | null
  /** 该 assistant 实际所用 provider id（多模型时每条各记自己的；单模型缺省回退会话级）。 */
  provider_id?: string | null
  providerId?: string | null
  /** 该 assistant 实际所用 model（多模型时每条各记自己的；单模型缺省回退会话级）。 */
  model?: string | null
  timestamp: number
}

export interface MessageUsage {
  input_tokens?: number | null
  inputTokens?: number | null
  output_tokens?: number | null
  outputTokens?: number | null
  total_tokens?: number | null
  totalTokens?: number | null
  cached_input_tokens?: number | null
  cachedInputTokens?: number | null
  cache_creation_input_tokens?: number | null
  cacheCreationInputTokens?: number | null
  reasoning_tokens?: number | null
  reasoningTokens?: number | null
}

export interface Attachment {
  id: string
  type: 'image' | 'file'
  name: string
  path: string
  /**
   * 内存虚拟文本附件（粘贴长文本生成的虚拟 txt）：正文随对话消息持久化，
   * 不生成独立磁盘文件；`path` 以 `memory://` 开头时存在。
   */
  content?: string
}

export interface PendingAttachment {
  id: string
  type: 'image' | 'file'
  name: string
  path: string
  /**
   * 内存文本附件（粘贴长文本自动生成的虚拟 txt）：正文只存浏览器内存、不落盘。
   * 存在即视为虚拟附件：点击卡片打开编辑弹窗而非系统打开；提交时随消息传给后端。
   */
  content?: string
}

export interface ChatProject {
  id: string
  name: string
  description?: string | null
  color?: string | null
  root_path?: string | null
  rootPath?: string | null
  created_at: number
  updated_at: number
  createdAt?: number
  updatedAt?: number
}

/** Chat 集(Set)：助手之上的人设分组。不带工作目录，持有系统提示词 + 默认助手。 */
export interface ChatSet {
  id: string
  name: string
  system_prompt?: string
  systemPrompt?: string
  default_assistant_id?: string | null
  defaultAssistantId?: string | null
  color?: string | null
  created_at: number
  updated_at: number
  createdAt?: number
  updatedAt?: number
}

export interface ChatAssistant {
  id: string
  name: string
  description?: string
  icon?: string
  color?: string
  source?: 'builtin' | 'user' | 'imported' | string
  system_prompt?: string
  systemPrompt?: string
  provider_id?: string
  providerId?: string
  model?: string
  /** 允许使用的 MCP 服务器 id 白名单。空 = 不可用任何 MCP。 */
  mcp_server_ids?: string[]
  mcpServerIds?: string[]
  /** 允许激活的技能 id 白名单。空 = 不可用任何技能。 */
  skill_ids?: string[]
  skillIds?: string[]
  enabled?: boolean
  installed?: boolean
  archived?: boolean
  built_in?: boolean
  builtIn?: boolean
  created_at: number
  updated_at: number
  createdAt?: number
  updatedAt?: number
}

export interface ChatAssistantSnapshot {
  id: string
  name: string
  description?: string
  source?: 'builtin' | 'user' | 'imported' | string
  system_prompt?: string
  systemPrompt?: string
  provider_id?: string
  providerId?: string
  model?: string
  mcp_server_ids?: string[]
  mcpServerIds?: string[]
  skill_ids?: string[]
  skillIds?: string[]
}

export type ContextUsageStatus =
  | 'normal'
  | 'warning'
  | 'critical'
  | 'compressed'
  | 'stale'
  | 'unknown'
  | string

export interface ContextUsageSegment {
  id: string
  label: string
  estimated_tokens?: number
  estimatedTokens?: number
  color?: string | null
}

export interface CompactionBoundaryRecord {
  id: string
  source_until_message_id?: string
  sourceUntilMessageId?: string
  display_after_message_id?: string | null
  displayAfterMessageId?: string | null
  token_estimate_before?: number
  tokenEstimateBefore?: number
  token_estimate_after?: number
  tokenEstimateAfter?: number
  summary_content?: string
  summaryContent?: string
  trigger?: 'manual' | 'auto' | 'agent_loop' | string
  created_at?: number
  createdAt?: number
}

export interface ConversationContextSummary {
  id: string
  content: string
  source_message_ids?: string[]
  sourceMessageIds?: string[]
  source_until_message_id?: string
  sourceUntilMessageId?: string
  token_estimate_before?: number
  tokenEstimateBefore?: number
  token_estimate_after?: number
  tokenEstimateAfter?: number
  created_at?: number
  createdAt?: number
  provider_id?: string
  providerId?: string
  model?: string
  stale?: boolean
}

export interface ConversationContextState {
  estimated_input_tokens?: number
  estimatedInputTokens?: number
  context_window_tokens?: number | null
  contextWindowTokens?: number | null
  context_window_estimated?: boolean
  contextWindowEstimated?: boolean
  usage_ratio?: number | null
  usageRatio?: number | null
  status?: ContextUsageStatus
  segments?: ContextUsageSegment[]
  last_measured_at?: number
  lastMeasuredAt?: number
  last_compressed_at?: number | null
  lastCompressedAt?: number | null
  compressed_message_count?: number
  compressedMessageCount?: number
  compression_count?: number
  compressionCount?: number
  summary?: ConversationContextSummary | null
  compaction_boundaries?: CompactionBoundaryRecord[]
  compactionBoundaries?: CompactionBoundaryRecord[]
  warning?: string | null
  warningMessage?: string | null
  context_source?: 'kivio_builtin' | 'external_cli' | string
  contextSource?: 'kivio_builtin' | 'external_cli' | string
  token_count_source?: 'cli_reported' | 'estimated' | 'provider_reported' | string
  tokenCountSource?: 'cli_reported' | 'estimated' | 'provider_reported' | string
  session_input_tokens?: number
  sessionInputTokens?: number
  session_output_tokens?: number
  sessionOutputTokens?: number
  external_agent_id?: string
  externalAgentId?: string
  external_model?: string
  externalModel?: string
}

export type AgentTodoStatus = 'pending' | 'in_progress' | 'completed' | 'cancelled'

export interface AgentTodoItem {
  id: string
  content: string
  description?: string | null
  status: AgentTodoStatus
  blocks?: string[]
  blocked_by?: string[]
  owner?: string | null
}

export interface AgentTodoState {
  items?: AgentTodoItem[]
  updated_at?: number
  updatedAt?: number
}

export type AgentPlanMode = 'act' | 'plan' | 'orchestrate'
export type AgentPlanStatus = 'empty' | 'draft' | 'approved'

export interface AgentPlanState {
  mode?: AgentPlanMode
  status?: AgentPlanStatus
  plan?: string | null
  updated_at?: number
  updatedAt?: number
}

export interface AgentRuntimeConfig {
  kind: 'builtin' | 'chat' | 'external'
  externalAgentId?: string | null
  external_agent_id?: string | null
  externalModel?: string | null
  external_model?: string | null
  externalReasoning?: string | null
  external_reasoning?: string | null
  externalSandbox?: string | null
  external_sandbox?: string | null
  externalAgentPreset?: string | null
  external_agent_preset?: string | null
}

/// 一条可从本地 CLI 导入的原生会话。后端 `ImportableSession` 的镜像。
export interface ImportableCliSession {
  agentId: string
  /** 该 CLI 自己的会话 id——续聊时 `--resume` / `session/load` 认的就是这个。 */
  sessionId: string
  title?: string | null
  /** 会话创建时所在的工作目录，必然等于当前项目根（后端已按此过滤）。 */
  cwd: string
  /** 最后活动时间，epoch **毫秒**（注意与 `ChatMessage.timestamp` 的秒不同）。 */
  updatedAt?: number | null
  /** `null` = 来源给不出条数（ACP `session/list` 不返回），界面显示"未知"而不是 0。 */
  messageCount?: number | null
  /** 由导入功能带进来的。 */
  alreadyImported: boolean
  /**
   * 已经有 Kivio 对话绑着这条原生会话时，那条对话的 id。
   *
   * **不等于 `alreadyImported`**：Kivio 自己创建的外部 CLI 对话运行时也会写绑定。
   * 两种都不能再导入（绑定是 1:1 的），但要说不同的话。
   */
  boundConversationId?: string | null
}

export interface CliImportResult {
  success: boolean
  imported: Array<{ agentId: string; sessionId: string; conversationId: string }>
  failures: Array<{ agentId: string; sessionId: string; error: string }>
}

export interface NativeProviderSummary {
  id: string
  name: string
  baseUrl?: string | null
  api?: string | null
  modelCount: number
  isDefault: boolean
}

export interface DetectedExternalAgent {
  id: string
  name: string
  available: boolean
  nativeProviders?: NativeProviderSummary[]
  path?: string | null
  version?: string | null
  models: Array<{ id: string; label: string; contextWindowTokens?: number | null; context_window_tokens?: number | null }>
  reasoningOptions?: Array<{ id: string; label: string }>
  reasoning_options?: Array<{ id: string; label: string }>
  sandboxOptions?: Array<{ id: string; label: string }>
  sandbox_options?: Array<{ id: string; label: string }>
  authStatus?: string | null
  auth_status?: string | null
  /** 设置页里被用户停用：不出现在运行时选择器，但已绑定它的旧会话照常。 */
  disabled?: boolean
  /** 该 CLI 的协议能否往在飞的轮次里注入一条用户消息（「立刻引导」）。 */
  supportsSteering?: boolean
  supports_steering?: boolean
  /** 该 CLI 是否支持在当前运行后原生排队继续处理（Pi / dsh）。 */
  supportsFollowUp?: boolean
  supports_follow_up?: boolean
}

export interface Conversation {
  id: string
  revision: number
  title: string
  provider_id: string
  model: string
  messages: ChatMessage[]
  active_skill_id?: string | null
  activeSkillId?: string | null
  assistant_id?: string | null
  assistantId?: string | null
  assistant_snapshot?: ChatAssistantSnapshot | null
  assistantSnapshot?: ChatAssistantSnapshot | null
  created_at: number
  updated_at: number
  pinned?: boolean
  /** 对话库归档：侧栏默认隐藏 */
  archived?: boolean
  folder?: string
  project_id?: string | null
  projectId?: string | null
  set_id?: string | null
  setId?: string | null
  context_state?: ConversationContextState
  contextState?: ConversationContextState
  agent_todo_state?: AgentTodoState
  agentTodoState?: AgentTodoState
  agent_plan_state?: AgentPlanState
  agentPlanState?: AgentPlanState
  agent_runtime?: AgentRuntimeConfig
  agentRuntime?: AgentRuntimeConfig
  knowledge_base_ids?: string[]
  knowledgeBaseIds?: string[]
  force_knowledge_search?: boolean
  forceKnowledgeSearch?: boolean
  thinking_level?: ThinkingLevel | null
  thinkingLevel?: ThinkingLevel | null
  /** 会话级三态联网搜索（任务 07-23）。缺省/null = 跟随全局 nativeTools.webSearch。 */
  web_search_mode?: WebSearchMode | null
  webSearchMode?: WebSearchMode | null
  /** 多模型一问多答（D2）：会话级持久化的多答模型集合（上限 4）。空或单元素 = 单模型现状。 */
  reply_models?: ModelRef[]
  replyModels?: ModelRef[]
  /** 多答组「选中条」（D5）：group_id → 被采纳进下一轮历史的 assistant message id。无记录取该组第一条。 */
  group_selections?: Record<string, string>
  groupSelections?: Record<string, string>
  /** 对话分支来源（方案 B）：本对话由某对话某消息处分叉而来。 */
  forked_from?: ForkOrigin | null
  forkedFrom?: ForkOrigin | null
}

/** 对话分支来源快照（方案 B）。 */
export interface ForkOrigin {
  conversation_id?: string
  conversationId?: string
  message_id?: string
  messageId?: string
  title: string
}

/** 一次回答所用的 (provider, model) 引用。多模型一问多答的会话级模型集元素。 */
export interface ModelRef {
  provider_id: string
  model: string
}

export type ThinkingLevel = 'off' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'

/** 会话级三态联网搜索模式（任务 07-23）。off=不联网；builtin=模型原生内置搜索；third_party=search_web 工具。 */
export type WebSearchMode = 'off' | 'builtin' | 'third_party'

export interface ConversationListItem {
  id: string
  revision?: number
  title: string
  preview: string
  provider_id: string
  model: string
  message_count: number
  created_at: number
  updated_at: number
  pinned?: boolean
  archived?: boolean
  folder?: string
  project_id?: string | null
  projectId?: string | null
  set_id?: string | null
  setId?: string | null
  assistant_id?: string | null
  assistantId?: string | null
  assistant_name?: string | null
  assistantName?: string | null
  agent_runtime?: AgentRuntimeConfig
  agentRuntime?: AgentRuntimeConfig
  forked_from?: ForkOrigin | null
  forkedFrom?: ForkOrigin | null
}

/** 全局搜索命中：列表项 + 首个匹配位置（后端 flatten 序列化）。 */
export interface ConversationSearchHit extends ConversationListItem {
  match_field?: string
  matchField?: string
  match_message_id?: string | null
  matchMessageId?: string | null
  match_snippet?: string | null
  matchSnippet?: string | null
}

/** 对话库书架 / 筛选维度 */
export type ConversationLibraryShelf =
  | 'all'
  | 'starred'
  | 'uncategorized'
  | 'recent7d'
  | 'archived'

export type ConversationLibrarySort = 'updated' | 'created' | 'title' | 'messages'
export type ConversationLibraryOrder = 'asc' | 'desc'
export type ConversationLibraryGroup = 'none' | 'day' | 'week' | 'project' | 'set'
export type ConversationLibraryDensity = 'comfortable' | 'compact'

export interface ConversationLibraryQuery {
  offset?: number
  limit?: number
  sort?: ConversationLibrarySort
  order?: ConversationLibraryOrder
  q?: string
  fullText?: boolean
  shelf?: ConversationLibraryShelf
  projectId?: string | null
  setId?: string | null
  assistantId?: string | null
  providerId?: string | null
  runtimeKind?: 'builtin' | 'external' | null
}

export interface ConversationLibraryPage {
  items: ConversationSearchHit[]
  total: number
}

export type ChatUserProfile = {
  displayName: string
  avatarUrl: string
}
