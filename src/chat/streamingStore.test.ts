import { describe, expect, it } from 'vitest'
import {
  getCoarse,
  getSnapshot,
  patchSnapshot,
  reset,
  setCoarse,
  setSnapshot,
  subscribeCoarse,
  subscribeSnapshot,
} from './streamingStore'
import { createEmptyStreamSnapshot } from './conversationRuns'

describe('streamingStore coarse slice', () => {
  it('no-op setCoarse does not allocate a new ref or notify', () => {
    reset()
    setCoarse({ streaming: false }) // already false
    const before = getCoarse()
    let notified = 0
    const unsub = subscribeCoarse(() => {
      notified++
    })
    setCoarse({ streaming: false, cancelling: false }) // unchanged fields
    unsub()
    expect(getCoarse()).toBe(before) // same reference
    expect(notified).toBe(0)
  })

  it('changing a field swaps the ref and notifies once', () => {
    reset()
    const before = getCoarse()
    let notified = 0
    const unsub = subscribeCoarse(() => {
      notified++
    })
    setCoarse({ streaming: true })
    unsub()
    expect(getCoarse()).not.toBe(before)
    expect(getCoarse().streaming).toBe(true)
    expect(notified).toBe(1)
  })

  it('reset clears streaming/frozen/cancelling but leaves streamError', () => {
    setCoarse({ streaming: true, streamFrozen: true, cancelling: true, streamError: 'boom' })
    reset()
    expect(getCoarse()).toMatchObject({ streaming: false, streamFrozen: false, cancelling: false })
    expect(getCoarse().streamError).toBe('boom')
  })
})

describe('streamingStore content slice', () => {
  it('setSnapshot always swaps the ref and notifies, even with identical content', () => {
    reset()
    const snap = createEmptyStreamSnapshot()
    let notified = 0
    const unsub = subscribeSnapshot(() => {
      notified++
    })
    setSnapshot(snap)
    const first = getSnapshot()
    setSnapshot(snap) // same source object (Chat re-pushes the mutated ref each frame)
    unsub()
    expect(first).not.toBe(snap) // copied out
    expect(getSnapshot()).not.toBe(first) // new ref per push
    expect(notified).toBe(2)
  })

  it('patchSnapshot merges into the current snapshot', () => {
    setSnapshot(createEmptyStreamSnapshot())
    patchSnapshot({ reasoningStreaming: false, content: 'hi' })
    expect(getSnapshot().content).toBe('hi')
    expect(getSnapshot().reasoningStreaming).toBe(false)
  })

  it('reset returns content to the empty idle snapshot', () => {
    setSnapshot(createEmptyStreamSnapshot())
    patchSnapshot({ content: 'x' })
    reset()
    expect(getSnapshot().content).toBe('')
    expect(getSnapshot().streaming).toBe(false)
  })
})

/**
 * Coarse gate: one full turn through the stream store.
 * Start → deltas → finish → reset must leave idle UI state for the next send.
 */
describe('streamingStore turn lifecycle (smoke)', () => {
  it('start → stream text → finish leaves idle content and non-streaming coarse state', () => {
    reset()
    setCoarse({ streaming: true, streamError: '' })
    setSnapshot({
      ...createEmptyStreamSnapshot(),
      runId: 'run-1',
      messageId: 'msg-1',
      streaming: true,
    })
    patchSnapshot({ content: '你好' })
    patchSnapshot({ content: '你好，世界' })
    expect(getSnapshot().content).toBe('你好，世界')
    expect(getCoarse().streaming).toBe(true)

    setCoarse({ streaming: false })
    patchSnapshot({ streaming: false })
    expect(getCoarse().streaming).toBe(false)
    expect(getSnapshot().streaming).toBe(false)
    expect(getSnapshot().content).toBe('你好，世界')

    reset()
    expect(getSnapshot().content).toBe('')
    expect(getCoarse().streaming).toBe(false)
  })
})

