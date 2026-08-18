import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { ChevronDown, RotateCw } from 'lucide-react'
import {
  defaultRangeExtractor,
  observeElementRect as observeVirtualRect,
  useVirtualizer,
  type Range,
  type ReactVirtualizerOptions,
} from '@tanstack/react-virtual'
import type { AgentPlanState, ChatMessage, ConversationContextState, DegradedAnswer } from './types'
import { MessageBubble } from './MessageBubble'
import { DegradedAnswerCard } from './DegradedAnswerCard'
import { MessageGroup } from './MessageGroup'
import { MessageNavigator } from './ChatMessageNavigator'
import { MessageContextMenu, type MessageMenuAnchor } from './MessageContextMenu'
import { AddSelectionToChat } from './AddSelectionToChat'
import { copyToClipboard } from '../utils/clipboard'
import { CompactionDivider } from './CompactionDivider'
import { CompactionInProgress } from './CompactionInProgress'
import { CompactionSummaryPanel } from './CompactionSummaryPanel'
import { resolveCompactionBoundaries, resolvePendingCompactionAfterIndex, type CompactionBoundaryView } from './compactionBoundary'
import { isExecutableAgentPlanText } from './agentPlan'
import { foldMessageGroups } from './messageGroups'
import {
  activeMessageNavigatorNodeId,
  buildMessageNavigatorNodes,
  visibleMessageNavigatorNodeIds,
  type MessageNavigatorNode,
} from './messageNavigator'
import { useStreamCoarse, useStreamSnapshot } from './streamingStore'
import { StreamStatusLine } from './StreamStatusLine'
import { getActiveGroup, useGroupVersion } from './groupStreamingStore'
import { useScrollFollow } from './scroll/useScrollFollow'
import {
  canReuseLiveRowHeight,
  chatMessageLayoutRevision,
  contentRevision,
  estimateMessageRenderHeight,
  estimateMessageRenderCost,
  getCachedRowMeasurement,
  layoutScopedVirtualKey,
  measureChatVirtualRow,
  sendReserveHeight,
  restoreMeasurementSnapshot,
  saveMeasurementSnapshot,
  setCachedRowMeasurement,
  shouldAdjustChatItemSizeChange,
} from './messageListVirtualization'
import type { Lang } from '../settings/i18n'
import { measureChatSurface, recordChatPerfSample, useChatPerfRenderProbe } from './chatPerformanceProbe'
import {
  beginMessageNavigationHydrate,
  beginStreamSettleEagerHydrate,
  endMessageNavigationHydrate,
  resetMessageNavigationStore,
} from './messageNavigationStore'
import { createLiveRowModel } from './liveRowModel'


export interface AssistantStreamStats {
  messageId: string
  tokensPerSec: number
  reasoningDurationMs?: number | null
  reasoningDurationMsBySegmentId?: Record<string, number>
}

export interface MessageListProps {
  conversationId?: string | null
  messages: ChatMessage[]
  renderRequestId?: number
  onInitialRender?: (conversationId: string, requestId: number) => void
  agentPlanState?: AgentPlanState | null
  assistantStreamStatsByMessageId?: Record<string, AssistantStreamStats>
  onUpdateMessage?: (messageId: string, content: string) => Promise<void>
  onRegenerateMessage?: (messageId: string, newContent?: string) => Promise<void>
  onForkMessage?: (messageId: string) => Promise<void>
  onRewindMessage?: (messageId: string) => Promise<void>
  onDeleteMessage?: (messageId: string) => Promise<void>
  onSaveMessageToNote?: (messageId: string) => Promise<boolean>
  onExecuteAgentPlan?: (messageId: string) => Promise<void> | void
  // 失败发送后线程末尾留下的孤儿用户消息：点「重试」用它的 id 重新生成。
  onRetryLastUser?: (messageId: string) => void
  // 多模型一问多答（任务 06-30）：多答组「选中条」映射 + 点选回调。
  groupSelections?: Record<string, string>
  onSetGroupSelection?: (groupId: string, messageId: string) => void
  contextState?: ConversationContextState | null
  compactionInProgress?: boolean
  animateCompactionBoundaryId?: string | null
  lang?: Lang
  /** 全局搜索：打开会话后滚到该消息并短暂高亮。 */
  focusMessageId?: string | null
  onFocusMessageHandled?: () => void
}

const LIST_EDGE_PADDING_PX = 16

// 内容宽度量化桶。layoutKey 里带着 contentWidth：若用原始 px，拖侧栏/Dock/改窗口宽时
// 每变 1px 就换一个 key 空间 —— TanStack itemSizeCache 全 miss（所有行退回估算高、
// totalSize 猛变、滚动位置跳），且 measurementBuckets 只留 8 个桶，一次拖动扫过上百个
// 宽度值会把原宽度的桶也挤掉。旧实现（virtua 时代）有 MIGRATION_STEP 量化，TanStack
// 重写时丢了（8791000）。行高对 32px 内的宽度差不敏感（换行差 ~4 个拉丁字符），
// 真实高度由 measureElement 兜底。
const CONTENT_WIDTH_BUCKET_PX = 32

// 导航器高亮同步的最小间隔。这趟同步是 querySelectorAll + 逐行 getBoundingClientRect，
// 若 virtualizer 在同一帧里刚写过 DOM，第一下 gBCR 就是整文档强制 reflow——每帧跑一次
// 正是滚动不顺滑的主因之一。高亮不需要 120fps，8 次/秒足够。
const NAVIGATOR_SYNC_INTERVAL_MS = 120
// 导航跳转后等目标行挂载 + 重内容 hydrate + 高度稳定的上限。
// 估算 → 实测 的纠正通常 1~3 帧；代码块同步 hydrate 后偶发再撑一帧。
const NAVIGATOR_SETTLE_MAX_FRAMES = 48
// 准备阶段：目标行自身高度稳定帧数。
const NAVIGATOR_SETTLE_STABLE_FRAMES = 3
// 跳转后 paint 前钉住：邻居重测 + 图片/异步块还会改 translateY。
const NAVIGATOR_HOLD_MAX_FRAMES = 28
const NAVIGATOR_HOLD_STABLE_FRAMES = 4
// 与 virtualizer overscan 对齐，跳转后首屏邻居尽量已测过高。
const NAVIGATOR_FORCE_MOUNT_RADIUS = 6
// 收尾再锁几帧，挡住 force-mount 拆除后的迟到测高。
const NAVIGATOR_UNLOCK_FRAMES = 10
// 只认 heavy island；data-chat-markdown-pending 从未写入，留着只会制造「假覆盖」。
const NAVIGATOR_PENDING_SELECTOR = '[data-chat-heavy-hydrated="false"], [data-chat-async-pending="true"]'
const NAVIGATOR_ALIGN_EPSILON_PX = 1
// 会话切换遮罩：重内容一直晃也不能无限等，超时强制揭开。
const OPEN_SETTLE_MAX_MS = 2_000




// 列表里每一项的统一形态。整条会话全量喂给虚拟列表（消息都在内存，virtualizer 只渲可见项），
// 屏外的气泡连同其 KaTeX host / Markdown / 图片 DOM 真正从 DOM 卸载。
type RenderItem =
  | { kind: 'spacer'; key: 'padding-top' | 'padding-bottom'; size: number }
  | { kind: 'message'; key: string; message: ChatMessage; sentModels?: GroupModelLabel[] }
  | { kind: 'group'; key: string; groupId: string; messages: ChatMessage[] }
  | { kind: 'live-group'; key: string; groupId: string }
  | { kind: 'streaming'; key: string; message: ChatMessage; messageStreaming: boolean; reasoningStreaming: boolean }
  | { kind: 'error'; key: 'error'; text: string; retryMessageId: string | null }
  | { kind: 'tail'; key: 'tail' }
  | { kind: 'compaction-divider'; key: string; boundary: CompactionBoundaryView; animate: boolean }
  | { kind: 'compaction-summary'; key: string; boundary: CompactionBoundaryView }
  | { kind: 'compaction-progress'; key: string; afterIndex: number }

function measurementKey(item: RenderItem): string {
  if (item.kind === 'message') {
    return `${item.key}:${chatMessageLayoutRevision(item.message)}`
  }
  if (item.kind === 'group') {
    return `${item.key}:${item.messages.map((message) => [
      message.id,
      message.provider_id ?? message.providerId ?? '',
      message.model ?? '',
      chatMessageLayoutRevision(message),
    ].join(':')).join('|')}`
  }
  if (item.kind === 'streaming') {
    return `${item.key}:${chatMessageLayoutRevision(item.message)}`
  }
  return item.key
}

function virtualItemIdentity(item: RenderItem): string {
  // Stable TanStack/React identity. Geometry changes go through measureChatVirtualRow
  // and the measurement cache; putting the revision in getItemKey remounts settled rows.
  return item.key
}

function streamErrorDegraded(error: string): DegradedAnswer {
  const normalized = error.toLowerCase()
  const kind: DegradedAnswer['kind'] =
    normalized.includes('stream_read_error')
      || normalized.includes('timeout')
      || normalized.includes('timed out')
      || normalized.includes('连接')
      || normalized.includes('网络')
      ? 'timeout'
      : normalized.includes('context') || normalized.includes('上下文')
        ? 'context_overflow'
        : normalized.includes('rate') || normalized.includes('quota') || normalized.includes('限流')
          ? 'rate_limited'
          : 'unknown'
  const reason = kind === 'timeout'
    ? '模型流式响应中途断开。'
    : '回复生成失败。'
  return { kind, reason, detail: error, text: reason }
}

// R8（多模型一问多答）：多答组的「本次所发模型」列表，渲染在该组对应 user 消息顶部。
type GroupModelLabel = { providerId: string | null; model: string | null }

