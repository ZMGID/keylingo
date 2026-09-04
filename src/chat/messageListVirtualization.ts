import type { VirtualItem } from '@tanstack/react-virtual'
import { normalizeToolCallStatus } from './toolStatus'
import type { ChatMessage } from './types'

/**
 * Chat row geometry shared by the TanStack virtualizer:
 * content-aware estimates, bounded measurement cache, and reserve height.
 */

const MEASUREMENT_CACHE_LIMIT = 8
type MeasurementBucket = {
  values: Map<string, number>
  touchedAt: number
}
const measurementBuckets = new Map<string, MeasurementBucket>()
let measurementClock = 0

type MeasurementSnapshot = {
  layoutKey: string
  revision: string
  measurements: VirtualItem[]
  touchedAt: number
}
const measurementSnapshots = new Map<string, MeasurementSnapshot>()

const CONTENT_REVISION_FULL_HASH_MAX = 8192
const CONTENT_REVISION_WINDOWS = 32
const CONTENT_REVISION_WINDOW_CHARS = 128

function fnv1aRange(text: string, start: number, end: number, hash: number): number {
  const last = Math.min(end, text.length)
  for (let index = Math.max(0, start); index < last; index += 1) {
    hash = Math.imul(hash ^ text.charCodeAt(index), 16777619)
  }
  return hash
}

/**
 * Change detector for measurement keys. Full FNV on typical messages; long
 * blobs hash length + evenly spaced windows so a cold switch does not scan
 * megabytes. Same-length mid-edits that land in a window still move the key;
 * mounted rows still correct via measureElement if a gap is missed.
 */
export function contentRevision(text: string | undefined | null): string {
  if (!text) return '0'
  let hash = 2166136261
  if (text.length <= CONTENT_REVISION_FULL_HASH_MAX) {
    hash = fnv1aRange(text, 0, text.length, hash)
    return `${text.length}:${(hash >>> 0).toString(36)}`
  }
  const lastStart = text.length - CONTENT_REVISION_WINDOW_CHARS
  const span = Math.max(1, lastStart)
  const lastWindow = CONTENT_REVISION_WINDOWS - 1
  for (let window = 0; window < CONTENT_REVISION_WINDOWS; window += 1) {
    const start = window === lastWindow
      ? lastStart
      : Math.floor((window * span) / lastWindow)
    hash = fnv1aRange(text, start, start + CONTENT_REVISION_WINDOW_CHARS, hash)
  }
  return `${text.length}:${(hash >>> 0).toString(36)}`
}

// Messages are replaced rather than mutated. WeakMap keeps stream rerenders cheap and
// lets revisions disappear with their message objects after a conversation is released.
const bodyLayoutRevisionByMessage = new WeakMap<ChatMessage, string>()
const layoutRevisionByMessage = new WeakMap<ChatMessage, string>()

