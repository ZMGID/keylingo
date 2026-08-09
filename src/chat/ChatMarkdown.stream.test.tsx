import { act, render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { ChatMarkdown } from './ChatMarkdown'
import { MarkdownStreamingContext } from './markdownStreaming'

describe('ChatMarkdown streaming stability', () => {
  it('流式中未闭合加粗由 Streamdown parseIncomplete 补全', async () => {
    const { container, rerender } = render(
      <MarkdownStreamingContext.Provider value={true}>
        <ChatMarkdown content={'前缀 **加粗'} />
      </MarkdownStreamingContext.Provider>,
    )

    // streaming 模式块更新可能走 transition；等一拍再断言。
    await act(async () => {
      await Promise.resolve()
    })
    expect(container.querySelector('[data-streamdown="strong"]')?.textContent).toBe('加粗')
    expect(container.textContent).toContain('前缀')
    expect(container.textContent).not.toContain('**')

    rerender(
      <MarkdownStreamingContext.Provider value={true}>
        <ChatMarkdown content={'前缀 **加粗文字'} />
      </MarkdownStreamingContext.Provider>,
    )
    await act(async () => {
      await Promise.resolve()
    })
    expect(container.querySelector('[data-streamdown="strong"]')?.textContent).toBe('加粗文字')
  })

  it('流式代码块不走 ChatHeavyIsland 延迟 hydrate', async () => {
    const { container } = render(
      <MarkdownStreamingContext.Provider value={true}>
        <ChatMarkdown content={'```ts\nconst x = 1\n```'} />
      </MarkdownStreamingContext.Provider>,
    )

    await act(async () => {
      await Promise.resolve()
    })
    expect(container.querySelector('[data-chat-heavy-island="true"]')).toBeNull()
    expect(container.querySelector('figure pre code')?.textContent).toContain('const x = 1')
  })
})
