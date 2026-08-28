import type {
  ChatStreamPayload,
  ChatSubagentPayload,
  ChatToolProgressPayload,
  ChatUserPromptPayload,
} from '../api/tauri'
import { mergeToolRecord, type ConversationStreamSnapshot } from './conversationRuns'
import { compareTimelineSegments, isExternalSubagentToolCall, segmentStepNumber, segmentToolCallId } from './segments'
import { normalizeToolCallStatus } from './toolStatus'
import type { ChatMessage, ChatMessageSegment, ToolCallRecord } from './types'

export function toolEventToRecord(payload: ChatToolProgressPayload): ToolCallRecord {
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

export function userPromptEventToRecord(payload: ChatUserPromptPayload): ToolCallRecord {
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

export function streamPayloadToSegment(payload: ChatStreamPayload): ChatMessageSegment | null {
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

export function streamTextDelta(payload: ChatStreamPayload): string {
  return payload.type === 'text_delta' ? payload.delta : ''
}

export function streamReasoningDelta(payload: ChatStreamPayload): string {
  return payload.type === 'reasoning_delta' ? payload.delta : ''
}

export function isStreamTerminal(payload: ChatStreamPayload): boolean {
  return payload.type === 'run_completed'
    || payload.type === 'run_cancelled'
    || payload.type === 'run_failed'
}

export function streamTerminalReason(payload: ChatStreamPayload): 'done' | 'cancelled' | 'error' | undefined {
  if (payload.type === 'run_completed') return 'done'
  if (payload.type === 'run_cancelled') return 'cancelled'
  if (payload.type === 'run_failed') return 'error'
  return undefined
}

export function hasStreamPreview(snapshot: ConversationStreamSnapshot | null | undefined): boolean {
  return Boolean(
    snapshot
    && (snapshot.content
      || snapshot.reasoning
      || snapshot.toolCalls.length > 0
      || snapshot.segments.length > 0),
  )
}

export function upsertStreamSegment(
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

export function nextSegmentOrder(segments: ChatMessageSegment[]): number {
  if (segments.length === 0) return 1
  return Math.max(...segments.map((segment) => segment.order ?? 0)) + 1
}

export function upsertToolStreamSegment(
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

export function findReasoningSegmentForText(
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

export function updateReasoningSegmentDuration(
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

export function applyStreamDeltaToSnapshot(
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

export function finalizeReasoningDurationOnDone(snapshot: ConversationStreamSnapshot) {
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

export function applyToolRecordToSnapshot(
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

export function messageToolCalls(message: ChatMessage): ToolCallRecord[] {
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

export function findSubagentToolIndex(tools: ToolCallRecord[], payload: ChatSubagentPayload): number {
  const exact = tools.findIndex((item) => matchesSubagentTool(item, payload))
  if (exact >= 0) return exact
  const running = tools
    .map((item, index) => ({ item, index }))
    .filter(({ item }) => item.status === 'running' && isExternalSubagentToolCall(item))
  return running.length === 1 ? running[0].index : -1
}

export function mergeSubagentProgress(
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