/** Fields that can change MessageBubble's body geometry (everything above its meta row). */
export function chatMessageBodyLayoutRevision(message: ChatMessage): string {
  const cached = bodyLayoutRevisionByMessage.get(message)
  if (cached !== undefined) return cached

  const tools = message.tool_calls ?? message.toolCalls ?? []
  // 状态走 normalizeToolCallStatus：流式事件里的工具记录是 `completed`，落库后是 `success`，
  // ToolCallBlock 渲染前也是先归一化的 —— 原始字符串不同并不代表卡片几何不同。
  // 直接比原始值会让 canReuseLiveRowHeight 在每一条带工具调用的回复上都判「不可复用」，
  // twin 首帧退回估高（探针实测 +501 vs 实高 380，scrollTop 337→459→337 抖一帧）。
  const toolRevision = tools.map((tool) => [
    tool.id,
    tool.name ?? tool.tool_name ?? tool.toolName,
    normalizeToolCallStatus(tool.status),
    contentRevision(tool.argument_preview ?? tool.argumentPreview ?? tool.argumentsPreview),
    contentRevision(tool.result_preview ?? tool.resultPreview),
    contentRevision(tool.error),
    (tool.artifacts ?? []).map((artifact) => [
      artifact.id,
      artifact.name,
      artifact.mime_type ?? artifact.mimeType,
      artifact.path ?? artifact.filePath ?? artifact.localPath,
      artifact.size_bytes ?? artifact.sizeBytes,
      contentRevision(artifact.data_url ?? artifact.dataUrl),
    ].join(':')).join(','),
  ].join(':')).join('|')
  const agentPlan = message.agent_plan ?? message.agentPlan
  const degraded = message.degraded
  // 有时间线分段时 MessageBubble 只渲染 segments（顶层 content / reasoning 的 ReasoningBlock 与
  // 「回答」区都被 !hasTimelineSegments 关掉），正文几何与这两个字符串无关。而它们在
  // live 与落库 twin 之间确实不同 —— 后端把多步文本 / 推理拼成 content 时的分隔符与前端
  // 流式累加的不一致（探针：content 325 vs 289、reasoning 414 vs 416，segments 逐字相同）。
  // 只在无分段的旧式消息上才让它们参与 revision。
  // （降级消息会按文本剔掉分段，可能退回 content 路径，保守起见仍算上正文。）
  const hasTimeline = (message.segments?.length ?? 0) > 0 && !degraded
  const revision = [
    hasTimeline ? 'seg' : contentRevision(message.content),
    hasTimeline ? 'seg' : contentRevision(message.reasoning),
    message.segments?.map((segment) => [
      segment.id,
      segment.kind,
      segment.phase,
      segment.order,
      segment.step_number ?? segment.stepNumber,
      segment.round,
      segment.tool_call_id ?? segment.toolCallId,
      contentRevision(segment.text),
    ].join(':')).join('|') ?? '',
    message.attachments?.map((attachment) => [
      attachment.id,
      attachment.type,
      attachment.name,
      attachment.path,
      contentRevision(attachment.content),
    ].join(':')).join('|') ?? '',
    (message.artifacts ?? []).map((artifact) => [
      artifact.id,
      artifact.name,
      artifact.mime_type ?? artifact.mimeType,
      artifact.path ?? artifact.filePath ?? artifact.localPath,
      artifact.size_bytes ?? artifact.sizeBytes,
      contentRevision(artifact.data_url ?? artifact.dataUrl),
    ].join(':')).join('|'),
    toolRevision,
    agentPlan ? [agentPlan.mode, agentPlan.status, contentRevision(agentPlan.plan)].join(':') : '',
    degraded ? [
      degraded.kind,
      contentRevision(degraded.reason),
      contentRevision(degraded.detail),
      contentRevision(degraded.text),
      degraded.toolSummaries?.map((summary) => `${summary.name}:${contentRevision(summary.preview)}`).join('|') ?? '',
    ].join(':') : '',
  ].join('::')
  bodyLayoutRevisionByMessage.set(message, revision)
  return revision
}

function visibleMetaRevision(message: ChatMessage): string {
  const runEntry = message.run_entry ?? message.runEntry
  const streamOutcome = message.stream_outcome ?? message.streamOutcome
  const usage = message.usage
  return [
    runEntry === 'regenerate' ? runEntry : '',
    streamOutcome === 'cancelled' || streamOutcome === 'error' || streamOutcome === 'interrupted'
      ? streamOutcome
      : '',
    usage ? [
      usage.input_tokens ?? usage.inputTokens,
      usage.output_tokens ?? usage.outputTokens,
      usage.total_tokens ?? usage.totalTokens,
      usage.cached_input_tokens ?? usage.cachedInputTokens,
      usage.cache_creation_input_tokens ?? usage.cacheCreationInputTokens,
      usage.reasoning_tokens ?? usage.reasoningTokens,
    ].join(':') : '',
  ].join('::')
}

/** Full geometry revision, including the assistant footer that can wrap onto another line. */
export function chatMessageLayoutRevision(message: ChatMessage): string {
  const cached = layoutRevisionByMessage.get(message)
  if (cached !== undefined) return cached
  const revision = [
    chatMessageBodyLayoutRevision(message),
    visibleMetaRevision(message),
  ].join('::')
  layoutRevisionByMessage.set(message, revision)
  return revision
}

/**
 * Seed the outside live row's height onto the settled twin when the body
 * matches. Footer extras (usage / stream_outcome / stats) are tens of pixels;
 * skipping the seed falls back to an estimate that can be hundreds short.
 * measureElement / RO add the footer delta on the next frame.
 */
