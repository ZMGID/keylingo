import { render, screen, within } from '@testing-library/react'
import { renderToString } from 'react-dom/server'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { ReasoningBlock } from './ReasoningBlock'

describe('ReasoningBlock', () => {
  it('renders a completed collapsed body at zero height on the first paint', () => {
    const html = renderToString(<ReasoningBlock reasoning="hidden until expanded" />)
    expect(html).toContain('aria-hidden="true"')
    expect(html).toContain('max-height:0px')
  })

  it('renders section shell without a scroll body for empty reasoning', () => {
    render(<ReasoningBlock reasoning="" />)
    const section = screen.getByLabelText('Thinking')
    expect(within(section).queryByTestId('reasoning-scroll')).not.toBeInTheDocument()
  })

  it('shows thinking title and a fixed scrolling text area while streaming', () => {
    render(<ReasoningBlock reasoning={'line one\nline two\nline three\nline four'} streaming />)
    const section = screen.getByLabelText('Thinking')
    expect(within(section).getByRole('button', { name: /Thinking/i })).toBeInTheDocument()
    expect(within(section).getByTestId('reasoning-frame')).toBeInTheDocument()
    expect(within(section).getByTestId('reasoning-scroll')).toHaveClass('is-streaming')
    expect(within(section).getByTestId('reasoning-text')).toHaveTextContent('line one')
    expect(within(section).getByTestId('reasoning-text')).toHaveTextContent('line four')
  })

  it('leaves max-height unset while streaming so growth never animates', () => {
    // 流式中若给 body 写 max-height，CSS 过渡会让内容高度逐帧变化（实测一次收起 18 帧），
    // 消息区的 ResizeObserver 钉底会逐帧重写 scrollTop —— 表现为整屏抖动。
    const { rerender, container } = render(<ReasoningBlock reasoning="alpha" streaming />)
    const body = container.querySelector('.chat-motion-reasoning-body') as HTMLElement
    expect(body.style.maxHeight).toBe('')

    rerender(<ReasoningBlock reasoning={'alpha\nbeta\ngamma'} streaming />)
    expect(body.style.maxHeight).toBe('')
  })

  it('renders markdown and code fences as plain thinking text', () => {
    render(<ReasoningBlock reasoning={'Before\n```ts\nconst x = 1\n```\nAfter'} streaming />)
    const section = screen.getByLabelText('Thinking')
    expect(section.querySelector('pre')).toBeNull()
    expect(section.querySelector('code')).toBeNull()
    expect(within(section).getByTestId('reasoning-text')).toHaveTextContent('```ts')
  })

  it('shows thinking duration beside the title when provided', () => {
    render(<ReasoningBlock reasoning="alpha" durationMs={65000} />)
    const section = screen.getByLabelText('Thinking')
    expect(within(section).getByRole('button', { name: /Thinking/i })).toHaveTextContent('Thinking')
    expect(within(section).getByRole('button', { name: /Thinking/i })).toHaveTextContent('1m 5s')
  })

  it('expands full reasoning after toggle', async () => {
    const user = userEvent.setup()
    const reasoning = 'alpha\nbeta\ngamma'
    render(<ReasoningBlock reasoning={reasoning} />)
    const section = screen.getByLabelText('Thinking')
    await user.click(within(section).getByRole('button', { name: /Thinking/i }))
    const text = within(section).getByTestId('reasoning-text')
    expect(within(section).getByTestId('reasoning-scroll')).toHaveClass('is-expanded')
    expect(text.textContent).toContain('alpha')
    expect(text.textContent).toContain('beta')
    expect(text.textContent).toContain('gamma')
  })

  it('hides markdown body after streaming completes while collapsed', () => {
    const { rerender } = render(<ReasoningBlock reasoning="done thinking" streaming />)
    rerender(<ReasoningBlock reasoning="done thinking" streaming={false} />)
    const section = screen.getByLabelText('Thinking')
    expect(within(section).getByRole('button', { name: /Thinking/i })).toBeInTheDocument()
    expect(within(section).getByTestId('reasoning-scroll').closest('[aria-hidden]')).toHaveAttribute('aria-hidden', 'true')
  })
})
