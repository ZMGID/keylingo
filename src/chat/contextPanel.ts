import type { I18n } from '../settings/i18n'
import type { ContextUsageSegment, ConversationContextState } from './types'

/** 与 `chat/agent/compaction.rs` 中 `AUTO_COMPACT_RATIO`（0.90）保持一致 */
export const CONTEXT_AUTO_COMPRESS_PERCENT = 90
export const CONTEXT_WARNING_PERCENT = 70
export const CONTEXT_CRITICAL_PERCENT = 95

export const CONTEXT_FREE_SEGMENT_ID = '__free__'
export const CONTEXT_FREE_COLOR = '#E8E8ED'

export function segmentTokens(segment: ContextUsageSegment): number {
  return segment.estimated_tokens ?? segment.estimatedTokens ?? 0
}

const SEGMENT_LABEL_KEY: Record<string, keyof I18n> = {
  system_prompt: 'contextSegmentSystemPrompt',
  assistant: 'contextSegmentAssistant',
  set: 'contextSegmentSet',
  runtime_context: 'contextSegmentRuntime',
  memory_l1: 'contextSegmentMemory',
  knowledge_base: 'contextSegmentKnowledgeBase',
  agent_plan: 'contextSegmentAgentPlan',
  agent_todo: 'contextSegmentAgentTodo',
  tool_definitions: 'contextSegmentToolDefinitions',
  native_tools: 'contextSegmentNativeTools',
  skills: 'contextSegmentSkills',
  mcp: 'contextSegmentMcp',
  summarized_conversation: 'contextSegmentSummarized',
  conversation: 'contextSegmentConversation',
  attachments: 'contextSegmentAttachments',
}

export function localizedSegmentLabel(segment: ContextUsageSegment, t: I18n): string {
  const key = SEGMENT_LABEL_KEY[segment.id]
  if (key && t[key]) return String(t[key])
  return segment.label
}

export type ContextBarSlice = {
  id: string
  label: string
  tokens: number
  color: string
  widthPercent: number
}

/** 面板只展示三大类：系统提示词 / 工具 / 对话消息（+ 剩余空间在条上）。 */
export const CONTEXT_GROUP_SYSTEM = 'system'
export const CONTEXT_GROUP_TOOLS = 'tools'
export const CONTEXT_GROUP_CONVERSATION = 'conversation'

const GROUP_COLORS: Record<string, string> = {
  [CONTEXT_GROUP_SYSTEM]: '#7A7A7A',
  [CONTEXT_GROUP_TOOLS]: '#7553CF',
  [CONTEXT_GROUP_CONVERSATION]: '#3B82F6',
}

/** 把细粒度 segment id 归到三大类。未知段并入对话消息。 */
export function contextSegmentGroupId(segmentId: string): string {
  if (
    segmentId === 'system_prompt'
    || segmentId === 'assistant'
    || segmentId === 'set'
    || segmentId === 'runtime_context'
    || segmentId === 'memory_l1'
    || segmentId === 'knowledge_base'
    || segmentId === 'skills'
  ) {
    return CONTEXT_GROUP_SYSTEM
  }
  if (
    segmentId === 'tool_definitions'
    || segmentId === 'native_tools'
    || segmentId === 'mcp'
    || segmentId === 'agent'
    || segmentId.startsWith('agent_')
  ) {
    return CONTEXT_GROUP_TOOLS
  }
  // conversation / attachments / summarized_conversation / external-session / …
  return CONTEXT_GROUP_CONVERSATION
}

function groupLabel(groupId: string, t: I18n): string {
  if (groupId === CONTEXT_GROUP_SYSTEM) return t.contextSegmentSystemPrompt
  if (groupId === CONTEXT_GROUP_TOOLS) return t.contextSegmentTools
  return t.contextSegmentConversation
}

export function buildContextBarSlices(
  segments: ContextUsageSegment[],
  estimatedInputTokens: number,
  contextWindowTokens: number | null,
  t: I18n,
): ContextBarSlice[] {
  const active = segments.filter((segment) => segmentTokens(segment) > 0)
  const window = contextWindowTokens ?? 0
  const denominator = window > 0 ? window : Math.max(estimatedInputTokens, 1)

  // 固定顺序：系统 → 工具 → 对话（与参考图一致）
  const order = [CONTEXT_GROUP_SYSTEM, CONTEXT_GROUP_TOOLS, CONTEXT_GROUP_CONVERSATION]
  const totals = new Map<string, number>(order.map((id) => [id, 0]))
  for (const segment of active) {
    const groupId = contextSegmentGroupId(segment.id)
    totals.set(groupId, (totals.get(groupId) ?? 0) + segmentTokens(segment))
  }

  const slices: ContextBarSlice[] = []
  for (const groupId of order) {
    const tokens = totals.get(groupId) ?? 0
    if (tokens <= 0) continue
    slices.push({
      id: groupId,
      label: groupLabel(groupId, t),
      tokens,
      color: GROUP_COLORS[groupId] ?? '#7A7A7A',
      widthPercent: Math.max(0, (tokens / denominator) * 100),
    })
  }

  if (window > 0) {
    const freeTokens = Math.max(0, window - estimatedInputTokens)
    if (freeTokens > 0) {
      slices.push({
        id: CONTEXT_FREE_SEGMENT_ID,
        label: t.contextSegmentFree,
        tokens: freeTokens,
        color: CONTEXT_FREE_COLOR,
        widthPercent: (freeTokens / window) * 100,
      })
    }
  }

  return slices
}

