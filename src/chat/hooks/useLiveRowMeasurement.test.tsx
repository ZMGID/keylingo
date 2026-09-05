import { StrictMode } from 'react'
import { act, render, renderHook } from '@testing-library/react'
import { Virtualizer } from '@tanstack/react-virtual'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { layoutScopedVirtualKey, measureChatVirtualRow } from '../messageListVirtualization'
import { useLiveRowMeasurement } from './useLiveRowMeasurement'

const layoutKey = 'conversation:848'
let resize: ResizeObserverCallback
let disconnect: ReturnType<typeof vi.fn>

beforeEach(() => {
  disconnect = vi.fn()
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: ResizeObserverCallback) { resize = callback }
    observe = vi.fn()
    disconnect = disconnect
  })
})

afterEach(() => vi.unstubAllGlobals())

function virtualizer(key: string, estimateSize: () => number) {
  const instance = new Virtualizer<HTMLDivElement, HTMLDivElement>({
    count: 1,
    getScrollElement: () => null,
    getItemKey: () => key,
    estimateSize,
    initialRect: { width: 848, height: 663 },
    observeElementRect: () => undefined,
    observeElementOffset: () => undefined,
    scrollToFn: () => undefined,
    measureElement: (element, entry) => measureChatVirtualRow(element, entry),
  })
  instance.getTotalSize()
  instance.isScrolling = true
  return instance
}

describe('useLiveRowMeasurement', () => {
  it.each(['live-turn-1', 'live-group-models'])('hands off %s before RO can report the collapsed height', (rowKey) => {
    const { result, rerender } = renderHook(
      ({ liveKey }: { liveKey: string | null }) => useLiveRowMeasurement(layoutKey, liveKey),
      { initialProps: { liveKey: rowKey as string | null } },
    )
    const element = document.createElement('div')
    element.dataset.index = '0'
    let height = 761
    Object.defineProperty(element, 'offsetHeight', { get: () => height })
    act(() => result.current.liveRowRef(element))
    expect(result.current.getLiveRowSize(rowKey)).toBe(761)

    // Freeze collapses reasoning, followed by the ref handoff in the same frame.
    height = 724
    act(() => result.current.liveRowRef(null))
    rerender({ liveKey: null })
    const instance = virtualizer(
      layoutScopedVirtualKey(layoutKey, rowKey),
      () => result.current.getLiveRowSize(rowKey) ?? 96,
    )
    expect(instance.getTotalSize()).toBe(761)
    act(() => result.current.measureRow(element, instance))
    expect(instance.getTotalSize()).toBe(724)
    expect(instance.elementsCache.get(layoutScopedVirtualKey(layoutKey, rowKey))).toBe(element)
    expect(result.current.getLiveRowSize(rowKey)).toBeUndefined()
    expect(disconnect).toHaveBeenCalledOnce()
  })

  it('keeps observing through StrictMode replay and cleans up on ref detach', () => {
    let measurement!: ReturnType<typeof useLiveRowMeasurement>
    function LiveRow() {
      measurement = useLiveRowMeasurement(layoutKey, 'live-turn-1')
      return <div ref={measurement.liveRowRef} />
    }
    const { rerender, unmount } = render(<StrictMode><LiveRow /></StrictMode>)
    const originalRef = measurement.liveRowRef
    act(() => resize([
      { borderBoxSize: [{ blockSize: 500, inlineSize: 848 }] } as unknown as ResizeObserverEntry,
    ], {} as ResizeObserver))
    rerender(<StrictMode><LiveRow /></StrictMode>)
    expect(measurement.getLiveRowSize('live-turn-1')).toBe(500)
    expect(measurement.liveRowRef).toBe(originalRef)
    expect(disconnect).not.toHaveBeenCalled()
    unmount()
    expect(disconnect).toHaveBeenCalledOnce()
  })

  it.each(['other-conversation:848', 'conversation:640'])('does not reuse a height in %s', (nextLayout) => {
    const { result, rerender } = renderHook(
      ({ layout }) => useLiveRowMeasurement(layout, 'live-turn-1'),
      { initialProps: { layout: layoutKey } },
    )
    const element = document.createElement('div')
    Object.defineProperty(element, 'offsetHeight', { value: 761 })
    act(() => result.current.liveRowRef(element))
    rerender({ layout: nextLayout })
    expect(result.current.getLiveRowSize('live-turn-1')).toBeUndefined()
  })

  it('leaves unrelated history on the normal deferred measurement path', () => {
    const { result } = renderHook(() => useLiveRowMeasurement(layoutKey, 'live-turn-1'))
    const live = document.createElement('div')
    Object.defineProperty(live, 'offsetHeight', { value: 761 })
    act(() => result.current.liveRowRef(live))
    const history = document.createElement('div')
    history.dataset.index = '0'
    Object.defineProperty(history, 'offsetHeight', { value: 300 })
    const instance = virtualizer(layoutScopedVirtualKey(layoutKey, 'older-answer'), () => 200)
    act(() => result.current.measureRow(history, instance))
    expect(instance.getTotalSize()).toBe(200)
    expect(result.current.getLiveRowSize('live-turn-1')).toBe(761)
  })
})
