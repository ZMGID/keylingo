import type { VirtualItem } from '@tanstack/react-virtual'

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
  // 行首围栏才算（正文里内联的 ``` 不算）；一对围栏 = 一个代码块。
  const fences = (text.match(/^\s{0,3}```/gm)?.length ?? 0) / 2
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
 * 用途只有一个：喂给 virtua 的 `itemSize`。**不给它的话 virtua 会拿已测量的行去外推屏外的行**，
 * 而我们把实挂载尾部砍到 3~4 条之后，它只能拿这几条去推上面十几条；行高差 30 倍，
 * 推出来必然错，错了就是「往上翻历史内容跳」「拖滚动条跳」。
 *
 * 这**不是**第二个高度估算器 —— itemSize 是 virtua 自己的输入，我们只是别让它瞎猜。
 */
const HEIGHT_PER_FENCE_PX = 24
const HEIGHT_PER_LINE_PX = 25

export function estimateRenderHeight(text: string, contentWidth = 560): number {
  if (!text) return 0
  const charsPerLine = Math.max(28, Math.min(100, Math.floor(contentWidth / 8)))
  let height = 0
  let inCode = false
  for (const line of text.split('\n')) {
    if (/^\s{0,3}```/.test(line)) {
      height += HEIGHT_PER_FENCE_PX
      inCode = !inCode
      continue
    }
    const wrappedLines = Math.max(1, Math.ceil(Math.max(1, line.length) / charsPerLine))
    height += wrappedLines * (inCode ? 20 : HEIGHT_PER_LINE_PX)
  }
  return Math.round(height)
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
    + attachmentCount * 120
    + artifactCount * 96
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
