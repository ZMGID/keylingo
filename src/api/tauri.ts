// Tauri 前端与 Rust 后端的桥接模块
// 所有 invoke 调用和事件监听都集中在这里，作为前后端的统一接口层

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getVersion } from '@tauri-apps/api/app'
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window'
import { normalizeThemeColorId } from '../themeColors'
import {
  subscribeChatProtocol,
  subscribeChatProtocolIssues,
  subscribeChatPython,
  syncChatProtocol,
  type ChatProtocolDelivery,
  type ChatProtocolIssue,
} from './chatProtocol'
import type {
  ChatProtocolEvent,
  ChatRunEventEnvelope,
  ChatSegmentPayload as GeneratedChatSegmentPayload,
  ChatRunPythonPayload as GeneratedChatRunPythonPayload,
} from '../generated/chatProtocol'

// ========== 类型定义 ==========

/** 是否运行在 Tauri 运行时(而非纯浏览器/SSR) */
export const isTauriRuntime = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

export type LensWebSearchResult = {
  title: string
  url: string
  content: string
  publishedDate?: string | null
  score?: number | null
}

export type LensWebSearchState = {
  status: 'searching' | 'done' | 'skipped' | 'error'
  query?: string
  reason?: string
  results?: LensWebSearchResult[]
  error?: string
}

// Lens 多轮对话消息类型（视觉模型）
// reasoning：推理模型（DeepSeek-R1 等）的思维链文本，仅本地展示，不回传后端
export type ExplainMessage = {
  role: 'user' | 'assistant'
  content: string
  reasoning?: string
  webSearch?: LensWebSearchState
}

// Lens 流式输出负载（事件名 lens-stream）
// reasoningDelta：思维链增量（推理模型才会有）
export type LensStreamPayload = {
  imageId: string
  kind: 'answer'
  delta: string
  reasoningDelta?: string
  done?: boolean
  reason?: 'done' | 'cancelled' | 'error'
  full?: string
}

export type ChatStreamSegment = GeneratedChatSegmentPayload

export type ChatStreamPayload = Extract<
  ChatRunEventEnvelope,
  { type: 'run_started' | 'text_delta' | 'reasoning_delta' | 'run_completed' | 'run_cancelled' | 'run_failed' }
> & { restoredFromSnapshot?: boolean }

export type ChatExternalSendAttachment = {
  id: string
  type: 'image' | 'file'
  name: string
  path: string
}

export type ChatExternalSendRequest = {
  id: string
  content: string
  attachments: ChatExternalSendAttachment[]
  /** 可选的多轮历史。非空 → 用历史预置一个新会话（不发消息、不触发回复）。 */
  messages?: { role: string; content: string }[]
}

export type ChatContextUsageSegment = {
  id: string
  label: string
  estimated_tokens?: number
  estimatedTokens?: number
  color?: string | null
}

