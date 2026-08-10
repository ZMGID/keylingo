import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  beginMessageNavigationHydrate,
  beginStreamSettleEagerHydrate,
  endMessageNavigationHydrate,
  isMessageNavigationEagerHydrate,
  resetMessageNavigationStore,
  STREAM_SETTLE_EAGER_MS,
} from './messageNavigationStore'

afterEach(() => {
  resetMessageNavigationStore()
  vi.useRealTimers()
})

describe('messageNavigationStore', () => {
  it('begin enables eager hydrate and end only clears matching generation', () => {
    expect(isMessageNavigationEagerHydrate()).toBe(false)

    const first = beginMessageNavigationHydrate()
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    const second = beginMessageNavigationHydrate()
    expect(second).not.toBe(first)
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    // 旧 generation 不能关掉新一轮 settle。
    endMessageNavigationHydrate(first)
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    endMessageNavigationHydrate(second)
    expect(isMessageNavigationEagerHydrate()).toBe(false)
  })

  it('reset clears eager regardless of generation', () => {
    beginMessageNavigationHydrate()
    expect(isMessageNavigationEagerHydrate()).toBe(true)
    resetMessageNavigationStore()
    expect(isMessageNavigationEagerHydrate()).toBe(false)
  })

  it('stream settle eager auto-ends after the settle window', () => {
    vi.useFakeTimers()
    beginStreamSettleEagerHydrate(STREAM_SETTLE_EAGER_MS)
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    vi.advanceTimersByTime(STREAM_SETTLE_EAGER_MS - 1)
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    vi.advanceTimersByTime(1)
    expect(isMessageNavigationEagerHydrate()).toBe(false)
  })

  it('a newer begin cancels the previous stream-settle auto-end', () => {
    vi.useFakeTimers()
    beginStreamSettleEagerHydrate(200)
    const manual = beginMessageNavigationHydrate()
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    // 旧 timer 不应把新 generation 关掉。
    vi.advanceTimersByTime(500)
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    endMessageNavigationHydrate(manual)
    expect(isMessageNavigationEagerHydrate()).toBe(false)
  })
})
