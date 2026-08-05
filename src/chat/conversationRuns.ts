export interface ConversationStreamSnapshot {
  runId: string | null
  streaming: boolean
  content: string
  reasoning: string
  reasoningStreaming: boolean
  toolCalls: import('./types').ToolCallRecord[]
  segments: import('./types').ChatMessageSegment[]
  startedAt: number | null
  reasoningStartedAt: number | null
  reasoningDurationMs: number | null
  reasoningStartedAtBySegmentId: Record<string, number>
  reasoningDurationMsBySegmentId: Record<string, number>
}

export function isConversationInFlight(
  inFlightConversations: ReadonlySet<string>,
  conversationId: string,
): boolean {
  return inFlightConversations.has(conversationId)
}

export function isConversationBusy(
  conversationId: string | null | undefined,
  inFlightConversations: ReadonlySet<string>,
  streamSnapshots: Record<string, ConversationStreamSnapshot>,
): boolean {
  if (!conversationId) return false
  if (inFlightConversations.has(conversationId)) return true
  return streamSnapshots[conversationId]?.streaming === true
}

export function collectGeneratingConversationIds(
  inFlightConversations: ReadonlySet<string>,
  streamSnapshots: Record<string, ConversationStreamSnapshot>,
  pendingToolConfirms: Record<string, readonly unknown[]>,
): Set<string> {
  const ids = new Set<string>(inFlightConversations)
  for (const [conversationId, snapshot] of Object.entries(streamSnapshots)) {
    if (snapshot.streaming) ids.add(conversationId)
  }
  for (const [conversationId, queue] of Object.entries(pendingToolConfirms)) {
    if (queue.length > 0) ids.add(conversationId)
  }
  return ids
}

export function createEmptyStreamSnapshot(): ConversationStreamSnapshot {
  return {
    runId: null,
    streaming: true,
    content: '',
    reasoning: '',
    reasoningStreaming: false,
    toolCalls: [],
    segments: [],
    startedAt: Date.now(),
    reasoningStartedAt: null,
    reasoningDurationMs: null,
    reasoningStartedAtBySegmentId: {},
    reasoningDurationMsBySegmentId: {},
  }
}
