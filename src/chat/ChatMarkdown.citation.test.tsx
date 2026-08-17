import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { ChatMarkdown } from './ChatMarkdown'
import type { CitationView } from './citations'

describe('ChatMarkdown citation popover', () => {
  it('renders the popover in the viewport layer instead of the clipped message container', () => {
    const citation: CitationView = {
      kind: 'web',
      n: 4,
      title: 'Introducing Claude Opus 5',
      url: 'https://www.anthropic.com/news/claude-opus-5',
      host: 'anthropic.com',
      snippet: 'A source snippet.',
    }
    const { container } = render(
      <ChatMarkdown content="See source [4]." citations={new Map([[4, citation]])} />,
    )

    fireEvent.click(screen.getByRole('button', { name: '来源 4' }))
    const popover = screen.getByRole('dialog', { name: '来源 4' })

    expect(popover).toHaveClass('fixed')
    expect(container.contains(popover)).toBe(false)
    expect(document.body.contains(popover)).toBe(true)

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: '来源 4' })).not.toBeInTheDocument()
  })
})
