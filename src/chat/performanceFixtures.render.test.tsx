import { act, render } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { MessageList } from './MessageList'
import {
  createChatPerformanceFixture,
  type ChatPerformanceFixtureId,
} from './performanceFixtures'
import { getChatPerfReport, resetChatPerfProbeForTests } from './chatPerformanceProbe'
import { createEmptyStreamSnapshot } from './conversationRuns'
import { reset, setCoarse, setSnapshot } from './streamingStore'

async function flush() {
  await act(async () => {
    await Promise.resolve()
  })
}

afterEach(() => {
  reset()
  setCoarse({ streaming: false, streamFrozen: false, cancelling: false, streamError: '' })
  resetChatPerfProbeForTests()
})

describe('chat performance fixtures render through MessageList', () => {
  it.each(['F1', 'F2', 'F3'] as ChatPerformanceFixtureId[])('%s records a real mounted window sample', async (id) => {
    const fixture = createChatPerformanceFixture(id)
    render(<MessageList messages={fixture.messages} conversationId={`fixture-${id}`} />)
    await flush()

    const report = getChatPerfReport()
    expect(report.samples.some((sample) => sample.name === 'message-list-window'
      && sample.detail?.startsWith(`fixture-${id}:`))).toBe(true)
  })

  it('F4 injects the exact streaming fixture into the live row', async () => {
    const fixture = createChatPerformanceFixture('F4')
    render(<MessageList messages={fixture.messages} conversationId="fixture-F4" />)

    act(() => {
      setSnapshot({
        ...createEmptyStreamSnapshot(),
        content: fixture.streamingContent ?? '',
        streaming: true,
      })
      setCoarse({ streaming: true, streamFrozen: false, cancelling: false })
    })
    await flush()

    const report = getChatPerfReport()
    expect(fixture.streamingContent).toHaveLength(20_000)
    expect(report.samples.some((sample) => sample.name === 'message-list-window'
      && sample.detail?.startsWith('fixture-F4:'))).toBe(true)
  })
})
