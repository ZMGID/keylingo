import { describe, expect, it } from 'vitest'
import {
  collectGeneratingConversationIds,
  createEmptyStreamSnapshot,
  isConversationBusy,
  isConversationInFlight,
  mergeToolRecord,
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

describe('mergeToolRecord', () => {
  it('keeps the existing structured content when the update carries none', () => {
    // 问用户答完之后，claude 回的 tool_result 那条更新不带 structured_content。
    // 直接展开会把「问了什么 + 选了什么」抹掉，消息流里只剩一行灰字。
    const answered = {
      id: 'tc1',
      structured_content: { askUser: { phase: 'answered', questions: [], answers: {} } },
      structuredContent: { askUser: { phase: 'answered', questions: [], answers: {} } },
    } as never
    // 注意 `structured_content: undefined` 是**显式**的：协议记录是按字段拼出来的
    // （`toolEventToRecord`），键总在、值可能是 undefined。对象展开不会用「缺失的键」
    // 覆盖已有值，但会用「值为 undefined 的键」覆盖 —— 这才是抹掉载荷的那一下。
    const toolResult = {
      id: 'tc1',
      status: 'success',
      result_preview: 'ok',
      structured_content: undefined,
      structuredContent: undefined,
    } as never
    const merged = mergeToolRecord(answered, toolResult)
    expect(merged.status).toBe('success')
    expect(merged.result_preview).toBe('ok')
    expect(merged.structured_content).toEqual({
      askUser: { phase: 'answered', questions: [], answers: {} },
    })
    expect(merged.structuredContent).toEqual(merged.structured_content)
  })

  it('lets a real structured payload win over the old one', () => {
    const previous = { id: 'tc1', structured_content: { askUser: { phase: 'awaiting' } } } as never
    const next = { id: 'tc1', structured_content: { askUser: { phase: 'answered' } } } as never
    expect(mergeToolRecord(previous, next).structured_content)
      .toEqual({ askUser: { phase: 'answered' } })
  })
})

/**
 * Coarse gate: a conversation is "busy" while either in-flight or still streaming.
 * UI uses this to disable send / show the stop button — if this drifts, double-send
 * and stuck "generating" indicators come back.
 */
describe('busy conversation gate (smoke)', () => {
  it('treats in-flight OR streaming OR pending tool confirm as generating', () => {
    const streaming = {
      'c-stream': { ...createEmptyStreamSnapshot(), streaming: true },
      'c-idle': { ...createEmptyStreamSnapshot(), streaming: false },
    }
    const pending = { 'c-confirm': [{ id: 't1' }], 'c-empty': [] }

    expect(isConversationBusy('c-flight', new Set(['c-flight']), streaming)).toBe(true)
    expect(isConversationBusy('c-stream', new Set(), streaming)).toBe(true)
    expect(isConversationBusy('c-idle', new Set(), streaming)).toBe(false)

    const generating = collectGeneratingConversationIds(
      new Set(['c-flight']),
      streaming,
      pending,
    )
    expect(Array.from(generating).sort()).toEqual(['c-confirm', 'c-flight', 'c-stream'])
  })
})

