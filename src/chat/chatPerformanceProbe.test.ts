import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  getChatPerfReport,
  evaluateChatPerfReport,
  onChatPerfProfiler,
  recordChatPerfLongTask,
  recordChatPerfSample,
  resetChatPerfProbeForTests,
  summarizeChatPerfReport,
} from './chatPerformanceProbe'

describe('chat performance browser report', () => {
  afterEach(() => {
    vi.useRealTimers()
    resetChatPerfProbeForTests()
    vi.restoreAllMocks()
    delete (globalThis as { __KIVIO_CHAT_PERF__?: boolean }).__KIVIO_CHAT_PERF__
  })

  it('keeps profiler, window samples, and long tasks in one executable report', () => {
    const perfGlobal = globalThis as { __KIVIO_CHAT_PERF__?: boolean }
    perfGlobal.__KIVIO_CHAT_PERF__ = true

    recordChatPerfSample({
      name: 'conversation-switch',
      durationMs: 17.456,
      mountedRows: 14,
      domNodes: 220,
      detail: 'F1:c1→c2',
    })
    recordChatPerfLongTask({ durationMs: 51.789, startTime: 123.456 })
    onChatPerfProfiler('MessageList', 'mount', 8.25, 12.5, 0, 0)

    const report = getChatPerfReport()
    expect(report.enabled).toBe(true)
    expect(report.samples).toEqual([{
      name: 'conversation-switch',
      durationMs: 17.5,
      mountedRows: 14,
      domNodes: 220,
      detail: 'F1:c1→c2',
    }])
    expect(report.longTasks).toEqual([{ durationMs: 51.8, startTime: 123.5 }])
    expect(report.buckets).toEqual([expect.objectContaining({
      name: 'MessageList',
      commits: 1,
      actualMs: 8.25,
      baseMs: 12.5,
      maxActualMs: 8.25,
      phase: 'mount',
    })])
  })

  it('is bounded and resettable for long manual browser sessions', () => {
    const perfGlobal = globalThis as { __KIVIO_CHAT_PERF__?: boolean }
    perfGlobal.__KIVIO_CHAT_PERF__ = true
    for (let index = 0; index < 2_050; index += 1) {
      recordChatPerfSample({ name: `sample-${index}`, durationMs: 1, mountedRows: 1, domNodes: 1 })
      recordChatPerfLongTask({ durationMs: index, startTime: index })
    }

    const report = getChatPerfReport()
    expect(report.samples).toHaveLength(2_000)
    expect(report.samples[0]?.name).toBe('sample-50')
    expect(report.longTasks).toHaveLength(2_000)
    expect(report.longTasks[0]?.durationMs).toBe(50)

    resetChatPerfProbeForTests()
    expect(getChatPerfReport().samples).toHaveLength(0)
    expect(getChatPerfReport().longTasks).toHaveLength(0)
  })

  it('keeps profiler totals after the console window is flushed', () => {
    const perfGlobal = globalThis as { __KIVIO_CHAT_PERF__?: boolean }
    perfGlobal.__KIVIO_CHAT_PERF__ = true
    vi.useFakeTimers()

    onChatPerfProfiler('MessageList', 'update', 4, 6, 0, 0)
    vi.advanceTimersByTime(500)

    expect(getChatPerfReport().buckets).toEqual([expect.objectContaining({
      name: 'MessageList',
      commits: 1,
      actualMs: 4,
    })])
  })

  it('summarizes and evaluates exported guardrails', () => {
    const perfGlobal = globalThis as { __KIVIO_CHAT_PERF__?: boolean }
    perfGlobal.__KIVIO_CHAT_PERF__ = true
    recordChatPerfSample({ name: 'message-list-window', durationMs: 20, mountedRows: 12, domNodes: 300 })
    recordChatPerfLongTask({ durationMs: 80, startTime: 10 })

    const report = getChatPerfReport()
    expect(summarizeChatPerfReport(report)).toMatchObject({
      maxMountedRows: 12,
      maxDomNodes: 300,
      maxLongTaskMs: 80,
    })
    expect(evaluateChatPerfReport(report, {
      maxMountedRows: 16,
      maxDomNodes: 400,
      maxLongTaskMs: 100,
    })).toEqual([])
    expect(evaluateChatPerfReport(report, { maxMountedRows: 8 })).toEqual(['mountedRows 12 > 8'])
  })
})
