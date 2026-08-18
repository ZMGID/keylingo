import { lazy, memo, Profiler, startTransition, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ProfilerOnRenderCallback, type ReactNode, type Ref } from 'react'
import { PanelRight } from 'lucide-react'
import { type ConversationSelectionScope, type ExtensionsNavItem } from './Sidebar'
import { ChatSidebarPane } from './ChatSidebarPane'
import { useChatRouting } from './hooks/useChatRouting'
import { useExternalSendQueue } from './hooks/useExternalSendQueue'
import { useMessageQueue } from './hooks/useMessageQueue'
import type { QueuedMessage } from './hooks/useMessageQueue'
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
  isChatSessionCenterPath,
  isChatSettingsPath,
  isChatSkillCenterPath,
  setHash,
} from './chatRoutes'
import { ApprovalCard } from './ApprovalCard'
import { AskUserBlock } from './AskUserBlock'
import { ChatTitlebar } from './ChatTitlebar'
import { ChatTitlebarActions } from './ChatTitlebarActions'
import {
  beginConversationTransition,
  cancelConversationTransition,
  completeConversationTransition,
  getConversationTransitionSnapshot,
  invalidateConversationTransition,
  isCurrentConversationTransition,
} from './conversationTransitionStore'
import type { ConversationLoadHint } from './conversationTransitionStore'
import type { AssistantStreamStats, MessageListProps } from './MessageList'
import type { InputBarProps } from './InputBar'
import { SessionUsageStrip } from './SessionUsageStrip'
import { ModelSelector } from './ModelSelector'
import { ThinkingLevelSelector } from './ThinkingLevelSelector'
import { ExternalModelSelector, RuntimePicker } from './RuntimePicker'
import { PermissionPicker } from './PermissionPicker'
import { deriveDshPresetModes, derivePermissionModes, useDetectedExternalAgents } from './permissionModes'
import { BackgroundJobsIndicator } from './BackgroundJobsIndicator'
import { ContextIndicator } from './ContextIndicator'
import { isExecutableAgentPlanText } from './agentPlan'
import {
  agentRuntimesEqual,
  BUILTIN_AGENT_RUNTIME,
  chatApi,
  normalizeAgentRuntime,
  type AgentRuntimeConfig,
} from './api'
import { loadLastAgentRuntime, saveLastAgentRuntime } from './lastAgentRuntime'
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
  ConversationSearchHit,
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
  type ChatSubagentPayload,
} from '../api/tauri'
import { getSettingsCached, refreshSettings, saveSettingsCached } from '../api/settingsCache'
import { OnboardingShell } from '../onboarding/OnboardingShell'
import type { SettingsShellHandle, SettingsTab } from '../settings/SettingsShell'
import { i18n, LangContext, type Lang } from '../settings/i18n'
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
import { RightDock, type DockPreviewRequest, type DockRevealRequest, type DockTab } from './dock/RightDock'
import { dockApi } from './dock/api'
import { insertTextIntoComposer } from './composerInsert'
import { onDockDiffPreviewRequest, onDockMarkdownPreviewRequest, onDockPreviewRequest, requestDockMarkdownPreview } from './dock/dockPreview'
import { IconButton } from '../components/Button'
import { normalizeToolCallStatus } from './toolStatus'
import { pickRandomChatEmptyGreeting, isTauriRuntime } from './utils'
import { hasEnabledNativeBuiltinTool, hasEnabledSkillRuntime } from '../utils/chatTools'
import { onChatImageViewerOpen, type ChatImageViewerItem } from './imageViewer'
import {
  collectGeneratingConversationIds,
  createEmptyStreamSnapshot,
  isConversationBusy,
  isConversationInFlight,
  mergeToolRecord,
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
  getActiveGroup,
  hasActiveGroup,
  resetGroups,
  restoreGroupArm,
  touchGroup,
} from './groupStreamingStore'
import { compareTimelineSegments, isExternalSubagentToolCall, isUserFollowUpToolCall, isUserSteerToolCall, segmentStepNumber, segmentToolCallId } from './segments'
import { latestCompactionBoundaryId, mergeCompactionContextState } from './compactionBoundary'
import { applyLiveContextUsage } from './contextPanel'
import { measureChatSurface, onChatPerfProfiler, useChatPerfLongTaskProbe, useChatPerfRenderProbe } from './chatPerformanceProbe'
import { ChatRouteKeepAlive } from './ChatRouteKeepAlive'
import { ChatConversationPane } from './ChatConversationPane'

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

const SessionCenter = lazy(() => import('./SessionCenter').then((module) => ({
  default: module.SessionCenter,
})))

const NotesCenter = lazy(() => import('./NotesCenter').then((module) => ({
  default: module.NotesCenter,
})))

type ChatView = 'conversation' | 'settings' | 'assistants' | 'skill' | 'mcp' | 'knowledge' | 'notes' | 'sessions' | 'onboarding'

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

/** 设置区的独立渲染边界。侧栏折叠、聊天流式状态变化不应重新执行设置页大树。 */
const ChatSettingsPane = memo(function ChatSettingsPane({
  settingsRef,
  exiting,
  className,
  initialTab,
  reserveTrafficLightSpace,
  onClose,
  onSettingsChange,
  onReady,
  onRequestPluginAiInstall,
  onRender,
}: {
  settingsRef: Ref<SettingsShellHandle>
  exiting: boolean
  className: string
  initialTab: SettingsTab
  reserveTrafficLightSpace: boolean
  onClose: () => void
  onSettingsChange: () => void
  onReady: () => void
  onRequestPluginAiInstall: (pluginId: string) => void | Promise<void>
  onRender: ProfilerOnRenderCallback
}) {
  return (
    <Suspense fallback={null}>
      <SettingsEnterPane
        key="settings"
        exiting={exiting}
        className={className}
      >
        <Profiler id="SettingsShell" onRender={onRender}>
          <SettingsShell
            ref={settingsRef}
            variant="embedded"
            initialTab={initialTab}
            reserveTrafficLightSpace={reserveTrafficLightSpace}
            onClose={onClose}
            onSettingsChange={onSettingsChange}
            onReady={onReady}
            onRequestPluginAiInstall={onRequestPluginAiInstall}
          />
        </Profiler>
      </SettingsEnterPane>
    </Suspense>
  )
})

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
  const name = (payload.name || '').toLowerCase()
  // claude 的计划批准：批的是卡片上那份计划，不是「一个叫 ExitPlanMode 的工具」。
  if (name === 'exitplanmode') return '批准这份计划，开始执行？'
  // claude 自己要求进入计划档：先探索、出方案，这一轮不动代码。
  if (name === 'enterplanmode') return '让 claude 先出方案，暂不改动代码？'
  const spec = TOOL_APPROVAL_VERBS[name]
  const target = payload.target?.trim()
  if (!spec || !target) return `允许调用工具 ${payload.name}？`
  const shown = spec.path ? target.split(/[\\/]/).filter(Boolean).pop() || target : target
  return `允许${spec.verb} ${shown}？`
}

/** 计划批准卡不给「总是允许」：那等于以后每份计划都自动批、还自动替你选落在哪一档，
 *  而用户当时想说的只是「这一份可以」。Claude Code 的计划提示也只有三选一、没有「别再问」。
 *  `EnterPlanMode` 相反 —— 它走的是普通工具提示那套，Claude Code 在那里是给「别再问」的。 */
function isPlanApproval(payload: ChatToolConfirmPayload): boolean {
  return (payload.name || '').toLowerCase() === 'exitplanmode'
}

/** claude 主动要求进入计划档。批准就够（CLI 自己切档），但会话配置要跟着写成 `plan`，
 *  否则底栏胶囊还显示「完全」而 claude 已经进只读了。 */
function isEnterPlanApproval(payload: ChatToolConfirmPayload): boolean {
  return (payload.name || '').toLowerCase() === 'enterplanmode'
}

/** 计划批准的三个选项，对齐 Claude Code 自己的那三条
 *  （Yes and bypass permissions / Yes manually approve edits / Tell Claude what to change）。
 *  `mode` 是批准后要把 CLI 切到的权限档位，同时会写回会话配置，让底栏胶囊跟着变 ——
 *  否则批准之后界面还显示「计划 (只读)」，而 claude 已经在改文件了。
 *
 *  末位是主按钮（Ctrl+↵）：批完计划最常见的意图就是「别再拦我了，去做」，
 *  Claude Code 自己的默认选中项也是 bypass 那条。 */
const PLAN_APPROVAL_ACTIONS: { label: string; mode: string }[] = [
  { label: '批准，逐步确认', mode: 'default' },
  { label: '批准并自动放行', mode: 'bypassPermissions' },
]

function streamPayloadToSegment(payload: ChatStreamPayload): ChatMessageSegment | null {
  const raw = payload.type === 'text_delta' || payload.type === 'reasoning_delta'
    ? payload.segment
    : null
  const id = raw?.id
  const kind = raw?.kind
  const phase = raw?.phase
  const order = raw?.order
  if (!id || !kind || !phase || order == null) return null

  const stepNumber = raw?.stepNumber ?? null
  const toolCallId = raw?.toolCallId ?? null
  return {
    id,
    kind,
    phase,
    order,
    step_number: stepNumber,
    stepNumber,
    round: raw?.round ?? null,
    text: raw?.text ?? null,
    tool_call_id: toolCallId,
    toolCallId,
  }
}

function streamTextDelta(payload: ChatStreamPayload): string {
  return payload.type === 'text_delta' ? payload.delta : ''
}

function streamReasoningDelta(payload: ChatStreamPayload): string {
  return payload.type === 'reasoning_delta' ? payload.delta : ''
}

function isStreamTerminal(payload: ChatStreamPayload): boolean {
  return payload.type === 'run_completed'
    || payload.type === 'run_cancelled'
    || payload.type === 'run_failed'
}

function streamTerminalReason(payload: ChatStreamPayload): 'done' | 'cancelled' | 'error' | undefined {
  if (payload.type === 'run_completed') return 'done'
  if (payload.type === 'run_cancelled') return 'cancelled'
  if (payload.type === 'run_failed') return 'error'
  return undefined
}

