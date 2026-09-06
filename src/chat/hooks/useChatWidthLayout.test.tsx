import { act, fireEvent, renderHook } from '@testing-library/react'
import { Virtualizer } from '@tanstack/react-virtual'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ScrollFollowHandle } from '../scroll/useScrollFollow'
import { useChatWidthLayout } from './useChatWidthLayout'

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks() })

function setup({ following = false, locked = false, live = false } = {}) {
  let resize!: ResizeObserverCallback
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: ResizeObserverCallback) { resize = callback }
    observe() {}
    disconnect() {}
  })
  const viewport = document.createElement('div')
  const content = viewport.appendChild(document.createElement('div'))
  const root = content.appendChild(document.createElement('div'))
  root.dataset.chatRowsRoot = ''
  const row = root.appendChild(document.createElement('div'))
  row.dataset.chatReadingRow = ''
  if (!live) row.dataset.index = '2'
  document.body.appendChild(viewport)
  let offset = 500
  let maxOffset = 2000
  let paintedStart = 460
  Object.defineProperty(content, 'clientWidth', { value: 848 })
  Object.defineProperty(viewport, 'scrollTop', {
    get: () => offset,
    set: (next: number) => { offset = Math.min(maxOffset, Math.max(0, next)) },
  })
  const rect = (top: number, height: number) => ({ top, bottom: top + height, height } as DOMRect)
  vi.spyOn(viewport, 'getBoundingClientRect').mockImplementation(() => rect(100, 600))
  vi.spyOn(root, 'getBoundingClientRect').mockImplementation(() => rect(120 - offset, 1500))
  vi.spyOn(row, 'getBoundingClientRect').mockImplementation(() => rect(120 - offset + paintedStart, 300))
  const follow: ScrollFollowHandle = {
    isFollowing: () => following,
    scrollToOffset: vi.fn((next) => { viewport.scrollTop = next }),
    markLayoutCompensation: vi.fn(),
    stickToBottom: vi.fn(), jumpToBottom: vi.fn(), releaseFollow: vi.fn(), pinIfFollowing: vi.fn(),
  }
  const navigationLocked = { current: locked }
  const hook = renderHook(() => useChatWidthLayout(content, viewport, follow, navigationLocked))
  const instance = new Virtualizer<HTMLDivElement, HTMLDivElement>({
    count: live ? 2 : 3,
    getScrollElement: () => null,
    getItemKey: index => `old:${index}`,
    estimateSize: index => [200, 260, 300][index],
    observeElementRect: () => undefined,
    observeElementOffset: () => undefined,
    scrollToFn: () => undefined,
  })
  instance.getTotalSize()
  const changeWidth = (width = 800) => {
    act(() => resize([{ contentRect: { width } } as ResizeObserverEntry], {} as ResizeObserver))
    instance.setOptions({ ...instance.options, getItemKey: index => `${width}:${index}`, estimateSize: () => 350 })
    instance.getTotalSize()
  }
  const restore = () => act(() => hook.result.current.restoreAnchor(instance))
  const cleanup = () => { hook.unmount(); viewport.remove() }
  return { ...hook, viewport, row, follow, instance, navigationLocked, changeWidth, restore, cleanup,
    paint: (start: number) => { paintedStart = start },
    clamp: (max: number) => { maxOffset = max },
  }
}

describe('useChatWidthLayout', () => {
  it('captures before row ResizeObservers can move the old layout', () => {
    const view = setup()
    act(() => view.result.current.prepareWidthChange(800))
    expect(view.result.current.anchorRef.current?.top).toBe(-20)
    expect(view.result.current.contentWidth).toBe(864)
    // The content observer runs later, after an earlier row observer has
    // already changed row positions. It must retain the original reading point.
    view.paint(520)
    view.changeWidth()
    expect(view.result.current.anchorRef.current?.top).toBe(-20)
    view.paint(700)
    view.restore()
    expect(view.viewport.scrollTop).toBe(740)
    expect(view.result.current.anchorRef.current).toBeNull()
    view.cleanup()
  })

  it.each([false, true])('preserves the reading offset across cache and DOM commits (live: %s)', (live) => {
    const view = setup({ live })
    view.changeWidth()
    expect(view.result.current.anchorRef.current?.row).toBe(view.row)
    expect(view.result.current.anchorRef.current?.index).toBe(live ? null : 2)
    expect(view.result.current.anchorRef.current?.top).toBe(-20)
    // A changed cache must not consume the anchor while DOM transforms are old.
    view.restore()
    expect(view.viewport.scrollTop).toBe(740)
    expect(view.result.current.anchorRef.current).not.toBeNull()
    view.paint(700)
    view.restore()
    expect(view.row.getBoundingClientRect().top).toBe(80)
    expect(view.result.current.anchorRef.current).toBeNull()
    view.cleanup()
  })

  it('survives temporary browser clamping while the old spacer is still committed', () => {
    const view = setup()
    view.changeWidth()
    view.clamp(600)
    view.restore()
    expect(view.viewport.scrollTop).toBe(600)
    expect(view.result.current.anchorRef.current).not.toBeNull()
    view.clamp(1200)
    view.paint(700)
    view.restore()
    expect(view.viewport.scrollTop).toBe(740)
    expect(view.result.current.anchorRef.current).toBeNull()
    view.cleanup()
  })

  it.each(['wheel', 'touchstart', 'pointerdown', 'keydown'])('yields to a new %s action during layout', (event) => {
    const view = setup()
    view.changeWidth()
    fireEvent(view.viewport, new Event(event))
    view.restore()
    expect(view.follow.scrollToOffset).not.toHaveBeenCalled()
    expect(view.result.current.anchorRef.current).toBeNull()
    view.cleanup()
  })

  it.each([{ following: true }, { locked: true }])('leaves following and navigation in control: %j', (options) => {
    const view = setup(options)
    view.changeWidth()
    view.restore()
    expect(view.result.current.contentWidth).toBe(800)
    expect(view.result.current.anchorRef.current).toBeNull()
    expect(view.follow.scrollToOffset).not.toHaveBeenCalled()
    view.cleanup()
  })

  it('keeps the original anchor when another resize arrives before transforms settle', () => {
    const view = setup()
    view.changeWidth()
    view.restore()
    view.changeWidth(768)
    expect(view.result.current.anchorRef.current?.top).toBe(-20)
    view.paint(700)
    view.restore()
    expect(view.viewport.scrollTop).toBe(740)
    expect(view.result.current.anchorRef.current).toBeNull()
    view.cleanup()
  })
})
