import { useState } from 'react'
import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useScrollFollow } from './useScrollFollow'

type ResizeObserverHarness = {
  callback: ResizeObserverCallback
  disconnected: boolean
}

function Harness() {
  const [viewport, setViewport] = useState<HTMLDivElement | null>(null)
  const [content, setContent] = useState<HTMLDivElement | null>(null)
  const { following, showJumpButton } = useScrollFollow({ viewport, content })
  return (
    <>
      <output data-testid="following">{String(following)}</output>
      <output data-testid="show-jump-button">{String(showJumpButton)}</output>
      <div ref={setViewport} data-testid="viewport">
        <div ref={setContent} />
      </div>
    </>
  )
}

describe('useScrollFollow scroll source timing', () => {
  const observers: ResizeObserverHarness[] = []
  const originalResizeObserver = window.ResizeObserver

  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
    window.ResizeObserver = originalResizeObserver
    observers.length = 0
  })

  function mount() {
    vi.useFakeTimers()
    window.ResizeObserver = class {
      private harness: ResizeObserverHarness

      constructor(callback: ResizeObserverCallback) {
        this.harness = { callback, disconnected: false }
        observers.push(this.harness)
      }

      observe() {}
      unobserve() {}
      disconnect() {
        this.harness.disconnected = true
      }
    } as unknown as typeof ResizeObserver

    render(<Harness />)
    const viewport = screen.getByTestId('viewport')
    let scrollTop = 100
    Object.defineProperties(viewport, {
      scrollTop: {
        configurable: true,
        get: () => scrollTop,
        set: (value: number) => { scrollTop = value },
      },
      scrollHeight: { configurable: true, get: () => 1000 },
      clientHeight: { configurable: true, get: () => 500 },
    })
    return viewport
  }

  it('classifies an ordinary scroll immediately when no layout growth occurred', () => {
    const viewport = mount()

    fireEvent.scroll(viewport)
    expect(screen.getByTestId('following')).toHaveTextContent('false')

  })

  it('still releases follow for a scroll with no resize evidence', () => {
    const viewport = mount()

    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(1))

    expect(screen.getByTestId('following')).toHaveTextContent('false')
  })

  it('does not show the jump button until the reader is meaningfully away from bottom', () => {
    const viewport = mount()

    viewport.scrollTop = 480
    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(1))
    expect(screen.getByTestId('following')).toHaveTextContent('false')
    expect(screen.getByTestId('show-jump-button')).toHaveTextContent('false')

    viewport.scrollTop = 200
    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(1))
    expect(screen.getByTestId('show-jump-button')).toHaveTextContent('true')
  })

  // 竞态回归：来源判定被推迟一拍（setTimeout(1)），而 resize 窗口的关闭是 rAF + 宏任务。
  // 真实浏览器里快帧下关闭会抢先于判定执行 —— scroll 事件派发于 scroll steps（窗口还开着），
  // 判定 timer 却在关闭之后才跑。token 对不上的补偿滚动（virtua shift 纠正 / 浏览器 clamp）
  // 此时必须仍按 self 记账（窗口状态在事件时同步抓取），否则流式中跟随莫名解除。
  it('releases when a later scroll is not accompanied by layout growth', () => {
    // rAF 桩成手动队列（mount() 里的 vi.useFakeTimers 会装假 rAF，须在其后再桩，
    // 且要桩 globalThis —— vitest 的 jsdom 环境把全局拷到 globalThis，模块裸调走它）。
    const viewport = mount()
    const rafQueue: FrameRequestCallback[] = []
    const rafStub = vi
      .spyOn(globalThis, 'requestAnimationFrame')
      .mockImplementation((cb) => rafQueue.push(cb))
    const cafStub = vi.spyOn(globalThis, 'cancelAnimationFrame').mockImplementation(() => {})
    try {
      expect(screen.getByTestId('following')).toHaveTextContent('true')

      // RO 触发：打开窗口 + contentGrowth 钉底（applyScrollTop 登记 token=1000），
      // 关闭链第一跳（rAF）入手动队列。
      act(() => {
        observers[0].callback([], {} as ResizeObserver)
      })

      // 手动执行 rAF 那一跳：关闭的 setTimeout(0) 现已排入队列，且**先于**判定 timer 入队。
      // 窗口此刻仍开着。
      act(() => {
        rafQueue.splice(0).forEach((cb) => cb(0))
      })

      // virtua 式直接写 scrollTop（不经过 applyScrollTop，token 对不上），随后 scroll 事件到达：
      // 事件时窗口开着（真实次序如此 —— scroll steps 先于当帧 rAF）。
      viewport.scrollTop = 300
      fireEvent.scroll(viewport)

      // 同一次推进里：关闭（先入队）→ 判定（后入队）。判定那一刻窗口已关，
      // 事件时抓取的窗口状态必须让这条滚动仍算 self。
      act(() => {
        vi.advanceTimersByTime(1)
      })

      expect(screen.getByTestId('following')).toHaveTextContent('false')
    } finally {
      rafStub.mockRestore()
      cafStub.mockRestore()
    }
  })

  it('keeps following when content growth scrolls before ResizeObserver and the pin token is gone', () => {
    const viewport = mount()
    let scrollHeight = 1000
    Object.defineProperty(viewport, 'scrollHeight', {
      configurable: true,
      get: () => scrollHeight,
    })

    // Consume the previous programmatic-scroll token without leaving the bottom.
    viewport.scrollTop = 500
    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(1))

    // Simulate a virtualizer/browser compensation scroll arriving before RO delivery.
    scrollHeight = 1200
    viewport.scrollTop = 500
    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(1))

    expect(screen.getByTestId('following')).toHaveTextContent('true')
  })

  it('still releases when the user scrolls up during the same content growth', () => {
    const viewport = mount()
    let scrollHeight = 1000
    Object.defineProperty(viewport, 'scrollHeight', {
      configurable: true,
      get: () => scrollHeight,
    })

    viewport.scrollTop = 500
    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(1))

    scrollHeight = 1200
    viewport.scrollTop = 400
    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(1))

    expect(screen.getByTestId('following')).toHaveTextContent('false')
  })

})
