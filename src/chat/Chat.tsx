import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { GitBranch, PanelRight, X } from 'lucide-react'
import { Sidebar, type ExtensionsNavItem } from './Sidebar'
import { useChatRouting } from './hooks/useChatRouting'
import { useExternalSendQueue } from './hooks/useExternalSendQueue'
import { useStreamRenderFrame } from './hooks/useStreamRenderFrame'
import { useTauriEvent } from './hooks/useTauriEvent'
import {
  clearConversationLocalState,
  type ConversationLocalState,
} from './conversationLocalState'
import {
  getRouteConversationId,
  hashPath,
  isChatAssistantCenterPath,
  isChatKnowledgeCenterPath,
  isChatMcpCenterPath,
  isChatNotesPath,
  isChatOnboardingRoute,
  isChatPluginCenterPath,
  isChatSettingsPath,
  isChatSkillCenterPath,
} from './chatRoutes'
import { ChatImageViewer } from './ChatImageViewer'
import { ApprovalCard } from './ApprovalCard'
import { ChatTitlebar } from './ChatTitlebar'
import { ChatTitlebarActions } from './ChatTitlebarActions'
import type { AssistantStreamStats } from './MessageList'
import { InputBar } from './InputBar'
import { ModelSelector } from './ModelSelector'
import { ThinkingLevelSelector } from './ThinkingLevelSelector'
import { ExternalModelSelector, RuntimePicker } from './RuntimePicker'
import { PermissionPicker } from './PermissionPicker'
import { BackgroundJobsIndicator } from './BackgroundJobsIndicator'
import { ContextIndicator } from './ContextIndicator'
import { AgentTodoIndicator } from './AgentTodoIndicator'
import { isExecutableAgentPlanText } from './agentPlan'
import {
  agentRuntimesEqual,
  BUILTIN_AGENT_RUNTIME,
  chatApi,
  normalizeAgentRuntime,
  type AgentRuntimeConfig,
} from './api'
import {
  chatTitlebarMacInsetClass,
  chatTitlebarRowClass,
  usesNativeTitlebar,
} from './platform'
import type {
  ChatProject,
  ChatSet,
  ChatMessage,
  ChatAssistant,
  Conversation,
  ConversationListItem,
  ConversationContextState,
  AgentPlanMode,
  AgentPlanState,
  AgentTodoState,
  ChatMessageSegment,
  PendingAttachment,
  SkillMeta,
  ToolCallRecord,
  ThinkingLevel,
  ModelRef,
  WebSearchMode,
} from './types'
import {
  api,
  builtinWebSearchSupported,
  type ChatSessionConsentPayload,
  type ChatHookPayload,
  type ChatStreamPayload,
  type ChatToolConfirmPayload,
  type ChatToolDefinition,
  type ChatMcpServer,
  type ChatToolProgressPayload,
  type ChatUserPromptPayload,
} from '../api/tauri'
import { getSettingsCached, refreshSettings, saveSettingsCached } from '../api/settingsCache'
import { OnboardingShell } from '../onboarding/OnboardingShell'
import type { SettingsShellHandle, SettingsTab } from '../settings/SettingsShell'
import { i18n, type Lang } from '../settings/i18n'
import { estimateTokens } from '../utils/tokens'
import {
  CHAT_MIN_SIZE_COLLAPSED,
  CHAT_MIN_SIZE_EXPANDED,
  forgetRememberedChatRoute,
  getRememberedChatSidebarCollapsed,
  getRememberedDockOpen,
  getRememberedDockTab,
  getRememberedDockWidth,
  getRememberedTreeExpanded,
  rememberChatSidebarCollapsed,
  rememberChatSize,
  rememberDockOpen,
  rememberDockTab,
  rememberDockWidth,
  rememberTreeExpanded,
} from './persistence'
import { ChatDotGridBackground } from './ChatDotGridBackground'
import { RightDock, type DockRevealRequest, type DockTab } from './dock/RightDock'
import { dockApi } from './dock/api'
import { insertTextIntoComposer } from './composerInsert'
import { IconButton } from '../components/Button'
import { normalizeToolCallStatus } from './toolStatus'
import { TypewriterText } from './TypewriterText'
import { pickRandomChatEmptyGreeting, isTauriRuntime } from './utils'
import { hasEnabledNativeBuiltinTool, hasEnabledSkillRuntime } from '../utils/chatTools'
import { onChatImageViewerOpen, type ChatImageViewerItem } from './imageViewer'
import {
  collectGeneratingConversationIds,
  createEmptyStreamSnapshot,
  isConversationBusy,
  isConversationInFlight,
  type ConversationStreamSnapshot,
} from './conversationRuns'
import {
  getCoarse as getStreamCoarse,
  patchSnapshot as patchStreamSnapshot,
  reset as resetStreamStore,
  setCoarse as setStreamCoarse,
  setSnapshot as setStreamSnapshot,
  useStreamCoarse,
} from './streamingStore'
import {
  beginGroup,
  endGroup,
  ensureGroupColumn,
  flushGroups,
  hasActiveGroup,
  resetGroups,
  touchGroup,
} from './groupStreamingStore'
import { compareTimelineSegments, segmentStepNumber, segmentToolCallId } from './segments'
import { latestCompactionBoundaryId, mergeCompactionContextState } from './compactionBoundary'
import { applyLiveContextUsage } from './contextPanel'

const AssistantCenter = lazy(() => import('./AssistantCenter').then((module) => ({
  default: module.AssistantCenter,
})))

// 共享 import thunk：lazy 与空闲预取复用同一次动态 import（模块缓存保证只加载一次）。
// SettingsShell 依赖图很大（Markdown/KaTeX、各设置面板），dev 下首次点开设置要现场编译
// 数百个模块而转圈数秒；挂载后空闲预取把这段成本移到用户点击之前。
const importSettingsShell = () => import('../settings/SettingsShell')

const SettingsShell = lazy(() => importSettingsShell().then((module) => ({
  default: module.SettingsShell,
})))

const SkillCenter = lazy(() => import('./SkillCenter').then((module) => ({
  default: module.SkillCenter,
})))

const McpCenter = lazy(() => import('./McpCenter').then((module) => ({
  default: module.McpCenter,
})))

const KnowledgeCenter = lazy(() => import('./KnowledgeCenter').then((module) => ({
  default: module.KnowledgeCenter,
})))

const PluginCenter = lazy(() => import('./PluginCenter').then((module) => ({
  default: module.PluginCenter,
})))

const NotesCenter = lazy(() => import('./NotesCenter').then((module) => ({
  default: module.NotesCenter,
})))

const MessageList = lazy(() => import('./MessageList').then((module) => ({
  default: module.MessageList,
})))

function MessageListLoading() {
  return (
    <div className="chat-themed-surface flex flex-1 items-center justify-center">
      <div className="h-5 w-5 animate-spin rounded-full border-2 border-neutral-300 border-t-neutral-800 dark:border-neutral-700 dark:border-t-neutral-200" />
    </div>
  )
}

type ChatView = 'conversation' | 'settings' | 'assistants' | 'skill' | 'mcp' | 'knowledge' | 'notes' | 'plugins' | 'onboarding'

interface ChatProps {
  onSettingsChange: () => void
  /**
   * 首屏内容就绪回调（一次性）。宿主（App）据此把窗口 show 从“App 挂载即弹出”推迟到
   * “Chat 首屏可渲染”，避免窗口弹出后仍在转圈。初始视图为设置页时，就绪信号来自
   * SettingsShell 的 onReady；其余视图挂载后即视为骨架就绪。
   */
  onContentReady?: () => void
}

/**
 * 设置页入场容器：先静态铺好起始态（下移 + 半透明），首帧绘制之后再加 --entered 触发过渡。
 *
 * 为什么不能直接用 CSS animation：animation 走墙钟时间，而 SettingsShell 那棵树很大，
 * 挂载帧的布局/绘制常吃掉上百毫秒 —— 等首帧真正画出来，动画已经跑完大半，体感就是"没有动画"
 * （退场没这问题，它作用于已绘制的元素）。双 rAF 把起点钉在首帧之后，整段位移必定可见。
 * 同款模式见 CompactionDivider。
 */
function SettingsEnterPane({ exiting, className, children }: {
  exiting: boolean
  className: string
  children: ReactNode
}) {
  const [entered, setEntered] = useState(false)

  useLayoutEffect(() => {
    let cancelled = false
    const frame = requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (!cancelled) setEntered(true)
      })
    })
    return () => {
      cancelled = true
      cancelAnimationFrame(frame)
    }
  }, [])

  const motion = exiting
    ? 'chat-motion-settings-out'
    : `chat-motion-settings-in${entered ? ' chat-motion-settings-in--entered' : ''}`

  return <div className={`${motion} ${className}`}>{children}</div>
}

/**
 * 记住用户在顶栏最后一次选的聊天模型与思考等级，作为新会话/空会话的默认（以用户的选择为准，
 * 取代旧的「默认模型」设置，也不再把思考等级硬回落到 high）。仅前端偏好，存 localStorage。
 */
const LAST_MODEL_KEY = 'kivio.chat.lastModel'
const LAST_THINKING_KEY = 'kivio.chat.lastThinkingLevel'

function loadLastModel(): { providerId: string; model: string } | null {
  try {
    const raw = window.localStorage.getItem(LAST_MODEL_KEY)
    if (!raw) return null
    const v = JSON.parse(raw)
    if (v && typeof v.providerId === 'string' && typeof v.model === 'string' && v.providerId) {
      return v
    }
  } catch {
    /* ignore */
  }
  return null
}

function saveLastModel(providerId: string, model: string): void {
  try {
    if (providerId) {
      window.localStorage.setItem(LAST_MODEL_KEY, JSON.stringify({ providerId, model }))
    }
  } catch {
    /* ignore */
  }
}

const VALID_THINKING_LEVELS: ReadonlySet<string> = new Set([
  'off',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
])

// 网络搜索模式全局默认（任务 07-23，与思考等级同款「记住上次选择」模式）：
// 选一次即成为新会话/未显式设置会话的默认，免去每个对话重复切换。
const LAST_WEB_SEARCH_MODE_KEY = 'kivio.chat.lastWebSearchMode'
const VALID_WEB_SEARCH_MODES: ReadonlySet<string> = new Set(['off', 'builtin', 'third_party'])

function loadLastWebSearchMode(): WebSearchMode | undefined {
  try {
    const raw = window.localStorage.getItem(LAST_WEB_SEARCH_MODE_KEY)
    return raw && VALID_WEB_SEARCH_MODES.has(raw) ? (raw as WebSearchMode) : undefined
  } catch {
    return undefined
  }
}

function saveLastWebSearchMode(mode: WebSearchMode): void {
  try {
    window.localStorage.setItem(LAST_WEB_SEARCH_MODE_KEY, mode)
  } catch {
    /* ignore */
  }
}

function loadLastThinkingLevel(): ThinkingLevel | null {
  try {
    const raw = window.localStorage.getItem(LAST_THINKING_KEY)
    return raw && VALID_THINKING_LEVELS.has(raw) ? (raw as ThinkingLevel) : null
  } catch {
    return null
  }
}

function saveLastThinkingLevel(level: ThinkingLevel | null): void {
  try {
    if (level) window.localStorage.setItem(LAST_THINKING_KEY, level)
    else window.localStorage.removeItem(LAST_THINKING_KEY)
  } catch {
    /* ignore */
  }
}









function scheduleIdleTask(callback: () => void, timeout = 1200): () => void {
  const idleWindow = window as Window & {
    requestIdleCallback?: (cb: () => void, options?: { timeout?: number }) => number
    cancelIdleCallback?: (handle: number) => void
  }
  if (idleWindow.requestIdleCallback && idleWindow.cancelIdleCallback) {
    const handle = idleWindow.requestIdleCallback(callback, { timeout })
    return () => idleWindow.cancelIdleCallback?.(handle)
  }

  const handle = window.setTimeout(callback, timeout)
  return () => window.clearTimeout(handle)
}

function toolEventToRecord(payload: ChatToolProgressPayload): ToolCallRecord {
  return {
    id: payload.id || payload.toolCallId,
    toolCallId: payload.toolCallId,
    conversationId: payload.conversationId,
    runId: payload.runId,
    messageId: payload.messageId,
    name: payload.name,
    source: payload.source,
    serverId: payload.serverId ?? undefined,
    status: normalizeToolCallStatus(payload.status),
    arguments: payload.argumentsPreview,
    argumentPreview: payload.argumentsPreview,
    argumentsPreview: payload.argumentsPreview,
    resultPreview: payload.resultPreview ?? undefined,
    error: payload.error ?? undefined,
    startedAt: payload.startedAt ?? undefined,
    completedAt: payload.completedAt ?? undefined,
    durationMs: payload.durationMs ?? undefined,
    round: payload.round,
    sensitive: payload.sensitive,
    artifacts: payload.artifacts ?? [],
    traceId: payload.traceId ?? undefined,
    spanId: payload.spanId ?? undefined,
    structuredContent: payload.structuredContent,
  }
}

function userPromptEventToRecord(payload: ChatUserPromptPayload): ToolCallRecord {
  return {
    id: payload.id || payload.toolCallId,
    toolCallId: payload.toolCallId,
    conversationId: payload.conversationId,
    runId: payload.runId,
    messageId: payload.messageId,
    name: payload.name || 'ask_user',
    source: payload.source || 'native',
    status: 'running',
    arguments: payload.prompt,
    args: payload.prompt,
    input: payload.prompt,
    sensitive: false,
    artifacts: [],
    structuredContent: payload.structuredContent ?? {
      askUser: {
        phase: 'awaiting',
        title: payload.prompt.title,
        questions: payload.prompt.questions,
        answers: {},
      },
    },
  }
}

/** 工具名 → 自然语言动词。`path` 表示操作对象是文件路径（标题里只显示文件名）。 */
const TOOL_APPROVAL_VERBS: Record<string, { verb: string; path?: boolean }> = {
  write: { verb: '写入', path: true },
  write_file: { verb: '写入', path: true },
  edit: { verb: '修改', path: true },
  edit_file: { verb: '修改', path: true },
  notebookedit: { verb: '修改', path: true },
  read: { verb: '读取', path: true },
  read_file: { verb: '读取', path: true },
  bash: { verb: '执行' },
  run_command: { verb: '执行' },
}

/**
 * 审批卡标题。后端认出操作对象（`target`）时拼「允许写入 xxx.md？」，认不出就退回工具名。
 * 工具名小写后匹配：内置 agent 报 `write`，外部 CLI 报自己的原名（claude 的 `Write`）。
 */
function toolApprovalTitle(payload: ChatToolConfirmPayload): string {
  const spec = TOOL_APPROVAL_VERBS[(payload.name || '').toLowerCase()]
  const target = payload.target?.trim()
  if (!spec || !target) return `允许调用工具 ${payload.name}？`
  const shown = spec.path ? target.split(/[\\/]/).filter(Boolean).pop() || target : target
  return `允许${spec.verb} ${shown}？`
}

function streamPayloadToSegment(payload: ChatStreamPayload): ChatMessageSegment | null {
  const raw = payload.segment ?? null
  const id = payload.segmentId ?? raw?.id
  const kind = payload.segmentKind ?? raw?.kind
  const phase = payload.phase ?? raw?.phase
  const order = payload.order ?? raw?.order
  if (!id || !kind || !phase || order == null) return null

  const stepNumber = raw?.step_number ?? raw?.stepNumber ?? payload.stepNumber ?? null
  const toolCallId = raw?.tool_call_id ?? raw?.toolCallId ?? payload.toolCallId ?? null
  return {
    id,
    kind,
    phase,
    order,
    step_number: stepNumber,
    stepNumber,
    round: raw?.round ?? payload.round ?? null,
    text: raw?.text ?? null,
    tool_call_id: toolCallId,
    toolCallId,
  }
}

function upsertStreamSegment(
  segments: ChatMessageSegment[],
  incoming: ChatMessageSegment,
  delta = '',
): ChatMessageSegment[] {
  const incomingToolCallId = segmentToolCallId(incoming)
  const index = segments.findIndex((segment) => (
    segment.id === incoming.id ||
    (incoming.kind === 'tool' &&
      segment.kind === 'tool' &&
      incomingToolCallId &&
      segmentToolCallId(segment) === incomingToolCallId)
  ))
  const existing = index >= 0 ? segments[index] : null
  const nextText = incoming.kind === 'tool'
    ? incoming.text ?? existing?.text ?? null
    : (() => {
        const base = existing?.text ?? incoming.text ?? ''
        const append = !existing && incoming.text && incoming.text === delta ? '' : delta
        return `${base}${append}`
      })()
  const existingStepNumber = existing ? segmentStepNumber(existing) : null
  const incomingStepNumber = segmentStepNumber(incoming)
  const nextSegment: ChatMessageSegment = {
    ...existing,
    ...incoming,
    step_number: incomingStepNumber ?? existingStepNumber ?? null,
    stepNumber: incomingStepNumber ?? existingStepNumber ?? null,
    tool_call_id: incoming.tool_call_id ?? incoming.toolCallId ?? existing?.tool_call_id ?? existing?.toolCallId ?? null,
    toolCallId: incoming.toolCallId ?? incoming.tool_call_id ?? existing?.toolCallId ?? existing?.tool_call_id ?? null,
    text: nextText,
  }
  const next = index < 0
    ? [...segments, nextSegment]
    : segments.map((segment, i) => (i === index ? nextSegment : segment))
  return next.sort(compareTimelineSegments)
}

function nextSegmentOrder(segments: ChatMessageSegment[]): number {
  if (segments.length === 0) return 1
  return Math.max(...segments.map((segment) => segment.order ?? 0)) + 1
}

function upsertToolStreamSegment(
  segments: ChatMessageSegment[],
  record: ToolCallRecord,
): ChatMessageSegment[] {
  const toolCallId = record.id || record.toolCallId || ''
  if (!toolCallId) return segments
  const exists = segments.some(
    (segment) => segment.kind === 'tool' && segmentToolCallId(segment) === toolCallId,
  )
  if (exists) return segments
  return upsertStreamSegment(segments, {
    id: `seg_tool_${toolCallId}`,
    kind: 'tool',
    phase: 'tool_loop',
    order: nextSegmentOrder(segments),
    round: record.round ?? 1,
    tool_call_id: toolCallId,
    toolCallId,
  })
}

function sameSegmentField<T>(left: T | null | undefined, right: T | null | undefined): boolean {
  return (left ?? null) === (right ?? null)
}

// 设置当前视图的流式错误（写 streamingStore 的 coarse 片）。模块级函数，调用点无需进
// useCallback 依赖。注意：与 setStreamErrorForConversation 不同，这里只改当前视图、不写
// streamErrorsRef（保持原 setStreamError(useState) 的语义）。
function setStreamError(error: string): void {
  setStreamCoarse({ streamError: error })
}

function findReasoningSegmentForText(
  segments: ChatMessageSegment[],
  textSegment: ChatMessageSegment,
): ChatMessageSegment | null {
  const reversedReasoning = [...segments]
    .reverse()
    .filter((item) => item.kind === 'reasoning')
  const textStepNumber = segmentStepNumber(textSegment)
  const textRound = textSegment.round ?? null

  return reversedReasoning.find((item) => (
    segmentStepNumber(item) === textStepNumber &&
    sameSegmentField(item.round, textRound) &&
    item.phase === textSegment.phase
  ))
    ?? reversedReasoning.find((item) => (
      segmentStepNumber(item) === textStepNumber &&
      sameSegmentField(item.round, textRound)
    ))
    ?? reversedReasoning.find((item) => segmentStepNumber(item) === textStepNumber)
    ?? reversedReasoning[0]
    ?? null
}

function updateReasoningSegmentDuration(
  snapshot: ConversationStreamSnapshot,
  segmentId: string,
  now = Date.now(),
) {
  const startedAt = snapshot.reasoningStartedAtBySegmentId[segmentId]
  if (startedAt == null) return
  snapshot.reasoningDurationMsBySegmentId = {
    ...snapshot.reasoningDurationMsBySegmentId,
    [segmentId]: Math.max(
      snapshot.reasoningDurationMsBySegmentId[segmentId] ?? 0,
      now - startedAt,
    ),
  }
}

