import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { ChevronDown, RotateCw } from 'lucide-react'
import {
  defaultRangeExtractor,
  measureElement as measureVirtualElement,
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
  estimateMessageRenderHeight,
  estimateMessageRenderCost,
  getCachedRowMeasurement,
  sendReserveHeight,
  restoreMeasurementSnapshot,
  saveMeasurementSnapshot,
  setCachedRowMeasurement,
  shouldAdjustChatItemSizeChange,
} from './messageListVirtualization'
import type { Lang } from '../settings/i18n'
import { measureChatSurface, recordChatPerfSample, useChatPerfRenderProbe } from './chatPerformanceProbe'
import { getChatPerformanceFlags } from './chatPerformanceFlags'

export interface AssistantStreamStats {
  messageId: string
  tokensPerSec: number
  reasoningDurationMs?: number | null
  reasoningDurationMsBySegmentId?: Record<string, number>
}

export interface MessageListProps {
  conversationId?: string | null
  messages: ChatMessage[]
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
}

const LIST_EDGE_PADDING_PX = 16

// 导航器高亮同步的最小间隔。这趟同步是 querySelectorAll + 逐行 getBoundingClientRect，
// 若 virtualizer 在同一帧里刚写过 DOM，第一下 gBCR 就是整文档强制 reflow——每帧跑一次
// 正是滚动不顺滑的主因之一。高亮不需要 120fps，8 次/秒足够。
const NAVIGATOR_SYNC_INTERVAL_MS = 120

// 列表里每一项的统一形态。整条会话全量喂给虚拟列表（消息都在内存，virtualizer 只渲可见项），
// 屏外的气泡连同其 KaTeX host / Markdown / 图片 DOM 真正从 DOM 卸载。
type RenderItem =
  | { kind: 'spacer'; key: 'padding-top' | 'padding-bottom'; size: number }
  | { kind: 'message'; key: string; message: ChatMessage; sentModels?: GroupModelLabel[] }
  | { kind: 'group'; key: string; groupId: string; messages: ChatMessage[] }
  | { kind: 'live-group'; key: string; groupId: string }
  | { kind: 'streaming'; key: 'streaming-assistant'; message: ChatMessage; messageStreaming: boolean; reasoningStreaming: boolean }
  | { kind: 'error'; key: 'error'; text: string; retryMessageId: string | null }
  | { kind: 'tail'; key: 'tail' }
  | { kind: 'compaction-divider'; key: string; boundary: CompactionBoundaryView; animate: boolean }
  | { kind: 'compaction-summary'; key: string; boundary: CompactionBoundaryView }
  | { kind: 'compaction-progress'; key: string; afterIndex: number }

function contentRevision(text: string | undefined): string {
  if (!text) return '0'
  let hash = 2166136261
  for (let index = 0; index < text.length; index += 1) {
    hash = Math.imul(hash ^ text.charCodeAt(index), 16777619)
  }
  return `${text.length}:${(hash >>> 0).toString(36)}`
}

function messageLayoutRevision(message: ChatMessage): string {
  const tools = message.tool_calls ?? message.toolCalls ?? []
  const toolRevision = tools.map((tool) => [
    tool.id,
    tool.name ?? tool.tool_name ?? tool.toolName,
    tool.status,
    contentRevision(tool.argument_preview ?? tool.argumentPreview ?? tool.argumentsPreview),
    contentRevision(tool.result_preview ?? tool.resultPreview),
    (tool.artifacts ?? []).map((artifact) => [
      artifact.name,
      artifact.mime_type ?? artifact.mimeType,
      artifact.path ?? artifact.filePath ?? artifact.localPath,
      artifact.size_bytes ?? artifact.sizeBytes,
    ].join(':')).join(','),
  ].join(':')).join('|')
  return [
    contentRevision(message.content),
    contentRevision(message.reasoning),
    message.segments?.map((segment) => `${segment.id}:${segment.kind}:${contentRevision(segment.text ?? undefined)}`).join('|') ?? '',
    message.attachments?.map((attachment) => `${attachment.id}:${attachment.type}:${attachment.name}`).join('|') ?? '',
    (message.artifacts ?? []).map((artifact) => `${artifact.name}:${artifact.mime_type ?? artifact.mimeType}:${artifact.path ?? artifact.filePath ?? artifact.localPath}:${artifact.size_bytes ?? artifact.sizeBytes}`).join('|'),
    toolRevision,
  ].join('::')
}