export function canReuseLiveRowHeight(live: ChatMessage, settled: ChatMessage): boolean {
  return chatMessageBodyLayoutRevision(live) === chatMessageBodyLayoutRevision(settled)
}

/**
 * A mounted absolute row must correct stale TanStack cache before paint. ResizeObserver
 * entries remain the cheap steady-state path; callback-ref mounts read the real DOM box.
 */
export function measureChatVirtualRow(
  element: HTMLElement,
  entry: ResizeObserverEntry | undefined,
  horizontal = false,
): number {
  if (entry?.borderBoxSize) {
    const box = entry.borderBoxSize[0]
    if (box) return Math.round(box[horizontal ? 'inlineSize' : 'blockSize'])
  }
  return element[horizontal ? 'offsetWidth' : 'offsetHeight']
}

/** Prevent TanStack's internal itemSizeCache from crossing width layouts. */
export function layoutScopedVirtualKey(layoutKey: string, rowKey: string): string {
  return `${layoutKey}:${rowKey}`
}

function measurementBucket(layoutKey: string): MeasurementBucket {
  const existing = measurementBuckets.get(layoutKey)
  if (existing) {
    existing.touchedAt = ++measurementClock
    return existing
  }
  const bucket: MeasurementBucket = { values: new Map(), touchedAt: ++measurementClock }
  measurementBuckets.set(layoutKey, bucket)
  if (measurementBuckets.size > MEASUREMENT_CACHE_LIMIT) {
    let oldestKey: string | null = null
    let oldestTime = Number.POSITIVE_INFINITY
    for (const [key, candidate] of measurementBuckets) {
      if (candidate.touchedAt < oldestTime) {
        oldestKey = key
        oldestTime = candidate.touchedAt
      }
    }
    if (oldestKey) measurementBuckets.delete(oldestKey)
  }
  return bucket
}

/**
 * Row heights are layout-dependent: the same Markdown wraps differently when the
 * sidebar changes the content width. Keep a small LRU by conversation + width
 * bucket so switching back does not remeasure every heavy row from scratch.
 */
export function getCachedRowMeasurement(layoutKey: string, rowKey: string): number | undefined {
  return measurementBucket(layoutKey).values.get(rowKey)
}

export function setCachedRowMeasurement(layoutKey: string, rowKey: string, size: number): void {
  if (!Number.isFinite(size) || size <= 0) return
  measurementBucket(layoutKey).values.set(rowKey, size)
}

export function clearRowMeasurementCache(): void {
  measurementBuckets.clear()
  measurementSnapshots.clear()
  measurementClock = 0
}

export function restoreMeasurementSnapshot(
  conversationId: string | null | undefined,
  layoutKey: string,
  revision: string,
): VirtualItem[] {
  if (!conversationId || !layoutKey || !revision) return []
  const snapshot = measurementSnapshots.get(conversationId)
  if (!snapshot || snapshot.layoutKey !== layoutKey || snapshot.revision !== revision) return []
  snapshot.touchedAt = ++measurementClock
  return snapshot.measurements
}

export function saveMeasurementSnapshot(
  conversationId: string | null | undefined,
  layoutKey: string,
  revision: string,
  measurements: VirtualItem[],
): void {
  if (!conversationId || !layoutKey || !revision || measurements.length === 0) return
  measurementSnapshots.delete(conversationId)
  measurementSnapshots.set(conversationId, {
    layoutKey,
    revision,
    measurements,
    touchedAt: ++measurementClock,
  })
  while (measurementSnapshots.size > MEASUREMENT_CACHE_LIMIT) {
    let oldestKey: string | null = null
    let oldestTime = Number.POSITIVE_INFINITY
    for (const [key, candidate] of measurementSnapshots) {
      if (candidate.touchedAt < oldestTime) {
        oldestKey = key
        oldestTime = candidate.touchedAt
      }
    }
    if (oldestKey) measurementSnapshots.delete(oldestKey)
  }
}

