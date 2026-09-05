import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ChatDisclosureBody } from './ChatDisclosureBody'

let paintedHeight = 0
let contentHeight = 240
let resize: ResizeObserverCallback
const disconnect = vi.fn()
const animations: Array<{ cancel: ReturnType<typeof vi.fn>; onfinish: (() => void) | null }> = []
const animate = vi.fn<(keyframes: Keyframe[], options: { duration: number; easing: string }) => Animation>(() => {
  const animation = { cancel: vi.fn(), onfinish: null as (() => void) | null }
  animations.push(animation)
  return animation as unknown as Animation
})

beforeEach(() => {
  paintedHeight = 0
  contentHeight = 240
  animations.length = 0
  animate.mockClear()
  disconnect.mockClear()
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: ResizeObserverCallback) { resize = callback }
    observe() {}
    disconnect = disconnect
  })
  vi.stubGlobal('matchMedia', () => ({ matches: false }))
  Object.defineProperty(HTMLElement.prototype, 'animate', { configurable: true, value: animate })
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    return { height: this.hasAttribute('data-chat-disclosure-body') ? paintedHeight : contentHeight } as DOMRect
  })
})

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  Reflect.deleteProperty(HTMLElement.prototype, 'animate')
})

describe('ChatDisclosureBody', () => {
  it('leaves collapsed details unmounted and starts opening at zero height', () => {
    const { rerender } = render(<ChatDisclosureBody open={false}>Details</ChatDisclosureBody>)
    expect(screen.queryByText('Details')).not.toBeInTheDocument()
    rerender(<ChatDisclosureBody open>Details</ChatDisclosureBody>)
    expect(screen.getByText('Details')).toBeInTheDocument()
    expect(animate.mock.calls[0][0]).toEqual([{ height: '0px' }, { height: '240px' }])
  })

  it('keeps content until closing finishes and reverses from the painted height', () => {
    const { rerender } = render(<ChatDisclosureBody open>Details</ChatDisclosureBody>)
    rerender(<ChatDisclosureBody open={false}>Details</ChatDisclosureBody>)
    const closing = animations[0]
    expect(screen.getByText('Details')).toBeInTheDocument()
    paintedHeight = 80
    rerender(<ChatDisclosureBody open>Details</ChatDisclosureBody>)
    expect(closing.cancel).toHaveBeenCalledOnce()
    expect(animate.mock.calls[1][0]).toEqual([{ height: '80px' }, { height: '240px' }])
    act(() => closing.onfinish?.())
    expect(screen.getByText('Details')).toBeInTheDocument()
    act(() => animations[1].onfinish?.())
    rerender(<ChatDisclosureBody open={false}>Details</ChatDisclosureBody>)
    act(() => animations[2].onfinish?.())
    expect(screen.queryByText('Details')).not.toBeInTheDocument()
  })

  it('retargets late-loading content without restarting the duration', () => {
    const { rerender, unmount } = render(<ChatDisclosureBody open={false}>Details</ChatDisclosureBody>)
    rerender(<ChatDisclosureBody open>Details</ChatDisclosureBody>)
    paintedHeight = 90
    contentHeight = 420
    act(() => resize([], {} as ResizeObserver))
    expect(animate.mock.calls[1][0]).toEqual([{ height: '90px' }, { height: '420px' }])
    expect(animate.mock.calls[1][1].duration).toBeLessThanOrEqual(animate.mock.calls[0][1].duration)
    unmount()
    expect(animations[1].cancel).toHaveBeenCalled()
    expect(disconnect).toHaveBeenCalled()
  })

  it('does not animate stream growth or automatic completion', () => {
    const { rerender } = render(<ChatDisclosureBody open animate={false}>First token</ChatDisclosureBody>)
    rerender(<ChatDisclosureBody open animate={false}>More tokens</ChatDisclosureBody>)
    rerender(<ChatDisclosureBody open={false} animate={false}>More tokens</ChatDisclosureBody>)
    expect(animate).not.toHaveBeenCalled()
    expect(screen.queryByText('More tokens')).not.toBeInTheDocument()
  })

  it('honors reduced motion and keeps hidden content inert when requested', () => {
    vi.stubGlobal('matchMedia', () => ({ matches: true }))
    const { rerender, container } = render(<ChatDisclosureBody open>Details</ChatDisclosureBody>)
    rerender(<ChatDisclosureBody open={false} keepMounted>Details</ChatDisclosureBody>)
    expect(animate).not.toHaveBeenCalled()
    expect(screen.getByText('Details')).toBeInTheDocument()
    expect((container.firstElementChild as HTMLElement).inert).toBe(true)
    expect(container.firstElementChild).toHaveAttribute('aria-hidden', 'true')
  })
})
