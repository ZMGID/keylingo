import type { I18n } from '../settings/i18n'
import type { ContextUsageSegment } from './types'

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

export function buildContextBarSlices(
  segments: ContextUsageSegment[],
  estimatedInputTokens: number,
  contextWindowTokens: number | null,
  t: I18n,
): ContextBarSlice[] {
  const active = segments.filter((segment) => segmentTokens(segment) > 0)
  const window = contextWindowTokens ?? 0
  const denominator = window > 0 ? window : Math.max(estimatedInputTokens, 1)

  // Agent 的计划/ask_user/待办段都很碎，合并成一行「Agent」，保留首段颜色与出现位置。
  const slices: ContextBarSlice[] = []
  let agentAgg: ContextBarSlice | null = null
  for (const segment of active) {
    const tokens = segmentTokens(segment)
    const widthPercent = Math.max(0, (tokens / denominator) * 100)
    if (segment.id.startsWith('agent_')) {
      if (!agentAgg) {
        agentAgg = { id: 'agent', label: 'Agent', tokens: 0, color: segment.color || '#7A7A7A', widthPercent: 0 }
        slices.push(agentAgg)
      }
      agentAgg.tokens += tokens
      agentAgg.widthPercent += widthPercent
      continue
    }
    slices.push({
      id: segment.id,
      label: localizedSegmentLabel(segment, t),
      tokens,
      color: segment.color || '#7A7A7A',
      widthPercent,
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
