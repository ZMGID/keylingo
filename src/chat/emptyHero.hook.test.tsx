import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  EMPTY_HERO_JAB_MS,
  EMPTY_HERO_MUTTER_MS,
  EMPTY_HERO_ROTATE_MAX_MS,
  EMPTY_HERO_ROTATE_MIN_MS,
  emptyHeroGreetings,
  emptyHeroMutter,
  useEmptyHeroJab,
  useEmptyHeroLine,
  useEmptyHeroMutter,
} from './emptyHero'

describe('useEmptyHeroLine', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.spyOn(Math, 'random').mockReturnValue(0)
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('空态按间隔轮换问候', () => {
    const greetings = emptyHeroGreetings('zh')
    const { result } = renderHook(() => useEmptyHeroLine({ lang: 'zh', seed: null, active: true }))
    expect(result.current).toBe(greetings[0])

    act(() => {
      vi.advanceTimersByTime(EMPTY_HERO_ROTATE_MIN_MS)
    })
    expect(result.current).toBe(greetings[1])

    act(() => {
      vi.advanceTimersByTime(EMPTY_HERO_ROTATE_MIN_MS * (greetings.length - 1))
    })
    expect(result.current).toBe(greetings[0])
  })

  it('助手名钉住，不轮换', () => {
    const { result } = renderHook(() => useEmptyHeroLine({
      lang: 'zh',
      assistantName: '翻译官',
      seed: null,
      active: true,
    }))
    expect(result.current).toBe('翻译官')
    act(() => {
      vi.advanceTimersByTime(EMPTY_HERO_ROTATE_MAX_MS * 3)
    })
    expect(result.current).toBe('翻译官')
  })
})

describe('useEmptyHeroJab', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.spyOn(Math, 'random').mockReturnValue(0)
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('点一下切吐槽，过一会收回', () => {
    const { result } = renderHook(() => useEmptyHeroJab('zh'))
    expect(result.current.jab).toBeNull()
    act(() => {
      result.current.onPoke(1)
    })
    expect(result.current.jab).toBe('？')
    act(() => {
      vi.advanceTimersByTime(EMPTY_HERO_JAB_MS)
    })
    expect(result.current.jab).toBeNull()
  })

  it('连点升到更冲的档', () => {
    const { result } = renderHook(() => useEmptyHeroJab('zh'))
    act(() => {
      result.current.onPoke(6)
    })
    expect(result.current.jab).toBe('急了')
  })
})

describe('useEmptyHeroMutter', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.spyOn(Math, 'random').mockReturnValue(0)
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('变形态时嘟囔一句，过一会收回；圆没有词', () => {
    const { result } = renderHook(() => useEmptyHeroMutter('zh'))
    expect(result.current.mutter).toBeNull()
    act(() => {
      result.current.onAntic('cloud')
    })
    expect(result.current.mutter).toBe('走神了')
    act(() => {
      vi.advanceTimersByTime(EMPTY_HERO_MUTTER_MS)
    })
    expect(result.current.mutter).toBeNull()
    act(() => {
      result.current.onAntic('circle')
    })
    expect(result.current.mutter).toBeNull()
  })

  it('不是每次都说：蹦比变形态更少开口', () => {
    expect(emptyHeroMutter('zh', 'egg', () => 0.5)).not.toBeNull()
    expect(emptyHeroMutter('zh', 'hop', () => 0.5)).toBeNull()
    expect(emptyHeroMutter('en', 'hop', () => 0.1)).toBe('Stretching.')
    expect(emptyHeroMutter('en', 'squircle', () => 0.9)).toBeNull()
  })
})
