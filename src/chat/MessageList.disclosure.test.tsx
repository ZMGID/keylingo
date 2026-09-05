import { act, fireEvent, render, screen } from '@testing-library/react'
import type { Virtualizer } from '@tanstack/react-virtual'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { MessageList } from './MessageList'

let list: Virtualizer<Element, Element>
vi.mock('@tanstack/react-virtual', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@tanstack/react-virtual')>()
  return {
    ...actual,
    useVirtualizer: (...args: Parameters<typeof actual.useVirtualizer>) => {
      list = actual.useVirtualizer(...args)
      return list
    },
  }
})

afterEach(() => vi.restoreAllMocks())

async function mount() {
  const { container } = render(<MessageList conversationId="disclosure-regression" messages={[
    { id: 'question', role: 'user', content: 'Question', timestamp: 1 },
    { id: 'answer', role: 'assistant', content: 'Answer', timestamp: 2, segments: [
      { id: 'process', kind: 'text', phase: 'tool_loop', order: 0, text: 'Process details' },
      { id: 'final', kind: 'text', phase: 'synthesis', order: 1, text: 'Answer' },
    ] },
    { id: 'next-question', role: 'user', content: 'Next question', timestamp: 3 },
  ]} />)
  await act(async () => { await Promise.resolve() })
  const viewport = container.querySelector<HTMLElement>('.chat-scroll-viewport')!
  const button = screen.getByRole('button', { name: /^Worked/ })
  const index = Number(button.closest<HTMLElement>('[data-chat-row-index]')!.dataset.chatRowIndex)
  vi.spyOn(viewport, 'getBoundingClientRect').mockReturnValue({ top: 0, bottom: 800, height: 800 } as DOMRect)
  vi.spyOn(button, 'getBoundingClientRect').mockReturnValue({ top: 50, bottom: 70 } as DOMRect)
  act(() => {
    list.resizeItem(index - 1, 200)
    list.resizeItem(index, 1000)
  })
  return { container, viewport, button, index }
}

describe('MessageList disclosure scroll anchoring', () => {
  it('does not follow an expanded or collapsed process at the bottom of the list', async () => {
    const { viewport, button, index } = await mount()
    const offset = list.getTotalSize() - list.scrollRect!.height
    list.scrollOffset = offset
    viewport.scrollTop = offset
    expect(list.options.anchorTo).toBe('end')
    fireEvent.click(button)
    act(() => list.resizeItem(index, 1400))
    // The real virtualizer used to move this by +400 even if its adjustment
    // callback returned false: anchorTo=end has its own near-bottom path.
    expect(viewport.scrollTop).toBe(offset)
    fireEvent.click(button)
    act(() => list.resizeItem(index, 1000))
    expect(viewport.scrollTop).toBe(offset)
  })

  it('preserves a visible clicked heading when its containing row starts above the viewport', async () => {
    const { viewport, button, index } = await mount()
    list.scrollOffset = 250
    viewport.scrollTop = 250
    fireEvent.click(button)
    act(() => list.resizeItem(index, 1400))
    expect(viewport.scrollTop).toBe(250)
  })

  it('positions the next message in the same observer delivery as the animated height', async () => {
    const { container, button, index } = await mount()
    fireEvent.click(button)
    const row = button.closest<HTMLElement>('[data-chat-row-index]')!
    row.querySelector('[data-chat-disclosure-body]')!.setAttribute('data-chat-disclosure-animating', 'true')
    const next = container.querySelector<HTMLElement>(`[data-chat-row-index="${index + 1}"]`)!
    const start = list.getVirtualItems().find(item => item.index === index + 1)!.start
    const entry = { target: row, borderBoxSize: [{ blockSize: 1400, inlineSize: 600 }] } as unknown as ResizeObserverEntry
    act(() => {
      list.options.measureElement(row, entry, list)
      // Assert inside the delivery, before act can flush a deferred render.
      expect(next.style.transform).toBe(`translateY(${start + 400}px)`)
    })
  })
})
