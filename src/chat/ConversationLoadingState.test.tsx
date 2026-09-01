import { act, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ConversationLoadingState } from './ConversationLoadingState'

function hasLogo(container: HTMLElement): boolean {
  return container.querySelector('.kv-stream-dot-logo') !== null
}

describe('ConversationLoadingState', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('shows the dot logo immediately when predicted heavy', () => {
    const { container } = render(<ConversationLoadingState showAnimation />)
    expect(hasLogo(container)).toBe(true)
  })

  it('falls back to the dot logo when a "small" conversation is still loading after the delay', () => {
    const { container } = render(<ConversationLoadingState showAnimation={false} />)
    expect(hasLogo(container)).toBe(false)
    act(() => {
      vi.advanceTimersByTime(149)
    })
    expect(hasLogo(container)).toBe(false)
    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(hasLogo(container)).toBe(true)
  })

  it('never shows the logo when the overlay unmounts before the delay elapses', () => {
    const { container, unmount } = render(<ConversationLoadingState showAnimation={false} />)
    unmount()
    act(() => {
      vi.advanceTimersByTime(300)
    })
    expect(container.querySelector('.chat-conversation-loading')).toBeNull()
  })
})
