import { act, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ChatMarkdown } from './ChatMarkdown'
import { MarkdownStreamingContext } from './markdownStreaming'

// 回归测试：流式生成中的 HTML 代码块不能挂 iframe —— srcDoc 每个 delta 换一次就是整篇重载，
// 页面闪 + 高度重测把聊天列表拽回底部。只有完成态才切回完整 Markdown/iframe。
const block = (body: string) => '```html\n<html><body>' + body + '\n'

const streaming = (content: string) => (
  <MarkdownStreamingContext.Provider value={true}>
    <ChatMarkdown content={content} />
  </MarkdownStreamingContext.Provider>
)

describe('HTML 预览挂载时机', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('生成中始终走轻量源码，完成后才挂 iframe', () => {
    const { container, rerender } = render(streaming(block('<p>a</p>')))
    expect(container.querySelector('iframe')).toBeNull()

    rerender(streaming(block('<p>ab</p>')))
    act(() => void vi.advanceTimersByTime(400))
    rerender(streaming(block('<p>abc</p>')))
    act(() => void vi.advanceTimersByTime(400))
    expect(container.querySelector('iframe')).toBeNull() // 还没静默

    act(() => void vi.advanceTimersByTime(1_200))
    expect(container.querySelector('iframe')).toBeNull()

    rerender(<ChatMarkdown content={block('<p>abc</p>')} />)
    expect(container.querySelector('iframe')?.getAttribute('srcdoc')).toContain('<p>abc</p>')
  })

  it('历史消息（非流式）首帧直接挂 iframe', () => {
    const { container } = render(<ChatMarkdown content={block('<p>done</p>')} />)
    expect(container.querySelector('iframe')?.getAttribute('srcdoc')).toContain('<p>done</p>')
  })
})
