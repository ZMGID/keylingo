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
  const { following } = useScrollFollow({ viewport, content })
  return (
    <>
      <output data-testid="following">{String(following)}</output>
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

  it('waits for ResizeObserver before classifying a compensating scroll', () => {
    const viewport = mount()

    fireEvent.scroll(viewport)
    expect(screen.getByTestId('following')).toHaveTextContent('true')

    act(() => {
      observers[0].callback([], {} as ResizeObserver)
      vi.advanceTimersByTime(1)
    })

    expect(screen.getByTestId('following')).toHaveTextContent('true')
  })

  it('still releases follow for a scroll with no resize evidence', () => {
    const viewport = mount()

    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(1))

    expect(screen.getByTestId('following')).toHaveTextContent('false')
  })
})
