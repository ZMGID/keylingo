import type { ChatMessage, ContextClearBoundaryRecord, ConversationContextState } from './types'

export interface ContextClearBoundaryView {
  afterIndex: number
  record: ContextClearBoundaryRecord
}

function readUntilId(record: ContextClearBoundaryRecord): string | null {
  return record.source_until_message_id ?? record.sourceUntilMessageId ?? null
}

export function collectClearRecords(
  contextState?: ConversationContextState | null,
): ContextClearBoundaryRecord[] {
  if (!contextState) return []
  return contextState.clear_boundaries ?? contextState.clearBoundaries ?? []
}

export function resolveClearBoundaries(
  messages: ChatMessage[],
  contextState?: ConversationContextState | null,
): ContextClearBoundaryView[] {
  const records = collectClearRecords(contextState)
  const views: ContextClearBoundaryView[] = []

  for (const record of records) {
    const untilId = readUntilId(record)
    if (!untilId) continue
    const afterIndex = messages.findIndex((message) => message.id === untilId)
    if (afterIndex < 0) continue
    views.push({ afterIndex, record })
  }

  views.sort((a, b) => a.afterIndex - b.afterIndex || (a.record.created_at ?? 0) - (b.record.created_at ?? 0))
  return views
}

export function latestClearBoundaryId(
  contextState?: ConversationContextState | null,
): string | null {
  const records = collectClearRecords(contextState)
  return records[records.length - 1]?.id ?? null
}

export function mergeClearContextState(
  prev: ConversationContextState | null | undefined,
  next: ConversationContextState,
): ConversationContextState {
  const prevRecords = collectClearRecords(prev)
  const nextRecords = collectClearRecords(next)
  if (nextRecords.length >= prevRecords.length) return next
  const byId = new Map<string, ContextClearBoundaryRecord>()
  for (const record of prevRecords) byId.set(record.id, record)
  for (const record of nextRecords) byId.set(record.id, record)
  const merged = [...byId.values()].sort(
    (a, b) => (a.created_at ?? a.createdAt ?? 0) - (b.created_at ?? b.createdAt ?? 0),
  )
  return {
    ...next,
    clear_boundaries: merged,
    clearBoundaries: merged,
  }
}
