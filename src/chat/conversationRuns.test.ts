import { describe, expect, it } from 'vitest'
import {
  collectGeneratingConversationIds,
  createEmptyStreamSnapshot,
  isConversationBusy,
  isConversationInFlight,
} from './conversationRuns'

describe('isConversationInFlight', () => {
  it('returns true when conversation is in the in-flight set', () => {
    expect(isConversationInFlight(new Set(['conv-1']), 'conv-1')).toBe(true)
    expect(isConversationInFlight(new Set(['conv-1']), 'conv-2')).toBe(false)
  })
})

describe('isConversationBusy', () => {
  it('returns false for missing conversation id', () => {
    expect(isConversationBusy(null, new Set(), {})).toBe(false)
    expect(isConversationBusy(undefined, new Set(['conv-1']), {})).toBe(false)
  })

  it('returns true when conversation is in-flight', () => {
    expect(isConversationBusy('conv-1', new Set(['conv-1']), {})).toBe(true)
  })

  it('returns true when snapshot is still streaming', () => {
    const snapshots = {
      'conv-1': { ...createEmptyStreamSnapshot(), streaming: true },
    }
    expect(isConversationBusy('conv-1', new Set(), snapshots)).toBe(true)
  })

  it('returns false when not in-flight and snapshot is idle', () => {
    const snapshots = {
      'conv-1': { ...createEmptyStreamSnapshot(), streaming: false },
    }
    expect(isConversationBusy('conv-1', new Set(), snapshots)).toBe(false)
  })
})

describe('collectGeneratingConversationIds', () => {
  it('merges in-flight, streaming snapshots, and pending tool confirms', () => {
    const ids = collectGeneratingConversationIds(
      new Set(['conv-a']),
      {
        'conv-b': { ...createEmptyStreamSnapshot(), streaming: true },
        'conv-c': { ...createEmptyStreamSnapshot(), streaming: false },
      },
      { 'conv-d': [{}], 'conv-e': [] },
    )
    expect(Array.from(ids).sort()).toEqual(['conv-a', 'conv-b', 'conv-d'])
  })
})

describe('createEmptyStreamSnapshot', () => {
  it('creates a streaming snapshot with empty content', () => {
    const snapshot = createEmptyStreamSnapshot()
    expect(snapshot.streaming).toBe(true)
    expect(snapshot.content).toBe('')
    expect(snapshot.toolCalls).toEqual([])
    expect(snapshot.startedAt).toBeTypeOf('number')
    expect(snapshot.reasoningStartedAtBySegmentId).toEqual({})
    expect(snapshot.reasoningDurationMsBySegmentId).toEqual({})
  })
})
