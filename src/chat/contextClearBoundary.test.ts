import { describe, expect, it } from 'vitest'
import { collectClearRecords, resolveClearBoundaries } from './contextClearBoundary'
import type { ChatMessage, ConversationContextState } from './types'

const messages: ChatMessage[] = [
  { id: 'm1', role: 'user', content: 'hello', timestamp: 1 },
  { id: 'm2', role: 'assistant', content: 'hi', timestamp: 2 },
  { id: 'm3', role: 'user', content: 'more', timestamp: 3 },
]

describe('contextClearBoundary', () => {
  it('anchors the divider after the cutoff message', () => {
    const contextState: ConversationContextState = {
      clear_boundaries: [{
        id: 'c1',
        source_until_message_id: 'm2',
        created_at: 10,
      }],
    }
    const views = resolveClearBoundaries(messages, contextState)
    expect(views).toHaveLength(1)
    expect(views[0]?.afterIndex).toBe(1)
  })

  it('skips records whose cutoff message is gone', () => {
    const contextState: ConversationContextState = {
      clear_boundaries: [{
        id: 'c1',
        source_until_message_id: 'm-deleted',
        created_at: 10,
      }],
    }
    expect(resolveClearBoundaries(messages, contextState)).toEqual([])
  })

  it('reads camelCase aliases', () => {
    const contextState: ConversationContextState = {
      clearBoundaries: [{
        id: 'c1',
        sourceUntilMessageId: 'm1',
        createdAt: 8,
      }],
    }
    expect(collectClearRecords(contextState)).toHaveLength(1)
    expect(resolveClearBoundaries(messages, contextState)[0]?.afterIndex).toBe(0)
  })
})