export type ChatContextSummary = {
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

export type CompactionBoundaryRecord = {
  id: string
  source_until_message_id?: string
  sourceUntilMessageId?: string
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

export type ChatContextState = {
  estimated_input_tokens?: number
  estimatedInputTokens?: number
  context_window_tokens?: number | null
  contextWindowTokens?: number | null
  context_window_estimated?: boolean
  contextWindowEstimated?: boolean
  usage_ratio?: number | null
  usageRatio?: number | null
  status?: string
  segments?: ChatContextUsageSegment[]
  last_measured_at?: number
  lastMeasuredAt?: number
  last_compressed_at?: number | null
  lastCompressedAt?: number | null
  compressed_message_count?: number
  compressedMessageCount?: number
  compression_count?: number
  compressionCount?: number
  summary?: ChatContextSummary | null
  compaction_boundaries?: CompactionBoundaryRecord[]
  compactionBoundaries?: CompactionBoundaryRecord[]
  warning?: string | null
  warningMessage?: string | null
}

export type ChatContextLiveUsage = {
  /** 此刻已用（分子）。口径与轮末权威值一致，真源在 Rust 侧。 */
  usedTokens: number
  /** 上下文窗口（分母）。`null` = 本次上报没带窗口，前端必须保留已知的旧值（分母粘滞）。 */
  contextWindowTokens?: number | null
}

/**
 * 上下文状态更新。两种形态共用这一条通道：
 * - `contextState` —— 轮末/手动刷新的**权威快照**（含分段、压缩计数、来源标签）。
 * - `live` —— 生成过程中的**活数**（只有分子 + 分母）。权威快照那次要读磁盘、列工具、算分段，
 *   不能放在每个增量上，所以实时这条刻意只带两个数。
 */
export type ChatContextPayload = {
  conversationId: string
  contextState?: ChatContextState
  live?: ChatContextLiveUsage
}

export type ChatCompactionPayload = {
  conversationId: string
  phase: 'started' | 'completed' | 'microcompacted' | 'failed' | string
  trigger?: 'manual' | 'auto' | 'agent_loop' | string
  boundary?: CompactionBoundaryRecord | null
}

export type ChatTodoStatus = 'pending' | 'in_progress' | 'completed'

export type ChatTodoItem = {
  id: string
  content: string
  status: ChatTodoStatus
}

export type ChatTodoState = {
  items?: ChatTodoItem[]
  updated_at?: number
  updatedAt?: number
}

export type ChatTodoPayload = {
  conversationId: string
  todoState: ChatTodoState
}

export type ChatPlanMode = 'act' | 'plan'
export type ChatPlanStatus = 'empty' | 'draft' | 'approved'

export type ChatPlanState = {
  mode?: ChatPlanMode
  status?: ChatPlanStatus
  plan?: string | null
  updated_at?: number
  updatedAt?: number
}

export type ChatPlanPayload = {
  conversationId: string
  planState: ChatPlanState
}

export type ChatToolStatus =
  | 'pending'
  | 'running'
  | 'success'
  | 'completed'
  | 'error'
  | 'skipped'
  | 'cancelled'

export type ChatToolArtifact = {
  name: string
  mime_type?: string
  mimeType?: string
  data_url?: string
  dataUrl?: string
  size_bytes?: number | null
  sizeBytes?: number | null
}

export type ChatToolProgressPayload = {
  conversationId: string
  runId: string
  messageId?: string
  toolCallId: string
  id?: string
  name: string
  source: string
  serverId?: string | null
  status: ChatToolStatus
  argumentsPreview?: string
  resultPreview?: string | null
  error?: string | null
  startedAt?: number | null
  completedAt?: number | null
  durationMs?: number | null
  round?: number
  sensitive?: boolean
  artifacts?: ChatToolArtifact[]
  traceId?: string | null
  spanId?: string | null
  structuredContent?: unknown
}

/** Live nested progress of a spawned sub-agent (P3), addressed to the parent
 *  tool card via `parentToolCallId`. */
export type ChatSubagentPayload = {
  parentConversationId: string
  parentRunId: string
  parentToolCallId: string
  taskId: string
  name: string
  /** 子代理实际运行的模型（含全局 Subagent 模型覆盖后的结果）。 */
  model?: string
  depth: number
  status: 'running' | 'completed' | 'failed' | 'cancelled'
  preview?: string
  steps?: string[]
}

export type AskUserPhase = 'awaiting' | 'answered' | 'skipped' | 'timeout' | 'cancelled'

export type AskUserOption = {
  id: string
  label: string
  description?: string | null
}

export type AskUserQuestion = {
  id: string
  prompt: string
  options: AskUserOption[]
  allow_multiple?: boolean
  allowMultiple?: boolean
  allow_custom?: boolean
  allowCustom?: boolean
}

export type AskUserPromptPayload = {
  title?: string | null
  questions: AskUserQuestion[]
}

export type AskUserAnswer = {
  selected_option_ids?: string[]
  selectedOptionIds?: string[]
  custom_text?: string | null
  customText?: string | null
}

export type ChatUserPromptPayload = {
  conversationId: string
  runId: string
  messageId?: string
  toolCallId: string
  id?: string
  name: string
  source: string
  prompt: AskUserPromptPayload
  structuredContent?: unknown
}

export type ChatToolConfirmPayload = {
  conversationId: string
  runId: string
  messageId?: string
  toolCallId: string
  name: string
  source: string
  serverId?: string | null
  /** 这次操作的对象（文件完整路径 / 命令首行），后端认得出时才有。用于拼自然语言标题。 */
  target?: string | null
  argumentsPreview?: string
  sensitivity?: string
}

export type ChatSessionConsentPayload = {
  conversationId: string
  runId: string
  messageId?: string
}

export type ChatToolDefinition = {
  id: string
  name: string
  description: string
  source: string
  serverId?: string | null
  serverName?: string | null
  inputSchema: unknown
  annotations?: unknown
  outputSchema?: unknown
  sensitive: boolean
}

export type ChatMcpServer = {
  id: string
  name: string
  enabled: boolean
  transport: 'stdio' | 'streamable_http' | string
  url: string
  command: string
  args: string[]
  env: Record<string, string>
  headers: Record<string, string>
  cwd?: string | null
  enabledTools: string[]
  /** 连接器目录 id（"github"/"notion"/... 或 "custom-xxx"）。非空表示由连接器页管理。 */
  connectorId?: string
  /** 连接器认证信息。Phase A 只用 { kind: 'token', accessToken }。 */
  auth?: {
    kind: string
    accessToken: string
    refreshToken?: string
    expiresAt?: number
    tokenEndpoint?: string
    clientId?: string
    scopes?: string[]
    /** 真实账户标识（邮箱 / 工作区名 / 用户名）。授权时尽力提取，拿不到则缺省。 */
    account?: string
  }
}

/** 单个 CLI 的 MCP 扫描分组。available = 该 CLI 配置文件存在（与是否解析出 server 无关）。 */
export type CliMcpGroup = {
  available: boolean
  servers: ChatMcpServer[]
}

/** 从本地 CLI 导入 MCP 的扫描结果（Claude Code / Codex / OpenCode / Pi 四组）。 */
export type CliImportScan = {
  claude: CliMcpGroup
  codex: CliMcpGroup
  opencode: CliMcpGroup
  pi: CliMcpGroup
}

/** MCP 持久连接状态，与后端 McpServerState（serde tag="kind"）一一对应。 */
export type McpServerState =
  | { kind: 'connecting' }
  | { kind: 'connected' }
  | { kind: 'error'; message: string }
  | { kind: 'disconnected' }

/** chat_mcp_server_status 命令返回的状态快照。 */
export type McpServerStatus = {
  serverId: string
  state: McpServerState
  handshakeCount: number
  stderrTail: string
}

/** mcp-server-state 事件载荷。serverName 在 reload/reap 路径可能缺省。 */
export type McpServerStatePayload = {
  serverId: string
  serverName?: string | null
  state: McpServerState
}

export type ChatNativeToolsConfig = {
  webSearch: boolean
  webFetch?: boolean
  skillRuntime?: boolean
  readFile?: boolean
  writeFile?: boolean
  editFile?: boolean
  runCommand?: boolean
  runPython?: boolean
  knowledgeSearch?: boolean
  workingDirectory?: string
  /** Legacy settings compatibility only. */
  workspaceRoots?: string[]
}

export type ChatRunPythonPayload = GeneratedChatRunPythonPayload

export type ChatPastedImageResult = {
  success: boolean
  path?: string
  name?: string
  error?: string | null
}

export type ChatClipboardFilesResult = {
  success: boolean
  files?: Array<{ path: string; name: string }>
  error?: string | null
}

export function defaultNativeTools(): ChatNativeToolsConfig {
  // Mirror the backend baseline (ChatNativeToolsConfig::default): native tools
  // are ON by default; safety is the execution-time consent gate. web_search
  // still only surfaces when a provider key is configured.
  return {
    webSearch: true,
    webFetch: true,
    skillRuntime: true,
    readFile: true,
    writeFile: true,
    editFile: true,
    runCommand: true,
    runPython: true,
    knowledgeSearch: true,
    workingDirectory: '',
    workspaceRoots: [],
  }
}

export type SkillFileEntry = {
  relativePath: string
  kind: 'skillmd' | 'reference' | 'script' | 'asset' | 'other' | string
  sizeBytes: number
}

export type AgentRuntimeConfig = {
  kind: 'builtin' | 'external'
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

export type ChatModeConfig = {
  systemPrompt?: string
  webSearch?: boolean
  webFetch?: boolean
  knowledgeSearch?: boolean
  memoryTools?: boolean
  mcpReadOnly?: boolean
}

export type ChatConfig = {
  streamEnabled?: boolean
  thinkingEnabled?: boolean
  maxOutputTokens?: number
  defaultLanguage?: string
  systemPrompt?: string
  userDisplayName?: string
  userAvatar?: string
  defaultAgentRuntime?: AgentRuntimeConfig
  /** 本地 CLI Agent 的每-CLI 覆盖，key = agent id。 */
  externalCliAgents?: Record<string, ExternalCliAgentConfig>
  /** Kivio Chat 运行时专属设置（与 Agent 分离）。 */
  chatMode?: ChatModeConfig
}

export type ExternalCliAgentConfig = {
  disabled?: boolean
  /** 自定义可执行文件路径；空 = 走 PATH 探测。 */
  path?: string
  env?: Array<{ key: string; value: string }>
  customModels?: Array<{ id: string; label: string }>
  /** 该 CLI 的第三方供应商（中转站）列表。 */
  providers?: ExternalCliProvider[]
  /** 当前默认供应商 id；Pi / OpenCode / dsh 的 providers 会全部并存。空 = 用 CLI 自己的默认配置。 */
  currentProvider?: string
}

/**
 * 一个第三方供应商。各 CLI 用到的字段不同：claude / gemini 用 `env`，
 * codex 用 `configToml` + `authJson`（物化成私有 CODEX_HOME）；
 * grok 用 `configToml`（把 models / model 段合并进 `~/.grok/config.toml`）；
 * kimi 用 `configToml`（把 providers / models / default_model 合并进 `~/.kimi-code/config.toml`）；
 * OpenCode / Pi 用 `configJson` + `authJson` + `defaultModel` 合并进原生配置；
 * dsh 用 `configJson` 在 Kivio 私有 profile 中挂载 `llm-pi-ai`，Key 通过 `env` 注入；
 * Pi 另用 `defaultReasoning` 写入终端默认 thinking 档位。
 */
export type ExternalCliProvider = {
  id: string
  name: string
  /** Disabled providers stay saved but are not materialized or injected. */
  disabled?: boolean
  remark?: string
  env?: Array<{ key: string; value: string }>
  configToml?: string
  configJson?: string
  authJson?: string
  /** Kivio-only model override state; never written into the CLI native config. */
  modelMetadataJson?: string
  /** Stable route/provider id used by OpenCode, Pi, and dsh model references. */
  nativeProviderId?: string
  defaultModel?: string
  defaultReasoning?: string
}

export type ChatMemoryConfig = {
  enabled: boolean
  toolWriteConfirm: boolean
}

export type ChatMemoryLayerContent = {
  layer: 'l1' | 'l2' | string
  content: string
  bytes: number
  maxBytes?: number | null
}

export type ChatMemoryState = {
  success: boolean
  l1: ChatMemoryLayerContent
  l2: ChatMemoryLayerContent
  dir: string
}

/** 对话生命周期 Hook 事件（对齐 Rust `chat::hooks::HookEvent`，8 个，顺序即对话流）。 */
export const HOOK_EVENTS = [
  'agent_start',
  'turn_start',
  'message_start',
  'message_end',
  'tool_execution_start',
  'tool_execution_end',
  'turn_end',
  'agent_end',
] as const

export type HookEvent = typeof HOOK_EVENTS[number]

/** 镜像 Rust `settings::HookDef`（camelCase 序列化）。 */
export type HookDef = {
  id: string
  name: string
  description: string
  event: HookEvent | string
  enabled: boolean
  type: 'command' | 'http'
  /** type === 'command' 时的脚本正文。 */
  script: string
  /** type === 'http' 时的目标。 */
  url: string
  method: string
  headers: Record<string, string>
  timeoutMs: number
}

/** Hook 执行失败上报。fire-and-forget，只展示警告不打断对话。 */
export type ChatHookPayload = {
  conversationId: string
  runId: string
  hookName: string
  event: string
  message: string
}

export type ChatToolsConfig = {
  enabled: boolean
  servers: ChatMcpServer[]
  /** 对话生命周期 Hooks。空数组 = 无 Hook = agent loop 零开销。 */
  hooks?: HookDef[]
  skillScanPaths: string[]
  skillAutoMatch?: boolean
  skillFallbackMode?: 'progressive' | 'skill_md_only' | 'legacy_full_body' | string
  /** Skill ids turned off in Settings; omitted ids are enabled. */
  disabledSkillIds?: string[]
  maxToolRounds: number | null
  toolTimeoutMs: number
  /** MCP 持久连接空闲超时（ms）：会话空闲超过此值后被回收，下次调用透明重连。 */
  mcpIdleTimeoutMs?: number
  approvalPolicy: 'readonly_auto_sensitive_confirm' | 'always_confirm' | 'auto' | string
  /** 同一时刻最多并行运行的子 agent 数（后端钳制 1..64，默认 12）。 */
  subAgentConcurrency?: number
  /** 子代理全局模型覆盖（providerId+model，皆空 = 跟随主对话模型）。agent 定义的 model 字段仍优先。 */
  subAgentProviderId?: string
  subAgentModel?: string
  /** 开发者「请求调试」开关：开启后每次 provider 调用被记录到内存环形缓冲（脱敏）。默认关。 */
  requestDebugEnabled?: boolean
  nativeTools: ChatNativeToolsConfig
}

export type SkillMeta = {
  id: string
  name: string
  description: string
  source: string
  path?: string | null
  recommendedTools: string[]
  disableModelInvocation?: boolean
  files?: SkillFileEntry[]
  triggers?: string[]
  argumentHint?: string | null
  arguments?: string[]
}

export type SkillDetail = SkillMeta & {
  body: string
}

/** Background tasks 面板的一条任务：内置后台命令或外部 CLI 后台任务（后台 Bash / 后台子代理）。 */
export type BackgroundTaskInfo = {
  id: string
  source: 'builtin' | 'external'
  /** builtin 恒为 'bash'；external 为 claude 的 task_type（local_bash / local_agent / …）。 */
  kind: string
  /** 命令行或任务描述。 */
  title: string
  status: 'running' | 'completed' | 'failed' | 'stopped'
  exitCode?: number | null
  pid?: number | null
  cwd?: string
  summary?: string | null
  conversationId?: string
  /** 仅 running 有展示意义（终态不显示时长）。 */
  elapsedSecs: number
  startedAtMs: number
}

// Lens 联网搜索状态/结果负载（事件名 lens-web-search）
export type LensWebSearchPayload = {
  imageId: string
  status: 'searching' | 'done' | 'skipped' | 'error'
  query?: string
  reason?: string
  results?: LensWebSearchResult[]
  error?: string
}

// 截图翻译流式负载（事件名 lens-translate-stream）
// kind: 'original' = OCR 阶段；'translated' = 翻译阶段
export type LensTranslateStreamPayload = {
  imageId: string
  kind?: 'original' | 'translated'
  delta?: string
  done?: boolean
  success?: boolean
  error?: string | null
}

export type LensReplaceGroup = {
  id: string
  leafIds: string[]
  sourceText: string
  translated: string
}

export type LensReplaceRenderSlot = {
  id: string
  groupId: string
  leafIds: string[]
  bounds: { x: number; y: number; width: number; height: number }
  anchor: { x: number; y: number; baselineY: number }
  flow: 'exact_line' | 'paragraph_flow' | 'cell_flow' | 'scene_patch'
  kind: 'cell' | 'line' | 'paragraph' | 'heading'
  align: 'left' | 'center' | 'right'
  verticalAlign: 'top' | 'center'
  sourceFontPx: number
  sourceColor: string
}

export type LensReplaceStreamPayload = {
  version: 2
  imageId: string
  phase: 'ocr' | 'processing' | 'done' | 'error'
  groups: LensReplaceGroup[]
  slots: LensReplaceRenderSlot[]
  cleanedImage?: string | null
  // 硬失败（整张替换翻译不可用）才带 error；局部降级（如修复回退、个别区域回退原文）只带 warning。
  error?: string | null
  warning?: string | null
}

function isReplacePayloadRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function replacePayloadNumber(record: Record<string, unknown>, key: string): number {
  const value = record[key]
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new Error(`lens-replace-stream.${key} must be a finite number`)
  }
  return value
}

function parseReplaceBounds(value: unknown): LensReplaceRenderSlot['bounds'] {
  if (!isReplacePayloadRecord(value)) throw new Error('lens-replace-stream slot bounds are invalid')
  const bounds = {
    x: replacePayloadNumber(value, 'x'),
    y: replacePayloadNumber(value, 'y'),
    width: replacePayloadNumber(value, 'width'),
    height: replacePayloadNumber(value, 'height'),
  }
  if (bounds.width <= 0 || bounds.height <= 0) throw new Error('lens-replace-stream slot bounds must be positive')
  return bounds
}

function parseReplaceStringArray(value: unknown, label: string): string[] {
  if (!Array.isArray(value) || value.some(item => typeof item !== 'string')) {
    throw new Error(`lens-replace-stream ${label} must be a string array`)
  }
  return value
}

function parseReplaceOptionalString(record: Record<string, unknown>, key: string): string | null | undefined {
  const value = record[key]
  if (value === undefined || value === null || typeof value === 'string') return value
  throw new Error(`lens-replace-stream.${key} must be a string or null`)
}

export function parseLensReplaceStreamPayload(value: unknown): LensReplaceStreamPayload {
  if (!isReplacePayloadRecord(value) || value.version !== 2) {
    throw new Error('lens-replace-stream requires protocol version 2')
  }
  if (typeof value.imageId !== 'string' || !value.imageId) throw new Error('lens-replace-stream.imageId is invalid')
  if (!['ocr', 'processing', 'done', 'error'].includes(String(value.phase))) {
    throw new Error('lens-replace-stream.phase is invalid')
  }
  if (!Array.isArray(value.groups) || !Array.isArray(value.slots)) {
    throw new Error('lens-replace-stream groups and slots must be arrays')
  }
  const groupIds = new Set<string>()
  const groups = value.groups.map((item, index): LensReplaceGroup => {
    if (!isReplacePayloadRecord(item)) throw new Error(`lens-replace-stream.groups[${index}] is invalid`)
    if (typeof item.id !== 'string' || !item.id || groupIds.has(item.id)) {
      throw new Error(`lens-replace-stream.groups[${index}].id is invalid or duplicate`)
    }
    if (typeof item.sourceText !== 'string' || typeof item.translated !== 'string') {
      throw new Error(`lens-replace-stream.groups[${index}] text is invalid`)
    }
    groupIds.add(item.id)
    return {
      id: item.id,
      leafIds: parseReplaceStringArray(item.leafIds, `groups[${index}].leafIds`),
      sourceText: item.sourceText,
      translated: item.translated,
    }
  })
  const slotIds = new Set<string>()
  const slots = value.slots.map((item, index): LensReplaceRenderSlot => {
    if (!isReplacePayloadRecord(item)) throw new Error(`lens-replace-stream.slots[${index}] is invalid`)
    if (typeof item.id !== 'string' || !item.id || slotIds.has(item.id)) {
      throw new Error(`lens-replace-stream.slots[${index}].id is invalid or duplicate`)
    }
    if (typeof item.groupId !== 'string' || !groupIds.has(item.groupId)) {
      throw new Error(`lens-replace-stream.slots[${index}].groupId is unknown`)
    }
    if (!isReplacePayloadRecord(item.anchor)) throw new Error(`lens-replace-stream.slots[${index}].anchor is invalid`)
    if (!['exact_line', 'paragraph_flow', 'cell_flow', 'scene_patch'].includes(String(item.flow))) {
      throw new Error(`lens-replace-stream.slots[${index}].flow is invalid`)
    }
    if (!['cell', 'line', 'paragraph', 'heading'].includes(String(item.kind))) {
      throw new Error(`lens-replace-stream.slots[${index}].kind is invalid`)
    }
    if (!['left', 'center', 'right'].includes(String(item.align))) {
      throw new Error(`lens-replace-stream.slots[${index}].align is invalid`)
    }
    if (!['top', 'center'].includes(String(item.verticalAlign))) {
      throw new Error(`lens-replace-stream.slots[${index}].verticalAlign is invalid`)
    }
    if (typeof item.sourceColor !== 'string' || typeof item.sourceFontPx !== 'number' || item.sourceFontPx <= 0) {
      throw new Error(`lens-replace-stream.slots[${index}] style is invalid`)
    }
    slotIds.add(item.id)
    return {
      id: item.id,
      groupId: item.groupId,
      leafIds: parseReplaceStringArray(item.leafIds, `slots[${index}].leafIds`),
      bounds: parseReplaceBounds(item.bounds),
      anchor: {
        x: replacePayloadNumber(item.anchor, 'x'),
        y: replacePayloadNumber(item.anchor, 'y'),
        baselineY: replacePayloadNumber(item.anchor, 'baselineY'),
      },
      flow: item.flow as LensReplaceRenderSlot['flow'],
      kind: item.kind as LensReplaceRenderSlot['kind'],
      align: item.align as LensReplaceRenderSlot['align'],
      verticalAlign: item.verticalAlign as LensReplaceRenderSlot['verticalAlign'],
      sourceFontPx: item.sourceFontPx,
      sourceColor: item.sourceColor,
    }
  })
  if (value.phase === 'done' && (groups.length === 0 || slots.length === 0)) {
    throw new Error('lens-replace-stream done payload requires groups and slots')
  }
  return {
    version: 2,
    imageId: value.imageId,
    phase: value.phase as LensReplaceStreamPayload['phase'],
    groups,
    slots,
    cleanedImage: parseReplaceOptionalString(value, 'cleanedImage'),
    error: parseReplaceOptionalString(value, 'error'),
    warning: parseReplaceOptionalString(value, 'warning'),
  }
}

// Lens 屏幕窗口元信息（macOS 实际数据；Windows 空数组）
export type LensWindowInfo = {
  id: number
  owner: string
  title: string
  x: number
  y: number
  width: number
  height: number
}

// 模型能力与定价信息（来自内置数据库或用户自定义）
export type ModelInfo = {
  displayName?: string
  contextWindow?: number
  maxOutput?: number
  /** 模型级采样温度；未设置时请求不发送 temperature。 */
  temperature?: number
  /** 用户显式清空温度时阻止回落到模型库默认值。仅用于 modelOverrides。 */
  omitTemperature?: boolean
  capabilities?: {
    vision?: boolean
    functionCalling?: boolean
    reasoning?: boolean
    streaming?: boolean
    webSearch?: boolean
    imageGeneration?: boolean
    embedding?: boolean
  }
  /** 嵌入模型的向量维度（默认/原生）。 */
  dimensions?: number
  /** 是否多语言（嵌入模型尤其关心）。 */
  multilingual?: boolean
  pricing?: {
    input?: number
    output?: number
    cachedInput?: number
  }
  /** 每模型额外请求体字段（原样 merge 进 chat/completions body 根部）。用于给严格 OpenAI-compat
   *  端点塞标准 schema 之外的私有旋钮，如 NVIDIA NIM / vLLM 的 chat_template_kwargs。仅用于 modelOverrides。 */
  extraBody?: Record<string, unknown>
  /** 该模型可选的思考等级（reasoning effort）。undefined=跟随模型库；`[]`=没有等级旋钮（请求不带
   *  等级字段）；否则只这几档可选可下发。模型库条目也用这个字段，用户覆盖优先。 */
  reasoningEfforts?: string[]
}

// AI 模型提供商配置
// apiKeys 支持多 key failover：第一个为主 key，其余为备用 key；
// 当某个 key 触发限流/配额/鉴权失败时后端会自动切下一个。
export type ProviderRequestConfig = {
  /** 附加到该供应商所有请求上的自定义头。同名时覆盖 CLI 身份预设。 */
  customHeaders?: { key: string; value: string }[]
  /** 是否跟随系统代理。默认 true；关掉走直连。 */
  useSystemProxy?: boolean
  /**
   * 遗留 on/off。sanitize/normalize 时迁移进 `promptCacheRetention`，新配置不再写入。
   * @deprecated 使用 promptCacheRetention
   */
  promptCaching?: boolean | null
  /**
   * Prompt 缓存策略（对齐 pi）：`none` | `short`（默认）| `long`。
   * short = 发默认档字段；long = 叠加长 TTL（Anthropic 1h / OpenAI 24h）；none = 不发。
   */
  promptCacheRetention?: 'none' | 'short' | 'long' | string
  /** '' 关闭 | 'claude_code' | 'codex' | 'grok' */
  cliIdentity?: string
  /** 身份版本号，空则用内置常量 */
  cliIdentityVersion?: string
}

export type ModelProvider = {
  id: string
  name: string
  apiKeys: string[]
  baseUrl: string
  availableModels: string[]
  enabledModels: string[]
  enabled: boolean
  apiFormat: string
  // 对请求体做 gzip 压缩再发送。默认 false。仅用于个别前置 WAF 会扫明文请求体、
  // 把 agent 工具/系统提示里的 shell/路径/SQL 文本误判为攻击而返回 403 的供应商。
  compressRequestBody?: boolean
  modelOverrides?: Record<string, ModelInfo>
  /** 「请求配置」：自定义头 / 代理 / prompt 缓存 / CLI 身份 */
  request?: ProviderRequestConfig
}

// 提供商连接测试输入（支持使用未保存的配置进行测试）
export type ProviderConnectionInput = {
  id?: string
  baseUrl: string
  apiKeys: string[]
  model?: string
  apiFormat?: string
  /** 编辑中（可能尚未保存）的请求配置。不传则后端回落已保存的那份。 */
  request?: ProviderRequestConfig
}

export type DefaultModelSelection = {
  providerId: string
  model: string
}

export type DefaultModelsConfig = {
  chat: DefaultModelSelection
  vision: DefaultModelSelection
  titleSummary: DefaultModelSelection
  compression: DefaultModelSelection
  imageGeneration: DefaultModelSelection
  advisor: DefaultModelSelection
}

// 应用设置数据结构
export type OcrEngine = 'off' | 'system' | 'rapid_ocr'
export type PdfStrategy = 'text' | 'force_ocr'

/** 第三方文档解析服务（MinerU / LlamaParse）。 */
export type DocProcessorProvider = {
  id: string
  name: string
  /** 'mineru' | 'llamaparse' */
  kind: string
  apiKeys: string[]
  baseUrl: string
  enabled: boolean
}

/** 知识库文档处理配置：Kivio 内置解析 + 可选第三方解析服务。 */
export type DocumentProcessingConfig = {
  ocrEngine: OcrEngine
  /** RapidOCR 模型档位(ocrEngine==='rapid_ocr' 时生效)。缺省 'high'(入库要精度)。 */
  rapidOcrTier?: RapidOcrTier
  pdfStrategy: PdfStrategy
  /** '' = Kivio 内置；否则为某第三方 provider id。 */
  activeProcessor: string
  /** 内置解析失败（如扫描版 PDF）时回退到第一个启用的第三方服务。 */
  fallbackToThirdParty: boolean
  providers: DocProcessorProvider[]
}

/** 知识库检索配置：hybrid(向量+关键词 RRF) 权重 + 可选全局 rerank。 */
export type KnowledgeBaseConfig = {
  /** 关键词(BM25)+向量 hybrid 融合开关（关掉=纯向量）。 */
  hybridEnabled: boolean
  weightVector: number
  weightKeyword: number
  /** 全局 rerank：留空即关闭。providerId 引用 providers[]。 */
  rerankProviderId: string
  rerankModel: string
  /** 入库分块目标 tokens（256–8192；只影响新导入/重建）。 */
  chunkTokens: number
  /** knowledge_search 默认返回片段数（1–20）；即最终上下文片段数 contextTopK。 */
  topK: number
  /** 每库融合候选池大小（20–200）。 */
  candidateK: number
  /** 送 rerank 的候选数（5–50；仅 rerank 开启时生效）。 */
  rerankTopK: number
  /** 相关性阈值（0–1；0=关闭）。rerank 开=对齐 relevance 分；关=向量命中的余弦相似度下限。 */
  minScore: number
}

export type WebSearchProviderId =
  | 'tavily'
  | 'exa'
  | 'exa_mcp'
  | 'ollama'
  | 'grok'
  | 'brave'
  | 'serper'
  | 'bocha'
  | 'zhipu'
  | 'tinyfish'
  | 'tinyfish_mcp'
  | 'searxng'

export type WebSearchMcpAuth = NonNullable<ChatMcpServer['auth']>

export type WebSearchConfig = {
  enabled: boolean
  provider: WebSearchProviderId
  tavilyApiKey: string
  tavilyBaseUrl?: string
  exaApiKey: string
  exaBaseUrl?: string
  exaMcpUrl?: string
  ollamaApiKey?: string
  ollamaBaseUrl?: string
  grokApiKey?: string
  grokModel?: string
  grokBaseUrl?: string
  grokSystemPrompt?: string
  braveApiKey?: string
  braveBaseUrl?: string
  serperApiKey?: string
  serperBaseUrl?: string
  bochaApiKey?: string
  bochaBaseUrl?: string
  zhipuApiKey?: string
  zhipuBaseUrl?: string
  tinyfishApiKey?: string
  tinyfishBaseUrl?: string
  tinyfishMcpUrl?: string
  tinyfishMcpAuth?: WebSearchMcpAuth | null
  searxngBaseUrl?: string
  maxResults: number
  searchDepth: 'ultra-fast' | 'fast' | 'basic' | 'advanced'
}

export type Settings = {
  hotkey: string
  chatHotkey: string
  /** 关闭 AI 客户端（chat 窗口）的全局热键。 */
  closeChatHotkey: string
  theme: 'system' | 'light' | 'dark'
  themeColor: string
  translucentSidebar: boolean
  uiFontScale?: number
  uiFontFamily?: string
  uiFontMono?: string
  targetLang: string
  autoPaste: boolean
  launchAtStartup: boolean
  /** 启动后不打开聊天窗口，进程留在托盘（适合开机自启后后台常驻） */
  launchMinimizedToTray: boolean
  translatorProviderId: string
  translatorModel: string
  chatProviderId: string
  chatModel: string
  defaultModels: DefaultModelsConfig
  chat?: ChatConfig
  chatMemory?: ChatMemoryConfig
  translatorPrompt?: string
  providers: ModelProvider[]
  chatTools: ChatToolsConfig
  documentProcessing?: DocumentProcessingConfig
  knowledgeBase?: KnowledgeBaseConfig
  /** 供应商自定义图标：provider id → 图标 key（见 chat/ModelIcon 的 PROVIDER_BRANDS） */
  providerIcons?: Record<string, string>
  retryEnabled: boolean
  retryAttempts: number
  screenshotTranslation: {
    enabled: boolean
    hotkey: string
    textHotkey: string
    replaceHotkey?: string
    replaceEnabled?: boolean
    providerId: string
    model: string
    directTranslate?: boolean
    /** 思考模式开关（默认 false）。OCR 模型 + 翻译模型都会注入对应字段 */
    thinkingEnabled?: boolean
    /** 流式输出开关（默认 true）。OCR + 翻译两步都用 SSE，token 逐字到达 */
    streamEnabled?: boolean
    /** 截图后是否保持全屏覆盖（默认 true）。false 时截图后窗口缩小为浮动 */
    keepFullscreenAfterCapture?: boolean
    /** 快速翻译结果卡左右宽度(px)。截图翻译与选中文本翻译共用，统一且可调（默认 480） */
    cardWidth?: number
    /** 使用系统 OCR(macOS Apple Vision / Windows OCR) 做文字识别,然后让 provider 翻译纯文本(默认 false)。
     *  true 时 provider 可以是任意文字模型;false 时 provider 必须是多模态视觉模型。
     *  从 vNext 起作 ocrMode 的降级镜像保留:System→true，其它→false。新代码应读 ocrMode。 */
    useSystemOcr?: boolean
    /** OCR 引擎选择(vNext+):
     *  - 'cloud_vision': 现有云端多模态 provider 一次完成 OCR+翻译
     *  - 'system': macOS Apple Vision / Windows.Media.Ocr 识别后交 provider 翻译
     *  - 'rapid_ocr': 本地 RapidOCR (PaddleOCR ONNX) 识别后交 provider 翻译。
     *    模型文件 + onnxruntime dylib 由用户在设置页面下载,安装包不带。
     *  缺省时由后端 sanitize_settings 按 useSystemOcr 自动迁移。 */
    ocrMode?: 'cloud_vision' | 'system' | 'rapid_ocr'
    /** RapidOCR 模型档位(ocrMode==='rapid_ocr' 时生效;替换翻译也跟随此档位)。
     *  缺省 'standard'(PP-OCRv5 mobile,快),'high' = PP-OCRv6 medium 高精度。 */
    rapidOcrTier?: RapidOcrTier
    /** 截图(OCR/视觉)翻译自定义提示词。空 → 内置截图模板 */
    prompt?: string
    /** 选中文本翻译自定义提示词。空 → 内置选中文本模板 */
    textPrompt?: string
    /** 替换翻译自定义提示词（仅翻译规则，JSON 输出契约固定）。空 → 内置替换模板 */
    replacePrompt?: string
  }
  /** 独立截图标注（截图 → 箭头/矩形/马赛克 → 复制/保存） */
  screenshotAnnotate?: {
    enabled: boolean
    hotkey: string
  }
  lens: {
    enabled: boolean
    hotkey: string
    providerId?: string
    model?: string
    defaultLanguage?: string
    streamEnabled?: boolean
    /** 思考模式开关（默认 true）。false 时 body 注入各厂商关闭思考的字段并集 */
    thinkingEnabled?: boolean
    systemPrompt?: string
    questionPrompt?: string
    /** 默认把 Lens 提问发送到 AI 客户端；关闭后使用旧的 Lens 浮窗内回答 */
    sendToChat?: boolean
    /** 消息排序：'asc' 老到新（默认），'desc' 新到老 */
    messageOrder?: 'asc' | 'desc'
    /** 进入截图选择态时是否显示顶部提示（默认 true） */
    showCaptureHint?: boolean
    /** Lens 联网搜索配置 */
    webSearch?: WebSearchConfig
  }
  settingsLanguage?: 'zh' | 'en'
  /** 首次使用引导：`pending` | `completed` | `skipped` */
  onboardingStatus?: 'pending' | 'completed' | 'skipped'
  /** 启动时静默检查 GH Releases 是否有新版（默认 true） */
  autoCheckUpdate?: boolean
  /** 截图自动归档开关（默认 false） */
  imageArchiveEnabled?: boolean
  /** 自动归档目标目录路径 */
  imageArchivePath?: string
  /** Obsidian 笔记库本地路径（空表示未配置） */
  obsidianVaultPath?: string
  /** 收藏并置顶的模型键（"providerId:model"）；顺序即置顶顺序。chat 模型选择器用。 */
  favoriteModels?: string[]
}

/** 能力插件（领域 CLI 等）状态 —— 设置 → 插件 */
export type PluginStatus = {
  id: string
  name: string
  description: string
  binary: string
  tags: string[]
  homepage: string
  repo: string
  installed: boolean
  enabled: boolean
  version: string | null
  path: string | null
  /** kivio | system | none */
  source: string
  /** 安装时落盘、启用才注入的 Skill */
  hasSkill: boolean
  /** 启用时挂到 chatTools.servers 的 MCP */
  hasMcp: boolean
  skillIds: string[]
  /** 本插件配置的 Skill 数量 */
  skillCount: number
  /** 本插件配置的 MCP 数量（通常 0/1） */
  mcpCount: number
  /** 启用后 Skill 文件是否已就绪 */
  skillActive: boolean
  /** 启用后 MCP 是否已写入 settings 且 enabled */
  mcpActive: boolean
  mcpServerId: string | null
  /** 当前系统是否可自动安装（安装命令不回传前端） */
  canInstall?: boolean
}

export type PluginActionResult = {
  ok: boolean
  message: string
  status: PluginStatus
}

/** 笔记元信息（列表用） */
export type NoteMeta = {
  id: string
  title: string
  /** 列表卡片预览：正文压成单行的前若干字符 */
  preview: string
  /** 单层文件夹名；空串 = 库根 */
  folder: string
  /** 'user' | 'chat' */
  origin: string
  createdAt: string
  updatedAt: string
}

/** 完整笔记（编辑器用） */
export type Note = {
  id: string
  title: string
  content: string
  /** 单层文件夹名；空串 = 库根 */
  folder: string
  /** 'user' | 'chat' */
  origin: string
  createdAt: string
  updatedAt: string
}

/** 交给 Kivio AI 的安装任务（含官方 README URL + Kivio 契约） */
export type PluginInstallBrief = {
  pluginId: string
  pluginName: string
  conversationTitle: string
  /** 官方 README raw URL，安装时须先 fetch */
  readmeUrls: string[]
  userMessage: string
}

export type UsageRange = 'today' | '1d' | '7d' | '30d'

export type UsageStatsQuery = {
  range?: UsageRange
  source?: string
  status?: string
  providerSearch?: string
  modelSearch?: string
  limit?: number
  offset?: number
}

export type UsageRecord = {
  id: string
  createdAt: number
  completedAt: number
  durationMs: number
  firstTokenMs?: number | null
  source: string
  operation: string
  providerId: string
  providerName: string
  model: string
  apiFormat: string
  status: string
  statusCode?: number | null
  usageSource: string
  inputTokens?: number | null
  outputTokens?: number | null
  totalTokens?: number | null
  cachedInputTokens?: number | null
  cacheCreationInputTokens?: number | null
  reasoningTokens?: number | null
  reasoningEffort?: string | null
  costUsd?: number | null
  costSource: string
  conversationId?: string | null
  messageId?: string | null
  errorKind?: string | null
}

export type UsageSummary = {
  totalRequests: number
  successfulRequests: number
  failedRequests: number
  missingUsageRequests: number
  providerReportedRequests: number
  totalTokens: number
  inputTokens: number
  outputTokens: number
  cachedInputTokens: number
  cacheCreationInputTokens: number
  reasoningTokens: number
  totalCostUsd: number
  averageDurationMs?: number | null
}

export type UsageTrendPoint = {
  date: string
  label: string
  requests: number
  totalTokens: number
  inputTokens: number
  outputTokens: number
  cachedInputTokens: number
  cacheCreationInputTokens: number
  costUsd: number
}

export type UsageGroupStats = {
  id: string
  label: string
  providerId?: string | null
  providerName?: string | null
  model?: string | null
  requestCount: number
  successCount: number
  totalTokens: number
  inputTokens: number
  outputTokens: number
  cachedInputTokens: number
  cacheCreationInputTokens: number
  costUsd: number
  averageDurationMs?: number | null
  lastUsedAt?: number | null
}

export type UsageStatsResponse = {
  summary: UsageSummary
  trend: UsageTrendPoint[]
  logs: UsageRecord[]
  providerStats: UsageGroupStats[]
  modelStats: UsageGroupStats[]
  totalLogs: number
  skippedRecords: number
}

/** 一条 provider 请求调试记录（内存环形缓冲，脱敏，仅开发者面板可见）。 */
export type RequestDebugRecord = {
  id: string
  createdAt: number
  durationMs: number
  providerId: string
  providerName: string
  model: string
  apiFormat: string
  operation: string
  source: string
  conversationId?: string | null
  messageId?: string | null
  status: string
  request: {
    url: string
    headers: Record<string, string>
    body: unknown
    stream: boolean
  }
  response: {
    statusCode?: number | null
    text?: string | null
    reasoning?: string | null
    toolCalls?: unknown
    finishReason?: string | null
    usage?: {
      // 后端 ModelUsage 无 rename_all，实际落盘是 snake_case；camelCase 字段保留以防未来统一。
      inputTokens?: number | null
      outputTokens?: number | null
      totalTokens?: number | null
      cachedInputTokens?: number | null
      cacheCreationInputTokens?: number | null
      reasoningTokens?: number | null
      input_tokens?: number | null
      output_tokens?: number | null
      total_tokens?: number | null
      cached_input_tokens?: number | null
      cache_creation_input_tokens?: number | null
      reasoning_tokens?: number | null
    } | null
    error?: string | null
  }
}

/** 更新检查结果（来自后端 GitHub Releases API 调用） */
export type UpdateInfo = {
  available: boolean
  /** true = 检查本身失败（网络 / api.github.com 被墙 / 限流），并非"已是最新"。前端据此区分展示。 */
  checkFailed?: boolean
  /** true = 走了 github.com atom 回退通道（api.github.com 不通时）。仅诊断用。 */
  viaFallback?: boolean
  /** 最新版本号（剥掉 v 前缀的 semver，如 "2.5.0"） */
  version?: string
  /** GitHub release tag (含 v 前缀，如 "v2.5.0") */
  tag?: string
  /** GH release 页面 URL，用于"去 GitHub 下载"按钮 */
  htmlUrl?: string
  /** Release notes / changelog (markdown) */
  body?: string
  publishedAt?: string
}

/** RapidOCR 模型档位:'standard' = PP-OCRv5 mobile(默认,快),'high' = PP-OCRv6 medium(高精度)。 */
export type RapidOcrTier = 'standard' | 'high'

/** RapidOCR 离线 OCR 状态:standard/high 两档各自独立报告就绪情况(dylib + 模型 + 字典是否齐全) */
export type RapidOcrStatus = {
  /** standard 档(PP-OCRv5 mobile)是否就绪 */
  standardAvailable: boolean
  /** high 档(PP-OCRv6 medium)是否就绪 */
  highAvailable: boolean
  /** app data 目录下的模型文件夹路径(用于 UI 展示) */
  modelDir?: string | null
}

/** RapidOCR 一键下载结果 */
export type RapidOcrInstallResult = {
  success: boolean
  /** 成功时是状态信息("RapidOCR 包下载完成"),失败时是错误片段 */
  message: string
}

export type OfflineModelFileStatus = {
  componentId: string
  fileName: string
  installedBytes: number
  downloadBytes: number
  ready: boolean
  state: 'ready' | 'missing' | 'invalid'
  error?: string | null
}

export type ReplaceTranslationPackStatus = {
  tier: RapidOcrTier
  ready: boolean
  totalBytes: number
  readyBytes: number
  missingBytes: number
  modelDir?: string | null
  files: OfflineModelFileStatus[]
}

export type OfflineModelProgress = {
  // 两个安装包共用同一事件名；面板按 pack 过滤，避免普通 RapidOCR 安装驱动替换翻译离线包 UI。
  pack: 'rapidocr' | 'replace_translation'
  componentId: string
  fileName: string
  downloadedBytes: number
  fileTotalBytes: number
  overallDownloadedBytes: number
  overallTotalBytes: number
  attempt: number
  state: 'downloading' | 'retrying' | 'verifying' | 'extracting' | 'completed' | 'failed'
  error?: string | null
}

function normalizeProvider(provider: ModelProvider): ModelProvider {
  return {
    ...provider,
    apiKeys: Array.isArray(provider.apiKeys) ? provider.apiKeys : [],
    availableModels: Array.isArray(provider.availableModels) ? provider.availableModels : [],
    enabledModels: Array.isArray(provider.enabledModels) ? provider.enabledModels : [],
    enabled: provider.enabled !== false,
    compressRequestBody: provider.compressRequestBody === true,
    apiFormat: normalizeProviderApiFormat(provider.apiFormat),
    request: {
      customHeaders: Array.isArray(provider.request?.customHeaders)
        ? provider.request.customHeaders
        : [],
      // 默认跟随系统代理 —— 与加这个开关之前的行为一致。
      useSystemProxy: provider.request?.useSystemProxy !== false,
      promptCacheRetention: resolvePromptCacheRetention(provider.request),
      cliIdentity: provider.request?.cliIdentity ?? '',
      cliIdentityVersion: provider.request?.cliIdentityVersion ?? '',
    },
  }
}

export function normalizeProviderApiFormat(apiFormat?: string): string {
  if (apiFormat === 'anthropic' || apiFormat === 'anthropic_messages') return 'anthropic_messages'
  if (apiFormat === 'openai_responses' || apiFormat === 'responses') return 'openai_responses'
  if (apiFormat === 'gemini' || apiFormat === 'google' || apiFormat === 'gemini_generate') return 'gemini'
  if (apiFormat === 'xai' || apiFormat === 'xai_responses' || apiFormat === 'grok') return 'xai_responses'
  return 'openai_chat'
}

/**
 * 当前 provider 是否支持模型原生内置联网搜索（任务 07-23）。
 * OpenAI Responses / Gemini / Anthropic Messages 支持；Chat Completions 不支持
 * （gpt-5 在其上开 web_search 会 400）。前端据此把「内置」选项置灰。
 * 与 Rust 侧 `model_metadata::builtin_web_search_supported` 保持一致。
 */
export type PromptCacheRetention = 'none' | 'short' | 'long'

/**
 * 规范化 / 迁移 prompt 缓存策略（与 Rust sanitize 同序）：
 * 1) retention 已是 none|short|long → 用之（忽略遗留 bool）
 * 2) 否则用遗留 bool：false→none，true→short
 * 3) 否则 short
 */
export function resolvePromptCacheRetention(
  request?: ProviderRequestConfig | null,
): PromptCacheRetention {
  const raw = (request?.promptCacheRetention ?? '').trim()
  if (raw === 'none' || raw === 'short' || raw === 'long') return raw
  if (request?.promptCaching === false) return 'none'
  if (request?.promptCaching === true) return 'short'
  return 'short'
}

/** 该协议是否有可发送的客户端缓存字段（Gemini / xAI 无）。 */
export function promptCachingSupported(apiFormat?: string): boolean {
  const kind = normalizeProviderApiFormat(apiFormat)
  return kind !== 'gemini' && kind !== 'xai_responses'
}

/** 当前策略是否会发客户端缓存字段。 */
export function promptCachingEnabled(request?: ProviderRequestConfig | null): boolean {
  return resolvePromptCacheRetention(request) !== 'none'
}

export function builtinWebSearchSupported(apiFormat?: string): boolean {
  const kind = normalizeProviderApiFormat(apiFormat)
  return (
    kind === 'openai_responses' ||
    kind === 'xai_responses' ||
    kind === 'gemini' ||
    kind === 'anthropic_messages'
  )
}

const CHAT_TOOL_MIN_ROUNDS = 1
const CHAT_TOOL_MAX_ROUNDS = 100

// 工具轮次默认不限（null）；缺失/非法输入一律归一到不限，与后端 default 对齐。
function normalizeMaxToolRounds(value: unknown): number | null {
  if (value === null || value === undefined) return null
  const parsed = Number(value)
  if (!Number.isFinite(parsed)) return null
  return Math.min(CHAT_TOOL_MAX_ROUNDS, Math.max(CHAT_TOOL_MIN_ROUNDS, Math.round(parsed)))
}

/**
 * 归一化 chatTools。**白名单重建**：这里逐字段列举，漏掉一个字段 = 该字段每次
 * 保存/读取都被静默丢弃（hooks 就这样丢过一次）。新增 ChatToolsConfig 字段时
 * 必须同步加到这里，`normalize_chat_tools_keeps_every_field` 会守门。
 */
export function normalizeChatTools(config?: Partial<ChatToolsConfig> | null): ChatToolsConfig {
  const current = config ?? {}
  return {
    enabled: current.enabled ?? false,
    servers: Array.isArray(current.servers) ? current.servers : [],
    hooks: Array.isArray(current.hooks) ? current.hooks : [],
    skillScanPaths: Array.isArray(current.skillScanPaths) ? current.skillScanPaths : [],
    skillAutoMatch: current.skillAutoMatch ?? true,
    skillFallbackMode: current.skillFallbackMode || 'progressive',
    disabledSkillIds: Array.isArray(current.disabledSkillIds) ? current.disabledSkillIds : [],
    maxToolRounds: normalizeMaxToolRounds(current.maxToolRounds),
    toolTimeoutMs: current.toolTimeoutMs ?? 60_000,
    mcpIdleTimeoutMs: current.mcpIdleTimeoutMs ?? 600_000,
    approvalPolicy: current.approvalPolicy || 'readonly_auto_sensitive_confirm',
    subAgentConcurrency: Math.min(64, Math.max(1, Math.round(current.subAgentConcurrency ?? 12))),
    subAgentProviderId: current.subAgentProviderId ?? '',
    subAgentModel: current.subAgentModel ?? '',
    requestDebugEnabled: current.requestDebugEnabled ?? false,
    nativeTools: {
      ...defaultNativeTools(),
      ...current.nativeTools,
      workingDirectory: typeof current.nativeTools?.workingDirectory === 'string'
        ? current.nativeTools.workingDirectory
        : (Array.isArray(current.nativeTools?.workspaceRoots) ? current.nativeTools.workspaceRoots[0] ?? '' : ''),
      workspaceRoots: Array.isArray(current.nativeTools?.workspaceRoots)
        ? current.nativeTools.workspaceRoots
        : [],
    },
  }
}

function normalizeChatMemory(config?: Partial<ChatMemoryConfig> | null): ChatMemoryConfig {
  const current = config ?? {}
  return {
    enabled: current.enabled ?? false,
    toolWriteConfirm: current.toolWriteConfirm ?? false,
  }
}

function normalizeDefaultModelSelection(selection?: Partial<DefaultModelSelection> | null): DefaultModelSelection {
  return {
    providerId: selection?.providerId ?? '',
    model: selection?.model ?? '',
  }
}

function normalizeDefaultModels(
  config?: Partial<DefaultModelsConfig> | null,
  legacyChat?: Partial<DefaultModelSelection> | null,
): DefaultModelsConfig {
  return {
    chat: normalizeDefaultModelSelection(config?.chat ?? legacyChat),
    vision: normalizeDefaultModelSelection(config?.vision),
    titleSummary: normalizeDefaultModelSelection(config?.titleSummary),
    compression: normalizeDefaultModelSelection(config?.compression),
    imageGeneration: normalizeDefaultModelSelection(config?.imageGeneration),
    advisor: normalizeDefaultModelSelection(config?.advisor),
  }
}

function isDefaultModelConfigured(selection: DefaultModelSelection): boolean {
  return selection.providerId.trim() !== ''
}

function providerHasUsableConfig(provider: ModelProvider): boolean {
  return provider.enabled !== false
    && provider.apiKeys.some((key) => key.trim() !== '')
    && provider.enabledModels.length > 0
}

function settingsHasUsableProviderConfig(settings: Partial<Settings>): boolean {
  return Array.isArray(settings.providers)
    && settings.providers.some(providerHasUsableConfig)
}

function normalizeOnboardingStatus(current: Partial<Settings>): 'pending' | 'completed' | 'skipped' {
  const raw = current.onboardingStatus?.trim()
  if (raw === 'completed' || raw === 'skipped' || raw === 'pending') return raw
  return settingsHasUsableProviderConfig(current) ? 'completed' : 'pending'
}

function prepareSettingsForSave(settings: Settings): Settings {
  const current = settings as Partial<Settings>
  const defaultModels = normalizeDefaultModels(current.defaultModels, {
    providerId: current.chatProviderId ?? '',
    model: current.chatModel ?? '',
  })

  return {
    ...settings,
    themeColor: normalizeThemeColorId(current.themeColor),
    defaultModels,
    chatProviderId: defaultModels.chat.providerId,
    chatModel: defaultModels.chat.model,
  }
}

/** 归一化设置（导出供单测钉死 externalCliAgents 等字段不被重建丢掉）。 */
export function normalizeSettings(settings: Settings): Settings {
  const current = settings as Partial<Settings>
  const defaultModels = normalizeDefaultModels(current.defaultModels, {
    providerId: current.chatProviderId ?? '',
    model: current.chatModel ?? '',
  })
  const effectiveChatModel = isDefaultModelConfigured(defaultModels.chat)
    ? defaultModels.chat
    : normalizeDefaultModelSelection(
      (current.chatProviderId?.trim()
        ? { providerId: current.chatProviderId, model: current.chatModel ?? '' }
        : current.lens?.providerId?.trim()
          ? { providerId: current.lens.providerId, model: current.lens.model ?? '' }
          : { providerId: current.translatorProviderId ?? '', model: current.translatorModel ?? '' }),
    )
  return {
    ...settings,
    hotkey: current.hotkey ?? 'CommandOrControl+Alt+T',
    chatHotkey: current.chatHotkey ?? 'CommandOrControl+Shift+K',
    closeChatHotkey: current.closeChatHotkey ?? 'CommandOrControl+Shift+W',
    theme: current.theme ?? 'system',
    themeColor: normalizeThemeColorId(current.themeColor),
    translucentSidebar: current.translucentSidebar ?? false,
    uiFontScale: current.uiFontScale ?? 1,
    uiFontFamily: current.uiFontFamily ?? '',
    uiFontMono: current.uiFontMono ?? '',
    targetLang: current.targetLang ?? 'auto',
    autoPaste: current.autoPaste ?? true,
    launchAtStartup: current.launchAtStartup ?? false,
    launchMinimizedToTray: current.launchMinimizedToTray ?? false,
    translatorProviderId: current.translatorProviderId ?? '',
    translatorModel: current.translatorModel ?? '',
    chatProviderId: effectiveChatModel.providerId,
    chatModel: effectiveChatModel.model,
    defaultModels,
    chat: {
      streamEnabled: current.chat?.streamEnabled ?? current.lens?.streamEnabled ?? true,
      thinkingEnabled: current.chat?.thinkingEnabled ?? current.lens?.thinkingEnabled ?? true,
      maxOutputTokens: current.chat?.maxOutputTokens ?? 8192,
      defaultLanguage: current.chat?.defaultLanguage ?? '',
      systemPrompt: current.chat?.systemPrompt ?? '',
      userDisplayName: current.chat?.userDisplayName ?? '',
      userAvatar: current.chat?.userAvatar ?? '',
      // 本地 CLI 覆盖（供应商列表 / 路径 / 停用）与默认运行时：之前重建 chat 时丢掉了，
      // 自动保存回写后「所有供应商」会一直空。
      defaultAgentRuntime: current.chat?.defaultAgentRuntime,
      externalCliAgents: current.chat?.externalCliAgents,
      chatMode: {
        systemPrompt: current.chat?.chatMode?.systemPrompt ?? '',
        webSearch: current.chat?.chatMode?.webSearch ?? true,
        webFetch: current.chat?.chatMode?.webFetch ?? true,
        knowledgeSearch: current.chat?.chatMode?.knowledgeSearch ?? true,
        memoryTools: current.chat?.chatMode?.memoryTools ?? true,
        mcpReadOnly: current.chat?.chatMode?.mcpReadOnly ?? true,
      },
    },
    chatMemory: normalizeChatMemory(current.chatMemory),
    providers: Array.isArray(current.providers) ? current.providers.map(normalizeProvider) : [],
    chatTools: normalizeChatTools(current.chatTools),
    retryEnabled: current.retryEnabled ?? true,
    retryAttempts: current.retryAttempts ?? 3,
    screenshotTranslation: {
      enabled: current.screenshotTranslation?.enabled ?? true,
      hotkey: current.screenshotTranslation?.hotkey ?? 'CommandOrControl+Shift+A',
      textHotkey: current.screenshotTranslation?.textHotkey ?? 'CommandOrControl+Shift+T',
      replaceHotkey: current.screenshotTranslation?.replaceHotkey ?? 'CommandOrControl+Shift+R',
      replaceEnabled: current.screenshotTranslation?.replaceEnabled ?? true,
      providerId: current.screenshotTranslation?.providerId ?? '',
      model: current.screenshotTranslation?.model ?? '',
      directTranslate: current.screenshotTranslation?.directTranslate ?? false,
      thinkingEnabled: current.screenshotTranslation?.thinkingEnabled ?? false,
      streamEnabled: current.screenshotTranslation?.streamEnabled ?? true,
      keepFullscreenAfterCapture: current.screenshotTranslation?.keepFullscreenAfterCapture ?? true,
      cardWidth: current.screenshotTranslation?.cardWidth ?? 480,
      useSystemOcr: current.screenshotTranslation?.useSystemOcr ?? false,
      ocrMode: current.screenshotTranslation?.ocrMode ?? 'cloud_vision',
      rapidOcrTier: current.screenshotTranslation?.rapidOcrTier === 'high' ? 'high' : 'standard',
      prompt: current.screenshotTranslation?.prompt ?? '',
      textPrompt: current.screenshotTranslation?.textPrompt ?? '',
      replacePrompt: current.screenshotTranslation?.replacePrompt ?? '',
    },
    screenshotAnnotate: {
      enabled: current.screenshotAnnotate?.enabled ?? true,
      hotkey: current.screenshotAnnotate?.hotkey ?? 'CommandOrControl+Shift+S',
    },
    lens: {
      enabled: current.lens?.enabled ?? true,
      hotkey: current.lens?.hotkey ?? 'CommandOrControl+Shift+G',
      providerId: current.lens?.providerId ?? '',
      model: current.lens?.model ?? '',
      defaultLanguage: current.lens?.defaultLanguage ?? '',
      streamEnabled: current.lens?.streamEnabled ?? true,
      thinkingEnabled: current.lens?.thinkingEnabled ?? true,
      systemPrompt: current.lens?.systemPrompt ?? '',
      questionPrompt: current.lens?.questionPrompt ?? '',
      sendToChat: current.lens?.sendToChat ?? true,
      messageOrder: current.lens?.messageOrder === 'desc' ? 'desc' : 'asc',
      showCaptureHint: current.lens?.showCaptureHint ?? true,
      webSearch: {
        enabled: current.lens?.webSearch?.enabled ?? false,
        provider: current.lens?.webSearch?.provider ?? 'tavily',
        tavilyApiKey: current.lens?.webSearch?.tavilyApiKey ?? '',
        tavilyBaseUrl: current.lens?.webSearch?.tavilyBaseUrl ?? 'https://api.tavily.com',
        exaApiKey: current.lens?.webSearch?.exaApiKey ?? '',
        exaBaseUrl: current.lens?.webSearch?.exaBaseUrl ?? 'https://api.exa.ai',
        exaMcpUrl: current.lens?.webSearch?.exaMcpUrl ?? 'https://mcp.exa.ai/mcp',
        ollamaApiKey: current.lens?.webSearch?.ollamaApiKey ?? '',
        ollamaBaseUrl: current.lens?.webSearch?.ollamaBaseUrl ?? 'https://ollama.com',
        grokApiKey: current.lens?.webSearch?.grokApiKey ?? '',
        grokModel: current.lens?.webSearch?.grokModel ?? 'grok-4-1-fast-non-reasoning',
        grokBaseUrl: current.lens?.webSearch?.grokBaseUrl ?? 'https://api.x.ai/v1',
        grokSystemPrompt: current.lens?.webSearch?.grokSystemPrompt
          ?? "You are a helpful search assistant. Search the web to find accurate and up-to-date information for the user's query. Provide a comprehensive answer with citations.",
        braveApiKey: current.lens?.webSearch?.braveApiKey ?? '',
        braveBaseUrl: current.lens?.webSearch?.braveBaseUrl ?? 'https://api.search.brave.com',
        serperApiKey: current.lens?.webSearch?.serperApiKey ?? '',
        serperBaseUrl: current.lens?.webSearch?.serperBaseUrl ?? 'https://google.serper.dev',
        bochaApiKey: current.lens?.webSearch?.bochaApiKey ?? '',
        bochaBaseUrl: current.lens?.webSearch?.bochaBaseUrl ?? 'https://api.bochaai.com',
        zhipuApiKey: current.lens?.webSearch?.zhipuApiKey ?? '',
        zhipuBaseUrl: current.lens?.webSearch?.zhipuBaseUrl ?? 'https://open.bigmodel.cn/api/paas/v4',
        tinyfishApiKey: current.lens?.webSearch?.tinyfishApiKey ?? '',
        tinyfishBaseUrl: current.lens?.webSearch?.tinyfishBaseUrl ?? 'https://api.search.tinyfish.ai',
        tinyfishMcpUrl: current.lens?.webSearch?.tinyfishMcpUrl ?? 'https://agent.tinyfish.ai/mcp',
        tinyfishMcpAuth: current.lens?.webSearch?.tinyfishMcpAuth ?? null,
        searxngBaseUrl: current.lens?.webSearch?.searxngBaseUrl ?? '',
        maxResults: current.lens?.webSearch?.maxResults ?? 5,
        searchDepth: current.lens?.webSearch?.searchDepth ?? 'basic',
      },
    },
    settingsLanguage: current.settingsLanguage ?? 'zh',
    onboardingStatus: normalizeOnboardingStatus(current),
    autoCheckUpdate: current.autoCheckUpdate ?? true,
    imageArchiveEnabled: current.imageArchiveEnabled ?? false,
    imageArchivePath: current.imageArchivePath ?? '',
    obsidianVaultPath: current.obsidianVaultPath ?? '',
    favoriteModels: current.favoriteModels ?? [],
  }
}

// 默认提示词模板
export type DefaultPromptTemplates = {
  translationTemplate: string
  screenshotTranslationTemplate?: string
  selectedTextTranslationTemplate?: string
  replaceTranslationTemplate?: string
  lensPrompts: {
    zh: { system: string; question: string }
    en: { system: string; question: string }
  }
  chatPrompts?: {
    zh: string
    en: string
  }
  /** Built-in Kivio Chat runtime prompt (exact string injected when chatMode.systemPrompt is empty). */
  chatRuntimePrompt?: string
}

// macOS 权限状态
export type PermissionStatus = {
  platform: 'macos' | 'other'
  accessibility: boolean
  screenRecording: boolean
}

// 事件取消监听函数类型
type Unlisten = () => void

/**
 * 通用的 Tauri 事件监听包装器
 * @param event 事件名称
 * @param handler 事件处理函数
 * @returns 取消监听的函数
 */
async function on<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> {
  const unlisten = await listen<T>(event, (event) => handler(event.payload))
  return () => {
    unlisten()
  }
}

async function onChatProtocol(
  handler: (payload: ChatProtocolEvent, delivery: ChatProtocolDelivery) => void,
): Promise<Unlisten> {
  return subscribeChatProtocol(handler)
}

// ========== API 导出 ==========

export const api = {
  // 设置相关
  getSettings: async () => normalizeSettings(await invoke<Settings>('get_settings')),
  // 某模型可选的思考等级列表（用户覆盖 modelOverrides → 模型库 reasoningEfforts → 家族兜底）。
  reasoningEffortsForModel: (model: string, providerId?: string) =>
    invoke<string[]>('chat_reasoning_efforts_for_model', { model, providerId }),
  getDefaultPromptTemplates: () => invoke<DefaultPromptTemplates>('get_default_prompt_templates'),
  listSystemFonts: () => invoke<string[]>('list_system_fonts').catch(() => [] as string[]),
  saveSettings: async (settings: Settings) =>
    normalizeSettings(await invoke<Settings>('save_settings', { settings: prepareSettingsForSave(settings) })),
  /** 轻量持久化收藏模型（不触发热键/托盘重注册，区别于 saveSettings 的全量事务保存）。 */
  setFavoriteModels: (models: string[]) =>
    invoke<void>('set_favorite_models', { models }),
  /** 轻量持久化快速翻译卡宽度（拖拽缩放记忆；高度始终自动）。 */
  setTranslateCardSize: (width: number) =>
    invoke<void>('set_translate_card_size', { width }),
  exportSettings: (path: string) => invoke<void>('export_settings', { path }),
  importSettings: async (path: string) =>
    normalizeSettings(await invoke<Settings>('import_settings', { path })),
  usageGetStats: (query?: UsageStatsQuery) =>
    invoke<UsageStatsResponse>('usage_get_stats', { query }),
  usageClear: () => invoke<void>('usage_clear'),
  getRequestDebugRecords: () =>
    invoke<RequestDebugRecord[]>('get_request_debug_records'),
  clearRequestDebugRecords: () => invoke<void>('clear_request_debug_records'),

  // 提供商相关
  fetchModels: (providerId: string, provider?: ProviderConnectionInput) =>
    invoke<string[]>('fetch_models', { providerId, provider }),
  testProviderConnection: (providerId: string, provider?: ProviderConnectionInput) =>
    invoke<{ success: boolean; error?: string }>('test_provider_connection', { providerId, provider }),

  // 测试网络搜索：用传入的（可能未保存的）配置真实跑一次搜索
  testWebSearch: (config: NonNullable<Settings['lens']['webSearch']>, query: string) =>
    invoke<{
      success: boolean
      provider?: string
      results?: { title: string; url: string; content: string }[]
      error?: string
    }>('test_web_search', { config, query }),

  // 权限相关（macOS）
  getPermissionStatus: () => invoke<PermissionStatus>('get_permission_status'),
  openPermissionSettings: (kind: 'accessibility' | 'screen-recording') =>
    invoke<void>('open_permission_settings', { kind }),

  // 应用信息
  getAppVersion: () => getVersion(),
  openSettingsWindow: () => invoke<void>('open_settings_window'),
  closeTranslatorWindow: () => invoke<void>('close_translator_window'),

  // 文本翻译
  translateText: (text: string) => invoke<string>('translate_text', { text }),
  commitTranslation: (text: string) => invoke<void>('commit_translation', { text }),

  // 外部链接
  openExternal: (url: string) => invoke<void>('open_external', { url }),
  /**
   * 打开模型输出里的本地文件链接（file:// / 绝对路径 / 相对路径）。
   * 相对路径由后端按会话工作目录解析，故要带 conversationId。扩展名/存在性/逃逸都在后端把关。
   */
  openLocalFile: (href: string, conversationId?: string | null) =>
    invoke<void>('open_local_file', { href, conversationId: conversationId ?? null }),
  openHtmlPreview: (html: string) => invoke<void>('open_html_preview', { html }),

  // 连接器 OAuth：跑完整 OAuth（PKCE + DCR + loopback，会打开浏览器授权）→
  // 返回物化好的 ChatMcpServer（不写 settings，由前端合并进 chatTools.servers 并保存）。
  connectorOauthConnect: (args: { catalogId?: string; url?: string; name?: string }) =>
    invoke<ChatMcpServer>('connector_oauth_connect', args),

  listObsidianVaults: () =>
    invoke<{ name: string; path: string }[]>('list_obsidian_vaults_cmd'),

  /** 能力插件列表（目录 + 安装/启用状态） */
  pluginsList: () => invoke<PluginStatus[]>('plugins_list'),
  // Cached (no-spawn) status for instant first paint; follow with pluginsList to refine.
  pluginsListCached: () => invoke<PluginStatus[]>('plugins_list_cached'),
  /** 取可选「让 AI 代装」任务 brief（含标准化安装文档） */
  pluginsInstallBrief: (id: string) => invoke<PluginInstallBrief>('plugins_install_brief', { id }),
  /** 运行当前系统对应的 GitHub README 安装命令 */
  pluginsRunOfficialInstall: (id: string) =>
    invoke<PluginActionResult>('plugins_run_official_install', { id }),
  pluginsSetEnabled: (id: string, enabled: boolean) =>
    invoke<PluginActionResult>('plugins_set_enabled', { id, enabled }),
  pluginsUninstall: (id: string) => invoke<PluginActionResult>('plugins_uninstall', { id }),

  /** 扩展 → 笔记 */
  notesList: () => invoke<NoteMeta[]>('notes_list'),
  notesRead: (id: string) => invoke<Note>('notes_read', { id }),
  notesCreate: (title: string, content: string, folder: string, origin: string) =>
    invoke<Note>('notes_create', { title, content, folder, origin }),
  notesUpdate: (id: string, title: string, content: string, folder: string) =>
    invoke<Note>('notes_update', { id, title, content, folder }),
  notesFoldersList: () => invoke<string[]>('notes_folders_list'),
  notesFolderCreate: (name: string) => invoke<string[]>('notes_folder_create', { name }),
  notesFolderRename: (oldName: string, newName: string) =>
    invoke<string[]>('notes_folder_rename', { old: oldName, new: newName }),
  notesFolderDelete: (name: string) => invoke<string[]>('notes_folder_delete', { name }),
  notesDelete: (id: string) => invoke<void>('notes_delete', { id }),
  /** 在系统文件管理器里打开笔记目录（用户可直接拖入外部 .md）。返回该目录路径。 */
  notesOpenFolder: () => invoke<string>('notes_open_folder'),
  /** 笔记目录的绝对路径，用于订阅 workspace:activity 自动刷新。 */
  notesDirPath: () => invoke<string>('notes_dir_path'),

  // 窗口控制
  /** 给当前（chat）窗口上 Mica，返回材质是否真的生效。Win10 没有 Mica 时为 false —— 这条
   *  路走后端而不是 window.setEffects()，因为 tauri 把 apply_mica 的失败静默吞掉了。 */
  chatWindowApplyMica: (dark: boolean): Promise<boolean> =>
    invoke('chat_window_apply_mica', { dark }),
  /** macOS：材质没上时把 NSWindow 设回 opaque，换回合成器的不透明快路径（台前调度不掉帧）。
   *  材质上了必须传 false，否则 Menu 材质被实色背景挡死。非 macOS 是 no-op。 */
  chatWindowSetOpaque: (opaque: boolean): Promise<void> =>
    invoke('chat_window_set_opaque', { opaque }),
  /** macOS 交通灯中心距内容顶缘的真实距离（CSS px）。取不到返回 null，前端退回默认值。 */
  chatTrafficLightCenterY: (): Promise<number | null> =>
    invoke('chat_traffic_light_center_y'),
  resizeWindow: async (width: number, height: number) => {
    const win = getCurrentWindow()
    await win.setSize(new LogicalSize(width, height))
  },
  closeWindow: async () => {
    const win = getCurrentWindow()
    await win.close()
  },
  minimizeWindow: async () => {
    const win = getCurrentWindow()
    await win.minimize()
  },
  toggleMaximizeWindow: async () => {
    const win = getCurrentWindow()
    await win.toggleMaximize()
  },
  showWindow: async () => {
    const win = getCurrentWindow()
    await win.show()
  },
  focusWindow: async () => {
    const win = getCurrentWindow()
    await win.setFocus()
  },
  startDragging: async () => {
    const win = getCurrentWindow()
    await win.startDragging()
  },

  /** 把聊天窗口上次停留的路由交给 Rust 持久化（null = 清除）。 */
  rememberChatLastRoute: async (route: string | null): Promise<void> => {
    try {
      await invoke('chat_remember_last_route', { route })
    } catch {
      // 路由持久化是尽力而为：失败只影响「重开回到哪条对话」，绝不打断交互。
    }
  },

  // 事件监听
  onOpenSettings: (listener: () => void) => on('open-settings', () => listener()),

  // 读取截图（lens ready 态拉缩略图用）
  explainReadImage: (imageId: string) =>
    invoke<{ success: boolean; data?: string; error?: string }>('explain_read_image', { imageId }),

  // Lens 模式
  onLensStream: (listener: (payload: LensStreamPayload) => void) =>
    on<LensStreamPayload>('lens-stream', (payload) => listener(payload)),
  onChatStream: (listener: (payload: ChatStreamPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event, delivery) => {
      if (event.scope !== 'run') return
      if (
        event.type === 'run_started'
        || event.type === 'text_delta'
        || event.type === 'reasoning_delta'
        || event.type === 'run_completed'
        || event.type === 'run_cancelled'
        || event.type === 'run_failed'
      ) {
        listener({ ...event, restoredFromSnapshot: delivery.source === 'snapshot' })
      }
    })
  },
  onChatContext: (listener: (payload: ChatContextPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.type === 'context_updated' && event.scope === 'conversation') {
        listener({ conversationId: event.conversationId, contextState: event.contextState as ChatContextState })
      } else if (event.type === 'context_usage_updated' && event.scope === 'run') {
        listener({ conversationId: event.conversationId, live: event.usage })
      }
    })
  },
  onChatCompaction: (listener: (payload: ChatCompactionPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope !== 'run' || event.type !== 'compaction_updated') return
      listener({
        conversationId: event.conversationId,
        phase: event.phase,
        trigger: event.trigger ?? undefined,
        boundary: event.boundary as CompactionBoundaryRecord | null,
      })
    })
  },
  onChatTodo: (listener: (payload: ChatTodoPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.type !== 'todo_updated') return
      listener({ conversationId: event.conversationId, todoState: event.todoState as ChatTodoState })
    })
  },
  onChatPlan: (listener: (payload: ChatPlanPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.type !== 'plan_updated') return
      listener({ conversationId: event.conversationId, planState: event.planState as ChatPlanState })
    })
  },
  onChatTool: (listener: (payload: ChatToolProgressPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope !== 'run' || event.type !== 'tool_updated') return
      const tool = event.tool
      listener({
        conversationId: event.conversationId,
        runId: event.runId,
        messageId: event.messageId,
        toolCallId: tool.id,
        ...tool,
        status: tool.status as ChatToolStatus,
        artifacts: tool.artifacts as ChatToolArtifact[],
      })
    })
  },
  /** 生命周期 Hook 执行失败（脚本非零退出 / 超时 / HTTP 非 2xx）。对话本身不受影响。 */
  onChatHook: (listener: (payload: ChatHookPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope !== 'run' || event.type !== 'hook_failed') return
      listener({
        conversationId: event.conversationId,
        runId: event.runId,
        hookName: event.hookName,
        event: event.event,
        message: event.message,
      })
    })
  },
  /** 流状态行的瞬态一行字（上游重试等，status_note_updated）。note=null 为显式清除。 */
  onChatStatusNote: (
    listener: (payload: { conversationId: string; runId: string; note: string | null }) => void,
  ) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope !== 'run' || event.type !== 'status_note_updated') return
      listener({
        conversationId: event.conversationId,
        runId: event.runId,
        note: event.note,
      })
    })
  },
  onChatSubagent: (listener: (payload: ChatSubagentPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope !== 'run' || event.type !== 'subagent_updated') return
      listener({
        parentConversationId: event.conversationId,
        parentRunId: event.runId,
        parentToolCallId: event.parentToolCallId,
        taskId: event.taskId,
        name: event.name,
        model: event.model ?? undefined,
        depth: event.depth,
        status: event.status as ChatSubagentPayload['status'],
        preview: event.preview ?? undefined,
        steps: event.steps,
      })
    })
  },
  onChatUserPrompt: (listener: (payload: ChatUserPromptPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope !== 'run' || event.type !== 'user_prompt_requested') return
      listener({
        conversationId: event.conversationId,
        runId: event.runId,
        messageId: event.messageId,
        toolCallId: event.toolCallId,
        id: event.toolCallId,
        name: event.name,
        source: event.source,
        prompt: event.prompt as AskUserPromptPayload,
        structuredContent: event.structuredContent,
      })
    })
  },
  onChatToolConfirm: (listener: (payload: ChatToolConfirmPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope !== 'run' || event.type !== 'tool_approval_requested') return
      listener({
        conversationId: event.conversationId,
        runId: event.runId,
        messageId: event.messageId,
        toolCallId: event.toolCallId,
        name: event.name,
        source: event.source,
        serverId: event.serverId,
        target: event.target,
        argumentsPreview: event.argumentsPreview,
        sensitivity: event.sensitivity,
      })
    })
  },
  /** 后端已按超时/取消处理掉某条审批 ⇒ 撤掉卡片，别让用户答一个没人听的问题。 */
  onChatToolConfirmWithdraw: (
    listener: (payload: { conversationId: string; toolCallId: string }) => void,
  ) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope === 'run' && event.type === 'tool_approval_withdrawn') {
        listener({ conversationId: event.conversationId, toolCallId: event.toolCallId })
      }
    })
  },
  onChatSessionConsent: (listener: (payload: ChatSessionConsentPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return onChatProtocol((event) => {
      if (event.scope === 'run' && event.type === 'session_consent_requested') {
        listener({
          conversationId: event.conversationId,
          runId: event.runId,
          messageId: event.messageId,
        })
      }
    })
  },
  onChatProtocolIssue: (
    listener: (notice: { issue: ChatProtocolIssue; conversationId?: string }) => void,
  ) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return Promise.resolve(subscribeChatProtocolIssues((issue, conversationId) => {
      listener({ issue, conversationId })
    }))
  },
  chatSyncState: (conversationId: string) => syncChatProtocol(conversationId),
  onChatOpenConversation: (listener: (payload: { conversationId: string; reload?: boolean | null; error?: string | null }) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return on<{ conversationId: string; reload?: boolean | null; error?: string | null }>('chat-open-conversation', (payload) => listener(payload))
  },
  onChatExternalSendReady: (listener: () => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return on<unknown>('chat-external-send-ready', () => listener())
  },
  onMcpServerState: (listener: (payload: McpServerStatePayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return on<McpServerStatePayload>('mcp-server-state', (payload) => listener(payload))
  },
  /** 保存设置时部分热键被占用未能注册的警告（保存本身已成功）。payload 是后端 HotkeyError JSON 字符串。 */
  onHotkeyWarning: (listener: (raw: string) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return on<string>('hotkey-warning', (payload) => listener(payload))
  },
  chatTakeExternalSends: () => {
    if (!isTauriRuntime()) {
      return Promise.resolve({ success: true, requests: [] as ChatExternalSendRequest[] })
    }
    return invoke<{ success: boolean; requests: ChatExternalSendRequest[]; error?: string | null }>('chat_take_external_sends')
  },
  chatMcpListTools: () =>
    invoke<{ success: boolean; tools: ChatToolDefinition[]; error?: string | null }>('chat_mcp_list_tools'),
  chatMcpTestServer: (server: ChatMcpServer, timeoutMs?: number) =>
    invoke<{ success: boolean; tools: ChatToolDefinition[]; error?: string | null }>(
      'chat_mcp_test_server',
      { server, timeoutMs },
    ),
  chatMcpImportJson: (path: string) =>
    invoke<{ success: boolean; servers: ChatMcpServer[]; error?: string | null }>(
      'chat_mcp_import_json',
      { path },
    ),
  /** 扫描本机已安装 CLI（Claude Code / Codex / OpenCode）的 MCP 配置，按 CLI 分组返回可导入项。 */
  chatCliImportScan: () => invoke<CliImportScan>('chat_cli_import_scan'),
  chatPiAgentDir: () => {
    if (!isTauriRuntime()) return Promise.resolve(null)
    return invoke<string | null>('chat_external_cli_pi_agent_dir')
  },
  chatMcpServerStatus: (serverId: string) =>
    invoke<McpServerStatus>('chat_mcp_server_status', { serverId }),
  chatMcpListToolDefs: (serverId: string) =>
    invoke<{ name: string; description: string }[]>('chat_mcp_list_tool_defs', { serverId }),
  /** 后台预热 MCP 连接（fire-and-forget）：不传 = 全部启用的 server；结果走 onMcpServerState 推送。 */
  chatMcpWarmup: (serverIds?: string[]) => {
    if (!isTauriRuntime()) return Promise.resolve()
    return invoke<void>('chat_mcp_warmup', { serverIds })
  },
  chatSkillsList: (skillScanPaths?: string[], projectCwd?: string) =>
    invoke<{ success: boolean; skills: SkillMeta[]; warnings?: string[]; error?: string | null }>(
      'chat_skills_list',
      { skillScanPaths, projectCwd },
    ),
  chatSkillsRead: (skillId: string, projectCwd?: string) =>
    invoke<{ success: boolean; skill?: SkillDetail | null; error?: string | null }>(
      'chat_skills_read',
      { skillId, projectCwd },
    ),
  chatSkillsImport: (path: string) =>
    invoke<{ success: boolean; skill?: SkillMeta | null; error?: string | null }>(
      'chat_skills_import',
      { path },
    ),
  /** 卸载个人/导入技能（内置与插件技能不可删）。 */
  chatSkillsUninstall: (id: string) => invoke<void>('chat_skills_uninstall', { id }),
  /** 技能市场：从 ClawHub 下载链 / GitHub 仓库 / 直链 zip 下载安装。 */
  chatSkillsInstallFromUrl: (url: string) =>
    invoke<{ success: boolean; skill?: SkillMeta | null; error?: string | null }>(
      'chat_skills_install_from_url',
      { url },
    ),
  chatSkillsOpenFolder: () =>
    invoke<{ success: boolean; path?: string | null; error?: string | null }>(
      'chat_skills_open_folder',
    ),
  /** Background tasks 面板列表（按对话过滤）：内置后台命令 + 外部 CLI 后台任务。 */
  chatListBackgroundTasks: (conversationId: string) =>
    invoke<BackgroundTaskInfo[]>('chat_list_background_tasks', { conversationId }),
  chatClearFinishedBackgroundTasks: (conversationId: string) =>
    invoke<void>('chat_clear_finished_background_tasks', { conversationId }),
  chatStopExternalBackgroundTask: (conversationId: string, taskId: string) =>
    invoke<void>('chat_stop_external_background_task', { conversationId, taskId }),
  chatKillBackgroundCommand: (jobId: string) =>
    invoke<void>('chat_kill_background_command', { jobId }),
  chatMemoryGet: () =>
    invoke<ChatMemoryState>('chat_memory_get'),
  chatMemorySave: (layer: 'l1' | 'l2', content: string) =>
    invoke<ChatMemoryLayerContent>('chat_memory_save', { layer, content }),
  chatMemoryOpenFolder: () =>
    invoke<{ success: boolean; path?: string | null; error?: string | null }>(
      'chat_memory_open_folder',
    ),
  chatSavePastedImage: (name: string, mimeType: string, dataBase64: string) =>
    invoke<ChatPastedImageResult>('chat_save_pasted_image', { name, mimeType, dataBase64 }),
  chatSavePastedAttachment: (name: string, dataBase64: string) =>
    invoke<ChatPastedImageResult>('chat_save_pasted_attachment', { name, dataBase64 }),
  chatReadClipboardFiles: () =>
    invoke<ChatClipboardFilesResult>('chat_read_clipboard_files'),
  // permissionMode 只有计划批准卡会传（三选一里用户选的那一档），决定批准后把 CLI 切到
  // 哪个权限模式。普通审批传 null。
  chatConfirmToolCall: (
    toolCallId: string,
    approved: boolean,
    always = false,
    permissionMode: string | null = null,
  ) =>
    invoke<void>('chat_confirm_tool_call', { toolCallId, approved, always, permissionMode }),
  chatRespondSessionConsent: (conversationId: string, granted: boolean) =>
    invoke<void>('chat_respond_session_consent', { conversationId, granted }),
  chatSubmitUserChoice: (
    toolCallId: string,
    answers: Record<string, AskUserAnswer>,
    skipped = false,
  ) =>
    invoke<void>('chat_submit_user_choice', { toolCallId, answers, skipped }),
  chatPythonComplete: (
    runId: string,
    content: string,
    isError: boolean,
    artifacts: ChatToolArtifact[] = [],
  ) =>
    invoke<void>('chat_python_complete', { runId, content, isError, artifacts }),
  onChatRunPython: (listener: (payload: ChatRunPythonPayload) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return subscribeChatPython(listener)
  },
  onChatAssistantsChanged: (listener: (assistantId: string) => void) => {
    if (!isTauriRuntime()) return Promise.resolve(() => {})
    return on<string>('chat-assistants-changed', (payload) => listener(payload))
  },
  onLensWebSearch: (listener: (payload: LensWebSearchPayload) => void) =>
    on<LensWebSearchPayload>('lens-web-search', (payload) => listener(payload)),
  onLensTranslateStream: (listener: (payload: LensTranslateStreamPayload) => void) =>
    on<LensTranslateStreamPayload>('lens-translate-stream', (payload) => listener(payload)),
  onLensReplaceStream: (listener: (payload: LensReplaceStreamPayload) => void) =>
    on<unknown>('lens-replace-stream', (payload) => {
      try {
        listener(parseLensReplaceStreamPayload(payload))
      } catch (error) {
        console.error('Invalid lens-replace-stream payload', error)
      }
    }),
  onLensCloseRequest: (listener: () => void) =>
    on('lens-close-request', () => listener()),
  lensListWindows: () => invoke<LensWindowInfo[]>('lens_list_windows'),
  lensCaptureWindow: (windowId: number) =>
    invoke<{ success: boolean; imageId?: string; error?: string }>('lens_capture_window', { windowId }),
  lensCaptureRegion: (params: {
    absoluteX: number
    absoluteY: number
    x: number
    y: number
    width: number
    height: number
    scaleFactor: number
    freezeFrameImageId?: string
  }) => invoke<{ success: boolean; imageId?: string; error?: string }>('lens_capture_region', params),
  lensRegisterAnnotatedImage: (base64Png: string) =>
    invoke<{ success: boolean; imageId?: string; error?: string }>(
      'lens_register_annotated_image', { base64Png }
    ),
  lensCopyImageToClipboard: (base64Png: string) =>
    invoke<{ success: boolean; error?: string }>('lens_copy_image_to_clipboard', { base64Png }),
  lensSaveAnnotatedPng: (base64Png: string, path: string) =>
    invoke<{ success: boolean; error?: string }>('lens_save_annotated_png', { base64Png, path }),
  lensTranslate: (imageId: string) =>
    invoke<{ success: boolean; original?: string; translated?: string; error?: string }>(
      'lens_translate', { imageId }
    ),
  lensTranslateText: (text: string, requestId: string) =>
    invoke<{ success: boolean; original?: string; translated?: string; error?: string }>(
      'lens_translate_text', { text, requestId }
    ),
  lensReplaceTranslate: (imageId: string) =>
    invoke<{ success: boolean; regionCount?: number; missingBytes?: number; warning?: string; error?: string }>(
      'lens_replace_translate', { imageId }
    ),
  lensAsk: (imageId: string, messages: ExplainMessage[], options?: { webSearch?: boolean }) =>
    invoke<{ success: boolean; response?: string; error?: string; webSearchResults?: LensWebSearchResult[] }>('lens_ask', {
      imageId,
      messages,
      webSearch: options?.webSearch,
    }),
  lensSendToChat: (imageId: string, question: string) =>
    invoke<{ success: boolean; requestId?: string; error?: string }>('lens_send_to_chat', {
      imageId,
      question,
    }),
  // 把 Lens 完整多轮历史 + 截图同步到 AI 客户端，预置成一个新会话（不触发回复，落地末尾可续聊）。
  lensSendHistoryToChat: (imageId: string, messages: ExplainMessage[]) =>
    invoke<{ success: boolean; requestId?: string; error?: string }>('lens_send_history_to_chat', {
      imageId,
      messages,
    }),
  lensCancelStream: () => invoke<void>('lens_cancel_stream'),
  // 让原生把 lens 浮窗内部 WKWebView 设为 first responder（修复复用窗口第二次打开偶尔不聚焦）。
  lensFocusWebview: () => invoke<void>('lens_focus_webview'),
  lensClose: () => invoke<void>('lens_close'),
  // 全局 Esc 兜底开关：select 全屏阶段由后端全局快捷键保证 Esc 必能退出（webview 未拿到
  // 键盘焦点/挂死时 JS keydown 收不到）；chat 模式离开 select 后关掉，把 Esc 语义交还 JS。
  lensSetEscapeGuard: (active: boolean) => invoke<void>('lens_set_escape_guard', { active }),
  // 取走 select 态复位载荷（frame + freezeFrameImageId 的 JSON）。冷挂载兜底用：lens:reset
  // 事件可能早于监听注册被丢，前端挂载时主动拉一次，丢事件也不丢冻结帧。无 pending 返回 null。
  lensTakeResetPayload: () => invoke<string | null>('lens_take_reset_payload'),
  // 把当前活跃 image 拷贝到 lens-history 持久目录，让重启后历史能继续提问
  lensCommitImageToHistory: (imageId: string) =>
    invoke<void>('lens_commit_image_to_history', { imageId }),
  // 历史淘汰一条记录时调用，删除 lens-history 中对应 PNG 防止目录无限增长
  lensDeleteHistoryImage: (imageId: string) =>
    invoke<void>('lens_delete_history_image', { imageId }),
  lensSetFloating: (rect: { x?: number; y?: number; width: number; height: number }) =>
    invoke<void>('lens_set_floating', { rect }),
  // macOS 走 AppKit 原生 NSAnimationContext + animator setFrame,一次 IPC 触发,Core Animation
  // 在合成器线程驱动剩余帧;duration_ms 必须与前端 TRANSITION_MS 对齐。非 macOS 平台是 snap 兜底。
  lensAnimateFloating: (args: { x: number; y: number; width: number; height: number; durationMs: number }) =>
    invoke<void>('lens_animate_floating', args),

  // 取走 Rust 端在 lens_request_internal 中抓到的选中文本（take 一次清一次）
  takeLensSelection: () => invoke<string>('take_lens_selection'),

  // ========== 自动更新（仅检查 + 跳转，不做自动下载安装） ==========

  /** 调后端 GitHub Releases API 检查最新版本 */
  checkUpdate: () => invoke<UpdateInfo>('check_github_latest_release'),

  /** 下载新版本安装包到 OS temp 目录，返回本地文件路径。下载进度通过 onUpdateDownloadProgress 派发 */
  downloadUpdate: (version: string) => invoke<string>('download_update_asset', { version }),

  /** 启动安装包并退出当前应用（macOS：cp 新 .app 到 /Applications + open；Windows：spawn NSIS installer） */
  installUpdate: (path: string) => invoke<void>('install_update_and_quit', { path }),

  /** 下载进度事件：每次百分比变化派发一次 */
  onUpdateDownloadProgress: (
    listener: (p: { percent: number; downloadedBytes: number; totalBytes: number }) => void,
  ) => on<{ percent: number; downloadedBytes: number; totalBytes: number }>(
    'update-download-progress',
    (payload) => listener(payload),
  ),

  /** 启动时若发现新版，后端 emit 此事件让 Settings UI 自动展示更新提示 */
  onUpdateAvailable: (listener: (info: UpdateInfo) => void) =>
    on<UpdateInfo>('update-available', (payload) => listener(payload)),

  // ========== RapidOCR 离线 OCR ==========

  /** 查询 RapidOCR 两档模型 + onnxruntime dylib 是否就绪(standard/high 各自独立) */
  rapidOcrStatus: () => invoke<RapidOcrStatus>('rapidocr_status'),

  /** 下载指定档位的 RapidOCR 包(onnxruntime dylib 共享 + 该档模型)到 app data 目录。
   *  阻塞到全部完成返回(standard ~30-50MB,high ~150MB),前端转圈圈等。 */
  rapidOcrInstall: (tier: RapidOcrTier) =>
    invoke<RapidOcrInstallResult>('rapidocr_install', { tier }),

  replaceTranslationPackStatus: (tier: RapidOcrTier) =>
    invoke<ReplaceTranslationPackStatus>('replace_translation_pack_status', { tier }),
  replaceTranslationPackInstall: (tier: RapidOcrTier) =>
    invoke<RapidOcrInstallResult>('replace_translation_pack_install', { tier }),
  onReplaceTranslationPackProgress: (listener: (progress: OfflineModelProgress) => void) =>
    on<OfflineModelProgress>('replace-translation-pack-progress', payload => listener(payload)),
}
