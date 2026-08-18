import { toolRecordRawName } from './segments'
import { normalizeToolCallStatus } from './toolStatus'
import type { ChatMessage, ChatMessageSegment, Conversation, ToolCallRecord } from './types'

export type TrajectoryKind = 'user' | 'assistant' | 'tool' | 'compacted'

export type TrajectoryStep = {
  id: string
  kind: TrajectoryKind
  title: string
  preview: string
  result?: string
  messageId: string
  toolCallId?: string
  error?: boolean
}

export type TrajectoryStats = {
  turns: number
  steps: number
  calls: number
  durationLabel: string | null
}

const PREVIEW_LIMIT = 140

export function compactTrajectoryText(value: unknown, limit = PREVIEW_LIMIT): string {
  const text = typeof value === 'string'
    ? value
    : value == null
      ? ''
      : (() => {
        try {
          return JSON.stringify(value)
        } catch {
          return String(value)
        }
      })()
  return text.replace(/\s+/g, ' ').trim().slice(0, limit)
}

function segmentToolCallId(segment: ChatMessageSegment): string {
  return segment.tool_call_id ?? segment.toolCallId ?? ''
}

function messageToolCalls(message: ChatMessage): ToolCallRecord[] {
  return message.tool_calls ?? message.toolCalls ?? []
}

function toolPreview(toolCall: ToolCallRecord): string {
  const preview = toolCall.argument_preview ?? toolCall.argumentPreview ?? toolCall.argumentsPreview
  if (typeof preview === 'string' && preview.trim()) return compactTrajectoryText(preview)
  const args = toolCall.arguments ?? toolCall.args ?? toolCall.input
  if (args && typeof args === 'object' && !Array.isArray(args)) {
    const record = args as Record<string, unknown>
    const preferred = record.path ?? record.file_path ?? record.filePath ?? record.query
      ?? record.pattern ?? record.command ?? record.url ?? record.name
    if (preferred != null) return compactTrajectoryText(preferred)
  }
  return compactTrajectoryText(args)
}

function toolResult(toolCall: ToolCallRecord): string {
  if (toolCall.error) return compactTrajectoryText(toolCall.error)
  const preview = toolCall.result_preview ?? toolCall.resultPreview
  if (typeof preview === 'string' && preview.trim()) return compactTrajectoryText(preview)
  return compactTrajectoryText(toolCall.result ?? toolCall.output)
}

function sortSegments(segments: ChatMessageSegment[]): ChatMessageSegment[] {
  return [...segments].sort((left, right) => (left.order ?? 0) - (right.order ?? 0))
}

function pushAssistantText(steps: TrajectoryStep[], message: ChatMessage, text: string, index: number) {
  const preview = compactTrajectoryText(text)
  if (!preview) return
  steps.push({
    id: `${message.id}:assistant:${index}`,
    kind: 'assistant',
    title: 'assistant',
    preview,
    messageId: message.id,
  })
}

function pushToolStep(steps: TrajectoryStep[], message: ChatMessage, toolCall: ToolCallRecord) {
  const status = normalizeToolCallStatus(toolCall.status)
  const result = toolResult(toolCall)
  steps.push({
    id: `${message.id}:tool:${toolCall.id || toolCall.toolCallId || toolCall.callId || steps.length}`,
    kind: 'tool',
    title: toolRecordRawName(toolCall) || 'tool',
    preview: toolPreview(toolCall),
    result: result || undefined,
    messageId: message.id,
    toolCallId: toolCall.id || toolCall.toolCallId || toolCall.callId || undefined,
    error: status === 'error',
  })
}

export function buildConversationTrajectory(messages: ChatMessage[]): TrajectoryStep[] {
  const steps: TrajectoryStep[] = []
  for (const message of messages) {
    if (message.role === 'user') {
      const preview = compactTrajectoryText(message.content)
      if (!preview) continue
      steps.push({
        id: `${message.id}:user`,
        kind: 'user',
        title: 'user',
        preview,
        messageId: message.id,
      })
      continue
    }

    const tools = messageToolCalls(message)
    const toolsById = new Map(
      tools.flatMap((toolCall) => {
        const ids = [toolCall.id, toolCall.toolCallId, toolCall.callId, toolCall.call_id]
          .filter((value): value is string => typeof value === 'string' && value.length > 0)
        return ids.map((id) => [id, toolCall] as const)
      }),
    )
    const usedTools = new Set<ToolCallRecord>()
    const segments = message.segments?.length ? sortSegments(message.segments) : []
    let assistantIndex = 0

    if (segments.length > 0) {
      for (const segment of segments) {
        if (segment.kind === 'tool') {
          const toolCall = toolsById.get(segmentToolCallId(segment))
          if (!toolCall || usedTools.has(toolCall)) continue
          usedTools.add(toolCall)
          pushToolStep(steps, message, toolCall)
          continue
        }
        if (segment.kind === 'text') {
          pushAssistantText(steps, message, segment.text ?? '', assistantIndex)
          assistantIndex += 1
        }
      }
    }

    for (const toolCall of tools) {
      if (usedTools.has(toolCall)) continue
      pushToolStep(steps, message, toolCall)
    }
    if (segments.length === 0) {
      pushAssistantText(steps, message, message.content, assistantIndex)
    }
  }
  return steps
}

export function filterTrajectorySteps(steps: TrajectoryStep[], query: string): TrajectoryStep[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return steps
  return steps.filter((step) => {
    const haystack = [step.title, step.preview, step.result].filter(Boolean).join(' ').toLowerCase()
    return haystack.includes(needle)
  })
}

export function summarizeTrajectory(steps: TrajectoryStep[], conversation?: Conversation | null): TrajectoryStats {
  const first = conversation?.messages[0]?.timestamp
  const last = conversation?.messages[conversation.messages.length - 1]?.timestamp
  return {
    turns: steps.filter((step) => step.kind === 'user').length,
    steps: steps.length,
    calls: steps.filter((step) => step.kind === 'tool').length,
    durationLabel: formatTrajectoryDuration(first, last),
  }
}

export function formatTrajectoryDuration(startSeconds?: number, endSeconds?: number): string | null {
  if (!startSeconds || !endSeconds || endSeconds <= startSeconds) return null
  const total = Math.max(1, Math.round(endSeconds - startSeconds))
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  if (hours > 0) return `${hours}h${minutes}m`
  if (minutes > 0) return `${minutes}m${seconds}s`
  return `${seconds}s`
}