/**
 * 估算一段 markdown 渲染出来大概有多少个 DOM 节点。纯字符串扫描，不解析 markdown。
 *
 * 两个系数是拿真实会话反推的，不是拍的：
 *   大对话 231 个围栏 / 52673 字符 → 估 4971，实测 5433 节点
 *   小对话   2 个围栏 / 47885 字符 → 估  359，实测  484 节点
 * 一个代码块的固定外壳 + token span 约 20 个节点，所以围栏权重 20；其余散文按字符摊。
 * 表格单独算只占 13%，并进字符项，少一个要调的旋钮。
 */
const COST_PER_FENCE = 20
const COST_CHARS_PER_UNIT = 150
const ESTIMATE_SAMPLE_CHARS = 8_192

function textEstimateSample(text: string): string {
  if (text.length <= ESTIMATE_SAMPLE_CHARS) return text
  const half = ESTIMATE_SAMPLE_CHARS / 2
  return `${text.slice(0, half)}\n${text.slice(-half)}`
}

function estimatedFenceCount(text: string): number {
  const sample = textEstimateSample(text)
  const sampleFences = (sample.match(/^\s{0,3}```/gm)?.length ?? 0) / 2
  if (sample.length === text.length) return sampleFences
  return sampleFences * (text.length / sample.length)
}

/**
 * MessageBubble 自身与非正文结构的成本。
 *
 * 工具组折叠时确实不会挂载组内 ToolCallBlock，但挂载 MessageBubble 仍会遍历 tool_calls / segments，
 * 做引用匹配、孤立工具筛选、分组和摘要计算；附件/产物还可能直接生成预览 DOM。旧判据只看
 * Markdown，导致“正文不多、工具很多”的会话被误判成轻会话、整本挂载。
 *
 * 这些权重估的是前端处理与摘要 DOM 的相对成本，不代表折叠工具详情已经挂进 DOM。
 */
const COST_PER_MESSAGE_SHELL = 6
const COST_PER_TOOL_CALL = 6
const COST_PER_TIMELINE_SEGMENT = 2
const COST_PER_ATTACHMENT = 12
const COST_PER_ARTIFACT = 12

export function estimateRenderCost(text: string): number {
  if (!text) return 0
  // 只抽样首尾固定字符。估算用于 virtualizer，不值得在会话切换时全量扫描正文。
  const fences = estimatedFenceCount(text)
  return Math.round(fences * COST_PER_FENCE + text.length / COST_CHARS_PER_UNIT)
}

export type MessageRenderCostInput = {
  texts: readonly string[]
  toolCallCount?: number
  timelineSegmentCount?: number
  attachmentCount?: number
  artifactCount?: number
}

/** 估算挂载一条 MessageBubble 的总成本：正文 + 消息外壳 + 折叠态仍需处理的结构化数据。 */
export function estimateMessageRenderCost({
  texts,
  toolCallCount = 0,
  timelineSegmentCount = 0,
  attachmentCount = 0,
  artifactCount = 0,
}: MessageRenderCostInput): number {
  const textCost = texts.reduce((sum, text) => sum + estimateRenderCost(text), 0)
  return textCost
    + COST_PER_MESSAGE_SHELL
    + toolCallCount * COST_PER_TOOL_CALL
    + timelineSegmentCount * COST_PER_TIMELINE_SEGMENT
    + attachmentCount * COST_PER_ATTACHMENT
    + artifactCount * COST_PER_ARTIFACT
}

/**
 * 估算一段 markdown 渲染出来**大概多高**（px）。和 estimateRenderCost 是两套系数：
 * 前者估节点数（决定渲染贵不贵），这里估像素（决定滚动条准不准），两者不成比例
 * —— 一个代码块外壳很贵但不高，一段长散文很便宜但很高。
 *
 * 用途只有一个：喂给 virtualizer 的 `estimateSize`。**不给它的话 virtualizer 会拿已测量的行去外推屏外的行**，
 * 而我们把实挂载尾部砍到 3~4 条之后，它只能拿这几条去推上面十几条；行高差 30 倍，
 * 推出来必然错，错了就是「往上翻历史内容跳」「拖滚动条跳」。
 *
 * 这**不是**第二个高度估算器 —— estimateSize 是 virtualizer 自己的输入，我们只是别让它瞎猜。
 */
const HEIGHT_PER_FENCE_PX = 24
const HEIGHT_PER_LINE_PX = 25

// CJK 字形约等于 2 个拉丁字符宽（15px 字号下 ~15px vs ~8px）。charsPerLine 按 width/8
// 是拉丁字宽——中文正文直接除会把换行数低估近一半，整条消息估高只有真实一半，
// 回翻/拖滚动条时估算→实测的纠正幅度巨大。与 Rust 侧 chunking.rs 的 CJK token
// 估算是同一类修正。范围取常用 CJK 统一表意 + 扩展 A + 兼容表意 + 全角标点/假名。
const CJK_CHAR_RE = /[\u2e80-\u9fff\uf900-\ufaff\uff00-\uffef\u3000-\u303f]/g

export function estimateRenderHeight(text: string, contentWidth = 560): number {
  if (!text) return 0
  const charsPerLine = Math.max(28, Math.min(100, Math.floor(contentWidth / 8)))
  const sample = textEstimateSample(text)
  const sampleNewlines = sample.match(/\n/g)?.length ?? 0
  const estimatedLogicalLines = Math.max(
    1,
    (sampleNewlines + 1) * (text.length / Math.max(1, sample.length)),
  )
  // 用样本里的 CJK 占比给全文加权：每个 CJK 字符按 2 个拉丁单位计宽。
  const cjkCount = sample.match(CJK_CHAR_RE)?.length ?? 0
  const cjkRatio = cjkCount / Math.max(1, sample.length)
  const effectiveLength = text.length * (1 + cjkRatio)
  const estimatedWrappedLines = Math.max(
    estimatedLogicalLines,
    Math.ceil(effectiveLength / charsPerLine),
  )
  return Math.round(
    estimatedWrappedLines * HEIGHT_PER_LINE_PX
    + estimatedFenceCount(text) * HEIGHT_PER_FENCE_PX,
  )
}

export type MessageRenderHeightInput = {
  texts: readonly string[]
  width: number
  toolCallCount?: number
  attachmentCount?: number
  artifactCount?: number
}

export function estimateMessageRenderHeight({
  texts,
  width,
  toolCallCount = 0,
  attachmentCount = 0,
  artifactCount = 0,
}: MessageRenderHeightInput): number {
  return 64
    + texts.reduce((sum, text) => sum + estimateRenderHeight(text, width), 0)
    + toolCallCount * 56
    // 附件卡固定 64px 高（与输入框同一形态），不再按原图像素估 120。
    + attachmentCount * 80
    + artifactCount * 96
}

export type ChatItemResizeContext = {
  scrollOffset: number
  scrollAdjustments: number
  itemSizeCache: ReadonlyMap<number | string | bigint, number>
  scrollDirection: 'forward' | 'backward' | null
}

/**
 * Resize-compensation baseline aligned with LiveAgent / TanStack 3.17 default:
 * - only rows starting above the reading anchor may shift scrollTop
 * - first measurements always compensate (estimate→actual must land)
 * - re-measurements during backward scroll are skipped
 *
 * The live-row growth carve-out (streaming append below a detached reader) is
 * applied by MessageList on top of this predicate.
 */
export function shouldAdjustChatItemSizeChange(
  item: Pick<VirtualItem, 'key' | 'start' | 'end'>,
  context: ChatItemResizeContext,
): boolean {
  const anchor = context.scrollOffset + context.scrollAdjustments
  // Upstream default: only above-viewport resizes may shift scrollTop.
  if (item.start >= anchor) return false
  // Upstream default: re-measurements are skipped during backward scroll;
  // first measurements always compensate.
  if (context.itemSizeCache.has(item.key) && context.scrollDirection === 'backward') {
    return false
  }
  return true
}

/**
 * 发送后尾部预留的高度。基准是**滚动视口**的实测高度，不是窗口高 —— ask_user 面板吊在输入框
 * 上方、在滚动区之外，它一出现视口就矮一大截，按窗口算的预留会比视口还高、把刚发出的那条消息
 * 整个顶出屏幕。所以再夹一道「视口 - 锚点行高 - 上下留白」：比例给多大，那条消息都得留在屏幕里。
 */
export const SEND_RESERVE_RATIO = 0.45

export function sendReserveHeight(viewportH: number, anchorH: number, edgePadding: number): number {
  return Math.max(
    0,
    Math.min(viewportH * SEND_RESERVE_RATIO, viewportH - anchorH - edgePadding * 2),
  )
}