// 把一条 chat-stream delta 累积进给定快照（会话单流 or 多答组某列共用）。
// 原地 mutate snapshot；segment 已由调用方算好。返回 void。
function applyStreamDeltaToSnapshot(
  snapshot: ConversationStreamSnapshot,
  payload: ChatStreamPayload,
  segment: ChatMessageSegment | null,
) {
  if (segment) {
    snapshot.segments = upsertStreamSegment(
      snapshot.segments,
      segment,
      segment.kind === 'reasoning' ? payload.reasoningDelta ?? '' : payload.delta ?? '',
    )
  }
  if (payload.reasoningDelta) {
    const now = Date.now()
    if (snapshot.reasoningStartedAt == null) {
      snapshot.reasoningStartedAt = now
    }
    if (segment?.kind === 'reasoning') {
      const segmentStartedAt = snapshot.reasoningStartedAtBySegmentId[segment.id] ?? now
      snapshot.reasoningStartedAtBySegmentId[segment.id] = segmentStartedAt
      updateReasoningSegmentDuration(snapshot, segment.id, now)
    }
    snapshot.streaming = true
    snapshot.reasoningStreaming = true
    snapshot.reasoning += payload.reasoningDelta
    snapshot.reasoningDurationMs = Math.max(
      snapshot.reasoningDurationMs ?? 0,
      now - snapshot.reasoningStartedAt,
    )
  }
  if (payload.delta) {
    if (snapshot.reasoningStreaming && snapshot.reasoningStartedAt != null) {
      snapshot.reasoningDurationMs = Math.max(
        snapshot.reasoningDurationMs ?? 0,
        Date.now() - snapshot.reasoningStartedAt,
      )
    }
    if (segment?.kind === 'text') {
      const activeReasoningSegment = findReasoningSegmentForText(snapshot.segments, segment)
      if (activeReasoningSegment) {
        updateReasoningSegmentDuration(snapshot, activeReasoningSegment.id)
      }
    }
    snapshot.streaming = true
    snapshot.reasoningStreaming = false
    snapshot.content += payload.delta
  }
}

// done 帧收尾：补齐最后一段 reasoning 的时长。原地 mutate。
function finalizeReasoningDurationOnDone(snapshot: ConversationStreamSnapshot) {
  if (snapshot.reasoningStartedAt != null && snapshot.reasoningStreaming) {
    snapshot.reasoningDurationMs = Math.max(
      snapshot.reasoningDurationMs ?? 0,
      Date.now() - snapshot.reasoningStartedAt,
    )
    const activeReasoningSegment = [...snapshot.segments]
      .reverse()
      .find((item) => item.kind === 'reasoning')
    if (activeReasoningSegment) {
      updateReasoningSegmentDuration(snapshot, activeReasoningSegment.id)
    }
  }
}

// 把一条 chat-tool record 累积进给定快照（会话单流 or 多答组某列共用）。原地 mutate。
function applyToolRecordToSnapshot(
  snapshot: ConversationStreamSnapshot,
  record: ToolCallRecord,
) {
  snapshot.streaming = true
  snapshot.reasoningStreaming = false
  const index = snapshot.toolCalls.findIndex((item) => item.id === record.id)
  snapshot.toolCalls = index < 0
    ? [...snapshot.toolCalls, record]
    : snapshot.toolCalls.map((item, i) => (i === index ? { ...item, ...record } : item))
  snapshot.segments = upsertToolStreamSegment(snapshot.segments, record)
}

function normalizeSkill(skill: import('../api/tauri').SkillMeta): SkillMeta {
  return {
    id: skill.id,
    name: skill.name,
    description: skill.description,
    source: skill.source,
    path: skill.path ?? undefined,
    recommendedTools: skill.recommendedTools,
    disableModelInvocation: skill.disableModelInvocation,
    files: skill.files,
  }
}

function skillRecommendedTools(skill?: SkillMeta | null): string[] {
  return skill?.recommended_tools ?? skill?.recommendedTools ?? []
}

function toolMatchesRecommendation(tool: ChatToolDefinition, recommended: string): boolean {
  const name = recommended.trim()
  if (!name) return false
  return (
    tool.name === name ||
    tool.id === name ||
    `${tool.serverId ?? ''}:${tool.name}` === name
  )
}

function attachmentExtension(name: string): string {
  return name.split('.').pop()?.toLowerCase() ?? ''
}

function documentSkillNameForAttachment(attachment: PendingAttachment): string | null {
  if (attachment.type === 'image') return null
  switch (attachmentExtension(attachment.name)) {
    case 'pdf':
      return 'pdf'
    case 'doc':
    case 'docx':
      return 'docx'
    case 'xls':
    case 'xlsx':
    case 'xlsm':
    case 'csv':
    case 'tsv':
      return 'xlsx'
    default:
      return null
  }
}

function findEnabledSkillId(skills: SkillMeta[], skillName: string): string | null {
  const normalized = skillName.toLowerCase()
  return skills.find((skill) => (
    skill.id.toLowerCase() === normalized || skill.name.toLowerCase() === normalized
  ))?.id ?? null
}

function inferSingleAttachmentSkillId(
  attachments: PendingAttachment[],
  skills: SkillMeta[],
): string | null {
  const skillNames = Array.from(new Set(
    attachments
      .map(documentSkillNameForAttachment)
      .filter((name): name is string => Boolean(name)),
  ))
  if (skillNames.length !== 1) return null
  return findEnabledSkillId(skills, skillNames[0])
}

function isLocallyCancelledPayload(
  payload: { conversationId: string; runId?: string },
  cancelledConversationId: string | null,
  cancelledRunId: string | null,
): boolean {
  if (cancelledConversationId !== payload.conversationId) return false
  return !cancelledRunId || !payload.runId || payload.runId === cancelledRunId
}

function isPlainBlankConversation(conversation: Conversation | null): boolean {
  return Boolean(
    conversation
    && conversation.messages.length === 0
    && !(conversation.assistant_id ?? conversation.assistantId),
  )
}

function conversationUsesModel(
  conversation: Conversation,
  providerId: string,
  model: string,
): boolean {
  return conversation.provider_id === providerId && conversation.model === model
}

function optimisticConversationTitle(content: string): string {
  const compact = content.replace(/\s+/g, ' ').trim()
  if (!compact) return '新对话'
  return compact.length > 30 ? `${compact.slice(0, 30)}...` : compact
}

function optimisticConversationListItem(
  conversation: Conversation,
  content: string,
): ConversationListItem {
  const preview = content.replace(/\s+/g, ' ').trim()
  const title = conversation.title === '新对话'
    ? optimisticConversationTitle(content)
    : conversation.title
  return {
    id: conversation.id,
    title,
    preview: preview.length > 100 ? `${preview.slice(0, 100)}...` : preview,
    provider_id: conversation.provider_id,
    model: conversation.model,
    message_count: Math.max(1, conversation.messages.length),
    created_at: conversation.created_at,
    updated_at: Math.floor(Date.now() / 1000),
    pinned: conversation.pinned,
    folder: conversation.folder,
    project_id: conversation.project_id ?? conversation.projectId ?? null,
    projectId: conversation.project_id ?? conversation.projectId ?? null,
    set_id: conversation.set_id ?? conversation.setId ?? null,
    setId: conversation.set_id ?? conversation.setId ?? null,
    assistant_id: conversation.assistant_id ?? conversation.assistantId ?? null,
    assistantId: conversation.assistant_id ?? conversation.assistantId ?? null,
    assistant_name:
      conversation.assistant_snapshot?.name
      ?? conversation.assistantSnapshot?.name
      ?? null,
    assistantName:
      conversation.assistant_snapshot?.name
      ?? conversation.assistantSnapshot?.name
      ?? null,
  }
}

type SendMessageOptions = {
  forceNewConversation?: boolean
  conversationOverride?: Conversation | null
}