function measurementKey(item: RenderItem): string {
  if (item.kind === 'message') {
    return `${item.key}:${messageLayoutRevision(item.message)}`
  }
  if (item.kind === 'group') {
    return `${item.key}:${item.messages.map((message) => `${message.id}:${messageLayoutRevision(message)}`).join('|')}`
  }
  if (item.kind === 'streaming') {
    return `${item.key}:${messageLayoutRevision(item.message)}`
  }
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
}: MessageListProps) {
  const chatPerfFlags = getChatPerformanceFlags()
  const useTanStackVirtualizer = chatPerfFlags.tanstackVirtualizer
  const externalizeLiveRow = chatPerfFlags.liveRowExternalization
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

  const scrollRef = useRef<HTMLDivElement | null>(null)
  // hook 需要通过 state 拿到元素以便重新绑定监听；virtualizer 需要 RefObject。回调 ref 同时喂两者。
  const [viewportEl, setViewportEl] = useState<HTMLDivElement | null>(null)
  const [contentEl, setContentEl] = useState<HTMLDivElement | null>(null)
  const [contentWidth, setContentWidth] = useState(712)
  const setScrollEl = useCallback((el: HTMLDivElement | null) => {
    scrollRef.current = el
    setViewportEl(el)
  }, [])

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
      const next = Math.max(280, Math.round(width))
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
  // 消息区内置右键菜单（原生菜单被全局屏蔽，见 main.tsx）。
  const [msgMenu, setMsgMenu] = useState<
    { anchor: MessageMenuAnchor; selectionText: string; messageText: string | null } | null
  >(null)

  // 底部跟随：contentGrowth 钉底 + 近底历史实挂载，避免与 virtualizer remeasure 互抢。
  const { handle: followHandle, showJumpButton } = useScrollFollow({
    viewport: viewportEl,
    content: contentEl,
    trackKeys: true,
  })

  // ResizeObserver 是主要的流式钉底驱动，但虚拟列表/浏览器可能先更新内容和 scrollTop，
  // 再投递 ResizeObserver，偶尔会漏掉这一轮高度变化。流式快照已经是最新 DOM 的提交信号，
  // 在 layout effect 里补一次同步；用户滚轮/触摸上移时 followHandle 会先变成 false，
  // 因此不会把用户主动停留在历史位置的视口重新拽到底部。
  useLayoutEffect(() => {
    if (!streaming || !followHandle.isFollowing()) return
    const viewport = scrollRef.current
    if (viewport) {
      const gap = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight
      if (gap <= 1) return
    }
    followHandle.stickToBottom()
  }, [followHandle, snapshot, streaming])

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

  const liveItem = useMemo<RenderItem | null>(() => {
    const hasLiveGroup = Boolean(liveGroup && (coarse.streaming || coarse.streamFrozen))
    const hasStreamingPreview =
      !hasLiveGroup &&
      (streaming || streamFrozen) &&
      (streamingContent || streamingReasoning || streamingToolCalls.length > 0 || streamingSegments.length > 0)
    if (hasLiveGroup && liveGroup) {
      return { kind: 'live-group', key: `live-group-${liveGroup.groupId}`, groupId: liveGroup.groupId }
    }
    if (hasStreamingPreview) {
      return {
        kind: 'streaming',
        key: 'streaming-assistant',
        messageStreaming: streaming && !streamFrozen,
        reasoningStreaming: reasoningStreaming && !streamFrozen,
        message: {
          id: 'streaming-assistant',
          role: 'assistant',
          content: streamingContent,
          reasoning: streamingReasoning || undefined,
          artifacts: [],
          tool_calls: streamingToolCalls,
          segments: streamingSegments,
          timestamp: Math.floor(Date.now() / 1000),
        },
      }
    }
    return null
  }, [
    coarse.streaming,
    coarse.streamFrozen,
    liveGroup,
    reasoningStreaming,
    streamFrozen,
    streaming,
    streamingContent,
    streamingReasoning,
    streamingSegments,
    streamingToolCalls,
  ])

  const fallbackLiveItem = externalizeLiveRow ? null : liveItem

  // 历史项只在消息/压缩边界/组模型身份变化时重建。默认高频流式文本不进入依赖；
  // 关闭 live row 外置开关时，刻意把 live item 放回 timeline 作为回退路径。
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
          key: `group-${item.groupId}`,
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
        list.push({ kind: 'message', key: message.id, message, sentModels })
        const index = messageIndexById.get(message.id)
        if (index != null) appendCompactionSlot(list, index)
      }
    }

    if (fallbackLiveItem) list.push(fallbackLiveItem)

    return list
  }, [appendCompactionSlot, fallbackLiveItem, folded, liveGroupModels, messageIndexById])

  // 默认路径把高频 live row 放到固定 tail；关闭开关时回退到普通 timeline，
  // 便于诊断外置 store / tail 测量问题而不改变消息数据和 virtualizer。
  const dynamicItem = externalizeLiveRow ? liveItem : null

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

  // 计算每一行的初始估算高度。真实高度由 TanStack Virtual 的 measureElement
  // 覆盖；估算只负责首次切换/首次滚动时快速建立窗口，不再把整份历史拆成两套 DOM。
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
    return map
  }, [contentWidth, historyItems, layoutKey, tailMeasurementKey])

  const tailItem = useMemo<RenderItem>(() => ({ kind: 'tail', key: 'tail' }), [])
  const itemCount = historyItems.length + 1
  const historyItemsRef = useRef<RenderItem[]>(historyItems)
  historyItemsRef.current = historyItems
  const itemAt = useCallback((index: number) => (
    index < historyItemsRef.current.length ? historyItemsRef.current[index] : tailItem
  ), [tailItem])
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
    () => restoreMeasurementSnapshot(conversationId, layoutKey, historyMeasurementRevision),
    [conversationId, historyMeasurementRevision, layoutKey],
  )
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: useTanStackVirtualizer ? itemCount : 0,
    enabled: useTanStackVirtualizer,
    getScrollElement: () => scrollRef.current,
    // TanStack's index navigation and measurement corrections must share the
    // same scroll authority as chat navigation and follow pinning. This keeps
    // programmatic scroll writes source-classified by useScrollFollow.
    scrollToFn: (offset, options) => followHandle.scrollToOffset(offset, options),
    observeElementRect: observeRect,
    estimateSize: (index) => estimateSizeRef.current.get(itemAt(index)?.key ?? 'tail') ?? 96,
    getItemKey: (index) => itemAt(index)?.key ?? `row-${index}`,
    initialMeasurementsCache,
    measureElement: (element, entry, instance) => {
      followHandle.markLayoutCompensation()
      const measured = measureVirtualElement(element, entry, instance)
      const key = element.dataset.chatItemKey
      if (key) {
        const size = Math.max(1, measured)
        const index = Number(element.dataset.index)
        const logicalKey = Number.isInteger(index) ? itemAt(index)?.key : undefined
        if (logicalKey) estimateSizeRef.current.set(logicalKey, size)
        setCachedRowMeasurement(layoutKey, key, size)
        return size
      }
      return measured
    },
    rangeExtractor: useCallback((range: Range) => {
      const indexes = defaultRangeExtractor(range)
      const tailIndex = itemCount - 1
      if (tailIndex >= 0 && !indexes.includes(tailIndex)) indexes.push(tailIndex)
      return indexes
    }, [itemCount]),
    overscan: 6,
    // jsdom/test environments have no layout box before the first observer tick;
    // a conservative initial viewport keeps the first render useful while the
    // real browser immediately replaces it with the measured client rect.
    initialRect: { width: 0, height: viewportEl?.clientHeight || 800 },
    anchorTo: 'end',
    scrollEndThreshold: 12,
    followOnAppend: false,
    useAnimationFrameWithResizeObserver: false,
    useFlushSync: false,
  })
  // TanStack's default is the safe baseline: only rows entirely above the
  // reading anchor move the detached viewport, and re-measurements during
  // backward scrolling do not adjust it. The one chat-specific exception is
  // a growing live tail row that straddles the viewport top: its new content is
  // appended below the reader and must not pull history downward.
  virtualizer.shouldAdjustScrollPositionOnItemSizeChange = (item, delta, instance) => {
    const shouldAdjust = shouldAdjustChatItemSizeChange(item, {
      scrollOffset: instance.scrollOffset ?? 0,
      scrollAdjustments: instance.scrollAdjustments,
      itemSizeCache: instance.itemSizeCache,
      scrollDirection: instance.scrollDirection,
    })
    if (!shouldAdjust) return false
    const viewportTop = (instance.scrollOffset ?? 0) + instance.scrollAdjustments
    const liveRowIndex = externalizeLiveRow
      ? historyItems.length
      : historyItems.findIndex((entry) => entry.kind === 'streaming' || entry.kind === 'live-group')
    const liveTail = liveRowIndex >= 0 && (streaming || streamFrozen || Boolean(liveGroup))
    if (
      liveTail
      && item.index === liveRowIndex
      && delta > 0
      && item.end > viewportTop
      && !followHandle.isFollowing()
    ) {
      return false
    }
    return true
  }
  const virtualItems = virtualizer.getVirtualItems()
  // Row ResizeObservers and TanStack's own viewport observer update mounted rows.
  // Avoid a blanket measure(): it clears the virtualizer's measured cache and makes
  // detached readers pay the estimate-to-real-height correction for every row.

  const saveMeasurementSnapshotRef = useRef<() => void>(() => {})
  saveMeasurementSnapshotRef.current = () => {
    if (!useTanStackVirtualizer || !viewportEl) return
    saveMeasurementSnapshot(
      conversationId,
      layoutKey,
      historyMeasurementRevision,
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

  const navigateToNavigatorNode = useCallback((node: MessageNavigatorNode) => {
    // 跳到上方消息：先脱离跟随，否则跟随纠正器会把视口又钉回底部。
    followHandle.releaseFollow()
    updateActiveNavigatorNode(node.id)

    const el = scrollRef.current
    // data-chat-row-index = 全历史逻辑下标（与 navigator targetRenderIndex 一致）
    const row = contentEl?.querySelector(
      `[data-chat-row-index="${node.targetRenderIndex}"]`,
    ) as HTMLElement | null
    if (row && el) {
      // 只滚这个视口。scrollIntoView 会连带滚动所有可滚祖先。
      // 瞬时跳转，不用 behavior:'smooth'：top 是按目标行**当前**的几何算出来的，而平滑滚动
      // 期间上方的行会被 virtualizer 重测（估算高度换成实测），目标位置在动画途中就挪走了 ——
      // 距离越远越容易落在错的地方。回到底部按钮不受影响，那个是自己驱动的 rAF，每帧重算目标。
      followHandle.scrollToOffset(
        row.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop,
      )
      return
    }

    // 当前目标未挂载时交给同一个 virtualizer 估算定位；真实行出现后
    // TanStack 会用测量结果收敛位置，不再需要独立的 revealedStart/scrollHeight 补偿链。
    if (node.targetRenderIndex >= 0 && node.targetRenderIndex < historyItems.length) {
      followHandle.markLayoutCompensation()
      virtualizer.scrollToIndex(node.targetRenderIndex, { align: 'start', behavior: 'auto' })
    }
  }, [contentEl, followHandle, historyItems.length, updateActiveNavigatorNode, virtualizer])

  const handleNavigatorStep = useCallback((direction: -1 | 1) => {
    const nodes = navigatorNodesRef.current
    if (nodes.length === 0) return
    const currentId = activeNavigatorNodeIdRef.current
    const currentIndex = Math.max(0, nodes.findIndex((node) => node.id === currentId))
    const nextIndex = Math.min(nodes.length - 1, Math.max(0, currentIndex + direction))
    navigateToNavigatorNode(nodes[nextIndex])
  }, [navigateToNavigatorNode])

  const handleJumpToBottom = useCallback(() => {
    followHandle.jumpToBottom()
  }, [followHandle])

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

  // 消息区右键：读取当前选中文本 + 命中的消息，弹内置菜单。两者都空则不弹（放行给全局屏蔽）。
  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    const selectionText = (window.getSelection()?.toString() ?? '').trim()
    const targetEl = (e.target as Element | null)?.closest?.('[data-message-id]') as HTMLElement | null
    const id = targetEl?.dataset.messageId ?? null
    let messageText: string | null = null
    if (id === 'streaming-assistant') {
      messageText = streamingContent.trim() || null
    } else if (id) {
      messageText = messages.find((m) => m.id === id)?.content?.trim() || null
    }
    if (!selectionText && !messageText) return
    e.preventDefault()
    const left = Math.min(e.clientX, window.innerWidth - 184)
    const top = Math.min(e.clientY, window.innerHeight - 96)
    setMsgMenu({ anchor: { left, top }, selectionText, messageText })
  }, [messages, streamingContent])

  const closeMsgMenu = useCallback(() => setMsgMenu(null), [])

  // 尾部作为 virtualizer 的单一 live row 挂载。它内部仍保留流式气泡、错误、状态线和
  // 发送预留，因此增长中的内容只会改变这一个行的测量，不会让整份历史重新参与布局。
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
      if (streaming) {
        wrap.style.minHeight = `${Math.round(reserve)}px`
        // 留白交还给 minHeight：不还的话上一轮量出来的高度会和 minHeight 叠成两段预留。
        spacer.style.height = `${LIST_EDGE_PADDING_PX}px`
        return
      }
      wrap.style.minHeight = ''
      if (!reserveHandoffRef.current || !row) {
        spacer.style.height = `${LIST_EDGE_PADDING_PX}px`
        return
      }
      // 跨度用 spacer 自己的顶边量，与 spacer 当前高度无关，避免自反馈。
      const span = spacer.getBoundingClientRect().top - row.getBoundingClientRect().bottom
      spacer.style.height = `${Math.max(LIST_EDGE_PADDING_PX, Math.round(reserve - span))}px`
    }

    apply()
    // 视口高度会变（ask_user 面板出现/消失、输入框长高、窗口 resize），每次都得重算预留。
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(apply)
    observer.observe(viewportEl)
    return () => observer.disconnect()
  }, [streaming, messages, contentEl, viewportEl])

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
        <div className="pb-0.5" data-chat-message-list-item={dynamicItem.kind} data-message-id="streaming-assistant">
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
          onNavigateStep={handleNavigatorStep}
        />
      )}
      <div
        ref={setScrollEl}
        onContextMenu={handleContextMenu}
        onScroll={scheduleNavigatorSync}
        className="chat-motion-view-in custom-scrollbar flex-1 overflow-y-auto"
      >
        <div ref={setContentEl} className="chat-message-list-inner mx-auto w-full max-w-4xl px-6">
          <div
            className="relative w-full"
            style={useTanStackVirtualizer ? { height: virtualizer.getTotalSize() } : undefined}
          >
            {useTanStackVirtualizer ? virtualItems.map((virtualItem) => {
              const item = virtualItem.index < historyItems.length
                ? historyItems[virtualItem.index]
                : tailItem
              if (!item) return null
              const logicalIndex = virtualItem.index < historyItems.length
                ? virtualItem.index
                : undefined
              return (
                <div
                  key={virtualItem.key}
                  ref={import.meta.env.MODE === 'test' ? undefined : virtualizer.measureElement}
                  data-index={virtualItem.index}
                  data-chat-item-key={item.kind === 'tail' ? tailMeasurementKey : measurementKey(item)}
                  data-chat-row-index={logicalIndex}
                  data-message-id={item.kind === 'message' ? item.message.id : undefined}
                  data-chat-message-list-item={item.kind}
                  className="absolute left-0 top-0 w-full pb-0.5"
                  style={{ transform: `translateY(${virtualItem.start}px)` }}
                >
                  {item.kind === 'tail' ? renderTail() : renderItem(item)}
                </div>
              )
            }) : (
              <>
                {historyItems.map((item, index) => (
                  <div
                    key={item.key}
                    data-index={index}
                    data-chat-row-index={index}
                    data-message-id={item.kind === 'message' ? item.message.id : undefined}
                    data-chat-message-list-item={item.kind}
                    className="w-full pb-0.5"
                  >
                    {renderItem(item)}
                  </div>
                ))}
                <div data-chat-message-list-item="tail" className="w-full pb-0.5">
                  {renderTail()}
                </div>
              </>
            )}
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
