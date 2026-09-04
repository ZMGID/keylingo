import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { STATUS_QUIPS, pickQuip, quipPlan, useStatusQuip } from './blobQuips'

describe('blobQuips', () => {
  it('闲置不说话；边沿心情进去就可能说且常驻，阶段心情等一会再说、说完收回；哪种都不是必说', () => {
    expect(quipPlan('idle')).toBeNull()
    expect(quipPlan('done')).toEqual({ first: 0, show: null, gap: [0, 0], chance: 0.4 })
    expect(quipPlan('error')?.show).toBeNull()
    expect(quipPlan('wait')?.show).toBeNull()
    for (const mood of ['think', 'search', 'work', 'speak'] as const) {
      const plan = quipPlan(mood)!
      expect(plan.first).toBeGreaterThanOrEqual(6000)
      expect(plan.show).toBeGreaterThan(0)
      expect(plan.gap[0]).toBeGreaterThan(plan.show! * 3)
    }
    for (const mood of ['think', 'search', 'work', 'speak', 'error', 'done', 'wait'] as const) {
      expect(quipPlan(mood)!.chance).toBeLessThanOrEqual(0.5)
    }
  })

  it('pickQuip 从对应池子里取、避开上一句、两种语言都有词', () => {
    expect(pickQuip('zh', 'idle')).toBeNull()
    expect(pickQuip('zh', 'done', null, () => 0)).toBe(STATUS_QUIPS.zh.done[0])
    expect(pickQuip('zh', 'done', STATUS_QUIPS.zh.done[0], () => 0)).toBe(STATUS_QUIPS.zh.done[1])
    expect(pickQuip('en', 'done', null, () => 0.999)).toBe(STATUS_QUIPS.en.done.at(-1))
    for (const lang of ['zh', 'en'] as const) {
      for (const mood of ['think', 'search', 'work', 'speak', 'error', 'done', 'wait'] as const) {
        expect(STATUS_QUIPS[lang][mood].length).toBeGreaterThan(2)
      }
    }
  })
})

describe('useStatusQuip', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.spyOn(Math, 'random').mockReturnValue(0)
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('思考：先憋够 first 再开口，挂 show 收回，隔 gap 再来一句不重样', () => {
    const plan = quipPlan('think')!
    const { result } = renderHook(() => useStatusQuip('think', 'zh'))
    expect(result.current).toBeNull()
    act(() => {
      vi.advanceTimersByTime(plan.first - 1)
    })
    expect(result.current).toBeNull()
    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(result.current).toBe(STATUS_QUIPS.zh.think[0])
    act(() => {
      vi.advanceTimersByTime(plan.show!)
    })
    expect(result.current).toBeNull()
    act(() => {
      vi.advanceTimersByTime(plan.gap[0])
    })
    expect(result.current).toBe(STATUS_QUIPS.zh.think[1])
  })

  it('没掷中就闭嘴，等下一个点再掷', () => {
    vi.spyOn(Math, 'random').mockReturnValue(0.99)
    const plan = quipPlan('search')!
    const { result } = renderHook(() => useStatusQuip('search', 'zh'))
    act(() => {
      vi.advanceTimersByTime(plan.first + plan.gap[1] * 3)
    })
    expect(result.current).toBeNull()
    const { result: done } = renderHook(() => useStatusQuip('done', 'zh'))
    act(() => {
      vi.advanceTimersByTime(10)
    })
    expect(done.current).toBeNull()
  })

  it('收工立刻说、常驻；换心情马上闭嘴重排', () => {
    const { result, rerender } = renderHook(({ mood }: { mood: 'done' | 'idle' }) => useStatusQuip(mood, 'en'), {
      initialProps: { mood: 'done' },
    })
    act(() => {
      vi.advanceTimersByTime(0)
    })
    expect(result.current).toBe(STATUS_QUIPS.en.done[0])
    act(() => {
      vi.advanceTimersByTime(60_000)
    })
    expect(result.current).toBe(STATUS_QUIPS.en.done[0])
    rerender({ mood: 'idle' })
    expect(result.current).toBeNull()
  })

  it('enabled=false 全程不吭声', () => {
    const { result } = renderHook(() => useStatusQuip('error', 'zh', false))
    act(() => {
      vi.advanceTimersByTime(10_000)
    })
    expect(result.current).toBeNull()
  })
})
