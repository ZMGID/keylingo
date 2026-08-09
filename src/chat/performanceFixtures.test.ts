import { describe, expect, it } from 'vitest'
import {
  createChatPerformanceFixture,
  summarizeChatPerformanceFixture,
} from './performanceFixtures'

describe('chat performance fixtures', () => {
  it('keeps F1 deterministic at 200 ordinary rows', () => {
    const fixture = createChatPerformanceFixture('F1')
    expect(fixture.messages).toHaveLength(200)
    expect(summarizeChatPerformanceFixture(fixture)).toMatchObject({
      id: 'F1',
      messageCount: 200,
      assistantCount: 100,
      codeBlockCount: 0,
    })
  })

  it('keeps F2 code-heavy enough to exercise row estimates', () => {
    const summary = summarizeChatPerformanceFixture(createChatPerformanceFixture('F2'))
    expect(summary.messageCount).toBe(40)
    expect(summary.codeBlockCount).toBeGreaterThanOrEqual(200)
  })

  it('contains every heavy content type in F3', () => {
    expect(summarizeChatPerformanceFixture(createChatPerformanceFixture('F3'))).toMatchObject({
      mermaidCount: 12,
      imageCount: 12,
      toolCallCount: 12,
    })
  })

  it('keeps F4 at exactly 20k streaming characters', () => {
    const fixture = createChatPerformanceFixture('F4')
    expect(fixture.streamingContent).toHaveLength(20_000)
    expect(summarizeChatPerformanceFixture(fixture).streamingLength).toBe(20_000)
  })
})
