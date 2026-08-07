export interface ConversationStreamSnapshot {
  runId: string | null
  /** 当前流对应的持久化 assistant 消息 id。恢复运行时用于避免历史草稿和实时预览双渲染。 */
  messageId: string | null
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
  /** 流状态行上的瞬态一行字（上游重试等，status_note_updated 事件）。正文一恢复流动就清。 */
  statusNote: string | null
}

/** 后到的 tool record 覆盖已有的，但**不许用「没有」覆盖「有」**（目前只针对
 *  `structured_content`：那是各类专属卡片的载荷）。
 *
 *  踩过的坑：问用户答完之后，claude 回的 `tool_result` 会再发一条不带 `structured_content`
 *  的 tool 更新，直接展开就把那张卡的 `askUser` 载荷（问题 + 用户选的答案）抹了 ——
 *  消息流里只剩一行几乎看不见的灰字，看着像「答完什么都没留下」。 */
export function mergeToolRecord(
  previous: import('./types').ToolCallRecord,
  next: import('./types').ToolCallRecord,
): import('./types').ToolCallRecord {
  const merged = { ...previous, ...next }
  const structured = next.structured_content ?? next.structuredContent
    ?? previous.structured_content ?? previous.structuredContent
  if (structured !== undefined) {
    merged.structured_content = structured
    merged.structuredContent = structured
  }
  return merged
}

export function isConversationInFlight(
  inFlightConversations: ReadonlySet<string>,
  conversationId: string,
): boolean {
  return inFlightConversations.has(conversationId)}

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
    messageId: null,
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
    statusNote: null,
  }
}