function MessageListBase({
  conversationId,
  messages,
  renderRequestId = 0,
  onInitialRender,
  agentPlanState = null,
  assistantStreamStatsByMessageId = {},
  onUpdateMessage,
  onRegenerateMessage,
  onForkMessage,
  onRewindMessage,
  onDeleteMessage,
  onSaveMessageToNote,
  onExecuteAgentPlan,
  onRetryLastUser,
  groupSelections = {},
  onSetGroupSelection,
  contextState = null,
  compactionInProgress = false,
  animateCompactionBoundaryId = null,
  lang = 'zh',
  focusMessageId = null,
  onFocusMessageHandled,
}: MessageListProps) {
  useChatPerfRenderProbe('MessageList', {
    conversationId,
    messages: messages.length,
  })
  // 流式预览状态直接订阅 streamingStore——只有本组件随每帧内容重渲，Chat/侧栏/输入栏不动。
  const coarse = useStreamCoarse()
  const snapshot = useStreamSnapshot()
  // 多答组实时流：订阅 group store 版本号，活跃组列内容更新时驱动重渲（仅需订阅，值本身不用）。
  useGroupVersion(conversationId)
  const liveGroup = conversationId ? getActiveGroup(conversationId) : undefined
  // Group column objects are mutated in place for every stream delta. Only model
  // identity changes should rebuild historical rows; content deltas stay in the
  // dedicated live-group row.
  const liveGroupModelsKey = liveGroup
    ? `${liveGroup.groupId}\0${liveGroup.columns
      .map((column) => `${column.providerId ?? ''}:${column.model ?? ''}`)
      .join('\0')}`
    : ''
  const liveGroupId = liveGroup?.groupId ?? null
  const liveGroupColumns = liveGroup?.columns
  const liveGroupModels = useMemo(() => (
    liveGroupId && liveGroupColumns
      ? {
        key: liveGroupModelsKey,
        groupId: liveGroupId,
        labels: liveGroupColumns.map((column) => ({
          providerId: column.providerId,
          model: column.model,
        })),
      }
      : null
  ), [liveGroupColumns, liveGroupId, liveGroupModelsKey])
  const streaming = coarse.streaming
  const streamFrozen = coarse.streamFrozen
  // live → 历史 的同帧：先开短窗 eager，再让本帧新挂的 DeferredCodeBlock 读到 flag。
  // 必须在 render 期同步调用，useEffect 会晚一帧，首挂仍走 180ms 延迟。
  const liveRowActive = streaming || streamFrozen
  const prevLiveRowActiveRef = useRef(liveRowActive)
  const liveEndingThisFrame = prevLiveRowActiveRef.current && !liveRowActive
  if (liveEndingThisFrame) {
    beginStreamSettleEagerHydrate()
  }
  prevLiveRowActiveRef.current = liveRowActive
  // Last measured outside-live height; filled every streaming layout, consumed on settle seed.
  const liveBubbleHeightRef = useRef(0)
  const lastLiveMessageRef = useRef<ChatMessage | null>(null)

  const error = coarse.streamError
  const streamingContent = snapshot.content
  const streamingReasoning = snapshot.reasoning
  const streamingReasoningDurationMs = snapshot.reasoningDurationMs
  const streamingReasoningDurationMsBySegmentId = snapshot.reasoningDurationMsBySegmentId
  const reasoningStreaming = snapshot.reasoningStreaming
  const streamingToolCalls = snapshot.toolCalls
  const streamingSegments = snapshot.segments

  // 恢复中的 Kivio Agent 会同时有两份状态：后端为崩溃恢复写入的 interrupted
  // assistant 草稿，以及协议快照驱动的实时预览。它们共用同一个 messageId；实时
  // 气泡存在时，历史侧不能再把同 id 的消息挂出来，否则整轮回答会显示两次。
  const historyMessages = useMemo(() => {
    if (!streaming && !streamFrozen) return messages
    const activeMessageIds = new Set<string>()
    if (snapshot.messageId) activeMessageIds.add(snapshot.messageId)
    if (liveGroup && (streaming || streamFrozen)) {
      for (const column of liveGroup.columns) {
        if (!column.messageId.startsWith('pending-')) activeMessageIds.add(column.messageId)
      }
    }
    if (activeMessageIds.size === 0) return messages
    return messages.filter((message) => !activeMessageIds.has(message.id))
  }, [liveGroup, messages, snapshot.messageId, streamFrozen, streaming])

  // Stable live keys for the in-list experiment + twin estimate identity on settle.
  // Default external path still aliases so the history twin reuses the live key
  // for measurement cache continuity (DOM is not reused across the outside→inside handoff).
  const liveRowModelRef = useRef(createLiveRowModel())
  const liveRowModelConversationRef = useRef<string | null | undefined>(conversationId)
  if (liveRowModelConversationRef.current !== conversationId) {
    liveRowModelRef.current.reset()
    liveRowModelConversationRef.current = conversationId
    liveBubbleHeightRef.current = 0
    lastLiveMessageRef.current = null
  }
  const historyAssistantIds = useMemo(
    () => messages.filter((message) => message.role === 'assistant').map((message) => message.id),
    [messages],
  )
  const historyGroupIds = useMemo(() => {
    const ids: string[] = []
    for (const message of messages) {
      const groupId = message.group_id ?? message.groupId
      if (groupId && message.role === 'assistant' && !ids.includes(groupId)) ids.push(groupId)
    }
    return ids
  }, [messages])
  const liveRowModel = liveRowModelRef.current
  const { liveKey: liveRowKey } = liveRowModel.sync({
    conversationId,
    liveActive: liveRowActive,
    liveGroupId: liveGroup && liveRowActive ? liveGroup.groupId : null,
    preferredTwinId: snapshot.messageId ?? null,
    historyAssistantIds,
    historyGroupIds,
  })

  const scrollRef = useRef<HTMLDivElement | null>(null)
  // hook 需要通过 state 拿到元素以便重新绑定监听；virtualizer 需要 RefObject。回调 ref 同时喂两者。
  const [viewportEl, setViewportEl] = useState<HTMLDivElement | null>(null)
  const [contentEl, setContentEl] = useState<HTMLDivElement | null>(null)
  // 初值取常见聊天列宽落进的桶（704 = 22×32），首个 RO tick 会立刻校正。
  const [contentWidth, setContentWidth] = useState(704)
  const setScrollEl = useCallback((el: HTMLDivElement | null) => {
    scrollRef.current = el
    setViewportEl(el)
  }, [])

  const committedRenderRequestRef = useRef(0)
  useLayoutEffect(() => {
    if (
      !contentEl
      || !conversationId
      || renderRequestId <= 0
      || committedRenderRequestRef.current === renderRequestId
    ) return
    let cancelled = false
    let readyRaf: number | null = null
    let previousHeight = -1
    let stableFrames = 0
    const startedAt = performance.now()

    const completeNow = () => {
      committedRenderRequestRef.current = renderRequestId
      onInitialRender?.(conversationId, renderRequestId)
    }

    const completeIfReady = (): boolean | null => {
      // 超时硬揭开：病理高度抖动 / 异常 pending 不能让遮罩永远盖住。
      if (performance.now() - startedAt >= OPEN_SETTLE_MAX_MS) {
        completeNow()
        return true
      }

      // 仍有未就绪的 ChatHeavyIsland：高度会在稍后猛涨。
      if (contentEl.querySelector(NAVIGATOR_PENDING_SELECTOR)) {
        previousHeight = -1
        stableFrames = 0
        // null：停 rAF 轮询，等 MutationObserver 看到标记变化后再测。
        return null
      }

      // 重内容到位后，虚拟列表还可能在下一帧用真实 DOM 高度修正 itemSizeCache /
      // totalSize。至少连续两帧高度不变，才把覆盖层交还给正文。
      const height = contentEl.scrollHeight
      if (height === previousHeight) stableFrames += 1
      else {
        previousHeight = height
        stableFrames = 0
      }
      if (stableFrames < 2) return false

      completeNow()
      return true
    }
    const scheduleReadyCheck = () => {
      if (cancelled || readyRaf !== null) return
      readyRaf = requestAnimationFrame(() => {
        readyRaf = null
        if (completeIfReady() === false) scheduleReadyCheck()
      })
    }

    const observer = new MutationObserver(() => {
      scheduleReadyCheck()
    })
    observer.observe(contentEl, {
      attributes: true,
      attributeFilter: ['data-chat-heavy-hydrated', 'data-chat-async-pending'],
      childList: true,
      subtree: true,
    })
    // 超时兜底：即使 observer/rAF 路径卡住也要揭开。
    const timeoutId = window.setTimeout(() => {
      if (cancelled || committedRenderRequestRef.current === renderRequestId) return
      completeNow()
    }, OPEN_SETTLE_MAX_MS)
    scheduleReadyCheck()
    return () => {
      cancelled = true
      observer.disconnect()
      if (readyRaf !== null) cancelAnimationFrame(readyRaf)
      window.clearTimeout(timeoutId)
    }
  }, [contentEl, conversationId, onInitialRender, renderRequestId])


  useLayoutEffect(() => {
    if (!contentEl) return
    const finish = measureChatSurface(
      'conversation-visible',
      contentEl,
      conversationId ?? 'empty',
    )
    return finish
  }, [conversationId, contentEl])

  useLayoutEffect(() => {
    if (!contentEl) return
    const updateWidth = (width: number) => {
      // 量化到桶再落 state：拖动过程中只在跨桶时重渲/换 layoutKey（见 CONTENT_WIDTH_BUCKET_PX）。
      const next = Math.max(280, Math.round(width / CONTENT_WIDTH_BUCKET_PX) * CONTENT_WIDTH_BUCKET_PX)
      setContentWidth((current) => current === next ? current : next)
    }
    const rect = contentEl.getBoundingClientRect()
    updateWidth(Math.max(0, rect.width - 48))
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width
      if (typeof width === 'number') updateWidth(width)
    })
    observer.observe(contentEl)
    return () => observer.disconnect()
  }, [contentEl])
  const prevMessageCountRef = useRef(0)
  const [activeNavigatorNodeId, setActiveNavigatorNodeId] = useState<string | null>(null)
  const [visibleNavigatorNodeIds, setVisibleNavigatorNodeIds] = useState<string[]>([])
  const navigatorNodesRef = useRef<MessageNavigatorNode[]>([])
  const activeNavigatorNodeIdRef = useRef<string | null>(null)
  const visibleNavigatorNodeIdsRef = useRef<string[]>([])
  const navigatorSettleRafRef = useRef<number | null>(null)
  const navigatorSettleGenerationRef = useRef(0)
  // endNavigatorSession 的解锁 rAF 链；clear 时必须 cancel，避免卸载后 setState。
  const navigatorUnlockRafRef = useRef<number | null>(null)
  const navigatorUnlockGenerationRef = useRef(0)

  // 导航「先渲染再跳」：在当前视口不动的前提下，把目标行附近强制挂进 DOM 测高。
  const [forceMountRenderIndex, setForceMountRenderIndex] = useState<number | null>(null)
  const forceMountRenderIndexRef = useRef<number | null>(null)
  forceMountRenderIndexRef.current = forceMountRenderIndex
  // 导航全程锁：禁止 virtualizer 因测高改 scrollTop（那是上下抽的主因）。
  const navigationLockRef = useRef(false)
  // 准备阶段冻结的阅读位置；hold 跳转前绝不能被动挪走。
  const navigatorFrozenScrollTopRef = useRef<number | null>(null)
  // 跳转后 paint 前钉住。jumped=false 时只在 layout 里跳一次，避免 rAF 跳完再被测高扯。
  const navigatorHoldRef = useRef<{
    generation: number
    targetIndex: number
    frames: number
    stable: number
    jumped: boolean
    lastHeight: number
    lastTotalSize: number
  } | null>(null)
  // 「回到底部」落地 hold：代码块/重内容在底部挂载后撑高，需连续钉底到稳定。
  const bottomHoldRef = useRef<{
    generation: number
    frames: number
    stable: number
    lastScrollHeight: number
  } | null>(null)
  const [navigatorHoldEpoch, setNavigatorHoldEpoch] = useState(0)
  const [bottomHoldEpoch, setBottomHoldEpoch] = useState(0)
  // Bumped when bottomHold ends: send-reserve 的 minHeight→spacer 转移已在 settle 帧
  // 立即完成（见 apply() 注释），这里只是 hold 期间几何变化后的幂等重算兜底
  // （hold 中 hydrate/图片可能改 span，结束时再对一次账）。
  const [reserveEpoch, setReserveEpoch] = useState(0)
  const [navigatorLockActive, setNavigatorLockActive] = useState(false)



  // 消息区内置右键菜单（原生菜单被全局屏蔽，见 main.tsx）。
  const [msgMenu, setMsgMenu] = useState<
    { anchor: MessageMenuAnchor; selectionText: string; messageText: string | null } | null
  >(null)

  // 底部跟随：外置 live 时 document-flow 高度变化 → RO contentGrowth 钉底。
  // growthSignal 在 scrollHeight 真变且 RO 未投递时补一枪（jsdom）。
  const streamGrowthSignal = streaming || streamFrozen
    ? `${streamingContent.length}:${streamingReasoning.length}:${streamingToolCalls.length}:${streamingSegments.length}`
    : null
  const { handle: followHandle, following, showJumpButton } = useScrollFollow({
    viewport: viewportEl,
    content: contentEl,
    trackKeys: true,
    growthSignal: streamGrowthSignal,
  })

  // 流式期间是否在跟随：交接时以它为准（不能只看交接瞬间的 isFollowing——
  // 外置 live 卸载会先把高度砸矮，scroll 可能被误判成 user 解除跟随）。
  const streamFollowIntentRef = useRef(true)
  if (streaming || streamFrozen) {
    streamFollowIntentRef.current = following
  }



  const legacyPlanMessageId = useMemo(() => {
    const legacyPlan = agentPlanState?.plan?.trim()
    if (!isExecutableAgentPlanText(legacyPlan)) return null
    const hasMessagePlan = historyMessages.some((message) => Boolean(
      isExecutableAgentPlanText((message.agent_plan ?? message.agentPlan)?.plan),
    ))
    if (hasMessagePlan) return null
    return [...historyMessages]
      .reverse()
      .find((message) => message.role === 'assistant' && message.content.trim() === legacyPlan)
      ?.id ?? null
  }, [agentPlanState, historyMessages])

  const messageIndexById = useMemo(() => {
    const map = new Map<string, number>()
    messages.forEach((message, index) => map.set(message.id, index))
    return map
  }, [messages])

  const boundaries = useMemo(
    () => resolveCompactionBoundaries(messages, contextState),
    [contextState, messages],
  )

  const boundariesByAfterIndex = useMemo(() => {
    const map = new Map<number, CompactionBoundaryView[]>()
    for (const boundary of boundaries) {
      const existing = map.get(boundary.afterIndex) ?? []
      existing.push(boundary)
      map.set(boundary.afterIndex, existing)
    }
    return map
  }, [boundaries])

  const folded = useMemo(() => foldMessageGroups(historyMessages), [historyMessages])

  const pendingCompactionAfterIndex = useMemo(
    () => (
      compactionInProgress
        ? resolvePendingCompactionAfterIndex(messages, contextState, animateCompactionBoundaryId)
        : null
    ),
    [animateCompactionBoundaryId, compactionInProgress, contextState, messages],
  )

  const appendCompactionItems = useCallback((
    list: RenderItem[],
    afterIndex: number,
  ) => {
    const boundaries = boundariesByAfterIndex.get(afterIndex)
    if (!boundaries) return
    for (const boundary of boundaries) {
      const recordId = boundary.record.id
      list.push({
        kind: 'compaction-divider',
        key: `compaction-divider-${recordId}`,
        boundary,
        animate: animateCompactionBoundaryId === recordId,
      })
      list.push({
        kind: 'compaction-summary',
        key: `compaction-summary-${recordId}`,
        boundary,
      })
    }
  }, [animateCompactionBoundaryId, boundariesByAfterIndex])

  const appendCompactionSlot = useCallback((
    list: RenderItem[],
    afterIndex: number,
  ) => {
    const hasBoundary = boundariesByAfterIndex.has(afterIndex)
    if (
      compactionInProgress
      && pendingCompactionAfterIndex === afterIndex
      && !hasBoundary
    ) {
      list.push({
        kind: 'compaction-progress',
        key: `compaction-progress-after-${afterIndex}`,
        afterIndex,
      })
      return
    }
    appendCompactionItems(list, afterIndex)
  }, [
    appendCompactionItems,
    boundariesByAfterIndex,
    compactionInProgress,
    pendingCompactionAfterIndex,
  ])

  // Live content updates every token, but the **row key** is stable (liveRowKey).
  // Keeping live out of the historyItems useMemo deps means committed rows do not rebuild
  // per token — same split LiveAgent uses (history cache + live tail).
  const liveItem = useMemo<RenderItem | null>(() => {
    if (!liveRowKey) return null
    const hasLiveGroup = Boolean(liveGroup && liveRowActive)
    if (hasLiveGroup && liveGroup) {
      return { kind: 'live-group', key: liveRowKey, groupId: liveGroup.groupId }
    }
    const hasStreamingPreview =
      streamingContent || streamingReasoning || streamingToolCalls.length > 0 || streamingSegments.length > 0
    // Keep the live row mounted for the whole run even before the first token so the
    // key is claimed early and the settle twin can adopt it.
    if (!hasStreamingPreview && !liveRowActive) return null
    if (!liveRowActive) return null
    return {
      kind: 'streaming',
      key: liveRowKey,
      messageStreaming: streaming && !streamFrozen,
      reasoningStreaming: reasoningStreaming && !streamFrozen,
      message: {
        // Prefer real message id when known so context-menu / navigator can target it;
        // the virtualizer row key is still liveRowKey (not this id).
        id: snapshot.messageId || 'streaming-assistant',
        role: 'assistant',
        content: streamingContent,
        reasoning: streamingReasoning || undefined,
        artifacts: [],
        tool_calls: streamingToolCalls,
        segments: streamingSegments,
        timestamp: Math.floor(Date.now() / 1000),
      },
    }
  }, [
    liveGroup,
    liveRowActive,
    liveRowKey,
    reasoningStreaming,
    snapshot.messageId,
    streamFrozen,
    streaming,
    streamingContent,
    streamingReasoning,
    streamingSegments,
    streamingToolCalls,
  ])
  if (liveItem?.kind === 'streaming') {
    lastLiveMessageRef.current = liveItem.message
  } else if (liveItem?.kind === 'live-group') {
    // A live multi-answer group does not share the geometry of one settled message.
    lastLiveMessageRef.current = null
  }

  // 历史项只在消息/压缩边界/组模型身份变化时重建。高频流式文本不进入依赖；
  // live 行单独挂在 virtualizer 外的文档流尾部。
  const historyItems = useMemo<RenderItem[]>(() => {
    const list: RenderItem[] = [
      { kind: 'spacer', key: 'padding-top', size: LIST_EDGE_PADDING_PX },
    ]

    // 多模型一问多答（任务 06-30）：把同一 group_id 的连续 assistant 消息折成一个 group item，
    // 横向并排多列；其余消息线性 push（折叠逻辑是纯函数 foldMessageGroups，便于单测）。
    // R8：先收集 group_id → 本次所发模型列表，给该组对应 user 消息加模型标签行。
    const sentModelsByGroup = new Map<string, GroupModelLabel[]>()
    for (const item of folded) {
      if (item.type === 'group') {
        sentModelsByGroup.set(
          item.groupId,
          item.messages.map((m) => ({
            providerId: m.provider_id ?? m.providerId ?? null,
            model: m.model ?? null,
          })),
        )
      }
    }
    // 流式态下本组 assistant 尚未落库 → 从实时列补出模型列表，让 user 消息标签即时出现。
    if (
      liveGroupModels
      && liveGroupModels.labels.length > 0
      && !sentModelsByGroup.has(liveGroupModels.groupId)
    ) {
      sentModelsByGroup.set(
        liveGroupModels.groupId,
        liveGroupModels.labels,
      )
    }

    for (const item of folded) {
      if (item.type === 'group') {
        list.push({
          kind: 'group',
          key: liveRowModel.resolveGroupKey(item.groupId),
          groupId: item.groupId,
          messages: item.messages,
        })
        const boundaryIndices = new Set<number>()
        for (const message of item.messages) {
          const index = messageIndexById.get(message.id)
          if (index != null) boundaryIndices.add(index)
        }
        for (const index of boundaryIndices) {
          appendCompactionSlot(list, index)
        }
      } else {
        const message = item.message
        const groupId = message.role === 'user' ? (message.group_id ?? message.groupId ?? null) : null
        const sentModels = groupId ? sentModelsByGroup.get(groupId) : undefined
        list.push({ kind: 'message', key: liveRowModel.resolveMessageKey(message.id), message, sentModels })
        const index = messageIndexById.get(message.id)
        if (index != null) appendCompactionSlot(list, index)
      }
    }

    return list
  // liveRowKey / origins intentionally omitted: settle aliases are applied when
  // historyMessages/folded change (the twin re-enters history). Token frames must
  // not rebuild committed rows.
  }, [appendCompactionSlot, folded, liveGroupModels, liveRowModel, messageIndexById])

  // Live rides the chrome tail outside the virtualizer. Token growth only
  // moves scrollHeight → contentGrowth pin.
  const dynamicItem = liveItem

  const errorItem = useMemo<RenderItem | null>(() => {
    if (!error) return null
    const last = messages[messages.length - 1]
    const retryMessageId = last && last.role === 'user' ? last.id : null
    return { kind: 'error', key: 'error', text: error, retryMessageId }
  }, [error, messages])

  const layoutKey = `${conversationId ?? 'empty'}:${contentWidth}`
  const tailMeasurementKey = streaming || streamFrozen
    ? `tail:live:${snapshot.runId ?? snapshot.messageId ?? 'anonymous'}`
    : `tail:settled:${error ? contentRevision(error) : 'empty'}`
  const historyMeasurementRevision = useMemo(
    () => historyItems.map(measurementKey).join('|'),
    [historyItems],
  )
  const measurementRevision = `${historyMeasurementRevision}|tail=${tailMeasurementKey}`

  // 计算每一行的初始估算高度。真实高度由 TanStack Virtual 的 measureElement
  // 覆盖；估算只负责首次切换/首次滚动时快速建立窗口，不再把整份历史拆成两套 DOM。
  //
  // Settle frame: seed the twin's height into the row-measurement cache *before*
  // estimates are read, so the first virtualizer layout matches the outside
  // bubble (layoutEffect seed is one paint too late → height collapse flash).
  // Keep this side effect outside useMemo — memo must stay pure.
  if (liveEndingThisFrame && liveBubbleHeightRef.current > 0) {
    const settlingId = snapshot.messageId
      || [...messages].reverse().find((message) => message.role === 'assistant')?.id
      || null
    if (settlingId) {
      const settling = messages.find((message) => message.id === settlingId)
      const liveMessage = lastLiveMessageRef.current
      if (settling && liveMessage && canReuseLiveRowHeight(liveMessage, settling)) {
        const rid = liveRowModel.resolveMessageKey(settling.id)
        const h = Math.round(liveBubbleHeightRef.current)
        setCachedRowMeasurement(layoutKey, `${rid}:${chatMessageLayoutRevision(settling)}`, h)
      }
    }
  }
  const estimatedSizeByKey = useMemo(() => {
    const map = new Map<string, number>()
    for (const item of historyItems) {
      if (item.kind === 'spacer') {
        map.set(item.key, item.size)
        continue
      }
      const rowKey = measurementKey(item)
      const cached = getCachedRowMeasurement(layoutKey, rowKey)
      if (cached !== undefined) {
        map.set(item.key, cached)
        continue
      }
      const messages = item.kind === 'message'
        ? [item.message]
        : item.kind === 'group' ? item.messages
          : item.kind === 'streaming' ? [item.message]
            : []
      let cost = 0
      let height = 0
      for (const message of messages) {
        const textSegments = (message.segments ?? []).filter((segment) => segment.kind === 'text')
        const texts = textSegments.length > 0
          ? textSegments.map((segment) => segment.text ?? '')
          : [message.content ?? '']
        const toolCalls = message.tool_calls ?? message.toolCalls ?? []
        const artifactCount = (message.artifacts ?? []).length
          + toolCalls.reduce((sum, toolCall) => sum + (toolCall.artifacts ?? []).length, 0)
        cost += estimateMessageRenderCost({
          texts,
          toolCallCount: toolCalls.length,
          timelineSegmentCount: (message.segments ?? []).length,
          attachmentCount: (message.attachments ?? []).length,
          artifactCount,
        })
        height += estimateMessageRenderHeight({
          texts,
          width: contentWidth,
          toolCallCount: toolCalls.length,
          attachmentCount: (message.attachments ?? []).length,
          artifactCount,
        })
      }
      const base = item.kind === 'group' ? 180 : 88
      // Mount cost decides the virtualized window; row size must be estimated in
      // rendered pixels so a long answer does not begin hundreds of pixels short.
      map.set(item.key, Math.max(base, height + (cost > 800 ? 24 : 0)))
    }
    map.set('tail', getCachedRowMeasurement(layoutKey, tailMeasurementKey) ?? 96)
    // liveEndingThisFrame in deps: re-read cache after the settle-frame seed above.
    return map
    // eslint-disable-next-line react-hooks/exhaustive-deps -- liveEndingThisFrame 刻意入依赖：settle 帧种子写入后强制重建，重读缓存
  }, [contentWidth, historyItems, layoutKey, liveEndingThisFrame, tailMeasurementKey])

  // Live is not a virtualizer row — chrome tail below carries live +
  // status/error/send-reserve, so token growth never remeasures a combined tail.
  const itemCount = historyItems.length
  const historyItemsRef = useRef<RenderItem[]>(historyItems)
  historyItemsRef.current = historyItems
  const itemAt = useCallback((index: number) => historyItemsRef.current[index], [])
  const estimateSizeRef = useRef(estimatedSizeByKey)
  estimateSizeRef.current = estimatedSizeByKey
  const observeRect: ReactVirtualizerOptions<HTMLDivElement, HTMLDivElement>['observeElementRect'] = useCallback((instance, callback) => {
    if (import.meta.env.MODE === 'test') {
      callback({ width: 1024, height: viewportEl?.clientHeight || 800 })
      return undefined
    }
    return observeVirtualRect(instance, callback)
  }, [viewportEl])
  const initialMeasurementsCache = useMemo(
    () => restoreMeasurementSnapshot(conversationId, layoutKey, measurementRevision),
    [conversationId, layoutKey, measurementRevision],
  )
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: itemCount,
    enabled: true,
    getScrollElement: () => scrollRef.current,
    // Share scroll authority with follow pinning (source-classified writes).
    // LiveAgent keeps anchorTo:end always on; the follow corrector re-pins any
    // residual gap to true scrollHeight (which includes outside chrome).
    // Do NOT replace end-anchor with pin-only here — delta compensation is what
    // keeps token growth smooth; the corrector fixes chrome geometry.
    scrollToFn: (offset, options) => followHandle.scrollToOffset(offset, options),
    observeElementRect: observeRect,
    estimateSize: (index) => {
      const item = itemAt(index)
      if (!item) return 96
      const cached = estimateSizeRef.current.get(item.key)
      if (cached !== undefined) return cached
      // Live tail: prefer last measured height under the stable live key.
      if (item.kind === 'streaming' || item.kind === 'live-group') return 160
      return 96
    },
    // Include the measured content width in TanStack's key space. A stable
    // message key must remain stable within one layout, but a width change is
    // a different geometry universe: old internal itemSizeCache entries must
    // not be reused for rows that have not been mounted again yet.
    getItemKey: (index) => {
      const item = itemAt(index)
      return layoutScopedVirtualKey(layoutKey, item ? virtualItemIdentity(item) : `row-${index}`)
    },
    initialMeasurementsCache,
    measureElement: (element, entry, instance) => {
      // TanStack's default sync path returns itemSizeCache when present. For
      // absolutely positioned chat rows that can leave the next row 10s of px
      // too early, so a mount must synchronously replace cache with real DOM size.
      const measured = measureChatVirtualRow(
        element,
        entry,
        instance.options.horizontal,
      )
      const key = element.dataset.chatItemKey
      if (key) {
        const size = Math.max(1, measured)
        const index = Number(element.dataset.index)
        const virtualKey = Number.isInteger(index) ? instance.options.getItemKey(index) : key
        const logicalKey = Number.isInteger(index) ? itemAt(index)?.key : undefined
        const previousSize = instance.itemSizeCache.get(virtualKey)
        if (previousSize === undefined || Math.abs(previousSize - size) > 0.5) {
          // Remeasure compensation. External live never hits this for token growth.
          followHandle.markLayoutCompensation()
        }
        // 行内还有未 hydrate 的 heavy island（Mermaid/HTML 预览等 fallback 与真身
        // 几何不同）时，只让 TanStack 内部照常测量，不写持久 measurement cache：
        // fallback 高度一旦入缓存，下次打开该会话会先按错误高度布局、hydrate 后再跳。
        if (!element.querySelector(NAVIGATOR_PENDING_SELECTOR)) {
          if (logicalKey) estimateSizeRef.current.set(logicalKey, size)
          setCachedRowMeasurement(layoutKey, key, size)
        }
        return size
      }
      return measured
    },
    rangeExtractor: useCallback((range: Range) => {
      let indexes = defaultRangeExtractor(range)
      // 消息导航：目标行附近强制挂载渲染测高，再一次性跳转。
      const forced = forceMountRenderIndex
      if (forced != null && itemCount > 0) {
        const from = Math.max(0, forced - NAVIGATOR_FORCE_MOUNT_RADIUS)
        const to = Math.min(itemCount - 1, forced + NAVIGATOR_FORCE_MOUNT_RADIUS)
        const set = new Set(indexes)
        for (let index = from; index <= to; index += 1) set.add(index)
        indexes = [...set].sort((a, b) => a - b)
      }
      return indexes
    }, [forceMountRenderIndex, itemCount]),


    overscan: 6,
    // jsdom/test environments have no layout box before the first observer tick;
    // a conservative initial viewport keeps the first render useful while the
    // real browser immediately replaces it with the measured client rect.
    initialRect: { width: 0, height: viewportEl?.clientHeight || 800 },
    // LiveAgent: always end-anchored. While following, live-row growth
    // compensates by total-size delta; residual gap is corrected by the
    // follow reducer (pin to full scrollHeight including chrome reserve).
    anchorTo: 'end',
    scrollEndThreshold: 12,
    followOnAppend: false,
    useAnimationFrameWithResizeObserver: false,
    useFlushSync: false,
  })
  virtualizer.shouldAdjustScrollPositionOnItemSizeChange = (item, _delta, instance) => {
    // 导航 prepare/hold 期间禁止测高改 scrollTop：那是点导航后上下抽的主因。
    if (navigationLockRef.current) return false
    return shouldAdjustChatItemSizeChange(item, {
      scrollOffset: instance.scrollOffset ?? 0,
      scrollAdjustments: instance.scrollAdjustments,
      itemSizeCache: instance.itemSizeCache,
      scrollDirection: instance.scrollDirection,
    })
  }

  const virtualItems = virtualizer.getVirtualItems()
  // Row ResizeObservers and TanStack's own viewport observer update mounted rows.
  // Avoid a blanket measure(): it clears the virtualizer's measured cache and makes
  // detached readers pay the estimate-to-real-height correction for every row.

  const saveMeasurementSnapshotRef = useRef<() => void>(() => {})
  saveMeasurementSnapshotRef.current = () => {
    if (!viewportEl) return
    saveMeasurementSnapshot(
      conversationId,
      layoutKey,
      measurementRevision,
      virtualizer.takeSnapshot(),
    )
  }
  useEffect(() => () => saveMeasurementSnapshotRef.current(), [])

  useLayoutEffect(() => {
    if (!contentEl) return
    recordChatPerfSample({
      name: 'message-list-window',
      durationMs: 0,
      mountedRows: contentEl.querySelectorAll('[data-chat-message-list-item]').length,
      domNodes: contentEl.querySelectorAll('*').length,
      detail: `${conversationId ?? 'empty'}:history=${historyItems.length}:visible=${virtualItems.length}`,
    })
  }, [contentEl, conversationId, historyItems.length, virtualItems.length])

  const navigatorNodes = useMemo(() => {
    // targetRenderIndex 仍是「全历史逻辑下标」，导航时用 data-chat-row-index 查找。
    const renderIndexByKey = new Map(historyItems.map((item, index) => [item.key, index]))
    return buildMessageNavigatorNodes({ folded, boundaries, renderIndexByKey })
  }, [boundaries, folded, historyItems])
  navigatorNodesRef.current = navigatorNodes
  const navigatorTurnCount = navigatorNodes.reduce(
    (count, node) => count + (node.kind === 'turn' ? 1 : 0),
    0,
  )
  // 滚动回调里读，走 ref：导航器没渲染（< 4 轮）就别做整列表测量。
  const navigatorEnabledRef = useRef(false)
  navigatorEnabledRef.current = navigatorTurnCount >= 4

  const updateActiveNavigatorNode = useCallback((nodeId: string | null) => {
    if (activeNavigatorNodeIdRef.current === nodeId) return
    activeNavigatorNodeIdRef.current = nodeId
    setActiveNavigatorNodeId(nodeId)
  }, [])

  const updateVisibleNavigatorNodes = useCallback((nodeIds: string[]) => {
    const previous = visibleNavigatorNodeIdsRef.current
    if (previous.length === nodeIds.length && previous.every((id, index) => id === nodeIds[index])) return
    visibleNavigatorNodeIdsRef.current = nodeIds
    setVisibleNavigatorNodeIds(nodeIds)
  }, [])

  const cancelNavigatorSettle = useCallback(() => {
    if (navigatorSettleRafRef.current !== null) {
      cancelAnimationFrame(navigatorSettleRafRef.current)
      navigatorSettleRafRef.current = null
    }
  }, [])

  const cancelNavigatorUnlock = useCallback(() => {
    if (navigatorUnlockRafRef.current !== null) {
      cancelAnimationFrame(navigatorUnlockRafRef.current)
      navigatorUnlockRafRef.current = null
    }
    navigatorUnlockGenerationRef.current += 1
  }, [])

  const setNavigationLock = useCallback((locked: boolean) => {
    navigationLockRef.current = locked
    setNavigatorLockActive((current) => (current === locked ? current : locked))
  }, [])

  const endNavigatorSession = useCallback((generation: number) => {
    if (generation !== navigatorSettleGenerationRef.current) return
    const hadBottomHold = bottomHoldRef.current != null
    navigatorHoldRef.current = null
    bottomHoldRef.current = null
    navigatorFrozenScrollTopRef.current = null
    if (forceMountRenderIndexRef.current != null) {
      setForceMountRenderIndex(null)
    }
    endMessageNavigationHydrate(generation)
    // Streaming kept reserve on wrap.minHeight through bottomHold. Now that hold
    // is done, re-run send-reserve so minHeight clears and the spacer takes the
    // remainder — otherwise total height stays inflated and stick-to-bottom
    // parks the viewport on empty space (content "jumps up").
    if (hadBottomHold) {
      setReserveEpoch((value) => value + 1)
    }
    // force-mount 拆除后还可能有迟到测高；锁多留几帧再开。
    cancelNavigatorUnlock()
    const unlockGeneration = navigatorUnlockGenerationRef.current
    let remaining = NAVIGATOR_UNLOCK_FRAMES
    const unlockTick = () => {
      navigatorUnlockRafRef.current = null
      if (unlockGeneration !== navigatorUnlockGenerationRef.current) return
      if (generation !== navigatorSettleGenerationRef.current) return
      remaining -= 1
      if (remaining <= 0) {
        setNavigationLock(false)
        return
      }
      navigatorUnlockRafRef.current = requestAnimationFrame(unlockTick)
    }
    navigatorUnlockRafRef.current = requestAnimationFrame(unlockTick)
  }, [cancelNavigatorUnlock, setNavigationLock])

  const clearNavigatorPrepare = useCallback(() => {
    cancelNavigatorSettle()
    cancelNavigatorUnlock()
    setNavigationLock(false)
    const hadBottomHold = bottomHoldRef.current != null
    navigatorHoldRef.current = null
    bottomHoldRef.current = null
    navigatorFrozenScrollTopRef.current = null
    if (forceMountRenderIndexRef.current != null) {
      setForceMountRenderIndex(null)
    }
    if (hadBottomHold) {
      setReserveEpoch((value) => value + 1)
    }
  }, [cancelNavigatorSettle, cancelNavigatorUnlock, setNavigationLock])



  const alignViewportToRowIndex = useCallback((targetRenderIndex: number) => {
    const el = scrollRef.current
    const row = contentEl?.querySelector(
      `[data-chat-row-index="${targetRenderIndex}"]`,
    ) as HTMLElement | null
    if (!row || !el) return false
    const nextOffset =
      row.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop
    // 只滚这个视口。scrollIntoView 会连带滚动所有可滚祖先。
    if (Math.abs(el.scrollTop - nextOffset) > NAVIGATOR_ALIGN_EPSILON_PX) {
      followHandle.scrollToOffset(nextOffset)
    }
    return true
  }, [contentEl, followHandle])

  const rowHasPendingMedia = useCallback((row: HTMLElement) => {
    if (row.querySelector(NAVIGATOR_PENDING_SELECTOR)) return true
    // 只等「还在加载」的图片。complete && naturalWidth===0 是坏图/空图终态，
    // 再当 pending 会把导航/回底 hold 拖满帧预算，跳转发粘。
    const images = row.querySelectorAll('img')
    for (const image of images) {
      if (!image.complete) return true
    }
    return false
  }, [])


  const readNavigatorTargetMetrics = useCallback((targetRenderIndex: number) => {
    const el = scrollRef.current
    const row = contentEl?.querySelector(
      `[data-chat-row-index="${targetRenderIndex}"]`,
    ) as HTMLElement | null
    if (!row || !el) {
      return { ready: false as const, height: 0, offsetPx: Number.POSITIVE_INFINITY }
    }
    if (rowHasPendingMedia(row)) {
      return { ready: false as const, height: 0, offsetPx: Number.POSITIVE_INFINITY }
    }
    const rowRect = row.getBoundingClientRect()
    const viewportRect = el.getBoundingClientRect()
    const height = rowRect.height
    if (!(height > 0)) {
      return { ready: false as const, height: 0, offsetPx: Number.POSITIVE_INFINITY }
    }
    return {
      ready: true as const,
      height,
      offsetPx: Math.abs(rowRect.top - viewportRect.top),
    }
  }, [contentEl, rowHasPendingMedia])

  /**
   * 先渲染后跳转：
   * 1. 准备阶段：强制挂载目标邻域 + eager hydrate。**不锁**测高补偿，
   *    让当前阅读位置保持稳定（锁住反而会和 force-mount 测高打架上下抽）。
   * 2. 就绪后上锁，只在 useLayoutEffect 里跳一次。
   * 3. hold：paint 前钉住，直到 offset/高度/totalSize 都稳定。
   */

  const prepareThenJumpToNavigatorNode = useCallback((
    generation: number,
    targetRenderIndex: number,
  ) => {
    cancelNavigatorSettle()
    // 准备阶段明确解锁：force-mount 上方行测高需要 shouldAdjust 保住当前画面。
    setNavigationLock(false)
    navigatorHoldRef.current = null
    navigatorFrozenScrollTopRef.current = null
    let frames = 0
    let previousHeight = -1
    let stableFrames = 0

    const startHold = () => {
      if (generation !== navigatorSettleGenerationRef.current) return
      // 从这一刻起锁住测高改 scroll；真正的跳转只发生在 layout（paint 前）。
      bottomHoldRef.current = null
      setNavigationLock(true)
      navigatorFrozenScrollTopRef.current = null
      navigatorHoldRef.current = {
        generation,
        targetIndex: targetRenderIndex,
        frames: 0,
        stable: 0,
        jumped: false,
        lastHeight: -1,
        lastTotalSize: -1,
      }
      setNavigatorHoldEpoch((value) => value + 1)
    }


    const tick = () => {
      navigatorSettleRafRef.current = null
      if (generation !== navigatorSettleGenerationRef.current) return

      frames += 1

      const { ready, height } = readNavigatorTargetMetrics(targetRenderIndex)
      if (!ready) {
        previousHeight = -1
        stableFrames = 0
        if (frames < NAVIGATOR_SETTLE_MAX_FRAMES) {
          navigatorSettleRafRef.current = requestAnimationFrame(tick)
        } else {
          startHold()
        }
        return
      }

      if (height === previousHeight) stableFrames += 1
      else {
        previousHeight = height
        stableFrames = 0
      }

      if (stableFrames >= NAVIGATOR_SETTLE_STABLE_FRAMES || frames >= NAVIGATOR_SETTLE_MAX_FRAMES) {
        startHold()
        return
      }
      navigatorSettleRafRef.current = requestAnimationFrame(tick)
    }

    navigatorSettleRafRef.current = requestAnimationFrame(tick)
  }, [
    cancelNavigatorSettle,
    readNavigatorTargetMetrics,
    setNavigationLock,
  ])

  // 跳转后、paint 前钉住目标。邻居 measure → translateY 变时，在用户看见前补 scrollTop。

  useLayoutEffect(() => {
    const hold = navigatorHoldRef.current
    if (!hold) return
    if (hold.generation !== navigatorSettleGenerationRef.current) {
      navigatorHoldRef.current = null
      return
    }

    setNavigationLock(true)
    alignViewportToRowIndex(hold.targetIndex)
    hold.jumped = true
    hold.frames += 1

    const metrics = readNavigatorTargetMetrics(hold.targetIndex)
    const totalSize = virtualizer.getTotalSize()
    const geometryStable = metrics.ready
      && metrics.offsetPx <= NAVIGATOR_ALIGN_EPSILON_PX
      && metrics.height === hold.lastHeight
      && totalSize === hold.lastTotalSize
      && hold.lastHeight > 0

    if (metrics.ready) {
      hold.lastHeight = metrics.height
      hold.lastTotalSize = totalSize
    }

    if (geometryStable) hold.stable += 1
    else hold.stable = 0

    if (hold.stable >= NAVIGATOR_HOLD_STABLE_FRAMES || hold.frames >= NAVIGATOR_HOLD_MAX_FRAMES) {
      endNavigatorSession(hold.generation)
      return
    }

    const raf = requestAnimationFrame(() => {
      if (navigatorHoldRef.current?.generation === hold.generation) {
        setNavigatorHoldEpoch((value) => value + 1)
      }
    })
    return () => cancelAnimationFrame(raf)
  }, [
    alignViewportToRowIndex,
    endNavigatorSession,
    navigatorHoldEpoch,
    readNavigatorTargetMetrics,
    setNavigationLock,
    virtualItems,
    virtualizer,
  ])


  const navigateToNavigatorNode = useCallback((node: MessageNavigatorNode) => {
    // 跳到上方消息：先脱离跟随，否则跟随纠正器会把视口又钉回底部。
    followHandle.releaseFollow()
    updateActiveNavigatorNode(node.id)

    if (node.targetRenderIndex < 0 || node.targetRenderIndex >= historyItems.length) {
      return
    }

    const generation = beginMessageNavigationHydrate()
    navigatorSettleGenerationRef.current = generation
    clearNavigatorPrepare()
    // 准备阶段不锁；force-mount 后由 virtualizer 自己补偿当前画面。
    setForceMountRenderIndex(node.targetRenderIndex)

    // 先渲染（当前画面不动），就绪后在 layout 里跳一次并 hold 消抽搐。
    prepareThenJumpToNavigatorNode(generation, node.targetRenderIndex)
  }, [
    clearNavigatorPrepare,
    followHandle,
    historyItems.length,
    prepareThenJumpToNavigatorNode,
    updateActiveNavigatorNode,
  ])


  useEffect(() => () => {
    clearNavigatorPrepare()
    resetMessageNavigationStore()
  }, [clearNavigatorPrepare])

  useEffect(() => {
    // 切换会话时清掉上一会话未完成的 settle，避免 eager / force-mount 残留。
    clearNavigatorPrepare()
    resetMessageNavigationStore()
  }, [clearNavigatorPrepare, conversationId])

  // 全局搜索：打开会话后滚到命中消息，并短暂闪一下高亮。
  // 「已处理 id」ref：effect deps 里有 historyItems——处理完到父级清 focusMessageId
  // 之间若消息列表又变（落库/工具更新），不挡一道会对同一个 id 反复重跳。
  // prop 回到 null 时重置，同一 id 之后仍可再次聚焦。
  const handledFocusRef = useRef<string | null>(null)
  useEffect(() => {
    if (!focusMessageId || !conversationId) {
      handledFocusRef.current = null
      return
    }
    if (handledFocusRef.current === focusMessageId) return
    let targetIndex = -1
    for (let i = 0; i < historyItems.length; i++) {
      const item = historyItems[i]
      if (item.kind === 'message' && item.message.id === focusMessageId) {
        targetIndex = i
        break
      }
      if (item.kind === 'group' && item.messages.some((m) => m.id === focusMessageId)) {
        targetIndex = i
        break
      }
    }
    if (targetIndex < 0) {
      // 消息尚未就绪（或 id 已失效）——等 historyItems 变化再试；若始终找不到则下一轮消息变更后仍可重试。
      // 空会话或 id 不在列表时清掉，避免卡住。
      if (messages.length > 0) {
        handledFocusRef.current = focusMessageId
        onFocusMessageHandled?.()
      }
      return
    }

    handledFocusRef.current = focusMessageId
    followHandle.releaseFollow()
    const generation = beginMessageNavigationHydrate()
    navigatorSettleGenerationRef.current = generation
    clearNavigatorPrepare()
    setForceMountRenderIndex(targetIndex)
    prepareThenJumpToNavigatorNode(generation, targetIndex)

    const flash = () => {
      const el = contentEl?.querySelector(
        `[data-message-id="${CSS.escape(focusMessageId)}"]`,
      ) as HTMLElement | null
      if (!el) return
      el.classList.add('kv-search-focus-flash')
      window.setTimeout(() => el.classList.remove('kv-search-focus-flash'), 1600)
    }
    // 等 force-mount + 对齐后再闪
    const t1 = window.setTimeout(flash, 80)
    const t2 = window.setTimeout(flash, 240)
    onFocusMessageHandled?.()
    return () => {
      window.clearTimeout(t1)
      window.clearTimeout(t2)
    }
  }, [
    clearNavigatorPrepare,
    contentEl,
    conversationId,
    focusMessageId,
    followHandle,
    historyItems,
    messages.length,
    onFocusMessageHandled,
    prepareThenJumpToNavigatorNode,
  ])








  /**
   * 回到底部：同样走「eager hydrate + 钉底 hold」。
   * 从历史中部一键到底时，尾部消息（尤其代码块）才挂载；若延迟 hydrate，
   * scrollHeight 会在落地后猛涨，贴底 pin 连跳几次就是抽一下。
   */
  const handleJumpToBottom = useCallback(() => {
    cancelNavigatorSettle()
    navigatorHoldRef.current = null
    navigatorFrozenScrollTopRef.current = null

    const generation = beginMessageNavigationHydrate()
    navigatorSettleGenerationRef.current = generation
    setNavigationLock(true)

    // 强制挂载尾部邻域（含 live 行），让代码块在钉底前就按真高度量好。
    const tailIndex = historyItems.length - 1
    if (tailIndex >= 0) {
      setForceMountRenderIndex(tailIndex)
    }

    followHandle.jumpToBottom()
    bottomHoldRef.current = {
      generation,
      frames: 0,
      stable: 0,
      lastScrollHeight: -1,
    }
    setBottomHoldEpoch((value) => value + 1)
  }, [
    cancelNavigatorSettle,
    followHandle,
    historyItems.length,
    setNavigationLock,
  ])

  // 「回到底部」落地 hold：paint 前连续钉底，直到 scrollHeight / 重内容稳定。
  useLayoutEffect(() => {
    const hold = bottomHoldRef.current
    if (!hold) return
    if (hold.generation !== navigatorSettleGenerationRef.current) {
      bottomHoldRef.current = null
      return
    }

    setNavigationLock(true)
    hold.frames += 1

    const viewport = scrollRef.current
    if (viewport) {
      const preGap = Math.max(0, viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight)
      // Only hard-pin when actually off-bottom. Pinning every frame at gap≈0
      // forces scrollTop writes that read as a settle flash.
      if (preGap > 12) followHandle.jumpToBottom()
      else followHandle.pinIfFollowing()
    }
    if (!viewport) {
      if (hold.frames >= NAVIGATOR_HOLD_MAX_FRAMES) {
        endNavigatorSession(hold.generation)
      } else {
        const raf = requestAnimationFrame(() => {
          if (bottomHoldRef.current?.generation === hold.generation) {
            setBottomHoldEpoch((value) => value + 1)
          }
        })
        return () => cancelAnimationFrame(raf)
      }
      return
    }

    const scrollHeight = viewport.scrollHeight
    const gap = Math.max(0, scrollHeight - viewport.scrollTop - viewport.clientHeight)
    const pendingHeavy = Boolean(
      contentEl?.querySelector(NAVIGATOR_PENDING_SELECTOR),
    )
    // 尾部已挂载行里若还有未完成图片，高度还会涨。
    let pendingMedia = pendingHeavy
    if (!pendingMedia && contentEl) {
      const rows = contentEl.querySelectorAll<HTMLElement>('[data-chat-row-index]')
      const start = Math.max(0, rows.length - 12)
      for (let index = start; index < rows.length; index += 1) {
        if (rowHasPendingMedia(rows[index])) {
          pendingMedia = true
          break
        }
      }
    }

    const geometryStable = !pendingMedia
      && gap <= 12
      && scrollHeight === hold.lastScrollHeight
      && scrollHeight > 0

    hold.lastScrollHeight = scrollHeight
    if (geometryStable) hold.stable += 1
    else hold.stable = 0

    if (hold.stable >= NAVIGATOR_HOLD_STABLE_FRAMES || hold.frames >= NAVIGATOR_HOLD_MAX_FRAMES) {
      endNavigatorSession(hold.generation)
      return
    }

    const raf = requestAnimationFrame(() => {
      if (bottomHoldRef.current?.generation === hold.generation) {
        setBottomHoldEpoch((value) => value + 1)
      }
    })
    return () => cancelAnimationFrame(raf)
  }, [
    bottomHoldEpoch,
    contentEl,
    endNavigatorSession,
    followHandle,
    rowHasPendingMedia,
    setNavigationLock,
    virtualItems,
  ])


  // 滚动监听：用 DOM 行几何更新导航器（兼容「上方虚拟 + 底部实挂载」）。
  // 跟随钉底由 useScrollFollow 独立处理。
  const syncNavigatorFromDom = useCallback(() => {
    const el = scrollRef.current
    if (!el) return
    const viewportTop = el.getBoundingClientRect().top
    const viewportBottom = viewportTop + el.clientHeight
    const readingY = viewportTop + el.clientHeight * 0.3
    const rows = el.querySelectorAll<HTMLElement>('[data-chat-row-index]')
    let activeIndex: number | null = null
    let firstVisible = Number.POSITIVE_INFINITY
    let lastVisible = -1
    // 行按文档顺序 = 纵向顺序：一旦某行顶边越过视口底边，后面的行全在屏下，直接停。
    for (const row of rows) {
      const index = Number(row.dataset.chatRowIndex)
      if (!Number.isFinite(index)) continue
      const rect = row.getBoundingClientRect()
      if (rect.top >= viewportBottom) break
      if (rect.bottom > viewportTop && rect.top < viewportBottom) {
        firstVisible = Math.min(firstVisible, index)
        lastVisible = Math.max(lastVisible, index)
      }
      if (rect.top <= readingY && rect.bottom >= readingY) {
        activeIndex = index
      }
    }
    if (activeIndex == null && lastVisible >= 0) activeIndex = lastVisible
    if (activeIndex != null) {
      updateActiveNavigatorNode(
        activeMessageNavigatorNodeId(navigatorNodesRef.current, activeIndex),
      )
    }
    if (lastVisible >= 0) {
      updateVisibleNavigatorNodes(visibleMessageNavigatorNodeIds(
        navigatorNodesRef.current,
        firstVisible,
        lastVisible,
      ))
    }
  }, [updateActiveNavigatorNode, updateVisibleNavigatorNodes])

  // 滚动回调只保留导航器的低频同步；虚拟窗口和尺寸补偿由同一个 virtualizer 管理。
  // 导航器同步是整列表测量（querySelectorAll + 逐行 gBCR，virtualizer 同帧写过 DOM 时
  // 第一下就是强制 reflow），节流到 NAVIGATOR_SYNC_INTERVAL_MS 一次 + 停下后补一次
  // 尾同步，导航器没渲染时完全不跑。
  const navigatorSyncRafRef = useRef<number | null>(null)
  const navigatorSyncLastTsRef = useRef(0)
  const navigatorSyncTrailingRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const scheduleNavigatorSync = useCallback(() => {
    if (navigatorSyncRafRef.current !== null) return
    navigatorSyncRafRef.current = requestAnimationFrame(() => {
      navigatorSyncRafRef.current = null
      if (!navigatorEnabledRef.current) return
      const now = performance.now()
      if (now - navigatorSyncLastTsRef.current >= NAVIGATOR_SYNC_INTERVAL_MS) {
        navigatorSyncLastTsRef.current = now
        if (navigatorSyncTrailingRef.current !== null) {
          clearTimeout(navigatorSyncTrailingRef.current)
          navigatorSyncTrailingRef.current = null
        }
        syncNavigatorFromDom()
        return
      }
      // 间隔内的滚动只重排尾同步：滚动一停就把最终位置对准。
      if (navigatorSyncTrailingRef.current !== null) clearTimeout(navigatorSyncTrailingRef.current)
      navigatorSyncTrailingRef.current = setTimeout(() => {
        navigatorSyncTrailingRef.current = null
        navigatorSyncLastTsRef.current = performance.now()
        syncNavigatorFromDom()
      }, NAVIGATOR_SYNC_INTERVAL_MS)
    })
  }, [syncNavigatorFromDom])

  const handleNavigatorScroll = useCallback(() => {
    // hold 期间硬钉：消息导航钉目标行；回到底部钉底。
    if (navigationLockRef.current) {
      const bottomHold = bottomHoldRef.current
      if (bottomHold && bottomHold.generation === navigatorSettleGenerationRef.current) {
        followHandle.jumpToBottom()
      } else {
        const hold = navigatorHoldRef.current
        if (hold?.jumped) {
          alignViewportToRowIndex(hold.targetIndex)
        }
      }
    }
    scheduleNavigatorSync()
  }, [alignViewportToRowIndex, followHandle, scheduleNavigatorSync])

  // 用户滚轮 = 用户接管视口。回底/导航 hold 期间若继续硬钉：wheel(up) 先解除跟随，
  // 下一个 scroll 事件又被 handleNavigatorScroll 的 jumpToBottom()（forceFollow）钉回，
  // gap>12 让 hold 的 stable 计数永不累积，拉锯会跑满 NAVIGATOR_HOLD_MAX_FRAMES(28)
  // + NAVIGATOR_UNLOCK_FRAMES(10) ≈ 470ms —— 体感就是「点完回底再上滚被反复拽回」。
  // 任何以纵向为主的滚轮都直接终止当前会话：prepare 阶段取消 settle 循环，hold 阶段
  // 走 endNavigatorSession（清 hold/force-mount、结束 eager hydrate、几帧后解锁）。
  // 只挂 wheel：原生滚动条拖动不派发 wheel，本来也进不了这条拉锯（scroll 源分类接管）。
  useEffect(() => {
    if (!viewportEl) return
    const handleWheel = (event: WheelEvent) => {
      if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return
      const sessionActive = navigationLockRef.current
        || navigatorSettleRafRef.current !== null
        || navigatorHoldRef.current !== null
        || bottomHoldRef.current !== null
      if (!sessionActive) return
      cancelNavigatorSettle()
      endNavigatorSession(navigatorSettleGenerationRef.current)
    }
    viewportEl.addEventListener('wheel', handleWheel, { passive: true })
    return () => viewportEl.removeEventListener('wheel', handleWheel)
  }, [cancelNavigatorSettle, endNavigatorSession, viewportEl])


  // 消息区右键：读取当前选中文本 + 命中的消息，弹内置菜单。两者都空则不弹（放行给全局屏蔽）。
  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    const selectionText = (window.getSelection()?.toString() ?? '').trim()
    const targetEl = (e.target as Element | null)?.closest?.('[data-message-id]') as HTMLElement | null
    const id = targetEl?.dataset.messageId ?? null
    let messageText: string | null = null
    const isLiveBubble = id === 'streaming-assistant'
      || Boolean(id && liveRowActive && (id === snapshot.messageId || id === liveRowKey))
    if (isLiveBubble) {
      messageText = streamingContent.trim() || null
    } else if (id) {
      messageText = messages.find((m) => m.id === id)?.content?.trim() || null
    }
    if (!selectionText && !messageText) return
    e.preventDefault()
    const left = Math.min(e.clientX, window.innerWidth - 184)
    const top = Math.min(e.clientY, window.innerHeight - 96)
    setMsgMenu({ anchor: { left, top }, selectionText, messageText })
  }, [liveRowActive, liveRowKey, messages, snapshot.messageId, streamingContent])

  const closeMsgMenu = useCallback(() => setMsgMenu(null), [])

  // 尾部：virtualizer 下方的文档流块。
  // 流式气泡 / 错误 / 状态线 / 发送预留都在这里，历史行不因 token 重测。
  const tailWrapRef = useRef<HTMLDivElement | null>(null)
  const tailSpacerRef = useRef<HTMLDivElement | null>(null)

  // 切换会话：重置跟随并瞬间定位到底部（ResizeObserver 首次投递也会兜底钉一次）。
  useLayoutEffect(() => {
    followHandle.stickToBottom()
    const lastNode = navigatorNodesRef.current[navigatorNodesRef.current.length - 1]
    updateActiveNavigatorNode(lastNode?.id ?? null)
    updateVisibleNavigatorNodes(lastNode ? [lastNode.id] : [])
  }, [conversationId, followHandle, updateActiveNavigatorNode, updateVisibleNavigatorNodes])

  // 自己发出新消息时强制回到底部（即使刚才正往上翻历史）。assistant 落库会替换列表外的
  // streaming 节点；若仍在跟随，完成这次结构交接后也明确补钉，不能只依赖 ResizeObserver 时序。
  useLayoutEffect(() => {
    const count = messages.length

    if (count > prevMessageCountRef.current) {
      const lastRole = messages[count - 1]?.role
      if (lastRole === 'user' || (lastRole === 'assistant' && followHandle.isFollowing())) {
        followHandle.stickToBottom()
      }
    }
    prevMessageCountRef.current = count
  }, [messages, followHandle])

  // live → 历史 交接。
  //
  // Primary path: live is OUTSIDE the virtualizer (document flow). Streaming
  // pin is contentGrowth-only and stable. On settle the outside bubble unmounts
  // and the twin remounts inside the virtualizer at estimate height — height
  // collapses then re-expands. Seed cache from the last live height and run the
  // same multi-frame bottomHold as "jump to bottom" so pin survives hydrate.
  // live 行高度用 RO 持续跟踪（settle 帧种子消费）。原实现在 layout effect 里每个
  // token querySelector + getBoundingClientRect —— 每帧一次强制布局读，长回答白白累积。
  // RO 回调在布局后、绘制前投递，此时读 gBCR 拿的是新鲜布局，不触发额外 reflow。
  useLayoutEffect(() => {
    if (!liveRowActive || !contentEl) return
    const liveEl = contentEl.querySelector(
      '[data-chat-message-list-item="streaming"], [data-chat-message-list-item="live-group"]',
    ) as HTMLElement | null
    if (!liveEl) return
    const record = () => {
      const height = liveEl.getBoundingClientRect().height
      if (height > 0) liveBubbleHeightRef.current = height
    }
    record()
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(record)
    observer.observe(liveEl)
    return () => observer.disconnect()
  }, [contentEl, liveRowActive])

  const liveScrollHandoffRef = useRef(liveRowActive)
  useLayoutEffect(() => {
    const wasLive = liveScrollHandoffRef.current
    liveScrollHandoffRef.current = liveRowActive
    if (!wasLive || liveRowActive) return
    if (!streamFollowIntentRef.current && !followHandle.isFollowing()) return

    const liveHeight = Math.round(liveBubbleHeightRef.current)
    liveBubbleHeightRef.current = 0
    if (liveHeight > 0) {
      const settlingId = snapshot.messageId
        || [...messages].reverse().find((message) => message.role === 'assistant')?.id
        || null
      if (settlingId) {
        const settling = messages.find((message) => message.id === settlingId)
        const liveMessage = lastLiveMessageRef.current
        if (settling && liveMessage && canReuseLiveRowHeight(liveMessage, settling)) {
          const rowKey = `${liveRowModel.resolveMessageKey(settling.id)}:${chatMessageLayoutRevision(settling)}`
          setCachedRowMeasurement(layoutKey, rowKey, liveHeight)
          estimateSizeRef.current.set(liveRowModel.resolveMessageKey(settling.id), liveHeight)
        }
      }
    }

    cancelNavigatorSettle()
    navigatorHoldRef.current = null
    navigatorFrozenScrollTopRef.current = null

    const generation = beginMessageNavigationHydrate()
    navigatorSettleGenerationRef.current = generation
    setNavigationLock(true)

    if (historyItems.length > 0) {
      setForceMountRenderIndex(historyItems.length - 1)
    }

    followHandle.markLayoutCompensation()
    // Prefer pin-if-following over forceFollow: avoids a hard jump when height
    // continuity already holds; bottomHold still re-pins while hydrate settles.
    followHandle.pinIfFollowing()
    if (followHandle.isFollowing() || streamFollowIntentRef.current) {
      followHandle.stickToBottom()
    }
    bottomHoldRef.current = {
      generation,
      frames: 0,
      stable: 0,
      lastScrollHeight: -1,
    }
    setBottomHoldEpoch((value) => value + 1)
  }, [
    cancelNavigatorSettle,
    followHandle,
    historyItems.length,
    layoutKey,
    liveRowActive,
    liveRowModel,
    messages,
    setNavigationLock,
    snapshot.messageId,
  ])

  // 发送后的尾部预留，两个阶段一处算：
  // - **运行中**：撑在尾部 wrapper 的 minHeight 上（是 min，回答长过它就自然吃掉，不用逐帧算）。
  // - **结束后**：同一段预留补到底部留白上。两阶段量的是同一段跨度（最后一条 user 的底边 →
  //   内容底边）、同一个 reserve 值，所以交接前后总高相等，视图不动（短回答不再往下沉）。
  //
  // 基准必须是**滚动视口**的高度，不是窗口高（dvh）：ask_user 面板吊在输入框上方、在滚动区
  // 之外，它一出现视口就矮一大截，按窗口算的预留会比视口还高，把上一条消息整个顶出屏幕。
  // 再夹一道 `视口 - 锚点行高`：不管比例给多大，那条刚发出的消息必须留在屏幕里。
  // 只在「本次会话里刚生成完」时接管留白：切换/打开会话不给预留，老会话的样子不变。
  const reserveHandoffRef = useRef(false)
  useLayoutEffect(() => {
    reserveHandoffRef.current = false
  }, [conversationId])
  useLayoutEffect(() => {
    const wrap = tailWrapRef.current
    const spacer = tailSpacerRef.current
    if (!wrap || !spacer || !viewportEl) return
    if (streaming) reserveHandoffRef.current = true

    const apply = () => {
      const lastUser = [...messages].reverse().find((m) => m.role === 'user')
      const row = lastUser && contentEl
        ? contentEl.querySelector(`[data-message-id="${CSS.escape(lastUser.id)}"]`)
        : null
      const anchorH = row?.getBoundingClientRect().height ?? 0
      const reserve = sendReserveHeight(viewportEl.clientHeight, anchorH, LIST_EDGE_PADDING_PX)
      // ⚠️ 只在 streaming/frozen 期间保留 minHeight，settle 帧**立即**转移，不等 bottomHold。
      // 曾经这里多一个 `|| bottomHoldRef.current`：settle 帧 live 气泡已经搬进 virtualizer，
      // wrapper 里只剩状态线 + spacer（~30px），再压 reserve 的 minHeight 就是往文档里
      // 凭空插一条 ~45% 视口高的空带 —— 钉底把答案顶上去（闪一下），hold 结束转移时
      // 又缩回来（抽一下）。立即转移在两种回答长度下总高都恒等：长回答 minHeight 本来
      // 就被内容吃掉（清掉不变高、row 已虚拟化卸载 → spacer 16px）；短回答 row 还在，
      // spacer = reserve − span 精确补齐。本 effect 在交接 effect 之后同一 commit 运行，
      // twin 已在 DOM 里，span 量得到。
      if (streaming || streamFrozen) {
        wrap.style.minHeight = `${Math.round(reserve)}px`
        // 留白交还给 minHeight：不还的话上一轮量出来的高度会和 minHeight 叠成两段预留。
        spacer.style.height = `${LIST_EDGE_PADDING_PX}px`
        return
      }
      const hadStreamingMinHeight = Boolean(wrap.style.minHeight)
      wrap.style.minHeight = ''
      if (!reserveHandoffRef.current || !row) {
        spacer.style.height = `${LIST_EDGE_PADDING_PX}px`
      } else {
        // 跨度用 spacer 自己的顶边量，与 spacer 当前高度无关，避免自反馈。
        const span = spacer.getBoundingClientRect().top - row.getBoundingClientRect().bottom
        spacer.style.height = `${Math.max(LIST_EDGE_PADDING_PX, Math.round(reserve - span))}px`
      }
      // minHeight → spacer transfer changes scrollHeight. Re-pin so we don't
      // freeze on the old (too large) bottom and leave a blank band under the answer.
      if (hadStreamingMinHeight && (streamFollowIntentRef.current || followHandle.isFollowing())) {
        followHandle.markLayoutCompensation()
        followHandle.stickToBottom()
      }
    }

    apply()
    // 视口高度会变（ask_user 面板出现/消失、输入框长高、窗口 resize），每次都得重算预留。
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(apply)
    observer.observe(viewportEl)
    return () => observer.disconnect()
  }, [streaming, streamFrozen, messages, contentEl, viewportEl, reserveEpoch, followHandle])

  const renderItem = useCallback(
    (item: RenderItem) => {
      switch (item.kind) {
        case 'spacer':
          return <div aria-hidden="true" style={{ height: item.size }} />
        case 'message': {
          const msg = item.message
          const assistantStats = msg.role === 'assistant'
            ? assistantStreamStatsByMessageId[msg.id]
            : undefined
          return (
            <MessageBubble
              message={msg}
              conversationId={conversationId}
              tokensPerSec={assistantStats?.tokensPerSec}
              reasoningDurationMs={assistantStats?.reasoningDurationMs}
              reasoningDurationMsBySegmentId={assistantStats?.reasoningDurationMsBySegmentId}
              sentModels={item.sentModels}
              onUpdateMessage={msg.role === 'assistant' ? onUpdateMessage : undefined}
              // 编辑/重生成入口在任何 run 在飞时都不可用（AC3）。streamFrozen 也算在飞：
              // 本地取消后 send invoke 尚未返回，此窗口内触发只会被 in-flight 兜底静默吞掉
              // （编辑文本会被无声丢弃），所以从入口处直接收起。
              onRegenerateMessage={streaming || streamFrozen ? undefined : onRegenerateMessage}
              onForkMessage={streaming || streamFrozen ? undefined : onForkMessage}
              onRewindMessage={streaming || streamFrozen ? undefined : onRewindMessage}
              onDeleteMessage={onDeleteMessage}
              onSaveMessageToNote={onSaveMessageToNote}
              agentPlanOverride={msg.id === legacyPlanMessageId ? agentPlanState : null}
              onExecuteAgentPlan={msg.role === 'assistant' ? onExecuteAgentPlan : undefined}
            />
          )
        }
        case 'group': {
          const selectedMessageId = groupSelections[item.groupId] ?? null
          return (
            <MessageGroup
              conversationId={conversationId}
              groupId={item.groupId}
              messages={item.messages}
              selectedMessageId={selectedMessageId}
              onSelectColumn={onSetGroupSelection}
              onUpdateMessage={onUpdateMessage}
              onRegenerateMessage={streaming || streamFrozen ? undefined : onRegenerateMessage}
              onForkMessage={streaming || streamFrozen ? undefined : onForkMessage}
              onDeleteMessage={onDeleteMessage}
              onSaveMessageToNote={onSaveMessageToNote}
            />
          )
        }
        case 'live-group':
          return (
            <MessageGroup
              conversationId={conversationId}
              groupId={item.groupId}
              messages={[]}
              onSaveMessageToNote={onSaveMessageToNote}
            />
          )
        case 'streaming':
          return (
            <MessageBubble
              message={item.message}
              conversationId={conversationId}
              messageStreaming={item.messageStreaming}
              reasoningStreaming={item.reasoningStreaming}
              reasoningDurationMs={streamingReasoningDurationMs}
              reasoningDurationMsBySegmentId={streamingReasoningDurationMsBySegmentId}
            />
          )
        case 'compaction-divider':
          return (
            <CompactionDivider
              boundary={item.boundary}
              lang={lang}
              animate={item.animate}
            />
          )
        case 'compaction-summary':
          return (
            <CompactionSummaryPanel
              boundary={item.boundary}
              lang={lang}
            />
          )
        case 'compaction-progress':
          return <CompactionInProgress lang={lang} />
        case 'error':
          return (
            <div className="chat-motion-fade-up flex flex-col items-start gap-2 py-3">
              <DegradedAnswerCard degraded={streamErrorDegraded(item.text)} />
              {item.retryMessageId && onRetryLastUser && (
                <button
                  type="button"
                  onClick={() => onRetryLastUser(item.retryMessageId!)}
                  className="inline-flex items-center gap-1 rounded-full border border-[var(--border-input)] bg-[var(--bg-input)] px-3 py-1 text-xs font-medium text-neutral-700 transition-colors hover:bg-neutral-50 active:scale-95 dark:bg-neutral-800 dark:text-neutral-200 dark:hover:bg-neutral-700"
                >
                  <RotateCw size={13} strokeWidth={2} />
                  重试
                </button>
              )}
            </div>
          )
      }
    },
    [
      conversationId,
      assistantStreamStatsByMessageId,
      agentPlanState,
      legacyPlanMessageId,
      onUpdateMessage,
      onRegenerateMessage,
      onForkMessage,
      onRewindMessage,
      onDeleteMessage,
      onSaveMessageToNote,
      onExecuteAgentPlan,
      onRetryLastUser,
      streaming,
      streamFrozen,
      groupSelections,
      onSetGroupSelection,
      streamingReasoningDurationMs,
      streamingReasoningDurationMsBySegmentId,
      lang,
    ],
  )

  const renderTail = useCallback(() => (
    <div ref={tailWrapRef}>
      {dynamicItem && (
        <div
          className="pb-0.5"
          data-chat-message-list-item={dynamicItem.kind}
          data-message-id={dynamicItem.kind === 'streaming' ? dynamicItem.message.id : undefined}
        >
          {renderItem(dynamicItem)}
        </div>
      )}
      {errorItem && (
        <div className="pb-0.5" data-chat-message-list-item={errorItem.kind}>
          {renderItem(errorItem)}
        </div>
      )}
      {(messages.length > 0 || streaming) && (
        <StreamStatusLine active={streaming && !streamFrozen && !liveGroup} />
      )}
      <div ref={tailSpacerRef} aria-hidden="true" style={{ height: LIST_EDGE_PADDING_PX }} />
    </div>
  ), [dynamicItem, errorItem, liveGroup, messages.length, renderItem, streaming, streamFrozen])

  return (
    <div className={`relative flex min-h-0 flex-1 flex-col ${navigatorTurnCount >= 4 ? 'has-message-navigator' : ''}`}>
      {navigatorTurnCount >= 4 && (
        <MessageNavigator
          nodes={navigatorNodes}
          activeNodeId={activeNavigatorNodeId}
          visibleNodeIds={visibleNavigatorNodeIds}
          onNavigate={navigateToNavigatorNode}

        />
      )}
      <div
        ref={setScrollEl}
        onContextMenu={handleContextMenu}
        onScroll={handleNavigatorScroll}
        className={`chat-scroll-viewport chat-motion-view-in custom-scrollbar flex-1 overflow-y-auto ${navigatorLockActive ? 'is-navigator-locking' : ''}`}


      >
        <div ref={setContentEl} className="chat-message-list-inner mx-auto w-full max-w-4xl px-6">
          <div
            className="relative w-full"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {virtualItems.map((virtualItem) => {
              const item = itemAt(virtualItem.index)
              if (!item) return null
              const messageId = item.kind === 'message'
                ? item.message.id
                : item.kind === 'streaming'
                  ? item.message.id
                  : undefined
              return (
                <div
                  key={virtualItem.key}
                  ref={import.meta.env.MODE === 'test' ? undefined : virtualizer.measureElement}
                  data-index={virtualItem.index}
                  data-chat-item-key={measurementKey(item)}
                  data-chat-row-index={virtualItem.index}
                  data-message-id={messageId}
                  data-chat-message-list-item={item.kind}
                  className="absolute left-0 top-0 w-full pb-0.5"
                  style={{ transform: `translateY(${virtualItem.start}px)` }}
                >
                  {renderItem(item)}
                </div>
              )
            })}
          </div>
          {/* Chrome always outside: live + status/error/send-reserve. */}
          <div data-chat-message-list-item="tail" className="w-full pb-0.5">
            {renderTail()}
          </div>
        </div>
      </div>
      {/* 上下边界渐变遮罩，纯覆盖层。颜色必须跟 .chat-main-pane 的底色走（浅色 --theme-surface-soft，暗色 #262629）——
          别用 var(--bg)，那个只在 .kv / .settings-embedded 作用域里定义，在聊天区是未定义值，整条 linear-gradient
          会静默失效（表现就是「加了没效果」）。不走 mask-image：那会让整个滚动容器每帧走遮罩合成，长列表上白给。 */}
      <div aria-hidden="true" className="pointer-events-none absolute inset-x-0 top-0 z-[1] h-6 bg-gradient-to-b from-[var(--theme-surface-soft)] to-transparent dark:from-[#262629]" />
      <div aria-hidden="true" className="pointer-events-none absolute inset-x-0 bottom-0 z-[1] h-8 bg-gradient-to-t from-[var(--theme-surface-soft)] to-transparent dark:from-[#262629]" />
      {showJumpButton && (
        <button
          type="button"
          onClick={handleJumpToBottom}
          aria-label="回到底部"
          title="回到底部"
          className="chat-motion-pop absolute bottom-4 left-1/2 z-10 flex h-9 w-9 -translate-x-1/2 items-center justify-center rounded-full border border-[var(--border-input)] bg-[var(--bg-input)] text-neutral-600 shadow-md backdrop-blur transition-transform duration-[var(--kv-dur-instant)] ease-[var(--kv-ease-spring)] hover:text-neutral-900 active:scale-90 dark:text-neutral-300 dark:hover:text-neutral-100"
        >
          <ChevronDown size={18} strokeWidth={2} />
        </button>
      )}
      {msgMenu && (
        <MessageContextMenu
          anchor={msgMenu.anchor}
          hasSelection={msgMenu.selectionText.length > 0}
          canCopyMessage={msgMenu.messageText != null}
          onCopySelection={() => void copyToClipboard(msgMenu.selectionText)}
          onCopyMessage={() => msgMenu.messageText && void copyToClipboard(msgMenu.messageText)}
          onClose={closeMsgMenu}
        />
      )}
      <AddSelectionToChat containerEl={viewportEl} lang={lang} />
    </div>
  )
}

// memo：列表本身订阅 streamingStore，父级 Chat 重渲（非流式 state 变化）时不跟着白渲。
export const MessageList = memo(MessageListBase)