/**
 * 生成过程中的「上下文占用活数」就地补进已有的上下文状态。
 *
 * **唯一的生产者是内置 agent 的压缩检查**（Rust 侧 `chat/agent/compaction.rs`），每个
 * planning 轮发一次，两个数与轮末权威计算出自同一对函数，所以轮末那次覆盖不会跳。
 * 外部 CLI 不走这条（它的占用一轮只由轮末权威计算更新一次）——曾经那条 350ms 节流的通道
 * 分子是单次请求快照、分母是上一轮的粘滞值，看着就是在跳，已删。
 *
 * 这里只动能由这两个数直接推出的字段，其余一律沿用旧值——尤其 `status` /
 * `token_count_source`：它们的判定阈值与口径在 Rust 侧，在前端再写一套就是两份口径。
 * 环形进度与「满度 N%」都读 `usage_ratio`，所以更新比例就够了。
 *
 * **分母粘滞**：`contextWindowTokens` 为 `null`/`undefined` 时保留已知的旧窗口 —— 冲掉旧值
 * 会让用量条在生成过程中退回「满度未知」。
 *
 * 分段按比例缩放（构成不变、只有总量涨）：分段的真实明细只有轮末那次权威计算算得出，
 * 但把它原样留在旧总量上会让进度条里出现一条对不上的缝。
 */
export function applyLiveContextUsage(
  prev: ConversationContextState | null | undefined,
  live: { usedTokens: number; contextWindowTokens?: number | null },
): ConversationContextState | null {
  if (!prev) return prev ?? null
  const used = Math.max(0, Math.round(live.usedTokens))
  const knownWindow = prev.context_window_tokens ?? prev.contextWindowTokens ?? null
  const window = live.contextWindowTokens ?? knownWindow
  const ratio = window && window > 0 ? used / window : null
  const previousUsed = prev.estimated_input_tokens ?? prev.estimatedInputTokens ?? 0
  const segments = scaleContextSegments(prev.segments ?? [], previousUsed, used)
  return {
    ...prev,
    estimated_input_tokens: used,
    estimatedInputTokens: used,
    context_window_tokens: window,
    contextWindowTokens: window,
    usage_ratio: ratio,
    usageRatio: ratio,
    segments,
  }
}

function scaleContextSegments(
  segments: ContextUsageSegment[],
  previousTotal: number,
  nextTotal: number,
): ContextUsageSegment[] {
  if (segments.length === 0) return segments
  const sum = segments.reduce((total, segment) => total + segmentTokens(segment), 0)
  const base = previousTotal > 0 ? previousTotal : sum
  if (base <= 0 || nextTotal === base) return segments
  const factor = nextTotal / base
  return segments.map((segment) => {
    const tokens = Math.max(0, Math.round(segmentTokens(segment) * factor))
    return { ...segment, estimated_tokens: tokens, estimatedTokens: tokens }
  })
}

export function compactPercent(ratio: number | null): string {
  if (ratio == null || !Number.isFinite(ratio)) return '--'
  return `${Math.max(0, Math.min(999, Math.round(ratio * 100)))}`
}

/**
 * 上下文用量条的「满度」文案。
 *
 * 三种 usageRatio 为空的情形要分开说，否则会误导：
 * - 窗口未知：CLI 既不报 `usage_update.size`、模型名也匹配不到任何静态表
 *   （如 cursor 选了不带 `context=` 的模型）。百分比**永远**算不出来，
 *   不能用「CLI 待上报」让用户空等。
 * - 窗口已知但用量还没到：外部 CLI 的正常中间态，「CLI 待上报」准确。
 * - 内置路径：走估算，「估算中」。
 */
export function fullnessLabel(
  usageRatio: number | null,
  isExternalContext: boolean,
  contextWindowTokens: number | null,
  t: I18n,
): string {
  if (usageRatio == null) {
    if (!contextWindowTokens) return t.contextFullnessWindowUnknown
    return isExternalContext ? t.contextFullnessCliPending : t.contextFullnessEstimated
  }
  return t.contextFullnessPercentFull.replace('{percent}', compactPercent(usageRatio))
}