function hasStreamPreview(snapshot: ConversationStreamSnapshot | null | undefined): boolean {
  return Boolean(
    snapshot
    && (snapshot.content
      || snapshot.reasoning
      || snapshot.toolCalls.length > 0
      || snapshot.segments.length > 0),
  )
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

// 把一条协议 delta 累积进给定快照（会话单流 or 多答组某列共用）。
// 原地 mutate snapshot；segment 已由调用方算好。返回 void。
function applyStreamDeltaToSnapshot(
  snapshot: ConversationStreamSnapshot,
  payload: ChatStreamPayload,
  segment: ChatMessageSegment | null,
) {
  const textDelta = streamTextDelta(payload)
  const reasoningDelta = streamReasoningDelta(payload)
  if (segment) {
    snapshot.segments = upsertStreamSegment(
      snapshot.segments,
      segment,
      segment.kind === 'reasoning' ? reasoningDelta : textDelta,
    )
  }
  if (reasoningDelta) {
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
    snapshot.reasoning += reasoningDelta
    snapshot.reasoningDurationMs = Math.max(
      snapshot.reasoningDurationMs ?? 0,
      now - snapshot.reasoningStartedAt,
    )
  }
  if (textDelta) {
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
    snapshot.content += textDelta
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

// 把一条协议 tool record 累积进给定快照（会话单流 or 多答组某列共用）。原地 mutate。
function applyToolRecordToSnapshot(
  snapshot: ConversationStreamSnapshot,
  record: ToolCallRecord,
) {
  snapshot.streaming = true
  snapshot.reasoningStreaming = false
  const index = snapshot.toolCalls.findIndex((item) => item.id === record.id)
  snapshot.toolCalls = index < 0
    ? [...snapshot.toolCalls, record]
    : snapshot.toolCalls.map((item, i) => (i === index ? mergeToolRecord(item, record) : item))
  snapshot.segments = upsertToolStreamSegment(snapshot.segments, record)
}

function messageToolCalls(message: ChatMessage): ToolCallRecord[] {
  return message.toolCalls ?? message.tool_calls ?? []
}

function toolStructured(tool: ToolCallRecord): Record<string, unknown> {
  const value = tool.structuredContent ?? tool.structured_content
  return value && typeof value === 'object' ? { ...(value as Record<string, unknown>) } : {}
}

function matchesSubagentTool(tool: ToolCallRecord, payload: ChatSubagentPayload): boolean {
  if (payload.parentToolCallId && tool.id === payload.parentToolCallId) return true
  const structured = toolStructured(tool)
  if (structured.backgroundTaskId === payload.taskId) return true
  if (structured.childSessionId === payload.taskId) return true
  const progress = structured.subagentProgress
  return Boolean(
    progress
    && typeof progress === 'object'
    && (progress as { taskId?: unknown }).taskId === payload.taskId,
  )
}

function findSubagentToolIndex(tools: ToolCallRecord[], payload: ChatSubagentPayload): number {
  const exact = tools.findIndex((item) => matchesSubagentTool(item, payload))
  if (exact >= 0) return exact
  // ponytail: dsh 派出回执是 jobId，session.event 是 child session.id。单卡时绑上去。
  const running = tools
    .map((item, index) => ({ item, index }))
    .filter(({ item }) => item.status === 'running' && isExternalSubagentToolCall(item))
  return running.length === 1 ? running[0].index : -1
}

function mergeSubagentProgress(
  tool: ToolCallRecord,
  payload: ChatSubagentPayload,
): ToolCallRecord {
  const existing = toolStructured(tool)
  const previous = existing.subagentProgress && typeof existing.subagentProgress === 'object'
    ? existing.subagentProgress as { preview?: string; steps?: string[] }
    : {}
  const steps = payload.steps?.length ? payload.steps : previous.steps ?? []
  const preview = payload.preview || previous.preview || ''
  const nextStructured = {
    ...existing,
    ...(payload.taskId
      && existing.backgroundTaskId !== payload.taskId
      ? { childSessionId: payload.taskId }
      : {}),
    subagentProgress: {
      taskId: existing.backgroundTaskId ?? payload.taskId,
      name: payload.name,
      model: payload.model ?? '',
      depth: payload.depth,
      status: payload.status,
      preview,
      steps,
    },
  }
  const terminal = payload.status !== 'running'
  return {
    ...tool,
    status: terminal
      ? payload.status === 'failed'
        ? 'error'
        : payload.status === 'cancelled'
          ? 'cancelled'
          : 'success'
      : tool.status,
    result_preview: terminal && payload.preview ? payload.preview : tool.result_preview,
    structuredContent: nextStructured,
    structured_content: nextStructured,
  }
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

/** 取会话最后一条 user/assistant 消息文本（侧栏 preview 口径，与 api.ts toListItem 一致）。 */
function conversationLastMessageContent(conversation: Conversation): string {
  for (let i = conversation.messages.length - 1; i >= 0; i--) {
    const message = conversation.messages[i]
    if (message.role === 'user' || message.role === 'assistant') {
      return message.content?.trim() ?? ''
    }
  }
  return ''
}

/** 用持久化后的真实会话替换侧栏乐观条目（同 id 原地替换，行实例不销毁，
 *  SwapTitle 才能感知标题从「截断第一句」变成「模型标题」并播放替换过渡）。
 *  keptConversation 为空（发送彻底失败）时退回移除条目。 */
function settleOptimisticConversationListItem(
  setOptimistic: (updater: (items: ConversationListItem[]) => ConversationListItem[]) => void,
  conversationId: string,
  keptConversation: Conversation | null,
): void {
  setOptimistic((items) =>
    keptConversation
      ? items.map((item) =>
          item.id === conversationId
            ? optimisticConversationListItem(
                keptConversation,
                conversationLastMessageContent(keptConversation),
              )
            : item,
        )
      : items.filter((item) => item.id !== conversationId),
  )
}

type SendMessageOptions = {
  forceNewConversation?: boolean
  conversationOverride?: Conversation | null
  /** 前置校验完成、消息正式进入本地发送流程；输入框可立即清空。 */
  onAccepted?: () => void
}

/** 稳定空数组：没有排队消息时不要每次渲染都造一个新引用。 */
const NO_QUEUED_MESSAGES: QueuedMessage[] = []
/** 轨迹未打开时不要把 displayMessages 灌进 Dock，避免流式每帧带动右侧栏。 */
const NO_TRAJECTORY_MESSAGES: ChatMessage[] = []

export default function Chat({ onSettingsChange, onContentReady }: ChatProps) {
  useChatPerfRenderProbe('Chat', { view: hashPath() })
  useChatPerfLongTaskProbe()
  const [chatView, setChatView] = useState<ChatView>(() => {
    const path = hashPath()
    if (isChatOnboardingRoute(path)) return 'onboarding'
    if (isChatSettingsPath(path)) return 'settings'
    if (isChatAssistantCenterPath(path)) return 'assistants'
    if (isChatSkillCenterPath(path)) return 'skill'
    if (isChatMcpCenterPath(path)) return 'mcp'
    if (isChatKnowledgeCenterPath(path)) return 'knowledge'
    if (isChatNotesPath(path)) return 'notes'
    if (isChatSessionCenterPath(path)) return 'sessions'
    // 旧 `#chat/plugins`：插件已迁设置，首屏落到设置页
    if (isChatPluginCenterPath(path)) return 'settings'
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
  const [currentConversation, setCurrentConversation] = useState<Conversation | null>(null)
  const [conversationRenderRequestId, setConversationRenderRequestId] = useState(0)
  /** 全局搜索跳转目标；MessageList 完成滚动后清空。 */
  const [focusMessageId, setFocusMessageId] = useState<string | null>(null)
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
    () => loadLastAgentRuntime() ?? BUILTIN_AGENT_RUNTIME,
  )
  const [skills, setSkills] = useState<SkillMeta[]>([])
  const [disabledSkillIds, setDisabledSkillIds] = useState<string[]>([])
  const [settingsInitialTab, setSettingsInitialTab] = useState<SettingsTab>(() =>
    isChatPluginCenterPath(hashPath()) ? 'plugins' : 'chat',
  )
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
  const [toolConfirmSubmittingId, setToolConfirmSubmittingId] = useState<string | null>(null)
  const [toolConfirmError, setToolConfirmError] = useState('')
  /** 待答的问用户询问：整张可作答的面板吊在**输入框上方**（与审批卡同一个槽位），
   *  消息流里只留一行痕迹。生成这一刻是停在这里等人的，把它放在视线和手都在的地方。 */
  const [pendingUserPrompt, setPendingUserPrompt] = useState<ChatUserPromptPayload | null>(null)
  const [pendingSessionConsent, setPendingSessionConsent] = useState<ChatSessionConsentPayload | null>(null)
  const [sessionConsentSubmittingConversationId, setSessionConsentSubmittingConversationId] = useState<string | null>(null)
  const [sessionConsentError, setSessionConsentError] = useState('')
  const [contextState, setContextState] = useState<ConversationContextState | null>(null)
  const [contextLoading, setContextLoading] = useState(false)
  // 压缩状态必须按会话记，不能用全局 boolean：压缩中切会话会把「压缩中」动画留在
  // 另一个会话上，而压缩事件按 conversationId 派发，收敛条件不能再是「是不是当前会话」
  // （那样后台会话的 completed 会被丢掉，标志永远清不掉）。手动与自动压缩共用这一个集合。
  const [compactingConversationIds, setCompactingConversationIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  )
  const markConversationCompacting = useCallback((conversationId: string, compacting: boolean) => {
    setCompactingConversationIds((previous) => {
      if (previous.has(conversationId) === compacting) return previous
      const next = new Set(previous)
      if (compacting) next.add(conversationId)
      else next.delete(conversationId)
      return next
    })
  }, [])
  const contextCompressing = currentConversation
    ? compactingConversationIds.has(currentConversation.id)
    : false
  const [animateCompactionBoundaryId, setAnimateCompactionBoundaryId] = useState<string | null>(null)
  const [contextError, setContextError] = useState('')
  // Hook 执行失败：非阻断警告条。ponytail: 只留最新一条 —— Hook 是旁路观测，
  // 堆一个可滚动的失败列表没有对应的用户动作。
  const [hookWarning, setHookWarning] = useState<ChatHookPayload | null>(null)
  const [protocolVersionMismatch, setProtocolVersionMismatch] = useState(false)
  const [imageViewerItem, setImageViewerItem] = useState<ChatImageViewerItem | null>(null)
  // 导入的对话：CLI 那边是否已经有新内容（ADR-0002）。只提示，不同步。
  const [importedHistoryStale, setImportedHistoryStale] = useState(false)
  const currentConversationIdRef = useRef<string | null>(null)
  // 始终指向最新 currentConversation。消息操作 handler（编辑/删除/重发）借此读取最新会话，
  // 而无需把 currentConversation 列进 useCallback 依赖——否则每次切模型/思考等级（currentConversation
  // 换引用）这些 handler 都换身份，打穿 MessageBubble 的 memo 导致全列表重渲（公式 remount 闪烁）。
  const currentConversationRef = useRef(currentConversation)
  currentConversationRef.current = currentConversation

  useEffect(() => {
    const id = currentConversation?.id
    if (!id) {
      setImportedHistoryStale(false)
      return
    }
    let cancelled = false
    void chatApi
      .importedHistoryStale(id)
      .then((stale) => {
        if (!cancelled) setImportedHistoryStale(stale)
      })
      // 检查不了就当没过期——这只是个提示，不该因为它报错打断打开对话。
      .catch(() => {
        if (!cancelled) setImportedHistoryStale(false)
      })
    return () => {
      cancelled = true
    }
  }, [currentConversation?.id])
  const activeRunIdRef = useRef<string | null>(null)
  const locallyCancelledConversationIdRef = useRef<string | null>(null)
  const locallyCancelledRunIdRef = useRef<string | null>(null)
  const inFlightConversationsRef = useRef<Set<string>>(new Set())
  const restoredRunIdsRef = useRef<Set<string>>(new Set())
  const pendingStreamDoneRef = useRef<Record<string, () => Promise<void>>>({})
  /** run 结束但落库 twin 尚未随 startTransition 提交时，冻结的预览等它落地再清（防收尾闪帧）。 */
  const pendingPreviewClearRef = useRef<{ conversationId: string; messageId: string | null } | null>(null)
  /** 写 ref 不触发渲染；这个 epoch 保证挂起标记一旦设置，落地 effect 至少跑一次（武装超时兜底）。 */
  const [previewClearEpoch, setPreviewClearEpoch] = useState(0)
  const streamSnapshotsRef = useRef<Record<string, ConversationStreamSnapshot>>({})
  const streamErrorsRef = useRef<Record<string, string>>({})
  const pendingToolConfirmsRef = useRef<Record<string, ChatToolConfirmPayload[]>>({})
  const toolConfirmSubmissionsRef = useRef<Set<string>>(new Set())
  /** 按会话排队（同审批卡）：切会话回来还在等的那条要还在。 */
  const pendingUserPromptsRef = useRef<Record<string, ChatUserPromptPayload[]>>({})
  const pendingSessionConsentsRef = useRef<Record<string, ChatSessionConsentPayload>>({})
  const sessionConsentSubmissionsRef = useRef<Set<string>>(new Set())
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
    pendingUserPrompts: pendingUserPromptsRef.current,
  }), [])

  const generatingConversationIdsRef = useRef<Set<string>>(new Set())
  const syncGeneratingConversationIds = useCallback(() => {
    const next = collectGeneratingConversationIds(
      inFlightConversationsRef.current,
      streamSnapshotsRef.current,
      pendingToolConfirmsRef.current,
    )
    const previous = generatingConversationIdsRef.current
    if (previous.size === next.size && [...previous].every((id) => next.has(id))) return
    generatingConversationIdsRef.current = next
    setGeneratingConversationIds(next)
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
    // 排队消息也一起剔除：会话没了，队列里那几条再没有能落到的地方（`drain` 也拿不到会话对象）。
    // 经 ref 调用是因为队列 hook 声明在下方（它要转发 handleSendMessage）。
    messageQueueRef.current.clearConversation(conversationId)
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
    const current = currentConversationRef.current
    if (
      conversation
      && current
      && conversation.id === current.id
      && conversation.revision < current.revision
    ) {
      return
    }
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

  /** 后台异步结果只能更新它发起时所属的会话，不能把用户后来打开的会话顶掉。 */
  const applyConversationIfCurrent = useCallback((expectedId: string, conversation: Conversation) => {
    if (currentConversationIdRef.current !== expectedId) return false
    applyConversation(conversation)
    return true
  }, [applyConversation])

  // 纯元数据更新（模型 / 思考等级 / 知识库挂载等）：合并后端返回的新元数据，但**保留现有
  // messages 数组引用**。否则每条消息都变成新对象，击穿 MessageBubble/ChatMarkdown 的 memo，
  // 历史消息里的 LaTeX 会整屏重渲闪一下。这类更新后端不会改 messages，沿用旧引用安全。
  const applyConversationMeta = useCallback((updated: Conversation) => {
    setCurrentConversation((prev) => {
      if (!prev || prev.id !== updated.id || updated.revision < prev.revision) return prev
      return { ...updated, messages: prev.messages }
    })
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

  const freezeStreamSnapshot = useCallback((conversationId: string): boolean => {
    const snapshot = streamSnapshotsRef.current[conversationId]
    if (!hasStreamPreview(snapshot)) return false
    // 流中断后保留已经收到的正文、思考和工具卡片；只停止动画，不销毁快照。
    cancelPendingFrame()
    snapshot.streaming = false
    snapshot.reasoningStreaming = false
    syncGeneratingConversationIds()
    if (currentConversationIdRef.current === conversationId) {
      setStreamSnapshot(snapshot)
      setStreamCoarse({ streaming: false, streamFrozen: true, cancelling: false })
      activeRunIdRef.current = snapshot.runId
      streamStartedAtRef.current = snapshot.startedAt
      streamingContentRef.current = snapshot.content
      streamingReasoningRef.current = snapshot.reasoning
    }
    return true
  }, [cancelPendingFrame, syncGeneratingConversationIds])

  /**
   * run 收尾时的预览清除（两条收尾路径共用）。⚠️ 不能直接 clearStreamingPreview：
   * 它的 store 更新走 SyncLane（useSyncExternalStore 防撕裂强制同步刷新），会抢在
   * applyConversation 的 setState（DefaultLane）/ reloadConversation 的 startTransition
   * 之前**单独提交一帧** —— 那一帧 live 已卸载、落库 twin 还没进已提交的 messages，
   * 整条回答消失又出现（实测 Δsh −294～−4288、scrollTop 被钳，就是「生成完闪/沉」）。
   * 同一同步代码块里先调 applyConversation 也没用，lane 优先级会把顺序反转。
   * twin 尚未出现在**已提交**的 messages（currentConversationRef 在 render 期赋值，
   * 语义即「已渲染的会话」）时，先冻结预览 —— 冻结同样是 SyncLane 先上屏，但
   * frozen=true 让 live 气泡留在原地，那一帧无害；等 pendingPreviewClear effect 看到
   * twin 真正落地再清，live→twin 就是同一 commit 的原子交换。
   */
  const settleStreamingPreview = useCallback((conversationId: string) => {
    const snapshot = streamSnapshotsRef.current[conversationId]
    const twinId = snapshot?.messageId ?? null
    const twinLanded = !twinId
      || (currentConversationRef.current?.messages ?? []).some((m) => m.id === twinId)
    if (!twinLanded && freezeStreamSnapshot(conversationId)) {
      pendingPreviewClearRef.current = { conversationId, messageId: twinId }
      setPreviewClearEpoch((value) => value + 1)
    } else {
      clearStreamingPreview()
    }
  }, [clearStreamingPreview, freezeStreamSnapshot])

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
      setPendingUserPrompt(null)
      setToolConfirmError('')
      setSessionConsentError('')
      setStreamCoarse({ streamError: '' })
      return
    }
    const snapshot = streamSnapshotsRef.current[conversationId]
    if (!snapshot) {
      clearStreamingPreview()
    } else {
      setStreamSnapshot(snapshot)
      setStreamCoarse({
        streaming: snapshot.streaming,
        streamFrozen: !snapshot.streaming && Boolean(streamErrorsRef.current[conversationId]) && hasStreamPreview(snapshot),
        cancelling: false,
      })
      activeRunIdRef.current = snapshot.runId
      streamStartedAtRef.current = snapshot.startedAt
      streamingContentRef.current = snapshot.content
      streamingReasoningRef.current = snapshot.reasoning
    }
    setStreamCoarse({ streamError: streamErrorsRef.current[conversationId] ?? '' })
    setPendingToolConfirm(pendingToolConfirmsRef.current[conversationId]?.[0] ?? null)
    setPendingSessionConsent(pendingSessionConsentsRef.current[conversationId] ?? null)
    setPendingUserPrompt(pendingUserPromptsRef.current[conversationId]?.[0] ?? null)
    setToolConfirmError('')
    setSessionConsentError('')
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
      setPendingUserPrompt(null)
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
      delete pendingUserPromptsRef.current[conversationId]
    }
    setPendingToolConfirm(null)
    setPendingSessionConsent(null)
    setPendingUserPrompt(null)
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
  const usesChatRuntime = activeAgentRuntime.kind === 'chat'
  // 底栏模式胶囊：内置 Agent = Act/Plan/Orchestrate；Kivio Chat 无此胶囊；本地 CLI = 沙盒档位。
  // CLI 没有档位时返回空表 → 胶囊隐藏。
  const detectedExternalAgents = useDetectedExternalAgents(currentConversation?.id ?? null)
  const activeAgentPlanMode = currentConversation?.agent_plan_state?.mode
    ?? currentConversation?.agentPlanState?.mode
    ?? 'act'
  const composerModes = useMemo(
    () => derivePermissionModes({
      target: 'composer',
      agentRuntime: activeAgentRuntime,
      agents: detectedExternalAgents,
      agentPlanMode: activeAgentPlanMode,
    }),
    [activeAgentRuntime, detectedExternalAgents, activeAgentPlanMode],
  )
  const composerPresets = useMemo(
    () => deriveDshPresetModes(activeAgentRuntime),
    [activeAgentRuntime],
  )
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
  const reloadConversationRef = useRef<((id: string, transitionRequestId?: number) => void) | null>(null)
  const handleRouteResetConversation = useCallback(() => {
    invalidateConversationTransition()
    currentConversationIdRef.current = null
    applyConversation(null)
    restoreStreamingPreview(null)
  }, [applyConversation, restoreStreamingPreview])
  const handleRouteLoadConversation = useCallback((conversationId: string) => {
    const requestId = beginConversationTransition(conversationId)
    reloadConversationRef.current?.(conversationId, requestId)
  }, [])

  const openEmbeddedSettingsForPlugins = useCallback(() => {
    setSettingsInitialTab('plugins')
    setChatView('settings')
    setHash('#chat/settings')
  }, [])

  const {
    syncConversationRoute,
    syncSettingsRoute,
    syncOnboardingRoute,
    syncAssistantCenterRoute,
    syncSkillCenterRoute,
    syncSessionCenterRoute,
    syncMcpCenterRoute,
    syncKnowledgeCenterRoute,
    syncNotesRoute,
  } = useChatRouting({
    onViewChange: setChatView,
    onLoadConversation: handleRouteLoadConversation,
    onResetConversation: handleRouteResetConversation,
    currentConversationIdRef,
    onOpenPluginsSettings: openEmbeddedSettingsForPlugins,
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

  const skillProjectCwdRef = useRef('')
  const loadSkills = useCallback(async () => {
    if (!isTauriRuntime()) {
      setSkills([])
      return
    }
    try {
      const result = await api.chatSkillsList(undefined, skillProjectCwdRef.current || undefined)
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
      void import('./SessionCenter')
      void import('./MessageList')
    }, 400)
  }, [])

  const openEmbeddedSettings = useCallback((tab: SettingsTab = 'chat') => {
    setSettingsInitialTab(tab)
    setChatView('settings')
    syncSettingsRoute()
  }, [syncSettingsRoute])

  const handleOpenChatSettings = useCallback(() => {
    openEmbeddedSettings('chat')
  }, [openEmbeddedSettings])

  const openAssistantCenter = useCallback(() => {
    setChatView('assistants')
    syncAssistantCenterRoute()
  }, [syncAssistantCenterRoute])

  const openSkillCenter = useCallback(() => {
    setChatView('skill')
    syncSkillCenterRoute()
  }, [syncSkillCenterRoute])

  const openSessionCenter = useCallback(() => {
    setChatView('sessions')
    syncSessionCenterRoute()
  }, [syncSessionCenterRoute])

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
    if (item === 'sessions') {
      openSessionCenter()
      return
    }
  }, [openAssistantCenter, openSkillCenter, openMcpCenter, openKnowledgeCenter, openNotesCenter, openSessionCenter])

  const extensionsActive = useMemo<ExtensionsNavItem | null>(() => {
    if (chatView === 'assistants') return 'assistants'
    if (chatView === 'skill') return 'skill'
    if (chatView === 'mcp') return 'mcp'
    if (chatView === 'knowledge') return 'knowledge'
    if (chatView === 'notes') return 'notes'
    if (chatView === 'sessions') return 'sessions'
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
    if (prev === 'skill' || prev === 'mcp' || prev === 'assistants' || prev === 'knowledge' || prev === 'settings') {
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

  const reloadConversation = useCallback(async (
    conversationId: string,
    options?: { force?: boolean; transitionRequestId?: number; allowNavigation?: boolean },
  ) => {
    if (isConversationInFlight(inFlightConversationsRef.current, conversationId) && !options?.force) {
      return
    }
    const transitionRequestId = options?.transitionRequestId
    const startingConversationId = currentConversationIdRef.current
    const canCommitResult = () => {
      if (transitionRequestId !== undefined) {
        return isCurrentConversationTransition(transitionRequestId, conversationId)
      }
      if (options?.allowNavigation) {
        return currentConversationIdRef.current === startingConversationId
          || currentConversationIdRef.current === conversationId
      }
      return currentConversationIdRef.current === conversationId
    }
    try {
      const conv = await chatApi.getConversation(conversationId)
      const transition = getConversationTransitionSnapshot()
      if (
        !canCommitResult()
        || (transition.loading && transition.targetConversationId !== conversationId)
      ) return
      if (transitionRequestId !== undefined || options?.allowNavigation) {
        currentConversationIdRef.current = conversationId
      }
      const renderRequestId = transitionRequestId
        ?? (transition.loading && transition.targetConversationId === conversationId
          ? transition.requestId
          : 0)
      startTransition(() => {
        applyConversation(conv)
        if (renderRequestId > 0) setConversationRenderRequestId(renderRequestId)
      })
      if (renderRequestId > 0 && conv.messages.length === 0) {
        window.requestAnimationFrame(() => {
          completeConversationTransition(conversationId, renderRequestId)
        })
      }
      restoreStreamingPreview(conversationId)
      setStreamCoarse({ cancelling: false })
    } catch (err) {
      const transition = getConversationTransitionSnapshot()
      if (
        !canCommitResult()
        || (transition.loading && transition.targetConversationId !== conversationId)
      ) return
      console.error('Failed to reload conversation:', err)
      // B2：reload 失败（尤其"对话不存在"）——把 ghost 从乐观列表/in-flight/快照剔除并刷新侧栏。
      dropConversationLocally(conversationId)
      forgetRememberedChatRoute()
      if (currentConversationIdRef.current === conversationId || currentConversationIdRef.current === null) {
        currentConversationIdRef.current = null
        applyConversation(null)
        syncConversationRoute(null)
      }
      if (transitionRequestId !== undefined) {
        cancelConversationTransition(transitionRequestId)
      } else if (transition.loading && transition.targetConversationId === conversationId) {
        cancelConversationTransition(transition.requestId)
      }
      refreshSidebar()
      setStreamError(typeof err === 'string' ? err : (err as Error).message || '对话加载失败，已从列表移除')
    }
  }, [applyConversation, dropConversationLocally, refreshSidebar, restoreStreamingPreview, syncConversationRoute])

  // 填充上面 useChatRouting 用来打破循环依赖的间接层。
  reloadConversationRef.current = (id: string, transitionRequestId?: number) => {
    void reloadConversation(id, { force: true, transitionRequestId })
  }

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
    if (!conversationId || compactingConversationIds.has(conversationId)) return
    markConversationCompacting(conversationId, true)
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
      // 清零不看「我还在不在这个会话」——切走后原来那个守卫永远不成立，标志会卡死。
      markConversationCompacting(conversationId, false)
    }
  }, [compactingConversationIds, markConversationCompacting, patchContextState, refreshSidebar])

  const finishStreamingRun = useCallback(
    async (payload: { reason?: string; conversationId?: string }) => {
      const conversationId = payload.conversationId ?? currentConversationIdRef.current
      const preservedPartial = payload.reason === 'error' && conversationId
        ? freezeStreamSnapshot(conversationId)
        : false
      // 兜底：run 结束时压缩必然已终止；防御后端遗漏终止事件把"压缩中"状态卡死。
      if (conversationId) markConversationCompacting(conversationId, false)
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
        // in-flight 也一起清：终局事件是「这一轮结束了」的权威。
        // 走 send/regenerate 的 run 到这里时它们的 finally 已经清过（这里是幂等的空操作）；
        // 而**恢复的 run**（restoredFromSnapshot，窗口重载后后端回放正在跑的那轮）没有 invoke
        // 归属它，只有这条路径能清 —— 漏了它侧栏那颗转圈就永远停不下来。
        clearConversationLocalState(localState(), conversationId, { inFlight: true })
        syncGeneratingConversationIds()
      }
      if (conversationId && currentConversationRef.current?.id === conversationId) {
        setPendingToolConfirm(null)
        setPendingSessionConsent(null)
        setPendingUserPrompt(null)
        // 见 settleStreamingPreview 注释：直接清会被 SyncLane 抢跑出一帧空 commit。
        if (!preservedPartial) settleStreamingPreview(conversationId)
      }
    },
    [freezeStreamSnapshot, localState, markConversationCompacting, refreshSidebar, reloadConversation, resetLocalCancellation, setStreamErrorForConversation, settleStreamingPreview, syncGeneratingConversationIds],
  )

  // 冻结预览的延迟清除：等 finishStreamingRun 记下的 twin 真正出现在 messages 里
  // （transition 提交后）再清，live→twin 就是同一 commit 的原子交换，不再闪帧。
  // 兜底：切走会话只丢标记（restoreStreamingPreview 接管视图）；1.5s 超时强制清
  // （防 messageId 与落库 id 不一致时冻结卡死）。
  useEffect(() => {
    const pending = pendingPreviewClearRef.current
    if (!pending) return
    if (currentConversationIdRef.current !== pending.conversationId || currentConversation?.id !== pending.conversationId) {
      pendingPreviewClearRef.current = null
      return
    }
    const landed = pending.messageId
      ? (currentConversation?.messages ?? []).some((m) => m.id === pending.messageId)
      : true
    if (landed) {
      pendingPreviewClearRef.current = null
      clearStreamingPreview()
      return
    }
    const timeout = window.setTimeout(() => {
      if (pendingPreviewClearRef.current === pending) {
        pendingPreviewClearRef.current = null
        if (currentConversationIdRef.current === pending.conversationId) clearStreamingPreview()
      }
    }, 1_500)
    return () => window.clearTimeout(timeout)
  }, [clearStreamingPreview, currentConversation, previewClearEpoch])

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
      setPendingUserPrompt(null)
    }
    clearConversationLocalState(localState(), conversationId)
    syncGeneratingConversationIds()
    if (currentConversationIdRef.current === conversationId) {
      // 见 settleStreamingPreview 注释：applyConversation 的 setState 是 DefaultLane，
      // 这里直接 clearStreamingPreview（SyncLane）会抢跑出一帧「live 已卸、twin 未至」。
      settleStreamingPreview(conversationId)
    }
  }, [applyConversation, localState, settleStreamingPreview, syncGeneratingConversationIds])

  useTauriEvent(api.onChatProtocolIssue, ({ issue, conversationId }) => {
    if (issue === 'version_mismatch') {
      setProtocolVersionMismatch(true)
    } else if (
      issue === 'resync_required'
      && conversationId
      && conversationId === currentConversationIdRef.current
    ) {
      void reloadConversation(conversationId)
    }
  }, [reloadConversation])

  useTauriEvent(api.onChatStream, (payload) => {
      if (isLocallyCancelledPayload(
        payload,
        locallyCancelledConversationIdRef.current,
        locallyCancelledRunIdRef.current,
      )) {
        return
      }
      const terminal = isStreamTerminal(payload)
      const terminalPayload = {
        conversationId: payload.conversationId,
        reason: streamTerminalReason(payload),
      }
      if (payload.type === 'run_started') {
        if (payload.restoredFromSnapshot) restoredRunIdsRef.current.add(payload.runId)
        // 不是本窗口发起的 run（后端自起的唤醒轮 / 别的窗口的 run）：此刻会话必然不在
        // in-flight（本窗口的 send/regenerate 在 invoke 前就标了）。这类 run 没有
        // sendMessage 的统一收尾可等，必须走恢复路径在终止帧上立即 finishStreamingRun
        // ——否则下面的 markConversationInFlight 会让终止分支把收尾推迟给一个永远
        // 不会返回的 invoke，转圈和停止键永远停不下来（实测：唤醒轮消息落地后卡住）。
        if (!isConversationInFlight(inFlightConversationsRef.current, payload.conversationId)) {
          restoredRunIdsRef.current.add(payload.runId)
        }
        const remainingApprovals = (pendingToolConfirmsRef.current[payload.conversationId] ?? [])
          .filter((item) => item.runId !== payload.runId)
        if (remainingApprovals.length > 0) {
          pendingToolConfirmsRef.current[payload.conversationId] = remainingApprovals
        } else {
          delete pendingToolConfirmsRef.current[payload.conversationId]
        }
        if (currentConversationIdRef.current === payload.conversationId) {
          setPendingToolConfirm(remainingApprovals[0] ?? null)
          setToolConfirmError('')
        }
        if (pendingSessionConsentsRef.current[payload.conversationId]?.runId === payload.runId) {
          delete pendingSessionConsentsRef.current[payload.conversationId]
          if (currentConversationIdRef.current === payload.conversationId) {
            setPendingSessionConsent(null)
            setSessionConsentError('')
          }
        }
        if (payload.recovery) {
          restoreGroupArm(
            payload.conversationId,
            payload.recovery.groupId,
            payload.recovery.groupSize,
            payload.recovery.armIndex,
            payload.messageId,
            payload.recovery.providerId,
            payload.recovery.model,
          )
        }
        markConversationInFlight(payload.conversationId)
        if (hasActiveGroup(payload.conversationId) && payload.messageId) {
          const column = ensureGroupColumn(payload.conversationId, payload.messageId)
          if (column) {
            Object.assign(column, createEmptyStreamSnapshot(), {
              runId: payload.runId,
              messageId: payload.messageId,
              streaming: true,
              startedAt: Date.now(),
            })
            touchGroup(payload.conversationId)
          }
          if (currentConversationIdRef.current === payload.conversationId) {
            setStreamCoarse({ streaming: true, streamFrozen: false, cancelling: false })
          }
          return
        }
        const restored = createEmptyStreamSnapshot()
        restored.runId = payload.runId
        restored.messageId = payload.messageId
        restored.streaming = true
        restored.startedAt = Date.now()
        streamSnapshotsRef.current[payload.conversationId] = restored
        syncGeneratingConversationIds()
        showStreamSnapshotIfCurrent(payload.conversationId, restored)
        return
      }
      if (!streamSnapshotsRef.current[payload.conversationId]) {
        if (!isConversationInFlight(inFlightConversationsRef.current, payload.conversationId)) {
          if (terminal) {
            void finishStreamingRun(terminalPayload)
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
        if (terminal) {
          finalizeReasoningDurationOnDone(column)
          column.streaming = false
          // 列结束是终止帧：立即 flush（不等下一帧），让该列完成态尽快可见。
          flushGroups(payload.conversationId)
          if (restoredRunIdsRef.current.delete(payload.runId)) {
            const group = getActiveGroup(payload.conversationId)
            if (group?.columns.every((item) => !item.streaming)) {
              endGroup(payload.conversationId)
              void finishStreamingRun(terminalPayload)
            }
          }
        } else {
          // 内容 delta 经 rAF 合帧（N 列高频 delta 不各自打爆 setState）。
          touchGroup(payload.conversationId)
        }
        // 组的整体「done / 持久化」交给 sendMessage 返回后的统一收尾；这里不触发 finishStreamingRun。
        return
      }
      const snapshot = ensureStreamSnapshot(payload.conversationId)
      if (payload.runId) {
        if (snapshot.runId && snapshot.runId !== payload.runId) return
        snapshot.runId = payload.runId
      }
      // 所有 run 事件都带有 messageId；补上它可以覆盖本窗口先创建的空快照，
      // 也让协议快照恢复时的历史草稿与实时预览能够按同一条消息互斥渲染。
      if (payload.messageId) snapshot.messageId = payload.messageId
      const segment = streamPayloadToSegment(payload)
      const textDelta = streamTextDelta(payload)
      const reasoningDelta = streamReasoningDelta(payload)
      // 正文/思考恢复流动 = 上游重试已经成功（没有显式的成功帧），状态行的重试尾巴即刻清除。
      if (textDelta || reasoningDelta) snapshot.statusNote = null
      if (segment) {
        snapshot.segments = upsertStreamSegment(
          snapshot.segments,
          segment,
          segment.kind === 'reasoning' ? reasoningDelta : textDelta,
        )
      }
      if (reasoningDelta) {
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
        snapshot.reasoning += reasoningDelta
        snapshot.reasoningDurationMs = Math.max(
          snapshot.reasoningDurationMs ?? 0,
          now - snapshot.reasoningStartedAt,
        )
      }
      if (textDelta) {
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
        snapshot.content += textDelta
      }
      syncGeneratingConversationIds()
      showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
      if (terminal) {
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
        if (restoredRunIdsRef.current.delete(payload.runId)) {
          void finishStreamingRun(terminalPayload)
          return
        }
        // invoke 未完成前不要 reload；延后到 flushPendingStreamDone，避免与 send 写盘竞态。
        if (isConversationInFlight(inFlightConversationsRef.current, payload.conversationId)) {
          pendingStreamDoneRef.current[payload.conversationId] = () => finishStreamingRun(terminalPayload)
          return
        }
        void finishStreamingRun(terminalPayload)
      }
  }, [ensureStreamSnapshot, finishStreamingRun, markConversationInFlight, showStreamSnapshotIfCurrent, syncGeneratingConversationIds])

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
    const conversationId = payload.conversationId
    if (!conversationId) return
    // 压缩状态按事件里的会话记，不看是不是当前会话：后台会话的 started/completed
    // 都要收进集合，否则切走再切回来会漏掉开始、或者永远等不到结束。
    if (payload.trigger !== 'manual') {
      markConversationCompacting(conversationId, payload.phase === 'started')
    }
    if (payload.phase === 'started') return
    // 下面这些改的是当前会话的展示状态（边界动画 / currentConversation），仍要按当前会话过滤。
    if (conversationId !== currentConversationIdRef.current) return
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
  }, [markConversationCompacting])

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
        touchGroup(payload.conversationId)
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
      // 插话卡到了 = 那条「立刻引导」真的进了模型历史，现在才把它从队列里摘掉。
      // （在此之前它一直留着，好让「没赶上轮次边界」退化成运行结束后的自动发送。）
      if (isUserSteerToolCall(record)) {
        const steerId = (record.structuredContent as { steer_id?: unknown } | undefined)?.steer_id
        if (typeof steerId === 'string') {
          messageQueueRef.current.confirmSteered(payload.conversationId, steerId)
        }
      } else if (isUserFollowUpToolCall(record)) {
        const followUpId = (record.structuredContent as { follow_up_id?: unknown } | undefined)?.follow_up_id
        if (typeof followUpId === 'string') {
          messageQueueRef.current.confirmFollowUp(payload.conversationId, followUpId)
        }
      }
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
  // 流状态行的瞬态一行字（上游重试等）：写进会话流快照，StreamStatusLine 每秒读。
  // 清除有两条路：后端显式 note=null，或正文/思考恢复流动（onChatStream 的 delta 分支）。
  useTauriEvent(api.onChatStatusNote, (payload) => {
    const snapshot = streamSnapshotsRef.current[payload.conversationId]
    if (!snapshot) return
    if (snapshot.runId && snapshot.runId !== payload.runId) return
    snapshot.statusNote = payload.note
    showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
  }, [showStreamSnapshotIfCurrent])

  useTauriEvent(api.onChatSubagent, (payload) => {
      // 父轮还在飞：写流快照。父轮已经收尾后 streamSnapshotsRef 仍可能留着死快照，
      // 不能再当直播通道，否则步骤写进看不见的对象，卡上永远「运行中…」。
      const inFlight = isConversationInFlight(
        inFlightConversationsRef.current,
        payload.parentConversationId,
      )
      if (inFlight) {
        const snapshot = ensureStreamSnapshot(payload.parentConversationId)
        // Match the active run when known; only drop when both ids are set and differ.
        if (payload.parentRunId && snapshot.runId && snapshot.runId !== payload.parentRunId) return
        const index = findSubagentToolIndex(snapshot.toolCalls, payload)
        if (index < 0) return
        snapshot.toolCalls = snapshot.toolCalls.map((item, i) => (
          i === index ? mergeSubagentProgress(item, payload) : item
        ))
        showStreamSnapshotIfCurrent(payload.parentConversationId, snapshot)
        return
      }
      if (currentConversationIdRef.current !== payload.parentConversationId) return
      setCurrentConversation((prev) => {
        if (!prev || prev.id !== payload.parentConversationId) return prev
        let changed = false
        const messages = prev.messages.map((message) => {
          const tools = messageToolCalls(message)
          const index = findSubagentToolIndex(tools, payload)
          if (index < 0) return message
          changed = true
          const nextTools = tools.map((item, i) => (
            i === index ? mergeSubagentProgress(item, payload) : item
          ))
          return { ...message, toolCalls: nextTools, tool_calls: nextTools }
        })
        return changed ? { ...prev, messages } : prev
      })
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
      // 同时排进「输入框上方」那张面板的队列：消息流里的那条只是痕迹，真正作答在面板上。
      const queue = pendingUserPromptsRef.current[payload.conversationId] ?? []
      const queued = queue.some((item) => item.toolCallId === payload.toolCallId)
      pendingUserPromptsRef.current[payload.conversationId] = queued ? queue : [...queue, payload]
      if (currentConversationIdRef.current === payload.conversationId) {
        setPendingUserPrompt(pendingUserPromptsRef.current[payload.conversationId][0] ?? null)
      }
      syncGeneratingConversationIds()
      showStreamSnapshotIfCurrent(payload.conversationId, snapshot)
  }, [ensureStreamSnapshot, showStreamSnapshotIfCurrent, syncGeneratingConversationIds])

  /** 面板用的工具记录：**必须记忆** —— 写在 JSX 里每渲染新建一个对象，会把卡片里
   *  「换了新询问就重置草稿」的 effect 变成每渲染都重置（用户选到一半的答案被清空）。 */
  const pendingUserPromptRecord = useMemo(
    () => (pendingUserPrompt ? userPromptEventToRecord(pendingUserPrompt) : null),
    [pendingUserPrompt],
  )

  /** 面板作答完（或那一轮结束了）就把它收起来。后端没有「已答复」事件 ——
   *  `resolve_user_prompt` 只清重放快照、不发事件，所以收起由前端自己负责。 */
  const dismissPendingUserPrompt = useCallback((conversationId: string, toolCallId?: string) => {
    const rest = (pendingUserPromptsRef.current[conversationId] ?? [])
      .filter((item) => toolCallId == null || item.toolCallId !== toolCallId)
    if (rest.length > 0) {
      pendingUserPromptsRef.current[conversationId] = rest
    } else {
      delete pendingUserPromptsRef.current[conversationId]
    }
    if (currentConversationIdRef.current === conversationId) {
      setPendingUserPrompt(rest[0] ?? null)
    }
  }, [])

  useTauriEvent(api.onChatToolConfirm, (payload) => {
    // 排队而不是覆盖：一条消息里并行调多个工具时，后端会同时挂着多条询问等答复
    // （按 request_id 路由）。覆盖会让用户没看见的那条静默超时 ⇒ 模型收到「用户拒绝」。
    const queue = pendingToolConfirmsRef.current[payload.conversationId] ?? []
    if (!queue.some((item) => item.toolCallId === payload.toolCallId)) {
      queue.push(payload)
    }
    pendingToolConfirmsRef.current[payload.conversationId] = queue
    syncGeneratingConversationIds()
    if (currentConversationIdRef.current === payload.conversationId) {
      setPendingToolConfirm(queue[0] ?? null)
      setToolConfirmError('')
      // 计划卡一出现就在右侧栏摊开整份计划 —— 卡片上那块小灰框读不完。
      if (isPlanApproval(payload) && payload.argumentsPreview?.trim()) {
        requestDockMarkdownPreview({ title: '计划', text: payload.argumentsPreview })
      }
    }
  }, [syncGeneratingConversationIds])

  useTauriEvent(api.onChatToolConfirmWithdraw, (payload) => {
    const rest = (pendingToolConfirmsRef.current[payload.conversationId] ?? [])
      .filter((item) => item.toolCallId !== payload.toolCallId)
    if (rest.length > 0) {
      pendingToolConfirmsRef.current[payload.conversationId] = rest
    } else {
      delete pendingToolConfirmsRef.current[payload.conversationId]
    }
    syncGeneratingConversationIds()
    if (currentConversationIdRef.current === payload.conversationId) {
      setPendingToolConfirm((current) =>
        current?.toolCallId === payload.toolCallId ? rest[0] ?? null : current)
      setToolConfirmError('')
    }
  }, [syncGeneratingConversationIds])

  const resolvePendingToolConfirm = useCallback(async (
    approved: boolean,
    always = false,
    permissionMode: string | null = null,
  ): Promise<boolean> => {
    const prompt = pendingToolConfirm
    if (!prompt || toolConfirmSubmissionsRef.current.has(prompt.toolCallId)) return false

    toolConfirmSubmissionsRef.current.add(prompt.toolCallId)
    setToolConfirmSubmittingId(prompt.toolCallId)
    setToolConfirmError('')
    try {
      await api.chatConfirmToolCall(prompt.toolCallId, approved, always, permissionMode)
      const rest = (pendingToolConfirmsRef.current[prompt.conversationId] ?? [])
        .filter((item) => item.toolCallId !== prompt.toolCallId)
      if (rest.length > 0) {
        pendingToolConfirmsRef.current[prompt.conversationId] = rest
      } else {
        delete pendingToolConfirmsRef.current[prompt.conversationId]
      }
      syncGeneratingConversationIds()
      if (currentConversationIdRef.current === prompt.conversationId) {
        setPendingToolConfirm(rest[0] ?? null)
        setToolConfirmError('')
      }
      return true
    } catch (error) {
      console.error('Failed to submit tool confirmation:', error)
      const isStillPending = (pendingToolConfirmsRef.current[prompt.conversationId] ?? [])
        .some((item) => item.toolCallId === prompt.toolCallId)
      if (currentConversationIdRef.current === prompt.conversationId && isStillPending) {
        setToolConfirmError(
          typeof error === 'string' ? error : (error as Error).message || '提交审批失败，请重试',
        )
      }
      return false
    } finally {
      toolConfirmSubmissionsRef.current.delete(prompt.toolCallId)
      setToolConfirmSubmittingId((current) => current === prompt.toolCallId ? null : current)
    }
  }, [pendingToolConfirm, syncGeneratingConversationIds])

  useTauriEvent(api.onChatSessionConsent, (payload) => {
    pendingSessionConsentsRef.current[payload.conversationId] = payload
    if (currentConversationIdRef.current === payload.conversationId) {
      setPendingSessionConsent(payload)
      setSessionConsentError('')
    }
  }, [])

  const resolvePendingSessionConsent = useCallback(async (granted: boolean): Promise<boolean> => {
    const prompt = pendingSessionConsent
    if (!prompt || sessionConsentSubmissionsRef.current.has(prompt.conversationId)) return false

    sessionConsentSubmissionsRef.current.add(prompt.conversationId)
    setSessionConsentSubmittingConversationId(prompt.conversationId)
    setSessionConsentError('')
    try {
      await api.chatRespondSessionConsent(prompt.conversationId, granted)
      if (pendingSessionConsentsRef.current[prompt.conversationId]?.runId === prompt.runId) {
        delete pendingSessionConsentsRef.current[prompt.conversationId]
      }
      if (currentConversationIdRef.current === prompt.conversationId) {
        setPendingSessionConsent(pendingSessionConsentsRef.current[prompt.conversationId] ?? null)
        setSessionConsentError('')
      }
      return true
    } catch (error) {
      console.error('Failed to submit session consent:', error)
      if (
        currentConversationIdRef.current === prompt.conversationId
        && pendingSessionConsentsRef.current[prompt.conversationId]?.runId === prompt.runId
      ) {
        setSessionConsentError(
          typeof error === 'string' ? error : (error as Error).message || '提交会话授权失败，请重试',
        )
      }
      return false
    } finally {
      sessionConsentSubmissionsRef.current.delete(prompt.conversationId)
      setSessionConsentSubmittingConversationId((current) => (
        current === prompt.conversationId ? null : current
      ))
    }
  }, [pendingSessionConsent])

  useEffect(() => {
    const conversationId = currentConversation?.id
    if (!conversationId) return
    void api.chatSyncState(conversationId).catch((error) => {
      console.error('Failed to synchronize chat protocol state:', error)
    })
  }, [currentConversation?.id])

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
          void reloadConversation(payload.conversationId, { force: true, allowNavigation: true })
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

  const handleSelectConversation = useCallback(async (
    conversationId: string,
    conversationHint?: ConversationLoadHint,
  ) => {
    // 重复点击已打开的会话：不重载。原因是全量走一遍会 beginConversationTransition
    // （>12 条消息还会铺 Logo 加载态）→ IPC 读盘 → applyConversation 换一个新的
    // conversation 对象；而 applyConversation 的 revision 守卫只挡「回退」
    // （`<`，同会话重读 revision 相等挡不住），MessageList 的测量缓存与
    // messageLayoutRevision 都按消息对象身份走，替换式更新会让整条会话重新估高重挂
    // ——用户看到的就是"又加载了一次"。
    // 例外：带 focusMessageId 的搜索跳转仍要生效，只是走 focus 而不是重载。
    if (
      currentConversationIdRef.current === conversationId
      && currentConversationRef.current?.id === conversationId
    ) {
      setFocusMessageId(conversationHint?.focusMessageId ?? null)
      // 路由可能因为停留在中心页（技能/MCP/设置…）而偏离当前会话，补一次对齐。
      syncConversationRoute(conversationId)
      return
    }
    const requestId = beginConversationTransition(conversationId, conversationHint)
    setAssistantStreamStatsByMessageId({})
    setHookWarning(null)
    setFocusMessageId(conversationHint?.focusMessageId ?? null)
    try {
      const conv = await chatApi.getConversation(conversationId)
      if (!isCurrentConversationTransition(requestId, conversationId)) return
      currentConversationIdRef.current = conversationId
      startTransition(() => {
        applyConversation(conv)
        setConversationRenderRequestId(requestId)
      })
      if (conv.messages.length === 0) {
        window.requestAnimationFrame(() => {
          completeConversationTransition(conversationId, requestId)
        })
      }
      restoreStreamingPreview(conversationId)
      syncConversationRoute(conversationId)
      setStreamError('')
    } catch (err) {
      if (!isCurrentConversationTransition(requestId, conversationId)) return
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
      cancelConversationTransition(requestId)
    }
  }, [applyConversation, dropConversationLocally, refreshSidebar, restoreStreamingPreview, syncConversationRoute])

  const handleConversationFirstCommit = useCallback((conversationId: string, requestId: number) => {
    window.requestAnimationFrame(() => {
      completeConversationTransition(conversationId, requestId)
    })
  }, [])

  const handleNewConversation = useCallback(async () => {
    invalidateConversationTransition()
    setSelectedProject(null)
    setSelectedSet(null)
    setAssistantStreamStatsByMessageId({})
    setDraftProviderId(activeProviderId)
    setDraftModel(activeModel)
    setDraftAgentRuntime(activeAgentRuntime)
    saveLastAgentRuntime(activeAgentRuntime)
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
    invalidateConversationTransition()
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
      if (currentConversationIdRef.current === conversationId) {
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
        setStreamError('')
      }
      refreshSidebar()
    } catch (err) {
      console.error('Failed to clear chat:', err)
      setStreamErrorForConversation(
        conversationId,
        typeof err === 'string' ? err : (err as Error).message || '清空对话失败',
      )
    }
  }, [applyConversation, clearConversationInFlight, localState, refreshSidebar, restoreStreamingPreview, setStreamErrorForConversation, syncConversationRoute])

  const handleStartAssistantChat = useCallback(async (assistant: ChatAssistant) => {
    const startingConversationId = currentConversationIdRef.current
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
      refreshSidebar()
      if (currentConversationIdRef.current === startingConversationId) {
        currentConversationIdRef.current = conv.id
        applyConversation(conv)
        restoreStreamingPreview(conv.id)
        syncConversationRoute(conv.id)
        setStreamError('')
      }
    } catch (err) {
      console.error('Failed to start assistant conversation:', err)
      if (currentConversationIdRef.current === startingConversationId) {
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '创建助手对话失败')
      }
    }
  }, [activeModel, activeProviderId, applyConversation, refreshSidebar, restoreStreamingPreview, selectedProject?.id, selectedProject?.name, selectedSet?.id, syncConversationRoute])

  const handleStartBuilderChat = useCallback(async () => {
    const startingConversationId = currentConversationIdRef.current
    setAssistantStreamStatsByMessageId({})
    try {
      const conv = await chatApi.createBuilderConversation(
        activeProviderId || undefined,
        activeModel || undefined,
        selectedProject?.id ?? null,
      )
      refreshSidebar()
      if (currentConversationIdRef.current === startingConversationId) {
        currentConversationIdRef.current = conv.id
        applyConversation(conv)
        restoreStreamingPreview(conv.id)
        syncConversationRoute(conv.id)
        setStreamError('')
      }
    } catch (err) {
      console.error('Failed to start builder conversation:', err)
      if (currentConversationIdRef.current === startingConversationId) {
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '创建搭建对话失败')
      }
    }
  }, [activeModel, activeProviderId, applyConversation, refreshSidebar, restoreStreamingPreview, selectedProject?.id, syncConversationRoute])

  const handleApplyAssistant = useCallback(async (assistantId: string | null) => {
    if (!currentConversation) return
    const conversationId = currentConversation.id
    try {
      const updated = await chatApi.updateConversation(conversationId, {
        assistantId: assistantId ?? '',
      })
      applyConversationIfCurrent(conversationId, updated)
      refreshSidebar()
      if (assistantId) void refreshContextStats(updated.id)
    } catch (err) {
      console.error('Failed to update conversation assistant:', err)
      setStreamErrorForConversation(
        conversationId,
        typeof err === 'string' ? err : (err as Error).message || '助手切换失败',
      )
    }
  }, [applyConversationIfCurrent, currentConversation, refreshContextStats, refreshSidebar, setStreamErrorForConversation])

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
    const startingConversationId = currentConversationIdRef.current
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
    refreshSidebar()
    if (currentConversationIdRef.current === startingConversationId) {
      currentConversationIdRef.current = conversation.id
      applyConversation(conversation)
      syncConversationRoute(conversation.id)
    }
    return conversation
  }, [activeModel, activeProviderId, applyConversation, currentConversation, draftAgentRuntime, refreshSidebar, selectedProject?.id, selectedProject?.name, selectedSet?.id, syncConversationRoute])

  const handleAgentPlanModeChange = useCallback(async (mode: AgentPlanMode) => {
    const startingConversationId = currentConversationIdRef.current
    let targetConversationId = currentConversation?.id ?? null
    try {
      const conversation = await ensureConversationForAgentPlan()
      targetConversationId = conversation.id
      const updated = await chatApi.setAgentPlanMode(conversation.id, mode)
      applyConversationIfCurrent(conversation.id, updated)
      void refreshContextStats(updated.id)
      refreshSidebar()
    } catch (err) {
      console.error('Failed to update agent plan mode:', err)
      if (targetConversationId) {
        setStreamErrorForConversation(
          targetConversationId,
          typeof err === 'string' ? err : (err as Error).message || 'Plan 模式切换失败',
        )
      } else if (currentConversationIdRef.current === startingConversationId) {
        setStreamError(typeof err === 'string' ? err : (err as Error).message || 'Plan 模式切换失败')
      }
    }
  }, [applyConversationIfCurrent, currentConversation?.id, ensureConversationForAgentPlan, refreshContextStats, refreshSidebar, setStreamErrorForConversation])

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
    const startingConversationId = currentConversationIdRef.current
    if (!trimmed && attachments.length === 0) return false
    if (!options.forceNewConversation && sendDisabledReason) {
      const targetId = options.conversationOverride?.id ?? currentConversationIdRef.current
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
        if (currentConversationIdRef.current === startingConversationId) {
          currentConversationIdRef.current = conversation.id
          applyConversation(conversation)
          syncConversationRoute(conversation.id)
        }
      } catch (err) {
        console.error('Failed to create conversation before send:', err)
        if (currentConversationIdRef.current === startingConversationId) {
          setStreamError(typeof err === 'string' ? err : (err as Error).message || '创建对话失败')
        }
        return false
      }
    }

    // 草稿运行时只落到「尚无消息」的会话（欢迎页选好 Agent → 首次发送建会话的场景）。已有消息的
    // 会话以其自身运行时为准：draft 在切换会话/重启后可能是陈旧的（如初始 BUILTIN），无条件回写
    // 会被后端「一 agent 一对话」绑定校验（check_runtime_switch_allowed）拒绝，把正常发送卡死。
    // 空会话判定与后端放行条件一致。
    if (
      (conversation.messages?.length ?? 0) === 0
      && !agentRuntimesEqual(normalizeAgentRuntime(conversation), draftAgentRuntime)
    ) {
      try {
        conversation = await chatApi.setAgentRuntime(conversation.id, draftAgentRuntime)
        applyConversationIfCurrent(conversation.id, conversation)
      } catch (err) {
        console.error('Failed to apply agent runtime before send:', err)
        setStreamErrorForConversation(
          conversation.id,
          typeof err === 'string' ? err : (err as Error).message || 'Agent 切换失败',
        )
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
          applyConversationIfCurrent(conversation.id, conversation)
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
          applyConversationIfCurrent(conversation.id, conversation)
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
          applyConversationIfCurrent(conversation.id, conversation)
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
          applyConversationIfCurrent(conversation.id, conversation)
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
          applyConversationIfCurrent(conversation.id, conversation)
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
    snapshot.messageId = null
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
    options.onAccepted?.()
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
    const attachmentSkillId = usesChatRuntime
      ? null
      : options.forceNewConversation
        ? inferSingleAttachmentSkillId(attachments, enabledSkills)
        : effectiveSkillId ?? inferSingleAttachmentSkillId(attachments, enabledSkills)

    let persistedConversation: Conversation | null = null
    let sendAccepted = false
    try {
      const updatedConv = await chatApi.sendMessage(
        conversationId,
        trimmed,
        attachments,
        attachmentSkillId,
      )
      persistedConversation = updatedConv
      sendAccepted = true
      if (currentConversationIdRef.current === conversationId) {
        applyAssistantStreamStats(updatedConv)
        setPendingUserMessage(null)
        setPendingUserMessageConversationId(null)
        // 原地替换而非移除：行不消失，SwapTitle 在标题文字变化时播放替换过渡。
        settleOptimisticConversationListItem(
          setOptimisticSidebarConversations,
          conversationId,
          updatedConv,
        )
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
      // 失败但保留了会话（带用户消息）→ 原地替换保持行存在；彻底失败 → 移除。
      settleOptimisticConversationListItem(
        setOptimisticSidebarConversations,
        conversationId,
        keptConversation ?? null,
      )
      if (keptConversation) refreshSidebar()
      const message = typeof err === 'string' ? err : (err as Error).message || '发送失败'
      setStreamErrorForConversation(conversationId, message)
      if (!freezeStreamSnapshot(conversationId)) clearStreamSnapshot(conversationId)
      // 用户消息已落盘 → 草稿清掉是对的；否则 InputBar 必须把原文回填，不能 return true。
      sendAccepted = Boolean(keptConversation)
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
        // 这一轮正常收尾 → 发出排队里的下一条（只发一条，它自己再跑一轮）。
        // 报错 / 用户按停止的 run 走不到这里：那时队列条目留着，由用户决定要不要发。
        void messageQueueRef.current.drain(persistedConversation)
      } else if (!(await flushPendingStreamDone(conversationId))) {
        if (!freezeStreamSnapshot(conversationId)) clearStreamSnapshot(conversationId)
      }
    }
    return sendAccepted
  }, [
    activeModel,
    activeProviderId,
    applyAssistantStreamStats,
    applyConversation,
    applyConversationIfCurrent,
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
    usesChatRuntime,
    ensureStreamSnapshot,
    finishStreamingRunWithConversation,
    flushPendingStreamDone,
    freezeStreamSnapshot,
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

  /** 设置 → 插件「让 AI 代装」：取规范 brief → 回聊天 → 新开对话并自动发送安装任务 */
  const handleRequestPluginAiInstall = useCallback(async (pluginId: string) => {
    const startingConversationId = currentConversationIdRef.current
    const brief = await api.pluginsInstallBrief(pluginId)
    if (currentConversationIdRef.current !== startingConversationId) return
    setExtensionsNavItem(null)
    setSettingsExiting(false)
    setChatView('conversation')
    setAssistantStreamStatsByMessageId({})
    let targetConversationId: string | null = null
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
      targetConversationId = conv.id
      refreshSidebar()
      if (currentConversationIdRef.current === startingConversationId) {
        currentConversationIdRef.current = conv.id
        applyConversation(conv)
        restoreStreamingPreview(conv.id)
        syncConversationRoute(conv.id)
      }
      const accepted = await handleSendMessageRef.current(brief.userMessage, [], {
        forceNewConversation: false,
        conversationOverride: conv,
      })
      if (!accepted) {
        throw new Error('发送安装任务失败（可能当前模型未配置或正在生成）')
      }
    } catch (err) {
      console.error('Failed to start plugin install chat:', err)
      if (
        currentConversationIdRef.current === startingConversationId
        || currentConversationIdRef.current === targetConversationId
      ) {
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '无法开始插件安装对话')
      }
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
    const startingConversationId = currentConversationIdRef.current
    try {
      const conversation = await chatApi.importExternalConversation(
        messages,
        attachmentPaths,
        activeProviderId || undefined,
        activeModel || undefined,
        selectedProject?.id ?? null,
      )
      refreshSidebar()
      if (currentConversationIdRef.current === startingConversationId) {
        currentConversationIdRef.current = conversation.id
        applyConversation(conversation)
        syncConversationRoute(conversation.id)
      }
      return true
    } catch (err) {
      console.error('Failed to import external conversation:', err)
      if (currentConversationIdRef.current === startingConversationId) {
        setStreamError(typeof err === 'string' ? err : (err as Error).message || '导入对话失败')
      }
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

  // 运行中的消息队列（Codex 式排队 + 立刻引导）。同上用 ref 转发 handleSendMessage，
  // 保持 drain 的身份稳定（它被 handleSendMessage 自己的 finally 调用，不能互相拖依赖）。
  const messageQueue = useMessageQueue({
    onSendMessage: (content, attachments, options) =>
      handleSendMessageRef.current(content, attachments, options),
    onRestoreToComposer: (message) => insertTextIntoComposer(message.content),
  })
  const messageQueueRef = useRef(messageQueue)
  messageQueueRef.current = messageQueue

  const currentQueuedMessages = currentConversation
    ? messageQueue.queued[currentConversation.id] ?? NO_QUEUED_MESSAGES
    : NO_QUEUED_MESSAGES
  // 「立刻引导」能不能给入口，取决于这一轮由谁在跑：
  //   - 内置 agent 循环 → 能（轮首注入，见 chat/agent/steering.rs）；
  //   - 外部 CLI → 看它的协议支不支持（`supportsSteering`，后端 RuntimeAgentDef 是唯一真源）。
  //     codex 有 `turn/steer`；pi 有 RPC `steer`；dsh 有 bridge `session/steer`。
  //     claude 的 stream-json 输入是顺序处理的、ACP 只有 prompt/cancel。
  //   - 多模型一问多答 → 一律不给：同会话 N 条并发 run，按 conversation 键的信箱定不到某条臂。
  const activeExternalAgentSupportsSteering = useMemo(() => {
    const agentId = activeAgentRuntime.externalAgentId
    if (!agentId) return false
    const agent = detectedExternalAgents.find((item) => item.id === agentId)
    return Boolean(agent?.supportsSteering ?? agent?.supports_steering)
  }, [activeAgentRuntime.externalAgentId, detectedExternalAgents])
  const activeExternalAgentSupportsFollowUp = useMemo(() => {
    const agentId = activeAgentRuntime.externalAgentId
    if (!agentId) return false
    const agent = detectedExternalAgents.find((item) => item.id === agentId)
    return Boolean(agent?.supportsFollowUp ?? agent?.supports_follow_up)
  }, [activeAgentRuntime.externalAgentId, detectedExternalAgents])
  const canSteerCurrentConversation =
    (usesExternalRuntime ? activeExternalAgentSupportsSteering : true)
    && activeReplyModels.length < 2
  // 自动 follow-up：内置循环终答后续跑；Pi / dsh 走原生下一轮。多模型一问多答同样不给。
  const canFollowUpCurrentConversation =
    (usesExternalRuntime ? activeExternalAgentSupportsFollowUp : true)
    && activeReplyModels.length < 2

  const handleQueueMessage = useCallback((content: string, attachments: PendingAttachment[]) => {
    const conversation = currentConversationRef.current
    if (!conversation) return
    const message = messageQueueRef.current.enqueue(conversation.id, content, attachments)
    if (message && canFollowUpCurrentConversation) {
      void messageQueueRef.current.followUp(conversation, message.id)
    }
  }, [canFollowUpCurrentConversation])

  const handleSteerQueuedMessage = useCallback((messageId: string) => {
    const conversationId = currentConversationIdRef.current
    if (!conversationId) return
    void messageQueueRef.current.steer(conversationId, messageId)
  }, [])

  const handleRemoveQueuedMessage = useCallback((messageId: string) => {
    const conversationId = currentConversationIdRef.current
    if (!conversationId) return
    messageQueueRef.current.remove(conversationId, messageId)
  }, [])

  const handleRestoreQueuedMessage = useCallback((messageId: string) => {
    const conversationId = currentConversationIdRef.current
    if (!conversationId) return
    messageQueueRef.current.restoreToComposer(conversationId, messageId)
  }, [])

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
      applyConversationIfCurrent(conversation.id, updated)
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
    applyConversationIfCurrent,
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
        applyConversationIfCurrent(conv.id, updated)
        refreshSidebar()
      } catch (err) {
        console.error('Failed to update message:', err)
        setStreamErrorForConversation(
          conv.id,
          typeof err === 'string' ? err : (err as Error).message || '保存失败',
        )
      }
    },
    [applyConversationIfCurrent, refreshSidebar, setStreamErrorForConversation],
  )

  const handleDeleteMessage = useCallback(
    async (messageId: string) => {
      const conv = currentConversationRef.current
      if (!conv) return
      if (!window.confirm('确定删除这条消息吗？')) return
      try {
        const updated = await chatApi.deleteMessage(conv.id, messageId)
        if (applyConversationIfCurrent(conv.id, updated)) {
          setAssistantStreamStatsByMessageId((prev) => {
            const next = { ...prev }
            delete next[messageId]
            return next
          })
        }
        refreshSidebar()
      } catch (err) {
        console.error('Failed to delete message:', err)
        setStreamErrorForConversation(
          conv.id,
          typeof err === 'string' ? err : (err as Error).message || '删除失败',
        )
      }
    },
    [applyConversationIfCurrent, refreshSidebar, setStreamErrorForConversation],
  )

  // 一键 rewind（「回到这里」）：截掉这条提问及其之后的所有消息，原文塞回输入框，用户改完再自己发。
  // 破坏性且不可撤销 → 先 confirm（与删除消息同一把关）。
  const handleRewindMessage = useCallback(
    async (messageId: string) => {
      const conv = currentConversationRef.current
      if (!conv) return
      if (!window.confirm('回到这里？这条提问及其之后的所有消息会被删除，原文放回输入框。')) return
      try {
        const { conversation, content } = await chatApi.rewindToMessage(conv.id, messageId)
        const applied = applyConversationIfCurrent(conv.id, conversation)
        if (applied) {
          setAssistantStreamStatsByMessageId({})
          setStreamError('')
          insertTextIntoComposer(content)
        }
        refreshSidebar()
        // 上下文用量后台补算（后端 rewind 故意不算，见那边注释）：几秒的 MCP 列表不该挡住 UI。
        void refreshContextStats(conversation.id)
      } catch (err) {
        console.error('Failed to rewind conversation:', err)
        setStreamErrorForConversation(
          conv.id,
          typeof err === 'string' ? err : (err as Error).message || '回到这里失败',
        )
      }
    },
    [applyConversationIfCurrent, refreshContextStats, refreshSidebar, setStreamErrorForConversation],
  )

  // 对话分支（方案 B）：在某条消息处建分支——把该消息及之前的消息复制进新对话，
  // 立即打开新对话（不自动发送）。源对话只读、不受影响。
  const handleForkMessage = useCallback(
    async (messageId: string) => {
      const conv = currentConversationRef.current
      if (!conv) return
      const startingConversationId = conv.id
      try {
        const forked = await chatApi.forkConversation(conv.id, messageId)
        refreshSidebar()
        if (currentConversationIdRef.current === startingConversationId) {
          setAssistantStreamStatsByMessageId({})
          currentConversationIdRef.current = forked.id
          applyConversation(forked)
          restoreStreamingPreview(forked.id)
          syncConversationRoute(forked.id)
          setStreamError('')
        }
      } catch (err) {
        console.error('Failed to fork conversation:', err)
        setStreamErrorForConversation(
          conv.id,
          typeof err === 'string' ? err : (err as Error).message || '建分支失败',
        )
      }
    },
    [applyConversation, refreshSidebar, restoreStreamingPreview, setStreamErrorForConversation, syncConversationRoute],
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
        setStreamErrorForConversation(
          conv.id,
          typeof err === 'string' ? err : (err as Error).message || '选中失败',
        )
      }
    },
    [applyConversationMeta, setStreamErrorForConversation],
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
        if (!freezeStreamSnapshot(conversationId)) clearStreamSnapshot(conversationId)
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
          if (!freezeStreamSnapshot(conversationId)) clearStreamSnapshot(conversationId)
        }
      }
    },
    [applyAssistantStreamStats, applyConversation, clearConversationInFlight, clearStreamSnapshot, ensureStreamSnapshot, finishStreamingRunWithConversation, flushPendingStreamDone, freezeStreamSnapshot, markConversationInFlight, refreshSidebar, reloadConversation, resetLocalCancellation, setStreamErrorForConversation, syncGeneratingConversationIds],
  )

  const handleRuntimeChange = useCallback(async (runtime: AgentRuntimeConfig) => {
    setDraftAgentRuntime(runtime)
    saveLastAgentRuntime(runtime)
    if (!currentConversation) return
    const conversationId = currentConversation.id
    try {
      const updated = await chatApi.setAgentRuntime(conversationId, runtime)
      applyConversationIfCurrent(conversationId, updated)
    } catch (err) {
      console.error('Failed to change agent runtime:', err)
      setStreamErrorForConversation(
        conversationId,
        typeof err === 'string' ? err : (err as Error).message || 'Agent 切换失败',
      )
    }
  }, [applyConversationIfCurrent, currentConversation, setStreamErrorForConversation])

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

  const handleExternalPresetChange = useCallback(async (preset: string) => {
    const next: AgentRuntimeConfig = {
      ...activeAgentRuntime,
      kind: 'external',
      externalAgentPreset: preset,
    }
    await handleRuntimeChange(next)
  }, [activeAgentRuntime, handleRuntimeChange])

  const persistApprovedExternalSandbox = useCallback(async (
    conversationId: string,
    runtime: AgentRuntimeConfig,
    sandbox: string,
  ) => {
    const next: AgentRuntimeConfig = {
      ...runtime,
      kind: 'external',
      externalSandbox: sandbox,
    }
    try {
      const updated = await chatApi.setAgentRuntime(conversationId, next)
      if (applyConversationIfCurrent(conversationId, updated)) {
        setDraftAgentRuntime(next)
        saveLastAgentRuntime(next)
      }
    } catch (error) {
      console.error('Failed to persist the post-approval permission mode:', error)
      setStreamErrorForConversation(
        conversationId,
        typeof error === 'string' ? error : (error as Error).message || '权限模式保存失败',
      )
    }
  }, [applyConversationIfCurrent, setStreamErrorForConversation])

  // 底栏胶囊选档：本地 CLI 写沙盒档位；内置 Agent 写 Act/Plan/Orchestrate；Chat 运行时无胶囊。
  const handleComposerModeChange = useCallback(async (value: string) => {
    if (usesExternalRuntime) {
      await handleExternalSandboxChange(value)
      return
    }
    await handleAgentPlanModeChange(value as AgentPlanMode)
  }, [handleAgentPlanModeChange, handleExternalSandboxChange, usesExternalRuntime])

  const handleModelChange = useCallback(async (providerId: string, model: string) => {
    setDraftProviderId(providerId)
    setDraftModel(model)
    saveLastModel(providerId, model) // 记住为全局默认

    if (!currentConversation) return
    const conversationId = currentConversation.id

    try {
      const updatedConv = await chatApi.updateConversation(conversationId, {
        providerId,
        model,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to change model:', err)
      setStreamErrorForConversation(
        conversationId,
        typeof err === 'string' ? err : (err as Error).message || '模型切换失败',
      )
    }
  }, [applyConversationMeta, currentConversation, setStreamErrorForConversation])

  const handleThinkingLevelChange = useCallback(async (level: ThinkingLevel | null) => {
    setDraftThinkingLevel(level)
    saveLastThinkingLevel(level) // 记住为全局默认，不再回落到 high
    if (!currentConversation) return
    const conversationId = currentConversation.id
    try {
      const updatedConv = await chatApi.updateConversation(conversationId, {
        thinkingLevel: level,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to change thinking level:', err)
      setStreamErrorForConversation(
        conversationId,
        typeof err === 'string' ? err : (err as Error).message || '思考等级切换失败',
      )
    }
  }, [applyConversationMeta, currentConversation, setStreamErrorForConversation])

  // 会话级三态联网搜索（任务 07-23）：设置模式,持久化到会话(欢迎页先存草稿),
  // 并记住为全局默认——之后所有新会话/未显式设置的会话自动沿用(与思考等级同款)。
  const handleSetWebSearchMode = useCallback(async (mode: WebSearchMode) => {
    setDraftWebSearchMode(mode)
    saveLastWebSearchMode(mode)
    if (!currentConversation) return
    const conversationId = currentConversation.id
    try {
      const updatedConv = await chatApi.updateConversation(conversationId, {
        webSearchMode: mode,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to change web search mode:', err)
      setStreamErrorForConversation(
        conversationId,
        typeof err === 'string' ? err : (err as Error).message || '联网搜索模式切换失败',
      )
    }
  }, [applyConversationMeta, currentConversation, setStreamErrorForConversation])

  // 多模型一问多答（任务 06-30 / D2）：变更多答模型集，持久化到会话（欢迎页先存草稿）。
  // 上限 4 由 UI 侧约束；这里直落 chatApi.updateConversation({ replyModels })。
  const handleChangeReplyModels = useCallback(async (models: ModelRef[]) => {
    setDraftReplyModels(models)
    if (!currentConversation) return
    const conversationId = currentConversation.id
    try {
      const updatedConv = await chatApi.updateConversation(conversationId, {
        replyModels: models,
      })
      applyConversationMeta(updatedConv)
    } catch (err) {
      console.error('Failed to update reply models:', err)
      setStreamErrorForConversation(
        conversationId,
        typeof err === 'string' ? err : (err as Error).message || '多答模型更新失败',
      )
    }
  }, [applyConversationMeta, currentConversation, setStreamErrorForConversation])

  const handleChangeKnowledgeBaseIds = useCallback(async (ids: string[]) => {
    // the draft is applied when the conversation is created on first send.
    setDraftKnowledgeBaseIds(ids)
    if (!currentConversation) return
    const conversationId = currentConversation.id
    try {
      const updatedConv = await chatApi.updateConversation(conversationId, {
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
    const conversationId = currentConversation.id
    try {
      const updatedConv = await chatApi.updateConversation(conversationId, {
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

  // 输入栏是聊天主区里除 MessageList 外最大的常驻子树。把它的 slot 和对象值稳定下来，
  // 配合 InputBar 自身的 memo，侧栏/设置路由等无关状态变化不会再让输入栏重跑整棵树。
  const composerCurrentAssistant = useMemo(
    () => currentAssistantSnapshot
      ? { id: currentAssistantSnapshot.id, name: currentAssistantSnapshot.name }
      : null,
    [currentAssistantSnapshot],
  )
  const composerKnowledgeBaseIds = useMemo(
    () => currentConversation
      ? (currentConversation.knowledge_base_ids ?? currentConversation.knowledgeBaseIds ?? [])
      : draftKnowledgeBaseIds,
    [
      currentConversation,
      draftKnowledgeBaseIds,
    ],
  )
  const composerForceKnowledgeSearch = currentConversation
    ? (currentConversation.force_knowledge_search ?? currentConversation.forceKnowledgeSearch ?? false)
    : draftForceKnowledgeSearch
  const composerContextSlot = useMemo(
    () => (
      <ContextIndicator
        contextState={contextState}
        messageCount={displayMessages.length}
        loading={contextLoading}
        compressing={contextCompressing}
        error={contextError}
        usesExternalRuntime={usesExternalRuntime}
        onRefresh={handleRefreshContext}
        onCompress={handleCompressContext}
        lang={uiLang}
      />
    ),
    [
      contextCompressing,
      contextError,
      contextLoading,
      contextState,
      displayMessages.length,
      handleCompressContext,
      handleRefreshContext,
      uiLang,
      usesExternalRuntime,
    ],
  )
  const composerUsageSlot = useMemo(
    () => (
      <SessionUsageStrip
        messages={displayMessages}
        lang={uiLang}
        apiFormats={providerApiFormats}
        defaultApiFormat={currentConversation ? (providerApiFormats[currentConversation.provider_id] ?? '') : ''}
        cacheIncludedInInput={
          usesExternalRuntime
            ? activeAgentRuntime.externalAgentId === 'codex'
            : undefined
        }
      />
    ),
    [
      activeAgentRuntime.externalAgentId,
      currentConversation,
      displayMessages,
      providerApiFormats,
      uiLang,
      usesExternalRuntime,
    ],
  )

  const emptyHeroGreeting = useMemo(
    () => ({
      key: emptyHeroGreetingKey,
      text: pickRandomChatEmptyGreeting(),
    }),
    [emptyHeroGreetingKey],
  )

  const setSidebarCollapsedPersisted = useCallback((collapsed: boolean) => {
    const finish = measureChatSurface(
      'sidebar-collapse',
      document.querySelector('.chat-window-shell'),
      collapsed ? 'collapsed' : 'expanded',
    )
    setSidebarCollapsed(collapsed)
    rememberChatSidebarCollapsed(collapsed)
    if (typeof requestAnimationFrame === 'function') {
      requestAnimationFrame(() => requestAnimationFrame(finish))
    } else {
      finish()
    }
  }, [])

  // ---------- Right Dock ----------
  const [dockOpen, setDockOpen] = useState(() => getRememberedDockOpen())
  const [dockWidth, setDockWidth] = useState(() => getRememberedDockWidth())
  const [dockTab, setDockTab] = useState<DockTab>(() => getRememberedDockTab())
  const [dockWorkdir, setDockWorkdir] = useState('')
  const [treeExpanded, setTreeExpanded] = useState<string[]>([])
  const [dockReveal, setDockReveal] = useState<DockRevealRequest>(null)
  const [dockPreview, setDockPreview] = useState<DockPreviewRequest>(null)
  const piNativeEnabled = usesExternalRuntime
    && activeAgentRuntime.externalAgentId === 'pi'
    && Boolean(currentConversation?.id)
  const trajectoryLive = dockOpen && dockTab === 'trajectory'
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

  useEffect(() => {
    const prev = skillProjectCwdRef.current
    skillProjectCwdRef.current = dockWorkdir
    if (prev !== dockWorkdir) void loadSkills()
  }, [dockWorkdir, loadSkills])

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

  // 标题栏后台任务状态灯：展开 Dock 并切到任务页。
  const handleOpenDockTasks = useCallback(() => {
    setDockTab('tasks')
    rememberDockTab('tasks')
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

  // 工具卡片点文件名 → dock 查看器预览。workdir 内的路径同时在树里定位；
  // workdir 外的绝对路径（如写到桌面的文件）用其所在目录作查看器根。
  useEffect(
    () =>
      onDockPreviewRequest((rawPath) => {
        const normalize = (value: string) => value.replace(/\\/g, '/').replace(/^\/\/\?\//, '')
        const target = normalize(rawPath.trim())
        if (!target) return
        const wd = normalize(dockWorkdir)
        const isAbsolute = /^(?:[a-zA-Z]:)?\//.test(target)
        let request: { workdir: string; path: string } | null = null
        let revealRel: string | null = null
        if (isAbsolute) {
          if (wd && target.toLowerCase().startsWith(`${wd.toLowerCase()}/`)) {
            revealRel = target.slice(wd.length + 1)
            request = { workdir: dockWorkdir, path: revealRel }
          } else {
            const idx = target.lastIndexOf('/')
            if (idx > 0) request = { workdir: target.slice(0, idx), path: target.slice(idx + 1) }
          }
        } else if (dockWorkdir) {
          revealRel = target.replace(/^\.\//, '')
          request = { workdir: dockWorkdir, path: revealRel }
        }
        if (!request) return
        setDockTab('files')
        rememberDockTab('files')
        setDockOpen(true)
        rememberDockOpen(true)
        if (revealRel) setDockReveal((prev) => ({ path: revealRel, nonce: (prev?.nonce ?? 0) + 1 }))
        const next = request
        setDockPreview((prev) => ({ kind: 'file', ...next, nonce: (prev?.nonce ?? 0) + 1 }))
      }),
    [dockWorkdir],
  )

  // 工具卡片点 +N -N 徽标 → dock 侧栏渲染整份带色 diff。
  useEffect(
    () =>
      onDockDiffPreviewRequest((payload) => {
        setDockTab('files')
        rememberDockTab('files')
        setDockOpen(true)
        rememberDockOpen(true)
        setDockPreview((prev) => ({ kind: 'diff', ...payload, nonce: (prev?.nonce ?? 0) + 1 }))
      }),
    [],
  )

  // claude 交计划（ExitPlanMode）→ dock 侧栏渲染整份计划。审批卡里那块 `max-h-40` 的
  // 灰框只够扫一眼，而「批不批这个计划」是要读完才能决定的。
  useEffect(
    () =>
      onDockMarkdownPreviewRequest((payload) => {
        setDockTab('files')
        rememberDockTab('files')
        setDockOpen(true)
        rememberDockOpen(true)
        setDockPreview((prev) => ({ kind: 'markdown', ...payload, nonce: (prev?.nonce ?? 0) + 1 }))
      }),
    [],
  )

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

  const handleSidebarSelectConversation = useCallback((
    id: string,
    conversation?: ConversationListItem | ConversationSearchHit,
    scope?: ConversationSelectionScope,
  ) => {
    const focusMessageId =
      conversation && 'match_message_id' in conversation
        ? conversation.match_message_id ?? conversation.matchMessageId ?? undefined
        : conversation && 'matchMessageId' in conversation
          ? conversation.matchMessageId ?? undefined
          : undefined
    runAfterLeavingSettings(() => {
      // 跨项目/集点击必须是一次原子导航。这里仅更新导航上下文，不调用
      // handleSelectProject/handleSelectSet（两者会清空会话并写 #chat）。
      if (scope) {
        setSelectedProject(scope.project)
        setSelectedSet(scope.set)
      }
      void handleSelectConversation(id, {
        messageCount: conversation?.message_count,
        focusMessageId: focusMessageId || undefined,
      })
    })
  }, [handleSelectConversation, runAfterLeavingSettings])

  const handlePiConversationChanged = useCallback((
    id: string,
    conversation?: Conversation,
    draft?: string,
  ) => {
    refreshSidebar()
    if (conversation) {
      currentConversationIdRef.current = id
      applyConversation(conversation)
      setChatView('conversation')
      syncConversationRoute(id)
    } else {
      handleSidebarSelectConversation(id)
    }
    if (draft?.trim()) {
      requestAnimationFrame(() => insertTextIntoComposer(draft))
    }
  }, [applyConversation, handleSidebarSelectConversation, refreshSidebar, syncConversationRoute])
  const handleSidebarNewConversation = useCallback(() => {
    runAfterLeavingSettings(() => void handleNewConversation())
  }, [handleNewConversation, runAfterLeavingSettings])

  const handleSidebarConversationDeleted = useCallback(() => {
    forgetRememberedChatRoute()
    applyConversation(null)
    // 对话库/中心页删当前会话时只清会话态，别写 #chat 把中心页冲掉
    const path = hashPath()
    if (getRouteConversationId() !== null || path === 'chat' || path === '') {
      syncConversationRoute(null)
    }
    refreshSidebar()
  }, [applyConversation, refreshSidebar, syncConversationRoute])

  const handleSidebarForceDropConversation = useCallback((id: string) => {
    // B3：侧栏删除时强制清掉该会话的 in-flight/快照/乐观项，
    // 使乐观合并不再保留它（删"generating"会话也能立即从侧栏消失）。
    dropConversationLocally(id)
  }, [dropConversationLocally])

  // 侧栏真实列表 refetch 落地 → 剪掉不再生成中的乐观条目。乐观项的生命期是
  // 「发送 → settle（原地换成模型标题，SwapTitle 播打字机）→ 下一次 refetch 接管」：
  // settle 时不能立即剪（refetch 未落地、新会话在真实列表里还没有，剪了行就卸载一帧、
  // 重建后打字机不播）；refetch 落地后真实条目已就位（同 key 无缝接管），或该会话已被
  // 归档/删除（不该再并回）——两种情况都该剪。仍在 generating 的保留（长跑 run 期间
  // 任何无关刷新不得把乐观标题打回「新对话」）。
  const handleSidebarConversationsLoaded = useCallback(() => {
    setOptimisticSidebarConversations((prev) => {
      const next = prev.filter((item) => generatingConversationIdsRef.current.has(item.id))
      return next.length === prev.length ? prev : next
    })
  }, [])

  const settingsPanelActive = chatView === 'settings' && extensionsNavItem === null

  const handleSidebarOpenExtensionsItem = useCallback((item: ExtensionsNavItem) => {
    // 设置页开着时点扩展项：先走退场（会 flush 自动保存），否则设置页被硬切走、动画不播。
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

  // 侧栏账户菜单：语言切换 / 检查更新 / 用量。都是全局行为，所以留在 Chat 这层，
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

  const handleSidebarOpenUsage = useCallback(() => {
    setExtensionsNavItem(null)
    openEmbeddedSettings('usage')
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
  // 六个中心页共用 key="center"：React 复用同一个 div，入场动画只在「从会话页进来」时跑一次。
  // 各页各自 key 的话每次互切都是新节点 → 重播 opacity 0→1，中间几帧透出背景，就是那下闪。
  const centerPageClass = `chat-motion-view-in chat-center-page relative flex min-h-0 min-w-0 flex-1 flex-col ${centerPagePadTop}`

  // 会话页顶栏控件。非 mac 渲染进全宽标题栏带（单行 chrome），mac 仍留在主区 52px 顶栏。
  // 抽成变量而非组件：依赖十余个 Chat 局部状态与回调，拆组件只会换来一长串 props。
  const conversationTitlebarControls = useMemo(() => (
    <>
      <div className="flex min-w-0 items-center gap-1">
        <div className="shrink-0" data-tauri-drag-region="false">
          <RuntimePicker
            agentRuntime={activeAgentRuntime}
            onRuntimeChange={handleRuntimeChange}
            conversationId={currentConversation?.id}
            locked={
              // 一 agent 一对话：有消息后锁死 kind/agent（内置 Kivio 与本地 CLI 一律）。
              !!currentConversation &&
              (currentConversation.messages?.length ?? 0) > 0
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
            approvalPolicy={approvalPolicy}
            onApprovalPolicyChange={handleApprovalPolicyChange}
          />
        </div>
        {!usesChatRuntime && (
          <div className="shrink-0" data-tauri-drag-region="false">
            <BackgroundJobsIndicator
              conversationId={currentConversation?.id ?? null}
              onOpen={handleOpenDockTasks}
            />
          </div>
        )}
      </div>
      <div className="min-w-5 flex-1" data-tauri-drag-region />
      {!usesChatRuntime && (
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
        </div>
      )}
    </>
  ), [
    activeAgentRuntime,
    activeModel,
    activeProviderId,
    approvalPolicy,
    currentConversation,
    dockOpen,
    draftThinkingLevel,
    handleApprovalPolicyChange,
    handleExternalModelChange,
    handleModelChange,
    handleOpenDockTasks,
    handleRuntimeChange,
    handleThinkingLevelChange,
    handleToggleDock,
    uiLang,
    usesChatRuntime,
    usesExternalRuntime,
  ])

  const handleTitlebarToggleSidebar = useCallback(() => {
    if (sidebarCollapsed) setSidebarCollapsedPersisted(false)
    else handleCollapseSidebar()
  }, [handleCollapseSidebar, setSidebarCollapsedPersisted, sidebarCollapsed])
  const handleTitlebarNewConversation = useCallback(() => {
    runAfterLeavingSettings(() => void handleNewConversation())
  }, [handleNewConversation, runAfterLeavingSettings])
  const handleDismissHookWarning = useCallback(() => setHookWarning(null), [])
  const handleCloseImageViewer = useCallback(() => setImageViewerItem(null), [])

  const inputBarProps = useMemo<InputBarProps>(() => ({
    onSend: handleSendMessage,
    onQueue: handleQueueMessage,
    disabled: isCurrentConversationBusy(),
    onCancel: handleCancelStream,
    cancelVisible: streamCoarse.streaming,
    cancelling: streamCoarse.cancelling,
    onOpenSettings: handleOpenChatSettings,
    onOpenTools: openSkillCenter,
    onNewChat: handleNewConversation,
    onCompactContext: handleCompressContext,
    onClearChat: handleClearChat,
    enabledTools,
    toolsDisabledReason,
    toolStatusHint,
    sendDisabledReason,
    agentPlanState: currentConversation?.agent_plan_state ?? currentConversation?.agentPlanState ?? null,
    agentTodoState: currentConversation?.agent_todo_state ?? currentConversation?.agentTodoState ?? null,
    onAgentPlanModeChange: handleAgentPlanModeChange,
    usesChatRuntime,
    enabledSkills: usesChatRuntime ? [] : slashSkills,
    onOpenSkillSettings: openSkillCenter,
    selectedProject,
    conversationProject,
    onSelectProject: handleSidebarSelectProject,
    showProjectEntry: true,
    selectedSet,
    onSelectSet: handleSidebarSelectSet,
    currentAssistant: composerCurrentAssistant,
    onOpenAssistantCenter: openAssistantCenter,
    onSelectAssistant: handleSelectAssistant,
    autoFocus: true,
    usesExternalRuntime,
    externalAgentName: activeAgentRuntime.externalAgentId ?? null,
    conversationId: currentConversation?.id ?? null,
    knowledgeBaseIds: composerKnowledgeBaseIds,
    onChangeKnowledgeBaseIds: handleChangeKnowledgeBaseIds,
    forceKnowledgeSearch: composerForceKnowledgeSearch,
    onToggleForceKnowledgeSearch: handleToggleForceKnowledgeSearch,
    mcpServers,
    onToggleMcpServer: handleToggleMcpServer,
    webSearchMode: activeWebSearchMode,
    onSetWebSearchMode: handleSetWebSearchMode,
    builtinWebSearchSupported: activeBuiltinWebSearchSupported,
    replyModels: activeReplyModels,
    onChangeReplyModels: handleChangeReplyModels,
    contextSlot: composerContextSlot,
    gitWorkdir: usesChatRuntime ? null : dockWorkdir || null,
    gitLang: uiLang,
    onOpenGitPanel: handleOpenDockGit,
    modeOptions: composerModes.options,
    modeValue: composerModes.current,
    onModeChange: handleComposerModeChange,
    presetOptions: composerPresets.options,
    presetValue: composerPresets.current,
    onPresetChange: handleExternalPresetChange,
    presetLocked: Boolean(currentConversation) && !currentConversationIsBlank,
    presetLockedReason: i18n[uiLang].chatAgentPresetLocked,
    usageSlot: composerUsageSlot,
  }), [
    activeAgentRuntime.externalAgentId,
    activeBuiltinWebSearchSupported,
    activeReplyModels,
    activeWebSearchMode,
    composerModes,
    composerPresets,
    composerContextSlot,
    composerCurrentAssistant,
    composerForceKnowledgeSearch,
    composerKnowledgeBaseIds,
    composerUsageSlot,
    conversationProject,
    currentConversation,
    currentConversationIsBlank,
    dockWorkdir,
    enabledTools,
    handleAgentPlanModeChange,
    handleCancelStream,
    handleChangeKnowledgeBaseIds,
    handleChangeReplyModels,
    handleClearChat,
    handleCompressContext,
    handleComposerModeChange,
    handleExternalPresetChange,
    handleOpenChatSettings,
    handleOpenDockGit,
    handleQueueMessage,
    handleSelectAssistant,
    handleSendMessage,
    handleSetWebSearchMode,
    handleSidebarSelectProject,
    handleSidebarSelectSet,
    handleToggleForceKnowledgeSearch,
    handleToggleMcpServer,
    handleNewConversation,
    isCurrentConversationBusy,
    mcpServers,
    openAssistantCenter,
    openSkillCenter,
    selectedProject,
    selectedSet,
    slashSkills,
    streamCoarse,
    toolsDisabledReason,
    toolStatusHint,
    sendDisabledReason,
    uiLang,
    usesChatRuntime,
    usesExternalRuntime,
  ])

  const messageListProps = useMemo<MessageListProps>(() => ({
    conversationId: currentConversation?.id,
    messages: displayMessages,
    renderRequestId: conversationRenderRequestId,
    onInitialRender: handleConversationFirstCommit,
    agentPlanState: currentConversation?.agent_plan_state ?? currentConversation?.agentPlanState ?? null,
    assistantStreamStatsByMessageId,
    onUpdateMessage: handleUpdateMessage,
    onRegenerateMessage: handleRegenerateMessage,
    onForkMessage: handleForkMessage,
    onRewindMessage: handleRewindMessage,
    onDeleteMessage: handleDeleteMessage,
    onSaveMessageToNote: handleSaveMessageToNote,
    onRetryLastUser: handleRegenerateMessage,
    onExecuteAgentPlan: handleExecuteAgentPlan,
    groupSelections: currentConversation?.group_selections ?? currentConversation?.groupSelections ?? {},
    onSetGroupSelection: handleSetGroupSelection,
    contextState,
    compactionInProgress: contextCompressing,
    animateCompactionBoundaryId: animateCompactionBoundaryId,
    lang: uiLang,
    focusMessageId,
    onFocusMessageHandled: () => setFocusMessageId(null),
  }), [
    animateCompactionBoundaryId,
    assistantStreamStatsByMessageId,
    contextCompressing,
    contextState,
    currentConversation,
    conversationRenderRequestId,
    displayMessages,
    handleConversationFirstCommit,
    handleDeleteMessage,
    handleExecuteAgentPlan,
    handleForkMessage,
    handleRegenerateMessage,
    handleRewindMessage,
    handleSaveMessageToNote,
    handleSetGroupSelection,
    handleUpdateMessage,
    uiLang,
    focusMessageId,
  ])

  const forkOrigin = useMemo(() => {
    const origin = currentConversation?.forked_from ?? currentConversation?.forkedFrom
    if (!origin) return null
    const sourceId = origin.conversation_id ?? origin.conversationId
    return sourceId ? { sourceId, title: origin.title } : null
  }, [currentConversation])

  const pendingSlot = useMemo(() => (
    (pendingToolConfirm || pendingSessionConsent || pendingUserPrompt) ? (
    <div className="shrink-0 px-6">
      <div className="mx-auto w-full max-w-4xl">
        {pendingUserPrompt && pendingUserPromptRecord && (
          <AskUserBlock
            variant="docked"
            toolCall={pendingUserPromptRecord}
            onResolved={() => dismissPendingUserPrompt(
              pendingUserPrompt.conversationId,
              pendingUserPrompt.toolCallId,
            )}
          />
        )}
        {pendingToolConfirm && (
          <ApprovalCard
            title={toolApprovalTitle(pendingToolConfirm)}
            subtitle={`${pendingToolConfirm.source}${pendingToolConfirm.serverId ? ` · ${pendingToolConfirm.serverId}` : ''}`}
            detail={pendingToolConfirm.argumentsPreview}
            error={toolConfirmError}
            actions={isPlanApproval(pendingToolConfirm)
              ? [
                {
                  label: '拒绝 / 让它改',
                  disabled: toolConfirmSubmittingId === pendingToolConfirm.toolCallId,
                  onSelect: () => { void resolvePendingToolConfirm(false) },
                },
                ...PLAN_APPROVAL_ACTIONS.map((action, index) => ({
                  label: action.label,
                  primary: index === PLAN_APPROVAL_ACTIONS.length - 1,
                  hint: index === PLAN_APPROVAL_ACTIONS.length - 1 ? 'Ctrl+↵' : undefined,
                  disabled: toolConfirmSubmittingId === pendingToolConfirm.toolCallId,
                  onSelect: () => {
                    void resolvePendingToolConfirm(true, false, action.mode).then((accepted) => {
                      if (accepted) {
                        return persistApprovedExternalSandbox(
                          pendingToolConfirm.conversationId,
                          activeAgentRuntime,
                          action.mode,
                        )
                      }
                    })
                  },
                })),
              ]
              : isEnterPlanApproval(pendingToolConfirm)
                ? [
                  {
                    label: '不用，直接做',
                    disabled: toolConfirmSubmittingId === pendingToolConfirm.toolCallId,
                    onSelect: () => { void resolvePendingToolConfirm(false) },
                  },
                  {
                    label: '总是允许',
                    disabled: toolConfirmSubmittingId === pendingToolConfirm.toolCallId,
                    onSelect: () => {
                      void resolvePendingToolConfirm(true, true).then((accepted) => {
                        if (accepted) {
                          return persistApprovedExternalSandbox(
                            pendingToolConfirm.conversationId,
                            activeAgentRuntime,
                            'plan',
                          )
                        }
                      })
                    },
                  },
                  {
                    label: '进入计划模式',
                    primary: true,
                    hint: 'Ctrl+↵',
                    disabled: toolConfirmSubmittingId === pendingToolConfirm.toolCallId,
                    onSelect: () => {
                      void resolvePendingToolConfirm(true).then((accepted) => {
                        if (accepted) {
                          return persistApprovedExternalSandbox(
                            pendingToolConfirm.conversationId,
                            activeAgentRuntime,
                            'plan',
                          )
                        }
                      })
                    },
                  },
                ]
                : [
                  {
                    label: '拒绝',
                    disabled: toolConfirmSubmittingId === pendingToolConfirm.toolCallId,
                    onSelect: () => { void resolvePendingToolConfirm(false) },
                  },
                  {
                    label: '总是允许',
                    disabled: toolConfirmSubmittingId === pendingToolConfirm.toolCallId,
                    onSelect: () => { void resolvePendingToolConfirm(true, true) },
                  },
                  {
                    label: '允许一次',
                    primary: true,
                    hint: 'Ctrl+↵',
                    disabled: toolConfirmSubmittingId === pendingToolConfirm.toolCallId,
                    onSelect: () => { void resolvePendingToolConfirm(true) },
                  },
                ]}
          />
        )}
        {pendingSessionConsent && (
          <ApprovalCard
            title="允许本次会话使用文件和命令工具？"
            subtitle="授权后，本会话内 Kivio 可读写、删除磁盘上的任意文件并执行任意终端命令（包括项目目录之外）。仅本次会话有效，重启后需重新授权。"
            error={sessionConsentError}
            actions={[
              {
                label: '拒绝',
                disabled: sessionConsentSubmittingConversationId === pendingSessionConsent.conversationId,
                onSelect: () => { void resolvePendingSessionConsent(false) },
              },
              {
                label: '允许本次会话',
                primary: true,
                hint: 'Ctrl+↵',
                disabled: sessionConsentSubmittingConversationId === pendingSessionConsent.conversationId,
                onSelect: () => { void resolvePendingSessionConsent(true) },
              },
            ]}
          />
        )}
      </div>
    </div>
    ) : null
  ), [
    activeAgentRuntime,
    dismissPendingUserPrompt,
    pendingSessionConsent,
    pendingToolConfirm,
    pendingUserPrompt,
    pendingUserPromptRecord,
    persistApprovedExternalSandbox,
    resolvePendingSessionConsent,
    resolvePendingToolConfirm,
    sessionConsentError,
    sessionConsentSubmittingConversationId,
    toolConfirmError,
    toolConfirmSubmittingId,
  ])

  return (
    <LangContext.Provider value={uiLang}>
    <Profiler id="ChatShell" onRender={onChatPerfProfiler}>
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
          onToggleSidebar={handleTitlebarToggleSidebar}
          onNewConversation={handleTitlebarNewConversation}
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
        <ChatSidebarPane
          onRender={onChatPerfProfiler}
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
          onConversationsLoaded={handleSidebarConversationsLoaded}
          onOpenExtensionsItem={handleSidebarOpenExtensionsItem}
          onOpenSettings={handleSidebarOpenSettings}
          onSelectLang={handleSidebarSelectLang}
          onCheckUpdate={handleSidebarCheckUpdate}
          onOpenUsage={handleSidebarOpenUsage}
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

        <ChatRouteKeepAlive
          activeKey={chatView === 'conversation' || chatView === 'settings' ? chatView : 'center'}
        >
        {chatView === 'onboarding' ? (
          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            <OnboardingShell
              onComplete={handleOnboardingExit}
              onSkip={handleOnboardingExit}
              onSettingsChange={onSettingsChange}
            />
          </div>
        ) : chatView === 'settings' ? (
          <ChatSettingsPane
            settingsRef={settingsRef}
            exiting={settingsExiting}
            className={`flex min-h-0 min-w-0 flex-1 flex-col${
              !usesNativeTitlebar && settingsPanelActive ? ' settings-embedded-under-strip' : ''
            }`}
            initialTab={settingsInitialTab}
            reserveTrafficLightSpace={(sidebarCollapsed || extensionsNavItem === null) && usesNativeTitlebar}
            onClose={handleSettingsClose}
            onSettingsChange={handleSettingsChange}
            onReady={emitContentReady}
            onRequestPluginAiInstall={handleRequestPluginAiInstall}
            onRender={onChatPerfProfiler}
          />
        ) : chatView === 'assistants' ? (
          <div key="center" className={centerPageClass}>
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
          <div key="center" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <SkillCenter
                onSkillsChanged={() => void loadSkills()}
                projectCwd={dockWorkdir || undefined}
              />
            </Suspense>
          </div>
        ) : chatView === 'mcp' ? (
          <div key="center" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <McpCenter />
            </Suspense>
          </div>
        ) : chatView === 'knowledge' ? (
          <div key="center" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <KnowledgeCenter />
            </Suspense>
          </div>
        ) : chatView === 'notes' ? (
          <div key="center" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <NotesCenter />
            </Suspense>
          </div>
        ) : chatView === 'sessions' ? (
          <div key="center" className={centerPageClass}>
            {centerPageTopStrip}
            <Suspense fallback={null}>
              <SessionCenter
                lang={uiLang}
                currentConversationId={currentConversation?.id}
                generatingConversationIds={generatingConversationIds}
                onSelectConversation={handleSidebarSelectConversation}
                onConversationDeleted={handleSidebarConversationDeleted}
                onForceDropConversation={handleSidebarForceDropConversation}
                onConversationsChanged={refreshSidebar}
              />
            </Suspense>
          </div>
        ) : (
          <ChatConversationPane
            titlebarControls={conversationTitlebarControls}
            usesNativeTitlebar={usesNativeTitlebar}
            sidebarCollapsed={sidebarCollapsed}
            titlebarRowClass={chatTitlebarRowClass}
            titlebarMacInsetClass={chatTitlebarMacInsetClass}
            onToggleSidebar={handleTitlebarToggleSidebar}
            onNewConversation={handleTitlebarNewConversation}
            protocolVersionMismatch={protocolVersionMismatch}
            showEmptyHero={showEmptyHero}
            currentAssistantName={currentAssistantSnapshot?.name ?? null}
            selectedProjectName={selectedProject?.name ?? null}
            selectedSetName={selectedSet?.name ?? null}
            emptyHeroGreeting={emptyHeroGreeting}
            inputBarProps={inputBarProps}
            messageListProps={messageListProps}
            hookWarning={hookWarning}
            currentConversationId={currentConversation?.id ?? null}
            onDismissHookWarning={handleDismissHookWarning}
            forkOrigin={forkOrigin}
            onSelectConversation={handleSelectConversation}
            importedHistoryStale={importedHistoryStale}
            pendingSlot={pendingSlot}
            queuedMessages={currentQueuedMessages}
            canSteerQueuedMessages={canSteerCurrentConversation}
            onSteerQueuedMessage={handleSteerQueuedMessage}
            onRemoveQueuedMessage={handleRemoveQueuedMessage}
            onRestoreQueuedMessage={handleRestoreQueuedMessage}
            lang={uiLang}
            imageViewerItem={imageViewerItem}
            onCloseImageViewer={handleCloseImageViewer}
            onRender={onChatPerfProfiler}
          />
        )}
        </ChatRouteKeepAlive>
        {chatView === 'conversation' && !usesChatRuntime && (
          <RightDock
            open={dockOpen}
            width={dockWidth}
            activeTab={dockTab}
            workdir={dockWorkdir}
            lang={uiLang}
            conversationId={currentConversation?.id ?? null}
            conversation={trajectoryLive ? currentConversation : null}
            messages={trajectoryLive ? displayMessages : NO_TRAJECTORY_MESSAGES}
            piNativeEnabled={trajectoryLive && piNativeEnabled}
            treeExpanded={treeExpanded}
            revealRequest={dockReveal}
            previewRequest={dockPreview}
            onToggleTab={handleDockTabChange}
            onWidthChange={handleDockWidthChange}
            onClose={handleCloseDock}
            onTreeExpandedChange={handleTreeExpandedChange}
            onInsertMention={handleInsertFileMention}
            onPiConversationChanged={handlePiConversationChanged}
            onFocusMessage={setFocusMessageId}
            onRevealInTree={handleDockRevealInTree}
          />
        )}
      </div>
      </div>
    </Profiler>
    </LangContext.Provider>
  )
}
