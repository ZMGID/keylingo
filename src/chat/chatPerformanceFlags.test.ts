// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  CHAT_PERFORMANCE_FLAG_KEYS,
  getChatPerformanceFlags,
  refreshChatPerformanceFlags,
  resetChatPerformanceFlagsForTests,
} from './chatPerformanceFlags'

describe('chat performance flags', () => {
  beforeEach(() => {
    const values = new Map<string, string>()
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => values.set(key, value),
        removeItem: (key: string) => values.delete(key),
      },
    })
  })

  afterEach(() => {
    window.localStorage.removeItem(CHAT_PERFORMANCE_FLAG_KEYS.tanstackVirtualizer)
    delete (globalThis as { __KIVIO_CHAT_PERF_FLAGS__?: unknown }).__KIVIO_CHAT_PERF_FLAGS__
    resetChatPerformanceFlagsForTests()
  })

  it('defaults all production paths on', () => {
    expect(getChatPerformanceFlags()).toEqual({
      tanstackVirtualizer: true,
      liveRowExternalization: true,
      lightweightStreamingMarkdown: true,
      settledMarkdownCache: true,
    })
  })

  it('accepts local overrides for a rollback', () => {
    window.localStorage.setItem(CHAT_PERFORMANCE_FLAG_KEYS.tanstackVirtualizer, '0')
    expect(getChatPerformanceFlags().tanstackVirtualizer).toBe(false)
  })

  it('global overrides take precedence over storage', () => {
    window.localStorage.setItem(CHAT_PERFORMANCE_FLAG_KEYS.tanstackVirtualizer, '0')
    ;(globalThis as { __KIVIO_CHAT_PERF_FLAGS__?: Record<string, boolean> }).__KIVIO_CHAT_PERF_FLAGS__ = {
      [CHAT_PERFORMANCE_FLAG_KEYS.tanstackVirtualizer]: true,
    }
    expect(getChatPerformanceFlags().tanstackVirtualizer).toBe(true)
  })

  it('can refresh diagnostic overrides at runtime', () => {
    expect(getChatPerformanceFlags().tanstackVirtualizer).toBe(true)
    window.localStorage.setItem(CHAT_PERFORMANCE_FLAG_KEYS.tanstackVirtualizer, '0')
    expect(refreshChatPerformanceFlags().tanstackVirtualizer).toBe(false)
  })
})