export default function Chat({ onSettingsChange, onContentReady }: ChatProps) {
  const [chatView, setChatView] = useState<ChatView>(() => {
    const path = hashPath()
    if (isChatOnboardingRoute(path)) return 'onboarding'
    if (isChatSettingsPath(path)) return 'settings'
    if (isChatAssistantCenterPath(path)) return 'assistants'
    if (isChatSkillCenterPath(path)) return 'skill'
    if (isChatMcpCenterPath(path)) return 'mcp'
    if (isChatKnowledgeCenterPath(path)) return 'knowledge'
    if (isChatNotesPath(path)) return 'notes'
    if (isChatPluginCenterPath(path)) return 'plugins'
    return 'conversation'
  })
  // 首屏就绪只发一次。初始视图是设置页则等 SettingsShell.onReady；否则挂载后即发。
  const contentReadyEmittedRef = useRef(false)
  const emitContentReady = useCallback(() => {
    if (contentReadyEmittedRef.current) return
    contentReadyEmittedRef.current = true
    onContentReady?.()
  }, [onContentReady])
  const initialViewIsSettingsRef = useRef(chatView === 'settings')
  useLayoutEffect(() => {
    // 初始设置页把就绪信号委托给 SettingsShell.onReady（数据就绪才可渲染）；
    // 其余初始视图（会话/助手/技能/引导）挂载即有骨架，直接发信号。
    if (!initialViewIsSettingsRef.current) emitContentReady()
  }, [emitContentReady])
  const [currentConversation, setCurrentConversation] = useState<Awaited<
    ReturnType<typeof chatApi.getConversation>
  > | null>(null)
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => getRememberedChatSidebarCollapsed())
  const [searchOpen, setSearchOpen] = useState(false)
  const [selectedProject, setSelectedProject] = useState<ChatProject | null>(null)
  const [selectedSet, setSelectedSet] = useState<ChatSet | null>(null)
  // 流式高频状态已移到 streamingStore（useSyncExternalStore）。Chat 只订阅 coarse 这一片
  // （streaming/streamFrozen/cancelling/streamError，边沿才变），用于 showEmptyHero / drain 判定；
  // 内容快照由 MessageList 直接订阅，避免每帧 token 拖着整个 Chat 重渲。
  const streamCoarse = useStreamCoarse()
  /** 发送中待显示的用户消息（与 conversation 分离，避免 route reload 冲掉） */
  const [pendingUserMessage, setPendingUserMessage] = useState<ChatMessage | null>(null)
  const [pendingUserMessageConversationId, setPendingUserMessageConversationId] = useState<string | null>(null)
  const [assistantStreamStatsByMessageId, setAssistantStreamStatsByMessageId] =
    useState<Record<string, AssistantStreamStats>>({})
  const [sidebarRefreshKey, setSidebarRefreshKey] = useState(0)
  const [optimisticSidebarConversations, setOptimisticSidebarConversations] =
    useState<ConversationListItem[]>([])
  const [generatingConversationIds, setGeneratingConversationIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  )
  const [sidebarProfileRefreshKey, setSidebarProfileRefreshKey] = useState(0)
  const [draftProviderId, setDraftProviderId] = useState('')
  const [draftModel, setDraftModel] = useState('')
  // 欢迎页（尚无会话）时挂载的知识库草稿；首次发送建会话时落到会话上。
  const [draftKnowledgeBaseIds, setDraftKnowledgeBaseIds] = useState<string[]>([])
  const [draftForceKnowledgeSearch, setDraftForceKnowledgeSearch] = useState(false)
  // 欢迎页思考等级草稿；首次发送建会话时落到会话上。null=跟随全局。
  const [draftThinkingLevel, setDraftThinkingLevel] = useState<ThinkingLevel | null>(loadLastThinkingLevel)
  // 欢迎页联网搜索模式草稿（任务 07-23）；首次发送建会话时落到会话上。undefined=跟随全局。
  const [draftWebSearchMode, setDraftWebSearchMode] = useState<WebSearchMode | undefined>(undefined)
  // 多模型一问多答（任务 06-30）：欢迎页（尚无会话）时的多答模型草稿；首次发送建会话时落到会话上。
  const [draftReplyModels, setDraftReplyModels] = useState<ModelRef[]>([])
  const [draftAgentRuntime, setDraftAgentRuntime] = useState<AgentRuntimeConfig>(
    BUILTIN_AGENT_RUNTIME,
  )
  const [skills, setSkills] = useState<SkillMeta[]>([])
  const [disabledSkillIds, setDisabledSkillIds] = useState<string[]>([])
  const [settingsInitialTab, setSettingsInitialTab] = useState<SettingsTab>('chat')
  const [uiLang, setUiLang] = useState<Lang>('zh')
  const [extensionsNavItem, setExtensionsNavItem] = useState<ExtensionsNavItem | null>(null)
  const [enabledTools, setEnabledTools] = useState<ChatToolDefinition[]>([])
  const [mcpServers, setMcpServers] = useState<ChatMcpServer[]>([])
  const [webSearchEnabled, setWebSearchEnabled] = useState(true)
  // provider id → apiFormat（任务 07-23）：用于判断当前模型是否支持内置搜索。
  const [providerApiFormats, setProviderApiFormats] = useState<Record<string, string>>({})
  const [enabledToolCount, setEnabledToolCount] = useState<number | null>(null)
  const [toolsDisabledReason, setToolsDisabledReason] = useState('')
  const [toolsRequested, setToolsRequested] = useState(false)
  const [approvalPolicy, setApprovalPolicy] = useState('readonly_auto_sensitive_confirm')
  const [pendingToolConfirm, setPendingToolConfirm] = useState<ChatToolConfirmPayload | null>(null)
  const [pendingSessionConsent, setPendingSessionConsent] = useState<ChatSessionConsentPayload | null>(null)
  const [contextState, setContextState] = useState<ConversationContextState | null>(null)
  const [contextLoading, setContextLoading] = useState(false)
  const [contextCompressing, setContextCompressing] = useState(false)
  const [agentLoopCompacting, setAgentLoopCompacting] = useState(false)
  const [animateCompactionBoundaryId, setAnimateCompactionBoundaryId] = useState<string | null>(null)
  const [contextError, setContextError] = useState('')
  // Hook 执行失败（chat-hook）：非阻断警告条。ponytail: 只留最新一条 —— Hook 是旁路观测，
  // 堆一个可滚动的失败列表没有对应的用户动作。
  const [hookWarning, setHookWarning] = useState<ChatHookPayload | null>(null)
  const [imageViewerItem, setImageViewerItem] = useState<ChatImageViewerItem | null>(null)
  const currentConversationIdRef = useRef<string | null>(null)
  // 始终指向最新 currentConversation。消息操作 handler（编辑/删除/重发）借此读取最新会话，
  // 而无需把 currentConversation 列进 useCallback 依赖——否则每次切模型/思考等级（currentConversation
  // 换引用）这些 handler 都换身份，打穿 MessageBubble 的 memo 导致全列表重渲（公式 remount 闪烁）。
  const currentConversationRef = useRef(currentConversation)
  currentConversationRef.current = currentConversation
  const activeRunIdRef = useRef<string | null>(null)
  const locallyCancelledConversationIdRef = useRef<string | null>(null)
  const locallyCancelledRunIdRef = useRef<string | null>(null)
  const inFlightConversationsRef = useRef<Set<string>>(new Set())
  const pendingStreamDoneRef = useRef<Record<string, () => Promise<void>>>({})
  const streamSnapshotsRef = useRef<Record<string, ConversationStreamSnapshot>>({})
  const streamErrorsRef = useRef<Record<string, string>>({})
  const pendingToolConfirmsRef = useRef<Record<string, ChatToolConfirmPayload>>({})
  const pendingSessionConsentsRef = useRef<Record<string, ChatSessionConsentPayload>>({})
  const streamStartedAtRef = useRef<number | null>(null)
  const streamingContentRef = useRef('')
  const streamingReasoningRef = useRef('')
  const settingsRef = useRef<SettingsShellHandle>(null)
  const pendingAfterSettingsCloseRef = useRef<(() => void) | null>(null)
  // A 合帧（render coalescing）：高频 stream/tool/subagent/userprompt 事件不再每条都同步
  // setState 重渲，而是把"待显示的快照"记到 ref，用 requestAnimationFrame 每帧最多 flush 一次。

  useEffect(() => onChatImageViewerOpen(setImageViewerItem), [])

  // 会话本地运行态的聚合视图：6 处「按会话清理」共用（见 conversationLocalState.ts）。
  // ref 仍各自独立持有 —— 读取侧有 30 处、语义各异，不适合一并打包。
  // 每次现取 .current 而非快照：flushPendingStreamDone 会整体替换
  // pendingStreamDoneRef.current，快照会指向旧对象。
  const localState = useCallback((): ConversationLocalState => ({
    inFlight: inFlightConversationsRef.current,
    pendingStreamDone: pendingStreamDoneRef.current,
    streamSnapshots: streamSnapshotsRef.current,
    streamErrors: streamErrorsRef.current,
    pendingToolConfirms: pendingToolConfirmsRef.current,
    pendingSessionConsents: pendingSessionConsentsRef.current,
  }), [])

  const syncGeneratingConversationIds = useCallback(() => {
    setGeneratingConversationIds(collectGeneratingConversationIds(
      inFlightConversationsRef.current,
      streamSnapshotsRef.current,
      pendingToolConfirmsRef.current,
    ))
  }, [])

  const markConversationInFlight = useCallback((conversationId: string) => {
    inFlightConversationsRef.current.add(conversationId)
    syncGeneratingConversationIds()
  }, [syncGeneratingConversationIds])

  const clearConversationInFlight = useCallback((conversationId: string) => {
    inFlightConversationsRef.current.delete(conversationId)
    syncGeneratingConversationIds()
  }, [syncGeneratingConversationIds])

  // 合帧抽成 useStreamRenderFrame。applyStreamSnapshotToState 定义在下方
  // （它依赖此处之后才声明的 setter），故经 ref 间接调用。
  const applyStreamSnapshotToStateRef = useRef<((s: ConversationStreamSnapshot) => void) | null>(null)
  const {
    cancelPendingFrame,
    cancelPendingFrameFor,
    showStreamSnapshotIfCurrent,
  } = useStreamRenderFrame({
    applySnapshot: (snapshot) => applyStreamSnapshotToStateRef.current?.(snapshot),
    currentConversationIdRef,
  })

  // B：彻底把一个会话从所有本地乐观/in-flight/快照状态中剔除（ghost 清理）。
  // 不触碰 currentConversation/route，由调用方按场景决定。
  const dropConversationLocally = useCallback((conversationId: string) => {
    clearConversationLocalState(localState(), conversationId, {
      inFlight: true, pendingStreamDone: true, streamErrors: true,
    })
    // 若该会话还挂着待刷新的合帧，连带取消，避免被剔除的 ghost 还闪一帧。
    cancelPendingFrameFor(conversationId)
    setOptimisticSidebarConversations((items) => items.filter((item) => item.id !== conversationId))
    syncGeneratingConversationIds()
  }, [cancelPendingFrameFor, localState, syncGeneratingConversationIds])

  const setStreamErrorForConversation = useCallback((conversationId: string, error: string) => {
    if (error) {
      streamErrorsRef.current[conversationId] = error
    } else {
      delete streamErrorsRef.current[conversationId]
    }
    if (currentConversationIdRef.current === conversationId) {
      setStreamCoarse({ streamError: error })
    }
  }, [])

  const isCurrentConversationBusy = useCallback(() => (
    isConversationBusy(
      currentConversationIdRef.current,
      inFlightConversationsRef.current,
      streamSnapshotsRef.current,
    )
  ), [])

  const applyConversation = useCallback((conversation: Conversation | null) => {
    // 兜底网：后端已在所有返回 Conversation 的命令出口剥离 model_messages/api_messages
    // （strip_transcripts_for_frontend），所以正常路径到这里已是轻量副本。这里再剥一次，确保
    // 任何遗漏/未来新增的后端出口都不会把这两份前端永不读的转录留进 React state。后端回放读盘
    // 上完整副本，不受影响。
    if (conversation?.messages) {
      for (const m of conversation.messages) {
        if (m.role !== 'assistant') continue
        m.model_messages = undefined
        m.modelMessages = undefined
        m.api_messages = undefined
        m.apiMessages = undefined
      }
    }
    setCurrentConversation(conversation)
    setContextState(conversation?.context_state ?? conversation?.contextState ?? null)
  }, [])

  // 纯元数据更新（模型 / 思考等级 / 知识库挂载等）：合并后端返回的新元数据，但**保留现有
  // messages 数组引用**。否则每条消息都变成新对象，击穿 MessageBubble/ChatMarkdown 的 memo，
  // 历史消息里的 LaTeX 会整屏重渲闪一下。这类更新后端不会改 messages，沿用旧引用安全。
  const applyConversationMeta = useCallback((updated: Conversation) => {
    setCurrentConversation((prev) =>
      prev && prev.id === updated.id ? { ...updated, messages: prev.messages } : updated,
    )
  }, [])

  const patchContextState = useCallback((nextState: ConversationContextState) => {
    setContextState((prev) => {
      const merged = mergeCompactionContextState(prev, nextState)
      setCurrentConversation((conversation) => conversation
        ? { ...conversation, context_state: merged, contextState: merged }
        : conversation)
      return merged
    })
  }, [])

  const patchAgentTodoState = useCallback((nextState: AgentTodoState) => {
    setCurrentConversation((prev) => prev
      ? { ...prev, agent_todo_state: nextState, agentTodoState: nextState }
      : prev)
  }, [])

  const patchAgentPlanState = useCallback((nextState: AgentPlanState) => {
    setCurrentConversation((prev) => prev
      ? { ...prev, agent_plan_state: nextState, agentPlanState: nextState }
      : prev)
  }, [])

  const clearStreamingPreview = useCallback(() => {
    // 取消挂起的合帧，避免旧快照在清空后又被刷回来产生空帧/串帧。
    cancelPendingFrame()
    // 内容回空闲 + streaming/frozen/cancelling 归位；streamError 不动（与原语义一致）。
    resetStreamStore()
    activeRunIdRef.current = null
    streamStartedAtRef.current = null
    streamingContentRef.current = ''
    streamingReasoningRef.current = ''
  }, [cancelPendingFrame])

  const ensureStreamSnapshot = useCallback((conversationId: string) => {
    const existing = streamSnapshotsRef.current[conversationId]
    if (existing) return existing
    const snapshot = createEmptyStreamSnapshot()
    streamSnapshotsRef.current[conversationId] = snapshot
    syncGeneratingConversationIds()
    return snapshot
  }, [syncGeneratingConversationIds])

  const restoreStreamingPreview = useCallback((conversationId: string | null) => {
    // 切换会话/恢复预览前取消任何挂起的合帧，避免上一个会话的快照被刷到当前视图。
    cancelPendingFrame()
    if (!conversationId) {
      clearStreamingPreview()
      setPendingToolConfirm(null)
      setPendingSessionConsent(null)
      setStreamCoarse({ streamError: '' })
      return
    }
    const snapshot = streamSnapshotsRef.current[conversationId]
    if (!snapshot) {
      clearStreamingPreview()
    } else {
      setStreamSnapshot(snapshot)
      setStreamCoarse({ streaming: snapshot.streaming, streamFrozen: false, cancelling: false })
      activeRunIdRef.current = snapshot.runId
      streamStartedAtRef.current = snapshot.startedAt
      streamingContentRef.current = snapshot.content
      streamingReasoningRef.current = snapshot.reasoning
    }
    setStreamCoarse({ streamError: streamErrorsRef.current[conversationId] ?? '' })
    setPendingToolConfirm(pendingToolConfirmsRef.current[conversationId] ?? null)
    setPendingSessionConsent(pendingSessionConsentsRef.current[conversationId] ?? null)
  }, [cancelPendingFrame, clearStreamingPreview])

  const applyStreamSnapshotToState = useCallback((snapshot: ConversationStreamSnapshot) => {
    setStreamSnapshot(snapshot)
    setStreamCoarse({ streaming: snapshot.streaming, cancelling: false })
    activeRunIdRef.current = snapshot.runId
    streamStartedAtRef.current = snapshot.startedAt
    streamingContentRef.current = snapshot.content
    streamingReasoningRef.current = snapshot.reasoning
  }, [])

  // 填充上面 useStreamRenderFrame 用来打破声明顺序依赖的间接层。
  applyStreamSnapshotToStateRef.current = applyStreamSnapshotToState

  useEffect(() => () => {
    // 卸载时清掉所有活跃多答组，避免遗留列快照。（挂起帧的取消已在 useStreamRenderFrame 内）
    resetGroups()
  }, [])

  const clearStreamSnapshot = useCallback((conversationId: string | null) => {
    if (!conversationId) return
    clearConversationLocalState(localState(), conversationId)
    syncGeneratingConversationIds()
    if (currentConversationIdRef.current === conversationId) {
      setPendingToolConfirm(null)
      setPendingSessionConsent(null)
      clearStreamingPreview()
    }
  }, [clearStreamingPreview, localState, syncGeneratingConversationIds])

  const cancelCurrentRunLocally = useCallback(() => {
    locallyCancelledConversationIdRef.current = currentConversationIdRef.current
    locallyCancelledRunIdRef.current = activeRunIdRef.current
    // 立即停掉"生成中"视觉（撤掉取消按钮 + 停 shimmer），但保留已生成文本：
    // 切到 frozen 态冻结展示，等 send invoke 返回持久化消息时由
    // finishStreamingRunWithConversation 无缝替换（clearStreamingPreview 会清除 frozen）。
    // 后续迟到的流事件已被 isLocallyCancelledPayload 过滤，预览不会再变动。
    setStreamCoarse({ streaming: false, streamFrozen: true })
    patchStreamSnapshot({ reasoningStreaming: false })
    const conversationId = currentConversationIdRef.current
    if (conversationId) {
      delete pendingToolConfirmsRef.current[conversationId]
      delete pendingSessionConsentsRef.current[conversationId]
    }
    setPendingToolConfirm(null)
    setPendingSessionConsent(null)
  }, [])

  const resetLocalCancellation = useCallback(() => {
    locallyCancelledConversationIdRef.current = null
    locallyCancelledRunIdRef.current = null
  }, [])

  const activeAgentRuntime = useMemo(
    () => (currentConversation ? normalizeAgentRuntime(currentConversation) : draftAgentRuntime),
    [currentConversation, draftAgentRuntime],
  )
  const usesExternalRuntime = activeAgentRuntime.kind === 'external' && !!activeAgentRuntime.externalAgentId
  const currentConversationIsBlank = isPlainBlankConversation(currentConversation)
  const activeProviderId = currentConversation && !currentConversationIsBlank
    ? currentConversation.provider_id
    : draftProviderId
  const activeModel = currentConversation && !currentConversationIsBlank
    ? currentConversation.model
    : draftModel
  // 会话级三态联网搜索（任务 07-23）：会话显式模式优先 → 记住的全局默认（上次选择）
  // → 全局 nativeTools.webSearch 开关。这样选一次内置即成为所有新对话的默认。
  const activeWebSearchMode = useMemo<WebSearchMode>(() => {
    if (currentConversation && !currentConversationIsBlank) {
      const explicit = currentConversation.webSearchMode ?? currentConversation.web_search_mode
      if (explicit) return explicit
    } else if (draftWebSearchMode) {
      return draftWebSearchMode
    }
    const remembered = loadLastWebSearchMode()
    if (remembered) return remembered
    return webSearchEnabled ? 'third_party' : 'off'
  }, [currentConversation, currentConversationIsBlank, draftWebSearchMode, webSearchEnabled])
  const activeBuiltinWebSearchSupported = useMemo(
    () => builtinWebSearchSupported(providerApiFormats[activeProviderId ?? '']),
    [providerApiFormats, activeProviderId],
  )
  // 多模型一问多答（任务 06-30）：当前生效的多答模型集（会话级持久 reply_models，欢迎页用草稿）。
  const activeReplyModels = useMemo<ModelRef[]>(
    () => (currentConversation && !currentConversationIsBlank
      ? currentConversation.reply_models ?? currentConversation.replyModels ?? []
      : draftReplyModels),
    [currentConversation, currentConversationIsBlank, draftReplyModels],
  )
  const storedActiveSkillId = currentConversation
    ? currentConversation.active_skill_id ?? currentConversation.activeSkillId ?? null
    : null
  // 当前会话自身所属项目（id + 名 folder）。传给输入栏，使从「最近」打开的项目内对话
  // 也能在项目按钮上显示其项目，即便导航态 selectedProject 已被清空。
  const conversationProject = useMemo<{ id: string; name: string } | null>(() => {
    const id = currentConversation?.project_id ?? currentConversation?.projectId ?? null
    if (!id) return null
    return { id, name: currentConversation?.folder ?? '' }
  }, [currentConversation?.project_id, currentConversation?.projectId, currentConversation?.folder])
  const enabledSkills = useMemo(
    () => skills.filter((skill) => !disabledSkillIds.includes(skill.id)),
    [disabledSkillIds, skills],
  )
  const slashSkills = useMemo(
    () => enabledSkills.map((skill) => ({
      id: skill.id,
      name: skill.name,
      description: skill.description,
      argumentHint: skill.argumentHint ?? skill.argument_hint ?? undefined,
      disableModelInvocation: skill.disableModelInvocation ?? skill.disable_model_invocation,
    })),
    [enabledSkills],
  )
  const effectiveSkillId = useMemo(() => {
    if (
      storedActiveSkillId
      && enabledSkills.some((skill) => skill.id === storedActiveSkillId)
    ) {
      return storedActiveSkillId
    }
    return null
  }, [enabledSkills, storedActiveSkillId])
  const effectiveSkill = useMemo(
    () => enabledSkills.find((skill) => skill.id === effectiveSkillId) ?? null,
    [effectiveSkillId, enabledSkills],
  )
  const effectiveSkillRecommendedTools = useMemo(
    () => skillRecommendedTools(effectiveSkill),
    [effectiveSkill],
  )

  const currentAssistantSnapshot =
    currentConversation?.assistant_snapshot ?? currentConversation?.assistantSnapshot ?? null
  const currentAssistantId =
    currentConversation?.assistant_id
    ?? currentConversation?.assistantId
    ?? currentAssistantSnapshot?.id
    ?? null

  const refreshToolIndicator = useCallback(async () => {
    if (!isTauriRuntime()) {
      setEnabledTools([])
      setEnabledToolCount(null)
      setToolsDisabledReason('')
      setToolsRequested(false)
      setApprovalPolicy('readonly_auto_sensitive_confirm')
      setMcpServers([])
      return
    }
    try {
      const settings = await getSettingsCached()
      const chatTools = settings.chatTools
      setMcpServers(chatTools?.servers ?? [])
      setWebSearchEnabled(chatTools?.nativeTools?.webSearch !== false)
      setProviderApiFormats(
        Object.fromEntries((settings.providers ?? []).map((p) => [p.id, p.apiFormat ?? ''])),
      )
      setApprovalPolicy(chatTools?.approvalPolicy || 'readonly_auto_sensitive_confirm')
      const nextDisabledSkillIds = chatTools?.disabledSkillIds ?? []
      setDisabledSkillIds((prev) =>
        prev.length === nextDisabledSkillIds.length
        && prev.every((id, index) => id === nextDisabledSkillIds[index])
          ? prev
          : nextDisabledSkillIds,
      )
      if (!chatTools) {
        setEnabledTools([])
        setEnabledToolCount(null)
        setToolsDisabledReason('')
        setToolsRequested(false)
        setApprovalPolicy('readonly_auto_sensitive_confirm')
        return
      }
      const anyMcpEnabled = chatTools.enabled && chatTools.servers.some((server) => server.enabled)
      const anyNativeEnabled = hasEnabledNativeBuiltinTool(chatTools.nativeTools)
      const skillRuntimeEnabled = hasEnabledSkillRuntime(chatTools.nativeTools)
      const requested = anyMcpEnabled || anyNativeEnabled || skillRuntimeEnabled
      setToolsRequested(requested)
      if (!requested) {
        setEnabledTools([])
        setEnabledToolCount(null)
        setToolsDisabledReason('')
        return
      }
      const result = await api.chatMcpListTools()
      const tools = result.success ? result.tools : []
      setEnabledTools(tools)
      setEnabledToolCount(tools.length)
      setToolsDisabledReason(result.success ? '' : result.error || '工具不可用')
    } catch (err) {
      setEnabledTools([])
      setToolsRequested(false)
      setEnabledToolCount(null)
      setApprovalPolicy('readonly_auto_sensitive_confirm')
      setToolsDisabledReason(err instanceof Error ? err.message : String(err))
    }
  }, [])

  const handleApprovalPolicyChange = useCallback(async (nextApprovalPolicy: string) => {
    setApprovalPolicy(nextApprovalPolicy)
    try {
      // 读-改-写：必须现读后端最新态（refreshSettings），不能用缓存快照。否则若后端 OAuth
      // 令牌刷新（mcp/manager.rs persist_refreshed_server）已改写 servers[].auth 而缓存未失效，
      // 这次整体保存会把刷新后的 token 覆盖回旧值。
      const settings = await refreshSettings()
      await saveSettingsCached({
        ...settings,
        chatTools: {
          ...settings.chatTools,
          approvalPolicy: nextApprovalPolicy,
        },
      })
      onSettingsChange()
    } catch (err) {
      console.error('Failed to update approval policy:', err)
      void refreshToolIndicator()
    }
  }, [onSettingsChange, refreshToolIndicator])

  const handleToggleMcpServer = useCallback(async (serverId: string) => {
    try {
      // 读-改-写：现读后端最新态，避免用缓存快照 map servers[] 时把后端刚 OAuth 刷新的
      // token 覆盖回旧值（见 handleApprovalPolicyChange 注释）。
      const settings = await refreshSettings()
      const servers = (settings.chatTools?.servers ?? []).map((server) =>
        server.id === serverId ? { ...server, enabled: !server.enabled } : server,
      )
      // 乐观更新本地列表（开关即时反馈），保存后由 refreshToolIndicator 校正。
      setMcpServers(servers)
      await saveSettingsCached({
        ...settings,
        chatTools: { ...settings.chatTools, servers },
      })
      onSettingsChange()
      await refreshToolIndicator()
    } catch (err) {
      console.error('Failed to toggle MCP server:', err)
      void refreshToolIndicator()
    }
  }, [onSettingsChange, refreshToolIndicator])

  const unavailableRecommendedTools = useMemo(
    () =>
      effectiveSkillRecommendedTools.filter(
        (recommended) => !enabledTools.some((tool) => toolMatchesRecommendation(tool, recommended)),
      ),
    [effectiveSkillRecommendedTools, enabledTools],
  )

  const toolStatusHint = useMemo(() => {
    if (toolsDisabledReason && (enabledToolCount ?? 0) === 0 && (toolsRequested || effectiveSkillRecommendedTools.length > 0)) {
      if (toolsDisabledReason.includes('不支持 tools') && effectiveSkillId) {
        return toolsDisabledReason
      }
      return effectiveSkillRecommendedTools.length > 0
        ? `当前 Skill 需要工具，但${toolsDisabledReason}`
        : toolsDisabledReason
    }
    if (toolsDisabledReason && (enabledToolCount ?? 0) === 0) {
      return ''
    }
    if (unavailableRecommendedTools.length > 0) {
      return `当前 Skill 推荐的工具不可用：${unavailableRecommendedTools.slice(0, 3).join(', ')}`
    }
    return ''
  }, [effectiveSkillId, effectiveSkillRecommendedTools.length, enabledToolCount, toolsDisabledReason, toolsRequested, unavailableRecommendedTools])

  const sendDisabledReason = effectiveSkillRecommendedTools.length > 0 ? toolStatusHint : ''


  // 路由簇抽成 useChatRouting（见 hooks/useChatRouting.ts）。
  // reloadConversation 定义在下方且自身依赖 syncConversationRoute（循环依赖），
  // 故经 ref 间接调用：hook 只读 ref.current，ref 在其定义后赋值。
  const reloadConversationRef = useRef<((id: string) => void) | null>(null)
  const handleRouteResetConversation = useCallback(() => {
    currentConversationIdRef.current = null
    applyConversation(null)
    restoreStreamingPreview(null)
  }, [applyConversation, restoreStreamingPreview])
  const handleRouteLoadConversation = useCallback((conversationId: string) => {
    reloadConversationRef.current?.(conversationId)
  }, [])

  const {
    syncConversationRoute,
    syncSettingsRoute,
    syncOnboardingRoute,
    syncAssistantCenterRoute,
    syncSkillCenterRoute,
    syncPluginCenterRoute,
    syncMcpCenterRoute,
    syncKnowledgeCenterRoute,
    syncNotesRoute,
  } = useChatRouting({
    onViewChange: setChatView,
    onLoadConversation: handleRouteLoadConversation,
    onResetConversation: handleRouteResetConversation,
    currentConversationIdRef,
  })

  const handleOnboardingExit = useCallback(() => {
    setChatView('conversation')
    syncConversationRoute(null)
  }, [syncConversationRoute])

  const refreshSidebar = useCallback(() => {
    setSidebarRefreshKey((key) => key + 1)
  }, [])

  const loadDefaultModel = useCallback(async () => {
    try {
      const settings = await getSettingsCached()
      setUiLang((settings.settingsLanguage as Lang) || 'zh')
      // 以用户最后一次在顶栏选的模型为默认；provider 仍存在时才用。localStorage 为空时
      // （首次运行）回落到 onboarding 写入的 defaultModels.chat，再回落 lens / 翻译模型。
      const last = loadLastModel()
      const lastProviderOk =
        last && (settings.providers || []).some((p) => p.id === last.providerId)
      const chatDefault = settings.defaultModels?.chat
      if (last && lastProviderOk) {
        setDraftProviderId(last.providerId)
        setDraftModel(last.model)
      } else if (chatDefault?.providerId) {
        setDraftProviderId(chatDefault.providerId)
        setDraftModel(chatDefault.model || '')
      } else if (settings.lens?.providerId) {
        setDraftProviderId(settings.lens.providerId)
        setDraftModel(settings.lens.model || '')
      } else {
        setDraftProviderId(settings.translatorProviderId || '')
        setDraftModel(settings.translatorModel || '')
      }
    } catch {
      setDraftProviderId('dev-provider')
      setDraftModel('dev-model')
    }
  }, [])

  const loadSkills = useCallback(async () => {
    if (!isTauriRuntime()) {
      setSkills([])
      return
    }
    try {
      const result = await api.chatSkillsList()
      if (result.success) {
        setSkills(result.skills.map(normalizeSkill))
        if (result.error) {
          console.warn('Some chat skills could not be loaded:', result.error)
        }
      } else {
        setSkills([])
        console.error('Failed to load chat skills:', result.error)
      }
    } catch (err) {
      console.error('Failed to load chat skills:', err)
    }
  }, [])

  useEffect(() => {
    void loadDefaultModel()
    const cancelIdleLoad = scheduleIdleTask(() => {
      void loadSkills()
    })
    return cancelIdleLoad
  }, [loadDefaultModel, loadSkills])

  useEffect(() => {
    return scheduleIdleTask(() => {
      void refreshToolIndicator()
    }, 1500)
  }, [refreshToolIndicator])

  // 开窗预热：后台把所有启用的 MCP server 连接并抓一次工具清单（fire-and-forget，
  // 连接池单飞保证幂等），首轮对话的工具收集不再现场握手。
  useEffect(() => {
    void api.chatMcpWarmup()
  }, [])

  // 空闲预取各中心页 chunk，避免首次切到设置/专家/技能/插件时才触发 lazy import 而转圈；
  // 预取后切换时 Suspense 不再挂起，chat-motion-view-in 动画得以播在真实内容上（而非 spinner）。
  useEffect(() => {
    return scheduleIdleTask(() => {
      void importSettingsShell()
      void import('./AssistantCenter')
      void import('./SkillCenter')
      void import('./McpCenter')
      void import('./KnowledgeCenter')
      void import('./NotesCenter')
      void import('./PluginCenter')
      void import('./MessageList')
    }, 400)
  }, [])

  const openEmbeddedSettings = useCallback((tab: SettingsTab = 'chat') => {
    setSettingsInitialTab(tab)
    setChatView('settings')
    syncSettingsRoute()
  }, [syncSettingsRoute])

  const openAssistantCenter = useCallback(() => {
    setChatView('assistants')
    syncAssistantCenterRoute()
  }, [syncAssistantCenterRoute])

  const openSkillCenter = useCallback(() => {
    setChatView('skill')
    syncSkillCenterRoute()
  }, [syncSkillCenterRoute])

  const openPluginCenter = useCallback(() => {
    setChatView('plugins')
    syncPluginCenterRoute()
  }, [syncPluginCenterRoute])

  const openMcpCenter = useCallback(() => {
    setChatView('mcp')
    syncMcpCenterRoute()
  }, [syncMcpCenterRoute])

  const openKnowledgeCenter = useCallback(() => {
    setChatView('knowledge')
    syncKnowledgeCenterRoute()
  }, [syncKnowledgeCenterRoute])

  const openNotesCenter = useCallback(() => {
    setChatView('notes')
    syncNotesRoute()
  }, [syncNotesRoute])

  const openExtensionsItem = useCallback((item: ExtensionsNavItem) => {
    setExtensionsNavItem(item)
    if (item === 'assistants') {
      openAssistantCenter()
      return
    }
    if (item === 'skill') {
      openSkillCenter()
      return
    }
    if (item === 'mcp') {
      openMcpCenter()
      return
    }
    if (item === 'knowledge') {
      openKnowledgeCenter()
      return
    }
    if (item === 'notes') {
      openNotesCenter()
      return
    }
    if (item === 'plugins') {
      openPluginCenter()
      return
    }
  }, [openAssistantCenter, openSkillCenter, openMcpCenter, openKnowledgeCenter, openNotesCenter, openPluginCenter])

  const extensionsActive = useMemo<ExtensionsNavItem | null>(() => {
    if (chatView === 'assistants') return 'assistants'
    if (chatView === 'skill') return 'skill'
    if (chatView === 'mcp') return 'mcp'
    if (chatView === 'knowledge') return 'knowledge'
    if (chatView === 'notes') return 'notes'
    if (chatView === 'plugins') return 'plugins'
    return null
  }, [chatView])

  const [settingsExiting, setSettingsExiting] = useState(false)
  const handleSettingsClose = useCallback(() => {
    // 先播退场下滑动画，动画结束再真正切视图卸载（与 CSS 时长对齐）。
    setSettingsExiting(true)
    window.setTimeout(() => {
      setSettingsExiting(false)
      setChatView('conversation')
      syncConversationRoute(currentConversationIdRef.current)
      void loadSkills()
      void refreshToolIndicator()
      const pending = pendingAfterSettingsCloseRef.current
      pendingAfterSettingsCloseRef.current = null
      pending?.()
    }, 220)
  }, [loadSkills, refreshToolIndicator, syncConversationRoute])

  // 中心页（技能/MCP/插件/专家）没有自己的返回按钮，离开靠侧栏选会话/新建等任意路径。
  // 统一在「回到会话视图」这个转变点刷新技能列表与工具指示器，
  // 保证中心页里的启停/增删在回到聊天后立即生效（替代原各页 onClose 的刷新职责）。
  const prevChatViewRef = useRef(chatView)
  useEffect(() => {
    const prev = prevChatViewRef.current
    prevChatViewRef.current = chatView
    if (chatView !== 'conversation' || prev === chatView) return
    if (prev === 'skill' || prev === 'mcp' || prev === 'assistants' || prev === 'plugins' || prev === 'knowledge') {
      void loadSkills()
      void refreshToolIndicator()
    }
  }, [chatView, loadSkills, refreshToolIndicator])

  const runAfterLeavingSettings = useCallback((action: () => void) => {
    if (chatView !== 'settings') {
      action()
      return
    }
    if (!settingsRef.current) {
      setChatView('conversation')
      syncConversationRoute(currentConversationIdRef.current)
      action()
      return
    }
    pendingAfterSettingsCloseRef.current = action
    settingsRef.current?.requestClose()
  }, [chatView, syncConversationRoute])

  const handleSettingsChange = useCallback(() => {
    onSettingsChange()
    void loadDefaultModel()
    void loadSkills()
    void refreshToolIndicator()
    setSidebarProfileRefreshKey((key) => key + 1)
  }, [loadDefaultModel, loadSkills, onSettingsChange, refreshToolIndicator])

  const reloadConversation = useCallback(async (conversationId: string, options?: { force?: boolean }) => {
    if (isConversationInFlight(inFlightConversationsRef.current, conversationId) && !options?.force) {
      return
    }
    try {
      const conv = await chatApi.getConversation(conversationId)
      currentConversationIdRef.current = conversationId
      applyConversation(conv)
      restoreStreamingPreview(conversationId)
      setStreamCoarse({ cancelling: false })
    } catch (err) {
      console.error('Failed to reload conversation:', err)
      // B2：reload 失败（尤其"对话不存在"）——把 ghost 从乐观列表/in-flight/快照剔除并刷新侧栏。
      dropConversationLocally(conversationId)
      forgetRememberedChatRoute()
      if (currentConversationIdRef.current === conversationId || currentConversationIdRef.current === null) {
        currentConversationIdRef.current = null
        applyConversation(null)
        syncConversationRoute(null)
      }
      refreshSidebar()
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '对话加载失败，已从列表移除')
    }
  }, [applyConversation, dropConversationLocally, refreshSidebar, restoreStreamingPreview, syncConversationRoute])

  // 填充上面 useChatRouting 用来打破循环依赖的间接层。
  reloadConversationRef.current = (id: string) => { void reloadConversation(id, { force: true }) }

  const refreshContextStats = useCallback(async (conversationId?: string) => {
    const targetConversationId = conversationId ?? currentConversationIdRef.current
    if (!targetConversationId) {
      setContextState(null)
      setContextError('')
      return
    }
    setContextLoading(true)
    setContextError('')
    try {
      const result = await chatApi.getContextStats(targetConversationId)
      if (currentConversationIdRef.current === targetConversationId) {
        patchContextState(result.contextState)
      }
    } catch (err) {
      if (currentConversationIdRef.current === targetConversationId) {
        setContextError(typeof err === 'string' ? err : (err as Error).message || '上下文统计失败')
      }
    } finally {
      if (currentConversationIdRef.current === targetConversationId) {
        setContextLoading(false)
      }
    }
  }, [patchContextState])

  const handleRefreshContext = useCallback(() => {
    const conversationId = currentConversationIdRef.current
    if (conversationId) void refreshContextStats(conversationId)
  }, [refreshContextStats])

  const handleCompressContext = useCallback(async () => {
    const conversationId = currentConversationIdRef.current
    if (!conversationId || contextCompressing) return
    setContextCompressing(true)
    setContextError('')
    try {
      const result = await chatApi.compressContext(conversationId)
      if (currentConversationIdRef.current === conversationId) {
        const latestId = latestCompactionBoundaryId(result.contextState)
        if (latestId) {
          setAnimateCompactionBoundaryId(latestId)
          window.setTimeout(() => {
            setAnimateCompactionBoundaryId((current) => (current === latestId ? null : current))
          }, 1800)
        }
        patchContextState(result.contextState)
        refreshSidebar()
        await new Promise<void>((resolve) => {
          window.setTimeout(resolve, 360)
        })
      }
    } catch (err) {
      if (currentConversationIdRef.current === conversationId) {
        setContextError(typeof err === 'string' ? err : (err as Error).message || '上下文压缩失败')
      }
    } finally {
      if (currentConversationIdRef.current === conversationId) {
        setContextCompressing(false)
      }
    }
  }, [contextCompressing, patchContextState, refreshSidebar])

  const finishStreamingRun = useCallback(
    async (payload: { reason?: string; conversationId?: string }) => {
      const conversationId = payload.conversationId ?? currentConversationIdRef.current
      // 兜底：run 结束时压缩必然已终止；防御后端遗漏终止事件把"压缩中"状态卡死。
      setAgentLoopCompacting(false)
      if (payload.reason !== 'cancelled') {
        resetLocalCancellation()
      }
      if (payload.reason === 'error' && conversationId) {
        setStreamErrorForConversation(
          conversationId,
          streamErrorsRef.current[conversationId] || '回复生成失败，请稍后重试。',
        )
      }
      if (conversationId && payload.reason !== 'cancelled') {
        if (currentConversationIdRef.current === conversationId) {
          await reloadConversation(conversationId, { force: true })
        }
        refreshSidebar()
      }
      if (conversationId) {
        clearConversationLocalState(localState(), conversationId)
        syncGeneratingConversationIds()
      }
      if (conversationId && currentConversationIdRef.current === conversationId) {
        setPendingToolConfirm(null)
        setPendingSessionConsent(null)
        clearStreamingPreview()
      }
    },
    [clearStreamingPreview, localState, refreshSidebar, reloadConversation, resetLocalCancellation, setStreamErrorForConversation, syncGeneratingConversationIds],
  )

  const flushPendingStreamDone = useCallback(async (conversationId?: string): Promise<boolean> => {
    if (conversationId) {
      const pending = pendingStreamDoneRef.current[conversationId]
      delete pendingStreamDoneRef.current[conversationId]
      if (!pending) return false
      await pending()
      return true
    }
    const pendingByConversation = pendingStreamDoneRef.current
    pendingStreamDoneRef.current = {}
    let flushed = false
    for (const pending of Object.values(pendingByConversation)) {
      await pending()
      flushed = true
    }
    return flushed
  }, [])

  const finishStreamingRunWithConversation = useCallback((
    conversationId: string,
    conversation: Conversation,
  ) => {
    if (currentConversationIdRef.current === conversationId) {
      applyConversation(conversation)
      setPendingToolConfirm(null)
      setPendingSessionConsent(null)
    }
    clearConversationLocalState(localState(), conversationId)
    syncGeneratingConversationIds()
    if (currentConversationIdRef.current === conversationId) {
      clearStreamingPreview()
    }
  }, [applyConversation, clearStreamingPreview, localState, syncGeneratingConversationIds])

  useTauriEvent(api.onChatStream, (payload) => {
      if (isLocallyCancelledPayload(
        payload,
        locallyCancelledConversationIdRef.current,
        locallyCancelledRunIdRef.current,
      )) {
        return
      }
      if (!streamSnapshotsRef.current[payload.conversationId]) {
        if (!isConversationInFlight(inFlightConversationsRef.current, payload.conversationId)) {
          if (payload.done) {
            void finishStreamingRun(payload)
          }
          return
        }
      }
      // 多答组分支（任务 06-30）：该会话处于多模型并发流时，按 messageId 路由到对应列，
      // 不动会话级单流快照（单模型路径零回归）。
      if (hasActiveGroup(payload.conversationId) && payload.messageId) {
        const column = ensureGroupColumn(
          payload.conversationId,
          payload.messageId,
        )
        if (!column) return
        const segment = streamPayloadToSegment(payload)
        applyStreamDeltaToSnapshot(column, payload, segment)
        if (payload.done) {
          finalizeReasoningDurationOnDone(column)
          column.streaming = false
          // 列结束是终止帧：立即 flush（不等下一帧），让该列完成态尽快可见。
          flushGroups()
        } else {
          // 内容 delta 经 rAF 合帧（N 列高频 delta 不各自打爆 setState）。
          touchGroup()
        }
        // 组的整体「done / 持久化」交给 sendMessage 返回后的统一收尾；这里不触发 finishStreamingRun。
        return
      }
      const snapshot = ensureStreamSnapshot(payload.conversationId)
      if (payload.runId) {
        if (snapshot.runId && snapshot.runId !== payload.runId) return
        snapshot.runId = payload.runId
      }
      const segment = streamPayloadToSegment(payload)
      if (segment) {
        snapshot.segments = upsertStreamSegment(
          snapshot.segments,
          segment,
          segment.kind === 'reasoning' ? payload.reasoningDelta ?? '' : payload.delta ?? '',
        )
      }
      if (payload.reasoningDelta) {
        const now = Date.now()
        if (snapshot.reasoningStartedAt == null) {
          snapshot.reasoningStartedAt = now
        }
        if (segment?.kind === 'reasoning') {
          const segmentStartedAt = snapshot.reasoningStartedAtBySegmentId[segment.id] ?? now
          snapshot.reasoningStartedAtBySegmentId[segment.id] = segmentStartedAt
          updateReasoningSegmentDuration(snapshot, segment.id, now)
        }
        snapshot.streaming = true
        snapshot.reasoningStreaming = true
        snapshot.reasoning += payload.reasoningDelta
        snapshot.reasoningDurationMs = Math.max(
          snapshot.reasoningDurationMs ?? 0,
          now - snapshot.reasoningStartedAt,
        )
      }
      if (payload.delta) {
        if (snapshot.reasoningStreaming && snapshot.reasoningStartedAt != null) {
          snapshot.reasoningDurationMs = Math.max(
            snapshot.reasoningDurationMs ?? 0,
            Date.now() - snapshot.reasoningStartedAt,
          )
        }
        if (segment?.kind === 'text') {
          const activeReasoningSegment = findReasoningSegmentForText(snapshot.segments, segment)
          if (activeReasoningSegment) {
            updateReasoningSegmentDuration(snapshot, activeReasoningSegment.id)
          }
        }
        snapshot.streaming = true
        snapshot.reasoningStreaming = false
        snapshot.content += payload.delta
      }
      syncGeneratingConversationIds()
      showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
      if (payload.done) {
        if (snapshot.reasoningStartedAt != null && snapshot.reasoningStreaming) {
          snapshot.reasoningDurationMs = Math.max(
            snapshot.reasoningDurationMs ?? 0,
            Date.now() - snapshot.reasoningStartedAt,
          )
          const activeReasoningSegment = [...snapshot.segments]
            .reverse()
            .find((item) => item.kind === 'reasoning')
          if (activeReasoningSegment) {
            updateReasoningSegmentDuration(snapshot, activeReasoningSegment.id)
          }
        }
        // done：立即 flush 最后一帧，别让合帧吞掉收尾内容。
        showStreamSnapshotIfCurrent(payload.conversationId, snapshot, true)
        // invoke 未完成前不要 reload；延后到 flushPendingStreamDone，避免与 send 写盘竞态。
        if (isConversationInFlight(inFlightConversationsRef.current, payload.conversationId)) {
          pendingStreamDoneRef.current[payload.conversationId] = () => finishStreamingRun(payload)
          return
        }
        void finishStreamingRun(payload)
      }
  }, [ensureStreamSnapshot, finishStreamingRun, showStreamSnapshotIfCurrent, syncGeneratingConversationIds])

  useTauriEvent(api.onChatContext, (payload) => {
    const currentConversationId = currentConversationIdRef.current
    if (!currentConversationId || payload.conversationId !== currentConversationId) {
      return
    }
    // 生成过程中的活数：只有分子 + 分母，就地补进现有状态（分段/压缩计数/来源标签留给
    // 轮末的权威快照）。不能走 patchContextState —— 那条要求一份完整的上下文状态对象。
    if (payload.live) {
      setContextState((prev) => {
        const next = applyLiveContextUsage(prev, payload.live!)
        if (!next || next === prev) return prev
        setCurrentConversation((conversation) => conversation
          ? { ...conversation, context_state: next, contextState: next }
          : conversation)
        return next
      })
      return
    }
    if (!payload.contextState) return
    patchContextState(payload.contextState)
    setContextError('')
  }, [patchContextState])

  useTauriEvent(api.onChatCompaction, (payload) => {
    const currentConversationId = currentConversationIdRef.current
    if (!currentConversationId || payload.conversationId !== currentConversationId) {
      return
    }
    if (payload.phase === 'started') {
      if (payload.trigger !== 'manual') {
        setAgentLoopCompacting(true)
      }
      return
    }
    if (payload.trigger !== 'manual') {
      setAgentLoopCompacting(false)
    }
    const boundary = payload.boundary
    if (boundary?.id) {
      setAnimateCompactionBoundaryId(boundary.id)
      window.setTimeout(() => {
        setAnimateCompactionBoundaryId((current) => (current === boundary.id ? null : current))
      }, 1800)
    }
    if (boundary && payload.phase === 'completed') {
      setCurrentConversation((conversation) => {
        if (!conversation) return conversation
        const prevState = conversation.context_state ?? conversation.contextState
        const existing = prevState?.compaction_boundaries ?? prevState?.compactionBoundaries ?? []
        if (existing.some((item) => item.id === boundary.id)) return conversation
        const nextBoundaries = [...existing, boundary]
        const nextState = {
          ...(prevState ?? {}),
          compaction_boundaries: nextBoundaries,
          compactionBoundaries: nextBoundaries,
        }
        setContextState(nextState)
        return { ...conversation, context_state: nextState, contextState: nextState }
      })
    }
  }, [])

  useTauriEvent(api.onChatTodo, (payload) => {
    const currentConversationId = currentConversationIdRef.current
    if (!currentConversationId || payload.conversationId !== currentConversationId) {
      return
    }
    patchAgentTodoState(payload.todoState)
  }, [patchAgentTodoState])

  useTauriEvent(api.onChatPlan, (payload) => {
    const currentConversationId = currentConversationIdRef.current
    if (!currentConversationId || payload.conversationId !== currentConversationId) {
      return
    }
    patchAgentPlanState(payload.planState)
  }, [patchAgentPlanState])

  useTauriEvent(api.onChatHook, (payload) => {
    const currentConversationId = currentConversationIdRef.current
    if (!currentConversationId || payload.conversationId !== currentConversationId) {
      return
    }
    setHookWarning(payload)
  }, [])

  useTauriEvent(api.onChatTool, (payload) => {
      if (isLocallyCancelledPayload(
        payload,
        locallyCancelledConversationIdRef.current,
        locallyCancelledRunIdRef.current,
      )) {
        return
      }
      // 忽略 invoke 结束后的迟到 tool 事件，否则会重新 setStreaming(true) 卡死输入栏。
      if (!isConversationInFlight(inFlightConversationsRef.current, payload.conversationId)) return
      // 多答组分支：按 messageId 路由到对应列。
      if (hasActiveGroup(payload.conversationId) && payload.messageId) {
        const column = ensureGroupColumn(payload.conversationId, payload.messageId)
        if (!column) return
        const record = toolEventToRecord(payload)
        applyToolRecordToSnapshot(column, record)
        touchGroup()
        return
      }
      const snapshot = ensureStreamSnapshot(payload.conversationId)
      if (payload.runId) {
        if (snapshot.runId && snapshot.runId !== payload.runId) return
        snapshot.runId = payload.runId
      }
      const record = toolEventToRecord(payload)
      snapshot.streaming = true
      snapshot.reasoningStreaming = false
      const index = snapshot.toolCalls.findIndex((item) => item.id === record.id)
      snapshot.toolCalls = index < 0
        ? [...snapshot.toolCalls, record]
        : snapshot.toolCalls.map((item, i) => (i === index ? { ...item, ...record } : item))
      snapshot.segments = upsertToolStreamSegment(snapshot.segments, record)
      syncGeneratingConversationIds()
      showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
  }, [ensureStreamSnapshot, showStreamSnapshotIfCurrent, syncGeneratingConversationIds])

  // Live nested sub-agent progress (P3): merge onto the parent tool card's
  // structuredContent.subagentProgress, addressed by parentToolCallId.
  useTauriEvent(api.onChatSubagent, (payload) => {
      // A `chat-subagent` progress event must address an existing snapshot for
      // the parent conversation (do NOT create one — that would resurrect a
      // finalized conversation). Accept whenever the conversation is in-flight
      // or a snapshot already exists.
      const existingSnapshot = streamSnapshotsRef.current[payload.parentConversationId]
      const inFlight = isConversationInFlight(
        inFlightConversationsRef.current,
        payload.parentConversationId,
      )
      if (!inFlight && !existingSnapshot) return
      const snapshot = ensureStreamSnapshot(payload.parentConversationId)
      // Match the active run when known; only drop when both ids are set and differ.
      if (payload.parentRunId && snapshot.runId && snapshot.runId !== payload.parentRunId) return
      const index = snapshot.toolCalls.findIndex((item) => item.id === payload.parentToolCallId)
      if (index < 0) return
      const progress = {
        taskId: payload.taskId,
        name: payload.name,
        model: payload.model ?? '',
        depth: payload.depth,
        status: payload.status,
        preview: payload.preview ?? '',
        steps: payload.steps ?? [],
      }
      // Sub-agents run blocking + single-result: the parent tool card transitions
      // running→done via the `chat-tool` flow (the inline result), while these
      // `chat-subagent` events drive the live nested progress (steps/preview).
      snapshot.toolCalls = snapshot.toolCalls.map((item, i) => {
        if (i !== index) return item
        const existing =
          item.structuredContent && typeof item.structuredContent === 'object'
            ? (item.structuredContent as Record<string, unknown>)
            : {}
        const nextStructured: Record<string, unknown> = {
          ...existing,
          subagentProgress: progress,
        }
        return {
          ...item,
          structuredContent: nextStructured,
        }
      })
      showStreamSnapshotIfCurrent(payload.parentConversationId, snapshot)
  }, [ensureStreamSnapshot, showStreamSnapshotIfCurrent])

  useTauriEvent(api.onChatUserPrompt, (payload) => {
      if (isLocallyCancelledPayload(
        payload,
        locallyCancelledConversationIdRef.current,
        locallyCancelledRunIdRef.current,
      )) {
        return
      }
      if (!isConversationInFlight(inFlightConversationsRef.current, payload.conversationId)) return
      const snapshot = ensureStreamSnapshot(payload.conversationId)
      if (payload.runId) {
        if (snapshot.runId && snapshot.runId !== payload.runId) return
        snapshot.runId = payload.runId
      }
      const record = userPromptEventToRecord(payload)
      snapshot.streaming = true
      snapshot.reasoningStreaming = false
      const index = snapshot.toolCalls.findIndex((item) => item.id === record.id)
      snapshot.toolCalls = index < 0
        ? [...snapshot.toolCalls, record]
        : snapshot.toolCalls.map((item, i) => (i === index ? { ...item, ...record } : item))
      syncGeneratingConversationIds()
      showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
  }, [ensureStreamSnapshot, showStreamSnapshotIfCurrent, syncGeneratingConversationIds])

  useTauriEvent(api.onChatToolConfirm, (payload) => {
    pendingToolConfirmsRef.current[payload.conversationId] = payload
    syncGeneratingConversationIds()
    if (currentConversationIdRef.current === payload.conversationId) {
      setPendingToolConfirm(payload)
    }
  }, [syncGeneratingConversationIds])

  const resolvePendingToolConfirm = useCallback((approved: boolean, always = false) => {
    if (!pendingToolConfirm) return
    delete pendingToolConfirmsRef.current[pendingToolConfirm.conversationId]
    syncGeneratingConversationIds()
    void api.chatConfirmToolCall(pendingToolConfirm.toolCallId, approved, always)
    setPendingToolConfirm(null)
  }, [pendingToolConfirm, syncGeneratingConversationIds])

  useTauriEvent(api.onChatSessionConsent, (payload) => {
    pendingSessionConsentsRef.current[payload.conversationId] = payload
    if (currentConversationIdRef.current === payload.conversationId) {
      setPendingSessionConsent(payload)
    }
  }, [])

  const resolvePendingSessionConsent = useCallback((granted: boolean) => {
    if (!pendingSessionConsent) return
    delete pendingSessionConsentsRef.current[pendingSessionConsent.conversationId]
    void api.chatRespondSessionConsent(pendingSessionConsent.conversationId, granted)
    setPendingSessionConsent(null)
  }, [pendingSessionConsent])

  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined
    let clientPromise: Promise<typeof import('./pyodideClient')> | null = null

    const setupListener = async () => {
      unlisten = await api.onChatRunPython((payload) => {
        if (cancelled) return
        void (async () => {
          try {
            clientPromise ??= import('./pyodideClient')
            const { runPythonInSandbox } = await clientPromise
            const outcome = await runPythonInSandbox(payload.code, payload.timeoutMs, payload.files)
            await api.chatPythonComplete(
              payload.runId,
              outcome.content,
              outcome.isError,
              outcome.artifacts,
            )
          } catch (err) {
            const message = err instanceof Error
              ? err.message || err.stack || err.name
              : String(err)
            await api.chatPythonComplete(
              payload.runId,
              `Python 沙盒调用失败：${message || 'Unknown error'}。不要使用 run_command/pip 安装或修改本机 Python 环境来绕过沙盒；请直接基于已有数据回答，除非用户明确要求修改本机环境。`,
              true,
              [],
            )
          }
        })()
      })
      if (cancelled) {
        unlisten()
      }
    }

    setupListener()
    return () => {
      cancelled = true
      unlisten?.()
      void clientPromise
        ?.then(({ disposePythonSandbox }) => disposePythonSandbox())
        .catch(() => {})
    }
  }, [])

  useEffect(() => {
    currentConversationIdRef.current = currentConversation?.id ?? null
  }, [currentConversation?.id])

  useEffect(() => {
    if (!currentConversation?.id || chatView !== 'conversation') {
      setContextLoading(false)
      return
    }
    void refreshContextStats(currentConversation.id)
  }, [chatView, currentConversation?.id, activeModel, effectiveSkillId, refreshContextStats])

  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined

    api.onOpenSettings(() => {
      if (cancelled) return
      const path = hashPath()
      if (!path.startsWith('chat')) return
      openEmbeddedSettings()
    }).then((dispose) => {
      if (cancelled) {
        dispose()
      } else {
        unlisten = dispose
      }
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [openEmbeddedSettings])


  useEffect(() => {
    if (!isTauriRuntime()) return
    let cancelled = false
    void getSettingsCached().then((settings) => {
      if (cancelled) return
      if (settings.onboardingStatus === 'pending' && !isChatOnboardingRoute(hashPath())) {
        syncOnboardingRoute()
      }
    }).catch((err) => {
      console.error('Failed to check onboarding status:', err)
    })
    return () => {
      cancelled = true
    }
  }, [syncOnboardingRoute])

  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined

    api.onChatOpenConversation((payload) => {
      if (cancelled || !payload.conversationId) return
      setChatView('conversation')
      if (getRouteConversationId() === payload.conversationId) {
        // hash 不变、不会触发 hashchange，按需显式重载。
        if (payload.reload !== false) {
          void reloadConversation(payload.conversationId, { force: true })
        }
      } else {
        // hash 变化统一走 loadFromRoute 加载；这里再显式 reload 会让同一对话读两遍。
        syncConversationRoute(payload.conversationId)
      }
      refreshSidebar()
    }).then((dispose) => {
      if (cancelled) dispose()
      else unlisten = dispose
    }).catch(err => console.error(err))

    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [refreshSidebar, reloadConversation, syncConversationRoute])

  const handleSelectConversation = useCallback(async (conversationId: string) => {
    setAssistantStreamStatsByMessageId({})
    setHookWarning(null)
    try {
      const conv = await chatApi.getConversation(conversationId)
      currentConversationIdRef.current = conversationId
      applyConversation(conv)
      restoreStreamingPreview(conversationId)
      syncConversationRoute(conversationId)
      setStreamError('')
    } catch (err) {
      console.error('Failed to load conversation:', err)
      // B2：点开一个不存在/加载失败的 ghost——从乐观列表 + in-flight + 快照剔除，
      // 清空当前会话并刷新侧栏，让 ghost 自动消失而不是卡住。
      dropConversationLocally(conversationId)
      if (currentConversationIdRef.current === conversationId) {
        currentConversationIdRef.current = null
        applyConversation(null)
      }
      forgetRememberedChatRoute()
      syncConversationRoute(null)
      refreshSidebar()
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '对话加载失败，已从列表移除')
    }
  }, [applyConversation, dropConversationLocally, refreshSidebar, restoreStreamingPreview, syncConversationRoute])

  const handleNewConversation = useCallback(async () => {
    setSelectedProject(null)
    setSelectedSet(null)
    setAssistantStreamStatsByMessageId({})
    setDraftProviderId(activeProviderId)
    setDraftModel(activeModel)
    setDraftAgentRuntime(activeAgentRuntime)
    setDraftKnowledgeBaseIds([])
    setDraftForceKnowledgeSearch(false)
    currentConversationIdRef.current = null
    forgetRememberedChatRoute()
    applyConversation(null)
    restoreStreamingPreview(null)
    syncConversationRoute(null)
    setPendingUserMessage(null)
    setPendingUserMessageConversationId(null)
    setContextError('')
    setContextLoading(false)
    setContextCompressing(false)
    setStreamError('')
  }, [
    activeAgentRuntime,
    activeModel,
    activeProviderId,
    applyConversation,
    restoreStreamingPreview,
    syncConversationRoute,
  ])

  const handleClearChat = useCallback(async () => {
    const conversationId = currentConversationIdRef.current
    if (conversationId && isConversationBusy(
      conversationId,
      inFlightConversationsRef.current,
      streamSnapshotsRef.current,
    )) {
      setStreamErrorForConversation(conversationId, '请先停止当前回复，再清空对话。')
      return
    }

    if (!conversationId) {
      setAssistantStreamStatsByMessageId({})
      setPendingUserMessage(null)
      setPendingUserMessageConversationId(null)
      setStreamError('')
      return
    }

    if (!window.confirm('Clear this chat? This will delete the current conversation history.')) {
      return
    }

    try {
      await chatApi.deleteConversation(conversationId)
      if (isConversationInFlight(inFlightConversationsRef.current, conversationId)) {
        await chatApi.cancelStream(conversationId)
      }
      clearConversationLocalState(localState(), conversationId, { streamErrors: true })
      clearConversationInFlight(conversationId)
      forgetRememberedChatRoute()
      currentConversationIdRef.current = null
      setAssistantStreamStatsByMessageId({})
      setPendingUserMessage(null)
      setPendingUserMessageConversationId(null)
      setContextState(null)
      setContextError('')
      applyConversation(null)
      restoreStreamingPreview(null)
      syncConversationRoute(null)
      refreshSidebar()
      setStreamError('')
    } catch (err) {
      console.error('Failed to clear chat:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '清空对话失败')
    }
  }, [applyConversation, clearConversationInFlight, localState, refreshSidebar, restoreStreamingPreview, setStreamErrorForConversation, syncConversationRoute])

  const handleStartAssistantChat = useCallback(async (assistant: ChatAssistant) => {
    setAssistantStreamStatsByMessageId({})
    try {
      const assistantProviderId = assistant.provider_id ?? assistant.providerId ?? ''
      const assistantModel = assistant.model ?? ''
      const conv = await chatApi.createConversation(
        assistantProviderId || activeProviderId || undefined,
        assistantModel || activeModel || undefined,
        selectedProject?.name,
        selectedProject?.id ?? null,
        assistant.id,
        selectedSet?.id ?? null,
      )
      currentConversationIdRef.current = conv.id
      applyConversation(conv)
      restoreStreamingPreview(conv.id)
      syncConversationRoute(conv.id)
      refreshSidebar()
      setStreamError('')
    } catch (err) {
      console.error('Failed to start assistant conversation:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '创建助手对话失败')
    }
  }, [activeModel, activeProviderId, applyConversation, refreshSidebar, restoreStreamingPreview, selectedProject?.id, selectedProject?.name, selectedSet?.id, syncConversationRoute])

  const handleStartBuilderChat = useCallback(async () => {
    setAssistantStreamStatsByMessageId({})
    try {
      const conv = await chatApi.createBuilderConversation(
        activeProviderId || undefined,
        activeModel || undefined,
        selectedProject?.id ?? null,
      )
      currentConversationIdRef.current = conv.id
      applyConversation(conv)
      restoreStreamingPreview(conv.id)
      syncConversationRoute(conv.id)
      refreshSidebar()
      setStreamError('')
    } catch (err) {
      console.error('Failed to start builder conversation:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '创建搭建对话失败')
    }
  }, [activeModel, activeProviderId, applyConversation, refreshSidebar, restoreStreamingPreview, selectedProject?.id, syncConversationRoute])

  const handleApplyAssistant = useCallback(async (assistantId: string | null) => {
    if (!currentConversation) return
    try {
      const updated = await chatApi.updateConversation(currentConversation.id, {
        assistantId: assistantId ?? '',
      })
      applyConversation(updated)
      refreshSidebar()
      if (assistantId) void refreshContextStats(updated.id)
    } catch (err) {
      console.error('Failed to update conversation assistant:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '助手切换失败')
    }
  }, [applyConversation, currentConversation, refreshContextStats, refreshSidebar])

  // 底栏弹层选择专家：有会话则切换该会话专家，无会话则以该专家开新对话；null=清除。
  const handleSelectAssistant = useCallback(async (assistant: ChatAssistant | null) => {
    if (!assistant) {
      await handleApplyAssistant(null)
      return
    }
    if (currentConversation) await handleApplyAssistant(assistant.id)
    else await handleStartAssistantChat(assistant)
  }, [currentConversation, handleApplyAssistant, handleStartAssistantChat])

  const ensureConversationForAgentPlan = useCallback(async () => {
    if (currentConversation) return currentConversation
    let conversation = await chatApi.createConversation(
      activeProviderId || undefined,
      activeModel || undefined,
      selectedProject?.name,
      selectedProject?.id ?? null,
      undefined,
      selectedSet?.id ?? null,
    )
    if (!agentRuntimesEqual(normalizeAgentRuntime(conversation), draftAgentRuntime)) {
      conversation = await chatApi.setAgentRuntime(conversation.id, draftAgentRuntime)
    }
    currentConversationIdRef.current = conversation.id
    applyConversation(conversation)
    syncConversationRoute(conversation.id)
    refreshSidebar()
    return conversation
  }, [activeModel, activeProviderId, applyConversation, currentConversation, draftAgentRuntime, refreshSidebar, selectedProject?.id, selectedProject?.name, selectedSet?.id, syncConversationRoute])

  const handleAgentPlanModeChange = useCallback(async (mode: AgentPlanMode) => {
    try {
      const conversation = await ensureConversationForAgentPlan()
      const updated = await chatApi.setAgentPlanMode(conversation.id, mode)
      applyConversation(updated)
      void refreshContextStats(updated.id)
      refreshSidebar()
    } catch (err) {
      console.error('Failed to update agent plan mode:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || 'Plan 模式切换失败')
    }
  }, [applyConversation, ensureConversationForAgentPlan, refreshContextStats, refreshSidebar])

  const handleSelectProject = useCallback((project: ChatProject | null) => {
    setSelectedProject(project)
    setSelectedSet(null)
    setAssistantStreamStatsByMessageId({})
    setPendingUserMessage(null)
    setPendingUserMessageConversationId(null)
    currentConversationIdRef.current = null
    applyConversation(null)
    restoreStreamingPreview(null)
    syncConversationRoute(null)
    setStreamError('')
  }, [applyConversation, restoreStreamingPreview, syncConversationRoute])

  const handleSelectSet = useCallback((set: ChatSet | null) => {
    setSelectedSet(set)
    setSelectedProject(null)
    setAssistantStreamStatsByMessageId({})
    setPendingUserMessage(null)
    setPendingUserMessageConversationId(null)
    currentConversationIdRef.current = null
    applyConversation(null)
    restoreStreamingPreview(null)
    syncConversationRoute(null)
    setStreamError('')
  }, [applyConversation, restoreStreamingPreview, syncConversationRoute])

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (chatView === 'settings') return
      const mod = e.metaKey || e.ctrlKey
      if (!mod) return
      if (e.key === 'n' || e.key === 'N') {
        e.preventDefault()
        void handleNewConversation()
      }
      if (e.key === 'k' || e.key === 'K') {
        e.preventDefault()
        setSearchOpen((open) => !open)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [chatView, handleNewConversation])

  const applyAssistantStreamStats = useCallback((updatedConv: Conversation) => {
    const lastAssistant = [...updatedConv.messages]
      .reverse()
      .find((message) => message.role === 'assistant')
    if (!lastAssistant || !streamStartedAtRef.current) return

    const elapsedSec = Math.max((Date.now() - streamStartedAtRef.current) / 1000, 0.1)
    const streamedText = `${streamingContentRef.current}${streamingReasoningRef.current ? `\n${streamingReasoningRef.current}` : ''}`
    const tokenEstimate = estimateTokens(
      streamedText.trim().length > 0
        ? streamedText
        : `${lastAssistant.content}${lastAssistant.reasoning ? `\n${lastAssistant.reasoning}` : ''}`,
    )
    const stats: AssistantStreamStats = {
      messageId: lastAssistant.id,
      tokensPerSec: tokenEstimate / elapsedSec,
      reasoningDurationMs: streamSnapshotsRef.current[updatedConv.id]?.reasoningDurationMs ?? null,
      reasoningDurationMsBySegmentId: streamSnapshotsRef.current[updatedConv.id]?.reasoningDurationMsBySegmentId ?? {},
    }
    setAssistantStreamStatsByMessageId((prev) => ({
      ...prev,
      [lastAssistant.id]: stats,
    }))
  }, [])

  const handleSendMessage = useCallback(async (
    content: string,
    attachments: PendingAttachment[] = [],
    options: SendMessageOptions = {},
  ) => {
    const trimmed = content.trim()
    if (!trimmed && attachments.length === 0) return false
    if (!options.forceNewConversation && sendDisabledReason) {
      const targetId = currentConversationIdRef.current
      if (targetId) {
        setStreamErrorForConversation(targetId, sendDisabledReason)
      } else {
        setStreamError(sendDisabledReason)
      }
      return false
    }

    let conversation = options.conversationOverride
      ?? (options.forceNewConversation ? null : currentConversation)
    if (
      conversation
      && !options.conversationOverride
      && isPlainBlankConversation(conversation)
      && !conversationUsesModel(conversation, activeProviderId, activeModel)
    ) {
      conversation = null
    }
    if (!conversation) {
      try {
        conversation = await chatApi.createConversation(
          activeProviderId || undefined,
          activeModel || undefined,
          selectedProject?.name,
          selectedProject?.id ?? null,
          undefined,
          selectedSet?.id ?? null,
        )
        currentConversationIdRef.current = conversation.id
        applyConversation(conversation)
        syncConversationRoute(conversation.id)
      } catch (err) {
        console.error('Failed to create conversation before send:', err)
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '创建对话失败')
        return false
      }
    }

    // 草稿运行时只落到「尚无消息」的会话（欢迎页选好 CLI → 首次发送建会话的场景）。已有消息的
    // 会话以其自身运行时为准：draft 在切换会话/重启后可能是陈旧的（如初始 BUILTIN），无条件回写
    // 会被后端会话-CLI 绑定校验（R3 check_runtime_switch_allowed）拒绝，把正常发送直接卡死；
    // 旧行为则是静默把外部会话重置回 draft，同样错误。空会话判定与后端放行条件一致。
    if (
      (conversation.messages?.length ?? 0) === 0
      && !agentRuntimesEqual(normalizeAgentRuntime(conversation), draftAgentRuntime)
    ) {
      try {
        conversation = await chatApi.setAgentRuntime(conversation.id, draftAgentRuntime)
        applyConversation(conversation)
      } catch (err) {
        console.error('Failed to apply agent runtime before send:', err)
        setStreamError(typeof err === 'string' ? err : (err as Error).message || 'Agent 切换失败')
        return false
      }
    }

    // Apply the welcome-page knowledge-base draft to the freshly-created
    // conversation (mounting on the welcome screen had no conversation yet).
    {
      const convKb = conversation.knowledge_base_ids ?? conversation.knowledgeBaseIds ?? []
      const sameKb =
        convKb.length === draftKnowledgeBaseIds.length &&
        convKb.every((id) => draftKnowledgeBaseIds.includes(id))
      if (draftKnowledgeBaseIds.length > 0 && !sameKb) {
        try {
          conversation = await chatApi.updateConversation(conversation.id, {
            knowledgeBaseIds: draftKnowledgeBaseIds,
          })
          applyConversation(conversation)
        } catch (err) {
          console.error('Failed to apply knowledge base draft before send:', err)
        }
      }
      // 同步「强制检索」草稿到新会话。
      const convForce =
        conversation.force_knowledge_search ?? conversation.forceKnowledgeSearch ?? false
      if (draftForceKnowledgeSearch && !convForce) {
        try {
          conversation = await chatApi.updateConversation(conversation.id, {
            forceKnowledgeSearch: true,
          })
          applyConversation(conversation)
        } catch (err) {
          console.error('Failed to apply force-knowledge-search draft before send:', err)
        }
      }
    }

    // 把全局默认思考等级套到「从未显式设过等级」的会话上（新会话 / 旧的 null 会话）。
    // 只在 convLevel 为 null 时应用——绝不覆盖用户为某个会话显式选的等级。
    if (draftThinkingLevel) {
      const convLevel = conversation.thinking_level ?? conversation.thinkingLevel ?? null
      if (convLevel === null) {
        try {
          conversation = await chatApi.updateConversation(conversation.id, {
            thinkingLevel: draftThinkingLevel,
          })
          applyConversation(conversation)
        } catch (err) {
          console.error('Failed to apply thinking level draft before send:', err)
        }
      }
    }

    // 会话级三态联网搜索（任务 07-23）：把欢迎页草稿或记住的全局默认落到新会话上
    // （仅当会话尚未显式设过模式时），后端 Builtin 注入依赖会话字段而非前端展示值。
    {
      const desiredMode = draftWebSearchMode ?? loadLastWebSearchMode()
      const convMode = conversation.web_search_mode ?? conversation.webSearchMode ?? null
      if (desiredMode && convMode === null) {
        try {
          conversation = await chatApi.updateConversation(conversation.id, {
            webSearchMode: desiredMode,
          })
          applyConversation(conversation)
        } catch (err) {
          console.error('Failed to apply web search mode draft before send:', err)
        }
      }
    }

    // 多模型一问多答（任务 06-30）：把欢迎页选好的多答模型草稿落到新会话上。
    {
      const convReplyModels = conversation.reply_models ?? conversation.replyModels ?? []
      const sameReply =
        convReplyModels.length === draftReplyModels.length &&
        convReplyModels.every((ref, i) =>
          ref.provider_id === draftReplyModels[i]?.provider_id
          && ref.model === draftReplyModels[i]?.model)
      if (draftReplyModels.length > 0 && !sameReply) {
        try {
          conversation = await chatApi.updateConversation(conversation.id, {
            replyModels: draftReplyModels,
          })
          applyConversation(conversation)
        } catch (err) {
          console.error('Failed to apply reply models draft before send:', err)
        }
      }
    }

    const conversationId = conversation.id
    if (isConversationInFlight(inFlightConversationsRef.current, conversationId)) {
      setStreamErrorForConversation(conversationId, '该对话正在生成中，请稍后再试')
      return false
    }
    setOptimisticSidebarConversations((items) => [
      optimisticConversationListItem(conversation, trimmed),
      ...items.filter((item) => item.id !== conversationId),
    ])

    const pendingUserId = `pending-user-${Date.now()}`
    const optimisticUserMessage: ChatMessage = {
      id: pendingUserId,
      role: 'user',
      content: trimmed,
      attachments: attachments.map((attachment) => ({
        id: attachment.id,
        type: attachment.type,
        name: attachment.name,
        path: attachment.path,
      })),
      timestamp: Math.floor(Date.now() / 1000),
    }

    resetLocalCancellation()
    const startedAt = Date.now()
    const snapshot = ensureStreamSnapshot(conversationId)
    snapshot.streaming = true
    snapshot.content = ''
    snapshot.reasoning = ''
    snapshot.reasoningStreaming = false
    snapshot.toolCalls = []
    snapshot.segments = []
    snapshot.startedAt = startedAt
    snapshot.reasoningStartedAt = null
    snapshot.reasoningDurationMs = null
    snapshot.reasoningStartedAtBySegmentId = {}
    snapshot.reasoningDurationMsBySegmentId = {}
    snapshot.runId = null
    syncGeneratingConversationIds()

    if (currentConversationIdRef.current === conversationId) {
      // 起新一轮：内容回空闲，coarse 置 streaming。
      resetStreamStore()
      setStreamCoarse({ streaming: true })
      setStreamErrorForConversation(conversationId, '')
      // 上一轮的 Hook 失败警告不该跨轮挂着——它描述的是已经结束的那次运行。
      setHookWarning(null)
      activeRunIdRef.current = null
      streamStartedAtRef.current = startedAt
      streamingContentRef.current = ''
      streamingReasoningRef.current = ''
      setPendingUserMessage(optimisticUserMessage)
      setPendingUserMessageConversationId(conversationId)
    }

    markConversationInFlight(conversationId)
    // 多模型一问多答（任务 06-30）：reply_models ≥2 且非 plan/orchestrate 模式时，后端会 fan-out
    // 出 N 条并发流。前端据此建多答组（占位 N 列），流事件按 messageId 路由到对应列。
    // 与后端 resolve_reply_arms 的判定保持一致（≤1 个臂 = 单模型路径，零回归）。
    const replyArms = conversation.reply_models ?? conversation.replyModels ?? []
    const convPlanMode =
      conversation.agent_plan_state?.mode ?? conversation.agentPlanState?.mode ?? 'act'
    const willFanOut = replyArms.length >= 2 && convPlanMode === 'act'
    if (willFanOut) {
      const groupId = `grp-local-${Date.now()}`
      beginGroup(
        conversationId,
        groupId,
        replyArms.map((ref) => ({ providerId: ref.provider_id, model: ref.model })),
      )
      // 多答组不走单流预览：清掉刚才置的会话级 streaming 占位，避免顶部多出一条空预览气泡。
      if (currentConversationIdRef.current === conversationId) {
        resetStreamStore()
        setStreamCoarse({ streaming: true })
      }
    }
    const attachmentSkillId = options.forceNewConversation
      ? inferSingleAttachmentSkillId(attachments, enabledSkills)
      : effectiveSkillId ?? inferSingleAttachmentSkillId(attachments, enabledSkills)

    let persistedConversation: Conversation | null = null
    try {
      const updatedConv = await chatApi.sendMessage(
        conversationId,
        trimmed,
        attachments,
        attachmentSkillId,
      )
      persistedConversation = updatedConv
      if (currentConversationIdRef.current === conversationId) {
        applyAssistantStreamStats(updatedConv)
        setPendingUserMessage(null)
        setPendingUserMessageConversationId(null)
        setOptimisticSidebarConversations((items) => items.filter((item) => item.id !== conversationId))
        applyConversation(updatedConv)
        refreshSidebar()
        if (!locallyCancelledConversationIdRef.current) {
          resetLocalCancellation()
        }
      } else {
        refreshSidebar()
      }
    } catch (err) {
      console.error('Failed to send message:', err)
      // 后端生成失败时保留了用户消息并随错误带回对话——套用它，让问题留在线程里可重试，
      // 而不是连问题一起消失（旧行为）。
      const keptConversation = (err as { conversation?: Conversation })?.conversation
      if (currentConversationIdRef.current === conversationId) {
        setPendingUserMessage(null)
        setPendingUserMessageConversationId(null)
        if (keptConversation) {
          applyConversation(keptConversation)
        }
      }
      setOptimisticSidebarConversations((items) => items.filter((item) => item.id !== conversationId))
      clearStreamSnapshot(conversationId)
      if (keptConversation) refreshSidebar()
      const message = typeof err === 'string' ? err : (err as Error).message || '发送失败'
      setStreamErrorForConversation(conversationId, message)
    } finally {
      clearConversationInFlight(conversationId)
      // 多答组收尾：sendMessage 返回时所有臂已结束，持久化后的会话已 applyConversation（含 N 条
      // 带 group_id 的 assistant 消息），实时流列已可丢弃，由 MessageGroup 渲染落库后的列。
      endGroup(conversationId)
      if (persistedConversation) {
        // invoke 已返回持久化后的完整对话且上面已 applyConversation。
        // 丢弃被延后的 finishStreamingRun(它会再次全量 reloadConversation),避免每轮随历史线性变慢。
        delete pendingStreamDoneRef.current[conversationId]
        finishStreamingRunWithConversation(conversationId, persistedConversation)
      } else if (!(await flushPendingStreamDone(conversationId))) {
        clearStreamSnapshot(conversationId)
      }
    }
    return true
  }, [
    activeModel,
    activeProviderId,
    applyAssistantStreamStats,
    applyConversation,
    clearConversationInFlight,
    clearStreamSnapshot,
    currentConversation,
    draftAgentRuntime,
    draftKnowledgeBaseIds,
    draftForceKnowledgeSearch,
    draftThinkingLevel,
    draftReplyModels,
    draftWebSearchMode,
    effectiveSkillId,
    enabledSkills,
    ensureStreamSnapshot,
    finishStreamingRunWithConversation,
    flushPendingStreamDone,
    markConversationInFlight,
    refreshSidebar,
    resetLocalCancellation,
    selectedProject?.id,
    selectedProject?.name,
    selectedSet?.id,
    sendDisabledReason,
    setStreamErrorForConversation,
    syncConversationRoute,
    syncGeneratingConversationIds,
  ])

  // 用 ref 持有最新 handleSendMessage，使下方的 drainExternalSends 保持稳定身份，
  // 避免其依赖抖动导致订阅 effect 反复 cleanup/重订阅（重订阅缝隙会丢掉外部发送事件）。
  const handleSendMessageRef = useRef(handleSendMessage)
  handleSendMessageRef.current = handleSendMessage

  /** 插件页「让 AI 安装」：取规范 brief → 回聊天 → 新开对话并自动发送安装任务 */
  const handleRequestPluginAiInstall = useCallback(async (pluginId: string) => {
    const brief = await api.pluginsInstallBrief(pluginId)
    setExtensionsNavItem(null)
    setChatView('conversation')
    setAssistantStreamStatsByMessageId({})
    try {
      let conv = await chatApi.createConversation(
        activeProviderId || undefined,
        activeModel || undefined,
        selectedProject?.name,
        selectedProject?.id ?? null,
        undefined,
        selectedSet?.id ?? null,
      )
      try {
        conv = await chatApi.updateConversation(conv.id, {
          title: brief.conversationTitle,
        })
      } catch {
        // 标题失败不阻断
      }
      currentConversationIdRef.current = conv.id
      applyConversation(conv)
      restoreStreamingPreview(conv.id)
      syncConversationRoute(conv.id)
      refreshSidebar()
      const accepted = await handleSendMessageRef.current(brief.userMessage, [], {
        forceNewConversation: false,
        conversationOverride: conv,
      })
      if (!accepted) {
        throw new Error('发送安装任务失败（可能当前模型未配置或正在生成）')
      }
    } catch (err) {
      console.error('Failed to start plugin install chat:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '无法开始插件安装对话')
      throw err
    }
  }, [
    activeModel,
    activeProviderId,
    applyConversation,
    refreshSidebar,
    restoreStreamingPreview,
    selectedProject?.id,
    selectedProject?.name,
    selectedSet?.id,
    syncConversationRoute,
  ])

  // 历史预置（Lens「在 AI 客户端继续」交接）：用最新 reactive 值（provider/model/project）创建带历史的新会话。
  // 同 handleSendMessageRef 思路用 ref 持有，保持 drainExternalSends 稳定身份。
  const importExternalConversation = useCallback(async (
    messages: { role: string; content: string }[],
    attachmentPaths: string[],
  ): Promise<boolean> => {
    try {
      const conversation = await chatApi.importExternalConversation(
        messages,
        attachmentPaths,
        activeProviderId || undefined,
        activeModel || undefined,
        selectedProject?.id ?? null,
      )
      currentConversationIdRef.current = conversation.id
      applyConversation(conversation)
      syncConversationRoute(conversation.id)
      refreshSidebar()
      return true
    } catch (err) {
      console.error('Failed to import external conversation:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '导入对话失败')
      return false
    }
  }, [activeModel, activeProviderId, applyConversation, refreshSidebar, selectedProject?.id, syncConversationRoute])
  const importExternalConversationRef = useRef(importExternalConversation)
  importExternalConversationRef.current = importExternalConversation

  const { drainExternalSends, hasPendingDrainRequest } = useExternalSendQueue({
    onEnterConversationView: () => setChatView('conversation'),
    onImportConversation: (messages, attachmentPaths) =>
      importExternalConversationRef.current(messages, attachmentPaths),
    onSendMessage: (content, attachments, options) =>
      handleSendMessageRef.current(content, attachments, options),
    onError: setStreamError,
  })

  const handleExecuteAgentPlan = useCallback(async (messageId: string) => {
    const conversation = currentConversation
    if (!conversation) return
    const planMessage = conversation.messages.find((message) => message.id === messageId)
    const messagePlan = planMessage?.agent_plan ?? planMessage?.agentPlan ?? null
    const messagePlanText = messagePlan?.plan?.trim() ?? ''
    const legacyPlan = conversation.agent_plan_state ?? conversation.agentPlanState ?? null
    const legacyPlanText = legacyPlan?.plan?.trim() ?? ''
    const isLegacyPlanMessage = Boolean(
      planMessage
      && !isExecutableAgentPlanText(messagePlanText)
      && isExecutableAgentPlanText(legacyPlanText)
      && planMessage.role === 'assistant'
      && planMessage.content.trim() === legacyPlanText,
    )
    const planText = isExecutableAgentPlanText(messagePlanText)
      ? messagePlanText
      : (isLegacyPlanMessage ? legacyPlanText : '')
    if (!isExecutableAgentPlanText(planText)) return
    if (isConversationInFlight(inFlightConversationsRef.current, conversation.id)) {
      setStreamErrorForConversation(conversation.id, '该对话正在生成中，请稍后再试')
      return
    }

    try {
      const updated = await chatApi.executeAgentPlan(
        conversation.id,
        isExecutableAgentPlanText(messagePlanText) ? messageId : undefined,
      )
      applyConversation(updated)
      refreshSidebar()
      void refreshContextStats(updated.id)
      void handleSendMessage('按这条计划开始执行。', [], { conversationOverride: updated })
    } catch (err) {
      console.error('Failed to execute agent plan:', err)
      setStreamErrorForConversation(
        conversation.id,
        typeof err === 'string' ? err : (err as Error).message || '执行计划失败',
      )
    }
  }, [
    applyConversation,
    currentConversation,
    handleSendMessage,
    refreshContextStats,
    refreshSidebar,
    setStreamErrorForConversation,
  ])

  useEffect(() => {
    let cancelled = false
    const disposers: Array<() => void> = []
    const register = (p: Promise<() => void>) => {
      p.then((dispose) => {
        if (cancelled) dispose()
        else disposers.push(dispose)
      }).catch((err) => console.error(err))
    }

    // 外部发送（如 Lens 交接）的投递不依赖某个一次性事件的时序：
    // 任意可靠时机都主动从后端取走 pending（chat_take_external_sends 幂等，取空即 no-op）。
    void drainExternalSends()
    // 1) 后端就绪事件
    register(api.onChatExternalSendReady(() => {
      if (!cancelled) void drainExternalSends()
    }))
    // 2) 窗口获得焦点 —— 覆盖复用窗口被重新唤起、以及冷启动时就绪事件丢失的情况
    register(
      import('@tauri-apps/api/window')
        .then(({ getCurrentWindow }) =>
          getCurrentWindow().onFocusChanged(({ payload: focused }) => {
            if (!cancelled && focused) void drainExternalSends()
          }),
        ),
    )

    return () => {
      cancelled = true
      disposers.forEach((dispose) => dispose())
    }
  }, [drainExternalSends])

  useEffect(() => {
    if (!streamCoarse.streaming && hasPendingDrainRequest()) {
      void drainExternalSends()
    }
  }, [drainExternalSends, hasPendingDrainRequest, streamCoarse.streaming])

  const handleUpdateMessage = useCallback(
    async (messageId: string, content: string) => {
      const conv = currentConversationRef.current
      if (!conv) return
      try {
        const updated = await chatApi.updateMessage(conv.id, messageId, content)
        applyConversation(updated)
        refreshSidebar()
      } catch (err) {
        console.error('Failed to update message:', err)
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '保存失败')
      }
    },
    [applyConversation, refreshSidebar],
  )

  const handleDeleteMessage = useCallback(
    async (messageId: string) => {
      const conv = currentConversationRef.current
      if (!conv) return
      if (!window.confirm('确定删除这条消息吗？')) return
      try {
        const updated = await chatApi.deleteMessage(conv.id, messageId)
        applyConversation(updated)
        setAssistantStreamStatsByMessageId((prev) => {
          const next = { ...prev }
          delete next[messageId]
          return next
        })
        refreshSidebar()
      } catch (err) {
        console.error('Failed to delete message:', err)
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '删除失败')
      }
    },
    [applyConversation, refreshSidebar],
  )

  // 对话分支（方案 B）：在某条消息处建分支——把该消息及之前的消息复制进新对话，
  // 立即打开新对话（不自动发送）。源对话只读、不受影响。
  const handleForkMessage = useCallback(
    async (messageId: string) => {
      const conv = currentConversationRef.current
      if (!conv) return
      try {
        const forked = await chatApi.forkConversation(conv.id, messageId)
        setAssistantStreamStatsByMessageId({})
        currentConversationIdRef.current = forked.id
        applyConversation(forked)
        restoreStreamingPreview(forked.id)
        syncConversationRoute(forked.id)
        setStreamError('')
        refreshSidebar()
      } catch (err) {
        console.error('Failed to fork conversation:', err)
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '建分支失败')
      }
    },
    [applyConversation, refreshSidebar, restoreStreamingPreview, syncConversationRoute],
  )

  const handleSaveMessageToNote = useCallback(
    async (messageId: string) => {
      const conv = currentConversationRef.current
      if (!conv) return false
      const message = conv.messages.find((m) => m.id === messageId)
      if (!message) return false
      const content = message.content?.trim() || ''
      if (!content) return false

      const firstLine = content
        .split('\n')
        .map((line) => line.trim())
        .find((line) => line.length > 0)
      const title = firstLine
        ? firstLine
            .replace(/^#+\s*/, '')
            .replace(/\*\*|__|\*|_|`>/g, '')
            .slice(0, 40)
            .trim() || '对话笔记'
        : '对话笔记'
      try {
        await api.notesCreate(title, content, '', 'chat')
        setStreamError('')
        return true
      } catch (err) {
        console.error('Failed to save message to note:', err)
        setStreamError(err instanceof Error ? err.message : String(err) || '存为笔记失败')
        return false
      }
    },
    [],
  )

  // 多答组「选中条」（任务 06-30 / D5）：标记某组进下一轮历史的列。默认第一列；用户点选改。
  const handleSetGroupSelection = useCallback(
    async (groupId: string, messageId: string) => {
      const conv = currentConversationRef.current
      if (!conv) return
      try {
        const updated = await chatApi.setGroupSelection(conv.id, groupId, messageId)
        applyConversationMeta(updated)
      } catch (err) {
        console.error('Failed to set group selection:', err)
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '选中失败')
      }
    },
    [applyConversationMeta],
  )

  const handleRegenerateMessage = useCallback(
    async (messageId: string, newContent?: string) => {
      const conv = currentConversationRef.current
      if (!conv) return

      const conversationId = conv.id
      // Busy 拒绝（AC3）：入口已在 MessageList 按 streaming/frozen 收起，这里是兜底。
      // 带编辑内容时静默 return 会无声丢掉用户改的文字，必须给出提示（与 handleSend 同文案）。
      if (isConversationInFlight(inFlightConversationsRef.current, conversationId)) {
        setStreamErrorForConversation(conversationId, '该对话正在生成中，请稍后再试')
        return
      }

      const messageIndex = conv.messages.findIndex(
        (message) => message.id === messageId,
      )
      if (messageIndex < 0) return

      // 助手消息：截到它之前重生成。用户消息：保留它（编辑时先替换内容）、只丢其后内容再重试。
      const keepTarget = conv.messages[messageIndex].role === 'user'
      const cutFrom = keepTarget ? messageIndex + 1 : messageIndex
      // 空白-only 的编辑内容按「未编辑」处理（纯重生成）：绝不能把 Some("") 发给后端——
      // 乐观截断已经执行，后端再报「消息内容不能为空」会留下截断了却没重生成的线程。
      const trimmedNewContent = newContent?.trim() || undefined
      const keptMessages = conv.messages.slice(0, cutFrom)
      if (keepTarget && trimmedNewContent) {
        keptMessages[messageIndex] = {
          ...keptMessages[messageIndex],
          content: trimmedNewContent,
        }
      }
      applyConversation({
        ...conv,
        messages: keptMessages,
      })
      const removedMessageIds = new Set(
        conv.messages.slice(cutFrom).map((message) => message.id),
      )
      setAssistantStreamStatsByMessageId((prev) => Object.fromEntries(
        Object.entries(prev).filter(([id]) => !removedMessageIds.has(id)),
      ))
      resetLocalCancellation()
      const startedAt = Date.now()
      const snapshot = ensureStreamSnapshot(conversationId)
      snapshot.streaming = true
      snapshot.content = ''
      snapshot.reasoning = ''
      snapshot.reasoningStreaming = false
      snapshot.toolCalls = []
      snapshot.segments = []
      snapshot.startedAt = startedAt
      snapshot.reasoningStartedAt = null
      snapshot.reasoningDurationMs = null
      snapshot.reasoningStartedAtBySegmentId = {}
      snapshot.reasoningDurationMsBySegmentId = {}
      snapshot.runId = null
      syncGeneratingConversationIds()

      if (currentConversationIdRef.current === conversationId) {
        // 起新一轮：内容回空闲，coarse 置 streaming。
        resetStreamStore()
        setStreamCoarse({ streaming: true })
        setStreamErrorForConversation(conversationId, '')
        activeRunIdRef.current = null
        streamStartedAtRef.current = startedAt
        streamingContentRef.current = ''
        streamingReasoningRef.current = ''
      }

      markConversationInFlight(conversationId)
      let persistedConversation: Conversation | null = null
      try {
        const updated = await chatApi.regenerateMessage(conversationId, messageId, trimmedNewContent)
        persistedConversation = updated
        if (currentConversationIdRef.current === conversationId) {
          applyAssistantStreamStats(updated)
          applyConversation(updated)
          refreshSidebar()
        } else {
          refreshSidebar()
        }
      } catch (err) {
        console.error('Failed to regenerate message:', err)
        setStreamErrorForConversation(
          conversationId,
          typeof err === 'string' ? err : (err as Error).message || '重新生成失败',
        )
        clearStreamSnapshot(conversationId)
        if (currentConversationIdRef.current === conversationId) {
          void reloadConversation(conversationId)
        }
      } finally {
        clearConversationInFlight(conversationId)
        if (persistedConversation) {
          // 同 handleSend:已有持久化对话,丢弃延后的全量重拉,直接套用。
          delete pendingStreamDoneRef.current[conversationId]
          finishStreamingRunWithConversation(conversationId, persistedConversation)
        } else if (!(await flushPendingStreamDone(conversationId))) {
          clearStreamSnapshot(conversationId)
        }
      }
    },
    [applyAssistantStreamStats, applyConversation, clearConversationInFlight, clearStreamSnapshot, ensureStreamSnapshot, finishStreamingRunWithConversation, flushPendingStreamDone, markConversationInFlight, refreshSidebar, reloadConversation, resetLocalCancellation, setStreamErrorForConversation, syncGeneratingConversationIds],
  )

  const handleRuntimeChange = useCallback(async (runtime: AgentRuntimeConfig) => {
    setDraftAgentRuntime(runtime)
    if (!currentConversation) return
    try {
      const updated = await chatApi.setAgentRuntime(currentConversation.id, runtime)
      applyConversation(updated)
    } catch (err) {
      console.error('Failed to change agent runtime:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || 'Agent 切换失败')
    }
  }, [applyConversation, currentConversation])

  const handleExternalModelChange = useCallback(async (model: string, reasoning?: string | null) => {
    // Route through handleRuntimeChange so the draft updates even before a conversation exists
    // (the draft is applied when the conversation is created on first send).
    const next: AgentRuntimeConfig = {
      ...activeAgentRuntime,
      kind: 'external',
      externalModel: model,
      externalReasoning: reasoning ?? activeAgentRuntime.externalReasoning ?? null,
    }
    await handleRuntimeChange(next)
  }, [activeAgentRuntime, handleRuntimeChange])

  const handleExternalSandboxChange = useCallback(async (sandbox: string) => {
    const next: AgentRuntimeConfig = {
      ...activeAgentRuntime,
      kind: 'external',
      externalSandbox: sandbox,
    }
    await handleRuntimeChange(next)
  }, [activeAgentRuntime, handleRuntimeChange])

  const handleModelChange = useCallback(async (providerId: string, model: string) => {
    setDraftProviderId(providerId)
    setDraftModel(model)
    saveLastModel(providerId, model) // 记住为全局默认

    if (!currentConversation) return

    try {
      const updatedConv = await chatApi.updateConversation(currentConversation.id, {
        providerId,
        model,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to change model:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '模型切换失败')
    }
  }, [applyConversationMeta, currentConversation])

  const handleThinkingLevelChange = useCallback(async (level: ThinkingLevel | null) => {
    setDraftThinkingLevel(level)
    saveLastThinkingLevel(level) // 记住为全局默认，不再回落到 high
    if (!currentConversation) return
    try {
      const updatedConv = await chatApi.updateConversation(currentConversation.id, {
        thinkingLevel: level,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to change thinking level:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '思考等级切换失败')
    }
  }, [applyConversationMeta, currentConversation])

  // 会话级三态联网搜索（任务 07-23）：设置模式,持久化到会话(欢迎页先存草稿),
  // 并记住为全局默认——之后所有新会话/未显式设置的会话自动沿用(与思考等级同款)。
  const handleSetWebSearchMode = useCallback(async (mode: WebSearchMode) => {
    setDraftWebSearchMode(mode)
    saveLastWebSearchMode(mode)
    if (!currentConversation) return
    try {
      const updatedConv = await chatApi.updateConversation(currentConversation.id, {
        webSearchMode: mode,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to change web search mode:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '联网搜索模式切换失败')
    }
  }, [applyConversationMeta, currentConversation])

  // 多模型一问多答（任务 06-30 / D2）：变更多答模型集，持久化到会话（欢迎页先存草稿）。
  // 上限 4 由 UI 侧约束；这里直落 chatApi.updateConversation({ replyModels })。
  const handleChangeReplyModels = useCallback(async (models: ModelRef[]) => {
    setDraftReplyModels(models)
    if (!currentConversation) return
    try {
      const updatedConv = await chatApi.updateConversation(currentConversation.id, {
        replyModels: models,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to update reply models:', err)
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '多答模型更新失败')
    }
  }, [applyConversationMeta, currentConversation])

  const handleChangeKnowledgeBaseIds = useCallback(async (ids: string[]) => {
    // the draft is applied when the conversation is created on first send.
    setDraftKnowledgeBaseIds(ids)
    if (!currentConversation) return
    try {
      const updatedConv = await chatApi.updateConversation(currentConversation.id, {
        knowledgeBaseIds: ids,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to update knowledge bases:', err)
    }
  }, [applyConversationMeta, currentConversation])

  const handleToggleForceKnowledgeSearch = useCallback(async () => {
    const next = !(currentConversation
      ? (currentConversation.force_knowledge_search ?? currentConversation.forceKnowledgeSearch ?? false)
      : draftForceKnowledgeSearch)
    setDraftForceKnowledgeSearch(next)
    if (!currentConversation) return
    try {
      const updatedConv = await chatApi.updateConversation(currentConversation.id, {
        forceKnowledgeSearch: next,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to update force knowledge search:', err)
    }
  }, [applyConversationMeta, currentConversation, draftForceKnowledgeSearch])

  const handleCancelStream = useCallback(async () => {
    const conversationId = currentConversationIdRef.current
    if (
      !conversationId
      || getStreamCoarse().cancelling
      || !isConversationBusy(
        conversationId,
        inFlightConversationsRef.current,
        streamSnapshotsRef.current,
      )
    ) {
      return
    }

    setStreamCoarse({ cancelling: true })
    cancelCurrentRunLocally()
    try {
      await chatApi.cancelStream(conversationId)
    } catch (err) {
      console.error('Failed to cancel chat stream:', err)
      setStreamErrorForConversation(
        conversationId,
        typeof err === 'string' ? err : (err as Error).message || '停止生成失败',
      )
    } finally {
      setStreamCoarse({ cancelling: false })
    }
  }, [cancelCurrentRunLocally, setStreamErrorForConversation])

  const displayMessages = useMemo(() => {
    const stored = currentConversation?.messages ?? []
    if (!pendingUserMessage || pendingUserMessageConversationId !== currentConversation?.id) return stored
    const alreadyStored = stored.some(
      (message) =>
        message.id === pendingUserMessage.id ||
        (message.role === 'user' &&
          message.content === pendingUserMessage.content &&
          message.timestamp >= pendingUserMessage.timestamp - 2),
    )
    return alreadyStored ? stored : [...stored, pendingUserMessage]
  }, [currentConversation?.id, currentConversation?.messages, pendingUserMessage, pendingUserMessageConversationId])

  const hasMessages = displayMessages.length > 0
  const showEmptyHero = chatView === 'conversation' && !hasMessages && !streamCoarse.streaming && !streamCoarse.streamError
  const emptyHeroGreetingKey = showEmptyHero ? currentConversation?.id : null

  const emptyHeroGreeting = useMemo(
    () => ({
      key: emptyHeroGreetingKey,
      text: pickRandomChatEmptyGreeting(),
    }),
    [emptyHeroGreetingKey],
  )

  const setSidebarCollapsedPersisted = useCallback((collapsed: boolean) => {
    setSidebarCollapsed(collapsed)
    rememberChatSidebarCollapsed(collapsed)
  }, [])

  // ---------- Right Dock ----------
  const [dockOpen, setDockOpen] = useState(() => getRememberedDockOpen())
  const [dockWidth, setDockWidth] = useState(() => getRememberedDockWidth())
  const [dockTab, setDockTab] = useState<DockTab>(() => getRememberedDockTab())
  const [dockWorkdir, setDockWorkdir] = useState('')
  const [treeExpanded, setTreeExpanded] = useState<string[]>([])
  const [dockReveal, setDockReveal] = useState<DockRevealRequest>(null)

  // 工作目录跟随当前会话 / 选中项目 / agent runtime 变化，由后端 dock_resolve_cwd 解析
  // （外部 agent 与内置 runtime 的实际写入目录不同，runtime 切换必须重解析）。
  useEffect(() => {
    const conversationId = currentConversation?.id ?? null
    const projectId = selectedProject?.id ?? null
    if (!conversationId && !projectId) {
      setDockWorkdir('')
      return
    }
    let cancelled = false
    dockApi
      .resolveCwd(conversationId, projectId)
      .then((cwd) => {
        if (!cancelled) setDockWorkdir(cwd)
      })
      .catch(() => {
        if (!cancelled) setDockWorkdir('')
      })
    return () => {
      cancelled = true
    }
  }, [currentConversation?.id, selectedProject?.id, activeAgentRuntime.kind])

  // 文件树展开状态按 workdir 持久化，workdir 切换时重新载入。
  useEffect(() => {
    setTreeExpanded(dockWorkdir ? getRememberedTreeExpanded(dockWorkdir) : [])
  }, [dockWorkdir])

  const handleToggleDock = useCallback(() => {
    setDockOpen((prev) => {
      rememberDockOpen(!prev)
      return !prev
    })
  }, [])

  const handleCloseDock = useCallback(() => {
    setDockOpen(false)
    rememberDockOpen(false)
  }, [])

  // 输入栏 Git 胶囊「在 Git 面板中打开」：展开 Dock 并切到 Git 页。
  const handleOpenDockGit = useCallback(() => {
    setDockTab('git')
    rememberDockTab('git')
    setDockOpen(true)
    rememberDockOpen(true)
  }, [])

  const handleDockWidthChange = useCallback((nextWidth: number) => {
    setDockWidth(nextWidth)
    rememberDockWidth(nextWidth)
  }, [])

  const handleDockTabChange = useCallback((tab: DockTab) => {
    setDockTab(tab)
    rememberDockTab(tab)
  }, [])

  const handleTreeExpandedChange = useCallback(
    (paths: string[]) => {
      setTreeExpanded(paths)
      if (dockWorkdir) rememberTreeExpanded(dockWorkdir, paths)
    },
    [dockWorkdir],
  )

  // Git 面板「在文件树中定位」：切到文件 tab 并展开定位。
  const handleDockRevealInTree = useCallback((path: string) => {
    setDockTab('files')
    rememberDockTab('files')
    setDockOpen(true)
    rememberDockOpen(true)
    setDockReveal((prev) => ({ path, nonce: (prev?.nonce ?? 0) + 1 }))
  }, [])

  // 文件树「插入 @ 引用」：经 composerInsert 文本信道注入输入框正文。
  const handleInsertFileMention = useCallback((path: string) => {
    insertTextIntoComposer(`@${path} `)
  }, [])

  const handleCollapseSidebar = useCallback(() => {
    setSidebarCollapsedPersisted(true)
  }, [setSidebarCollapsedPersisted])

  useEffect(() => {
    if (!isTauriRuntime()) return
    let cancelled = false

    void (async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      const { LogicalSize } = await import('@tauri-apps/api/dpi')
      const min = sidebarCollapsed ? CHAT_MIN_SIZE_COLLAPSED : CHAT_MIN_SIZE_EXPANDED
      const win = getCurrentWindow()
      // 最大化/全屏时不要动 min-size 或 size：Windows 上 setMinSize 会触发重排、把最大化状态取消掉
      // （表现为切换侧边栏后窗口退出最大化）。尺寸约束对铺满屏幕的窗口也没意义，等恢复到可调窗口再应用。
      if ((await win.isMaximized()) || (await win.isFullscreen())) return
      if (cancelled) return
      await win.setMinSize(new LogicalSize(min.width, min.height))
      if (cancelled) return

      if (!sidebarCollapsed) {
        const scaleFactor = await win.scaleFactor()
        const size = await win.innerSize()
        const logical = size.toLogical(scaleFactor)
        if (logical.width < min.width) {
          const nextHeight = Math.max(logical.height, min.height)
          await win.setSize(new LogicalSize(min.width, nextHeight))
          rememberChatSize(min.width, nextHeight)
        }
      }
    })().catch((err) => {
      console.error('[Chat] Failed to update window min size:', err)
    })

    return () => {
      cancelled = true
    }
  }, [sidebarCollapsed])

  const handleSidebarSelectProject = useCallback((project: ChatProject | null) => {
    runAfterLeavingSettings(() => handleSelectProject(project))
  }, [handleSelectProject, runAfterLeavingSettings])

  const handleSidebarSelectSet = useCallback((set: ChatSet | null) => {
    runAfterLeavingSettings(() => handleSelectSet(set))
  }, [handleSelectSet, runAfterLeavingSettings])

  const handleSidebarSelectConversation = useCallback((id: string) => {
    runAfterLeavingSettings(() => void handleSelectConversation(id))
  }, [handleSelectConversation, runAfterLeavingSettings])

  const handleSidebarNewConversation = useCallback(() => {
    runAfterLeavingSettings(() => void handleNewConversation())
  }, [handleNewConversation, runAfterLeavingSettings])

  const handleSidebarConversationDeleted = useCallback(() => {
    forgetRememberedChatRoute()
    applyConversation(null)
    syncConversationRoute(null)
    refreshSidebar()
  }, [applyConversation, refreshSidebar, syncConversationRoute])

  const handleSidebarForceDropConversation = useCallback((id: string) => {
    // B3：侧栏删除时强制清掉该会话的 in-flight/快照/乐观项，
    // 使乐观合并不再保留它（删"generating"会话也能立即从侧栏消失）。
    dropConversationLocally(id)
  }, [dropConversationLocally])

  const settingsPanelActive = chatView === 'settings' && extensionsNavItem === null

  const handleSidebarOpenExtensionsItem = useCallback((item: ExtensionsNavItem) => {
    // 设置页开着时点扩展项：先走退场（含未保存改动确认），否则设置页被硬切走、动画不播。
    // 侧栏在设置页下常驻可点（见下方 collapsed 注释）后这条路径才可达。
    runAfterLeavingSettings(() => openExtensionsItem(item))
  }, [openExtensionsItem, runAfterLeavingSettings])

  const handleSidebarOpenSettings = useCallback(() => {
    const settingsPanelOpen = chatView === 'settings' && extensionsNavItem === null
    if (settingsPanelOpen) {
      if (settingsRef.current) {
        settingsRef.current.requestClose()
      } else {
        handleSettingsClose()
      }
      return
    }
    setExtensionsNavItem(null)
    openEmbeddedSettings('chat')
  }, [chatView, extensionsNavItem, handleSettingsClose, openEmbeddedSettings])

  // 侧栏账户菜单：语言切换 / 检查更新。都是全局行为，所以留在 Chat 这层，
  // 侧栏只负责触发（它拿不到 settings 也不该自己全量保存）。
  const handleSidebarSelectLang = useCallback((next: Lang) => {
    setUiLang(next)
    void (async () => {
      try {
        const settings = await getSettingsCached()
        await saveSettingsCached({ ...settings, settingsLanguage: next })
      } catch (err) {
        console.error('Failed to save UI language:', err)
      }
    })()
  }, [])

  // 检查更新：设置「关于」页已有完整流程（检查中 / 有新版 / 已最新 + 下载入口），
  // 这里只负责把用户送过去，不重造一套。
  const handleSidebarCheckUpdate = useCallback(() => {
    setExtensionsNavItem(null)
    openEmbeddedSettings('about')
  }, [openEmbeddedSettings])

  const handleSidebarSearchOpenChange = useCallback((open: boolean) => {
    if (open) {
      runAfterLeavingSettings(() => setSearchOpen(true))
      return
    }
    setSearchOpen(false)
  }, [runAfterLeavingSettings])

  // 中心页（专家/技能/MCP/插件）去掉了整行「返回聊天」顶栏后，窗口顶部不再可拖拽；
  // 且侧栏收起时页面上没有任何展开侧栏/离开中心页的入口（会被困住）。
  // 用一条浮在内容 padding 区上的细拖拽带兜底：始终可拖动窗口，
  // 侧栏收起时在带内浮出「展开侧栏 + 新建聊天」，与会话页收起态的顶栏行为一致。
  // 带高 24px（低于各中心页 pt-7/py-6 的内容起点），不遮挡任何可交互内容；
  // 收起态按钮行复用会话页收起态顶栏的同一套行高/缩进类（52px 行 + mac 交通灯缩进），
  // 保证收起/展开、中心页/会话页之间按钮位置完全不跳。
  //
  // 仅 macOS 需要：Windows / Linux 的 ChatTitlebar 是一条常驻全宽带，
  // 拖拽区与那两枚按钮本就在带里且不随侧栏收展移动，这条兜底带纯属重复。
  const centerPageTopStrip = usesNativeTitlebar ? (
    <div className="absolute inset-x-0 top-0 z-20 h-6" data-tauri-drag-region>
      {sidebarCollapsed && (
        <div
          className={`chat-titlebar-row ${chatTitlebarRowClass} ${chatTitlebarMacInsetClass} chat-titlebar-row--collapsed-mac`}
          data-tauri-drag-region
        >
          <ChatTitlebarActions
            sidebarExpanded={false}
            onToggleSidebar={() => setSidebarCollapsedPersisted(false)}
            onNewConversation={() => void handleNewConversation()}
          />
        </div>
      )}
    </div>
  ) : null

  // 中心页为上方那条收起态按钮行让出的高度（Windows / Linux 无此行，见 centerPageTopStrip）。
  const centerPagePadTop = usesNativeTitlebar && sidebarCollapsed ? 'pt-12' : ''
  // 扩展中心页共用的外壳：与会话主区同款浮起卡片（见 .chat-center-page）。
  const centerPageClass = `chat-motion-view-in chat-center-page relative flex min-h-0 min-w-0 flex-1 flex-col ${centerPagePadTop}`

  // 会话页顶栏控件。非 mac 渲染进全宽标题栏带（单行 chrome），mac 仍留在主区 52px 顶栏。
  // 抽成变量而非组件：依赖十余个 Chat 局部状态与回调，拆组件只会换来一长串 props。
  const conversationTitlebarControls = (
    <>
      <div className="flex min-w-0 items-center gap-1">
        <div className="shrink-0" data-tauri-drag-region="false">
          <RuntimePicker
            agentRuntime={activeAgentRuntime}
            onRuntimeChange={handleRuntimeChange}
            conversationId={currentConversation?.id}
            locked={
              !!currentConversation &&
              (currentConversation.messages?.length ?? 0) > 0 &&
              usesExternalRuntime
            }
          />
        </div>
        <div className="min-w-0 max-w-full shrink" data-tauri-drag-region="false">
          {usesExternalRuntime ? (
            <ExternalModelSelector
              agentRuntime={activeAgentRuntime}
              onModelChange={handleExternalModelChange}
              conversationId={currentConversation?.id}
            />
          ) : (
            <ModelSelector
              currentProviderId={activeProviderId}
              currentModel={activeModel}
              onModelChange={handleModelChange}
            />
          )}
        </div>
        {!usesExternalRuntime && (
          <div className="shrink-0 chat-thinking-pill-wrap" data-tauri-drag-region="false">
            <ThinkingLevelSelector
              currentProviderId={activeProviderId}
              currentModel={activeModel}
              value={
                currentConversation
                  ? (currentConversation.thinking_level
                      ?? currentConversation.thinkingLevel
                      ?? draftThinkingLevel)
                  : draftThinkingLevel
              }
              onChange={handleThinkingLevelChange}
            />
          </div>
        )}
        <div className="shrink-0" data-tauri-drag-region="false">
          <PermissionPicker
            agentRuntime={activeAgentRuntime}
            conversationId={currentConversation?.id}
            onSandboxChange={handleExternalSandboxChange}
            approvalPolicy={approvalPolicy}
            onApprovalPolicyChange={handleApprovalPolicyChange}
          />
        </div>
        <div className="shrink-0" data-tauri-drag-region="false">
          <BackgroundJobsIndicator />
        </div>
      </div>
      <div className="min-w-5 flex-1" data-tauri-drag-region />
      <div className="flex min-w-0 shrink items-center justify-end gap-1">
        <div className="shrink-0" data-tauri-drag-region="false">
          <IconButton
            label={i18n[uiLang].dockToggle}
            size="sm"
            variant="ghost"
            className={dockOpen ? 'bg-black/5 text-neutral-800 dark:bg-white/10 dark:text-neutral-100' : ''}
            onClick={handleToggleDock}
          >
            <PanelRight size={15} />
          </IconButton>
        </div>
        <AgentTodoIndicator todoState={currentConversation?.agent_todo_state ?? currentConversation?.agentTodoState ?? null} />
      </div>
    </>
  )

  return (
    <div
      className={`chat-window-shell${usesNativeTitlebar ? ' chat-window-shell--native-titlebar' : ''}`}
    >
      {!usesNativeTitlebar && (
        <ChatTitlebar
          sidebarExpanded={!sidebarCollapsed}
          /* 与下方 <Sidebar collapsed> 取反同源：设置页里侧栏也是收起的，
             只看 sidebarCollapsed 会在设置页多留 240px 空档。 */
          sidebarVisible={!(sidebarCollapsed || settingsPanelActive)}
          settingsMode={settingsPanelActive}
          onToggleSidebar={() => {
            if (sidebarCollapsed) setSidebarCollapsedPersisted(false)
            else handleCollapseSidebar()
          }}
          onNewConversation={() => {
            runAfterLeavingSettings(() => void handleNewConversation())
          }}
        >
          {chatView === 'conversation' ? conversationTitlebarControls : null}
        </ChatTitlebar>
      )}
      <div className="flex min-h-0 w-full flex-1">
        {chatView !== 'onboarding' ? (
        /* 设置页自带 200px 导航栏，聊天侧栏此时借用已有的折叠过渡整体滑出（不再直接卸载，
           否则左列会先空一帧、且关闭时侧栏是瞬间 pop 回来的）。退场期保持折叠，
           等视图真正切回会话后再滑入，与会话页入场同时发生 —— 否则侧栏会在设置页
           淡出的同时把它挤窄。 */
        <Sidebar
          lang={uiLang}
          currentConversationId={currentConversation?.id}
          generatingConversationIds={generatingConversationIds}
          optimisticConversations={optimisticSidebarConversations}
          selectedProject={selectedProject}
          onSelectProject={handleSidebarSelectProject}
          selectedSet={selectedSet}
          onSelectSet={handleSidebarSelectSet}
          onSelectConversation={handleSidebarSelectConversation}
          onNewConversation={handleSidebarNewConversation}
          onConversationDeleted={handleSidebarConversationDeleted}
          onForceDropConversation={handleSidebarForceDropConversation}
          onOpenExtensionsItem={handleSidebarOpenExtensionsItem}
          onOpenSettings={handleSidebarOpenSettings}
          onSelectLang={handleSidebarSelectLang}
          onCheckUpdate={handleSidebarCheckUpdate}
          settingsActive={settingsPanelActive}
          extensionsActive={extensionsActive}
          collapsed={sidebarCollapsed || settingsPanelActive}
          onToggleCollapsed={handleCollapseSidebar}
          refreshKey={sidebarRefreshKey}
          profileRefreshKey={sidebarProfileRefreshKey}
          searchOpen={searchOpen}
          onSearchOpenChange={handleSidebarSearchOpenChange}
        />
        ) : null}

        {chatView === 'onboarding' ? (
          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            <OnboardingShell
              onComplete={handleOnboardingExit}
              onSkip={handleOnboardingExit}
              onSettingsChange={onSettingsChange}
            />
          </div>
        ) : chatView === 'settings' ? (
          /* Suspense 必须包在动画容器外面：放在里面时 fallback=null 会让容器先带着空白挂载、
             动画播在空白上，lazy 内容晚几帧才 pop 进来（进场看起来"没有动画"）。
             包在外面则容器与内容一起挂载，动画全程播在真实内容上。 */
          <Suspense fallback={null}>
            <SettingsEnterPane
              key="settings"
              exiting={settingsExiting}
              className={`flex min-h-0 min-w-0 flex-1 flex-col${
                !usesNativeTitlebar && settingsPanelActive ? ' settings-embedded-under-strip' : ''
              }`}
            >
              <SettingsShell
                ref={settingsRef}
                variant="embedded"
                initialTab={settingsInitialTab}
                reserveTrafficLightSpace={(sidebarCollapsed || extensionsNavItem === null) && usesNativeTitlebar}
                onClose={handleSettingsClose}
                onSettingsChange={handleSettingsChange}
                onReady={emitContentReady}
              />
            </SettingsEnterPane>
          </Suspense>
        ) : chatView === 'assistants' ? (
          <div key="assistants" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <AssistantCenter
                skills={enabledSkills}
                currentAssistantId={currentAssistantId}
                onStartAssistantChat={(assistant) => void handleStartAssistantChat(assistant)}
                onStartBuilder={() => void handleStartBuilderChat()}
                onApplyAssistant={currentConversation ? (assistantId) => void handleApplyAssistant(assistantId) : undefined}
              />
            </Suspense>
          </div>
        ) : chatView === 'skill' ? (
          <div key="skill" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <SkillCenter onSkillsChanged={() => void loadSkills()} />
            </Suspense>
          </div>
        ) : chatView === 'mcp' ? (
          <div key="mcp" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <McpCenter />
            </Suspense>
          </div>
        ) : chatView === 'knowledge' ? (
          <div key="knowledge" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <KnowledgeCenter />
            </Suspense>
          </div>
        ) : chatView === 'notes' ? (
          <div key="notes" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <NotesCenter />
            </Suspense>
          </div>
        ) : chatView === 'plugins' ? (
          <div key="plugins" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <PluginCenter onRequestAiInstall={handleRequestPluginAiInstall} />
            </Suspense>
          </div>
        ) : (
          <div className="chat-motion-pane-in chat-main-pane relative flex min-w-0 flex-1 flex-col">
            {/* 图片查看器为浮层(见下方 overlay),不替换主面板 —— 否则会卸载 InputBar,
                丢掉待发送附件 / 草稿。here 起正常内容,始终挂载。 */}
            <>
                {/* 非 mac 的顶栏控件已并进全宽标题栏带（见上方 ChatTitlebar），此行只 mac 渲染。 */}
                {usesNativeTitlebar && (
                <header
              className={`chat-titlebar-row ${chatTitlebarRowClass} min-w-0 gap-2 ${
                sidebarCollapsed
                  ? `${chatTitlebarMacInsetClass} chat-titlebar-row--collapsed-mac pr-3`
                  : 'px-6'
              }`}
              data-tauri-drag-region
            >
              {/* mac 收起态把这两枚按钮借到主区顶栏、给交通灯让位。 */}
              {sidebarCollapsed && (
                <ChatTitlebarActions
                  sidebarExpanded={false}
                  onToggleSidebar={() => setSidebarCollapsedPersisted(false)}
                  onNewConversation={() => {
                    runAfterLeavingSettings(() => void handleNewConversation())
                  }}
                />
              )}
              {conversationTitlebarControls}
                </header>
                )}

                <div className="flex min-h-0 flex-1 flex-col">
                  {showEmptyHero ? (
                    <div className="chat-empty-hero flex flex-1 flex-col items-center justify-center px-6 pb-16">
                  <ChatDotGridBackground />
                  <div className="chat-empty-hero-stack relative z-10 w-full max-w-4xl space-y-8">
                    <h2
                      className="chat-motion-fade-up chat-empty-hero-title text-center text-[1.75rem] font-medium leading-snug tracking-[-0.02em] text-neutral-900 dark:text-neutral-50 sm:text-[2rem]"
                      aria-label={
                        currentAssistantSnapshot
                          ? currentAssistantSnapshot.name
                          : selectedProject
                            ? `Start in “${selectedProject.name}”`
                            : emptyHeroGreeting.text
                      }
                    >
                      {currentAssistantSnapshot ? (
                        currentAssistantSnapshot.name
                      ) : selectedProject ? (
                        `Start in “${selectedProject.name}”`
                      ) : (
                        <TypewriterText
                          key={emptyHeroGreeting.key}
                          text={emptyHeroGreeting.text}
                          active={showEmptyHero}
                        />
                      )}
                    </h2>
                    <div
                      className="chat-motion-fade-up"
                      style={{ ['--chat-motion-delay' as string]: '120ms' }}
                    >
                    <InputBar
                      layout="inline"
                      onSend={(content, attachments) => void handleSendMessage(content, attachments)}
                      disabled={isCurrentConversationBusy()}
                      onCancel={() => void handleCancelStream()}
                      cancelVisible={streamCoarse.streaming}
                      cancelling={streamCoarse.cancelling}
                      onOpenSettings={() => openEmbeddedSettings('chat')}
                      onOpenTools={() => openSkillCenter()}
                      onNewChat={() => void handleNewConversation()}
                      onCompactContext={() => void handleCompressContext()}
                      onClearChat={() => void handleClearChat()}
                      enabledTools={enabledTools}
                      toolsDisabledReason={toolsDisabledReason}
                      toolStatusHint={toolStatusHint}
                      sendDisabledReason={sendDisabledReason}
                      agentPlanState={currentConversation?.agent_plan_state ?? currentConversation?.agentPlanState ?? null}
                      onAgentPlanModeChange={handleAgentPlanModeChange}
                      enabledSkills={slashSkills}
                      onOpenSkillSettings={openSkillCenter}
                      selectedProject={selectedProject}
                      conversationProject={conversationProject}
                      onSelectProject={handleSidebarSelectProject}
                      showProjectEntry
                      currentAssistant={currentAssistantSnapshot ? { id: currentAssistantSnapshot.id, name: currentAssistantSnapshot.name } : null}
                      onOpenAssistantCenter={openAssistantCenter}
                      onSelectAssistant={handleSelectAssistant}
                      autoFocus
                      usesExternalRuntime={usesExternalRuntime}
                      externalAgentName={activeAgentRuntime.externalAgentId ?? null}
                      conversationId={currentConversation?.id ?? null}
                      knowledgeBaseIds={currentConversation ? (currentConversation.knowledge_base_ids ?? currentConversation.knowledgeBaseIds ?? []) : draftKnowledgeBaseIds}
                      onChangeKnowledgeBaseIds={handleChangeKnowledgeBaseIds}
                      forceKnowledgeSearch={currentConversation ? (currentConversation.force_knowledge_search ?? currentConversation.forceKnowledgeSearch ?? false) : draftForceKnowledgeSearch}
                      onToggleForceKnowledgeSearch={handleToggleForceKnowledgeSearch}
                      mcpServers={mcpServers}
                      onToggleMcpServer={handleToggleMcpServer}
                      webSearchMode={activeWebSearchMode}
                      onSetWebSearchMode={handleSetWebSearchMode}
                      builtinWebSearchSupported={activeBuiltinWebSearchSupported}
                      replyModels={activeReplyModels}
                      onChangeReplyModels={handleChangeReplyModels}
                      contextSlot={
                        <ContextIndicator
                          contextState={contextState}
                          messageCount={displayMessages.length}
                          loading={contextLoading}
                          compressing={contextCompressing}
                          error={contextError}
                          usesExternalRuntime={usesExternalRuntime}
                          onRefresh={handleRefreshContext}
                          onCompress={() => void handleCompressContext()}
                          lang={uiLang}
                        />
                      }
                      gitWorkdir={dockWorkdir || null}
                      gitLang={uiLang}
                      onOpenGitPanel={handleOpenDockGit}
                    />
                    </div>
                  </div>
                </div>
                  ) : (
                    <>
                  {hookWarning && hookWarning.conversationId === currentConversation?.id && (
                    <div className="flex items-start gap-2 px-4 pt-2">
                      <div className="flex min-w-0 flex-1 items-start gap-2 rounded-md bg-amber-50 px-2.5 py-1.5 text-[11px] leading-4 text-amber-700 dark:bg-amber-400/10 dark:text-amber-200">
                        <span className="min-w-0 flex-1">
                          {i18n[uiLang].chatHookFailed
                            .replace('{name}', hookWarning.hookName || hookWarning.event)
                            .replace('{event}', hookWarning.event)}
                          {` — ${hookWarning.message}`}
                        </span>
                        <IconButton
                          size="xs"
                          variant="ghost"
                          label={i18n[uiLang].chatHookDismiss}
                          onClick={() => setHookWarning(null)}
                        >
                          <X size={12} />
                        </IconButton>
                      </div>
                    </div>
                  )}
                  {(() => {
                    const origin = currentConversation?.forked_from ?? currentConversation?.forkedFrom
                    if (!origin) return null
                    const sourceId = origin.conversation_id ?? origin.conversationId
                    if (!sourceId) return null
                    return (
                      <div className="flex justify-center px-4 pt-2">
                        <button
                          type="button"
                          onClick={() => void handleSelectConversation(sourceId)}
                          className="inline-flex max-w-full items-center gap-1 rounded-full bg-neutral-100 px-2.5 py-1 text-[11px] text-neutral-500 transition-colors hover:bg-neutral-200 hover:text-neutral-700 dark:bg-neutral-800 dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-neutral-200"
                          title={`分叉自「${origin.title}」，点击回到源对话`}
                        >
                          <GitBranch size={12} strokeWidth={2} className="shrink-0" />
                          <span className="truncate">分叉自 {origin.title}</span>
                        </button>
                      </div>
                    )
                  })()}
                  <Suspense fallback={<MessageListLoading />}>
                    <MessageList
                      key={currentConversation?.id ?? 'empty'}
                      conversationId={currentConversation?.id}
                      messages={displayMessages}
                      agentPlanState={currentConversation?.agent_plan_state ?? currentConversation?.agentPlanState ?? null}
                      assistantStreamStatsByMessageId={assistantStreamStatsByMessageId}
                      onUpdateMessage={handleUpdateMessage}
                      onRegenerateMessage={handleRegenerateMessage}
                      onForkMessage={handleForkMessage}
                      onDeleteMessage={handleDeleteMessage}
                      onSaveMessageToNote={handleSaveMessageToNote}
                      onRetryLastUser={handleRegenerateMessage}
                      onExecuteAgentPlan={handleExecuteAgentPlan}
                      groupSelections={currentConversation?.group_selections ?? currentConversation?.groupSelections ?? {}}
                      onSetGroupSelection={handleSetGroupSelection}
                      contextState={contextState}
                      compactionInProgress={contextCompressing || agentLoopCompacting}
                      animateCompactionBoundaryId={animateCompactionBoundaryId}
                      lang={uiLang}
                    />
                  </Suspense>
                  {/* ponytail: 只挂在有消息的分支。审批必然发生在一次 run 里，此时至少已有一条用户消息，空态 hero 不可能出卡。 */}
                  {(pendingToolConfirm || pendingSessionConsent) && (
                    <div className="shrink-0 px-6">
                      <div className="mx-auto w-full max-w-4xl">
                        {pendingToolConfirm && (
                          <ApprovalCard
                            title={toolApprovalTitle(pendingToolConfirm)}
                            subtitle={`${pendingToolConfirm.source}${pendingToolConfirm.serverId ? ` · ${pendingToolConfirm.serverId}` : ''}`}
                            detail={pendingToolConfirm.argumentsPreview}
                            actions={[
                              { label: '拒绝', onSelect: () => resolvePendingToolConfirm(false) },
                              { label: '总是允许', onSelect: () => resolvePendingToolConfirm(true, true) },
                              { label: '允许一次', primary: true, hint: 'Ctrl+↵', onSelect: () => resolvePendingToolConfirm(true) },
                            ]}
                          />
                        )}
                        {pendingSessionConsent && (
                          <ApprovalCard
                            title="允许本次会话使用文件和命令工具？"
                            subtitle="授权后，本会话内 Kivio 可读写、删除磁盘上的任意文件并执行任意终端命令（包括项目目录之外）。仅本次会话有效，重启后需重新授权。"
                            actions={[
                              { label: '拒绝', onSelect: () => resolvePendingSessionConsent(false) },
                              { label: '允许本次会话', primary: true, hint: 'Ctrl+↵', onSelect: () => resolvePendingSessionConsent(true) },
                            ]}
                          />
                        )}
                      </div>
                    </div>
                  )}
                  <InputBar
                    onSend={(content, attachments) => void handleSendMessage(content, attachments)}
                    disabled={isCurrentConversationBusy()}
                    onCancel={() => void handleCancelStream()}
                    cancelVisible={streamCoarse.streaming}
                    cancelling={streamCoarse.cancelling}
                    onOpenSettings={() => openEmbeddedSettings('chat')}
                    onOpenTools={() => openSkillCenter()}
                    onNewChat={() => void handleNewConversation()}
                    onCompactContext={() => void handleCompressContext()}
                    onClearChat={() => void handleClearChat()}
                    enabledTools={enabledTools}
                    toolsDisabledReason={toolsDisabledReason}
                    toolStatusHint={toolStatusHint}
                    sendDisabledReason={sendDisabledReason}
                    agentPlanState={currentConversation?.agent_plan_state ?? currentConversation?.agentPlanState ?? null}
                    onAgentPlanModeChange={handleAgentPlanModeChange}
                    enabledSkills={slashSkills}
                    onOpenSkillSettings={openSkillCenter}
                    selectedProject={selectedProject}
                    conversationProject={conversationProject}
                    onSelectProject={handleSidebarSelectProject}
                    showProjectEntry
                    currentAssistant={currentAssistantSnapshot ? { id: currentAssistantSnapshot.id, name: currentAssistantSnapshot.name } : null}
                    onOpenAssistantCenter={openAssistantCenter}
                    onSelectAssistant={handleSelectAssistant}
                    autoFocus
                    usesExternalRuntime={usesExternalRuntime}
                    externalAgentName={activeAgentRuntime.externalAgentId ?? null}
                    conversationId={currentConversation?.id ?? null}
                    knowledgeBaseIds={currentConversation ? (currentConversation.knowledge_base_ids ?? currentConversation.knowledgeBaseIds ?? []) : draftKnowledgeBaseIds}
                    onChangeKnowledgeBaseIds={handleChangeKnowledgeBaseIds}
                    forceKnowledgeSearch={currentConversation ? (currentConversation.force_knowledge_search ?? currentConversation.forceKnowledgeSearch ?? false) : draftForceKnowledgeSearch}
                    onToggleForceKnowledgeSearch={handleToggleForceKnowledgeSearch}
                    mcpServers={mcpServers}
                    onToggleMcpServer={handleToggleMcpServer}
                    webSearchMode={activeWebSearchMode}
                    onSetWebSearchMode={handleSetWebSearchMode}
                    builtinWebSearchSupported={activeBuiltinWebSearchSupported}
                    replyModels={activeReplyModels}
                    onChangeReplyModels={handleChangeReplyModels}
                    contextSlot={
                      <ContextIndicator
                        contextState={contextState}
                        messageCount={displayMessages.length}
                        loading={contextLoading}
                        compressing={contextCompressing}
                        error={contextError}
                        usesExternalRuntime={usesExternalRuntime}
                        onRefresh={handleRefreshContext}
                        onCompress={() => void handleCompressContext()}
                        lang={uiLang}
                      />
                    }
                    gitWorkdir={dockWorkdir || null}
                    gitLang={uiLang}
                    onOpenGitPanel={handleOpenDockGit}
                  />
                    </>
                  )}
                </div>
              </>
            {imageViewerItem && (
              <div className="absolute inset-0 z-40 flex flex-col">
                <ChatImageViewer
                  item={imageViewerItem}
                  onClose={() => setImageViewerItem(null)}
                />
              </div>
            )}
          </div>
        )}
        {chatView === 'conversation' && (
          <RightDock
            open={dockOpen}
            width={dockWidth}
            activeTab={dockTab}
            workdir={dockWorkdir}
            lang={uiLang}
            treeExpanded={treeExpanded}
            revealRequest={dockReveal}
            onToggleTab={handleDockTabChange}
            onWidthChange={handleDockWidthChange}
            onClose={handleCloseDock}
            onTreeExpandedChange={handleTreeExpandedChange}
            onInsertMention={handleInsertFileMention}
            onRevealInTree={handleDockRevealInTree}
          />
        )}
      </div>
    </div>
  )
}
