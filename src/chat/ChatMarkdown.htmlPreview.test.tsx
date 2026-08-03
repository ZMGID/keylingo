import { act, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ChatMarkdown } from './ChatMarkdown'
import { MarkdownStreamingContext } from './markdownStreaming'

// 回归测试：流式生成中的 HTML 代码块不能挂 iframe —— srcDoc 每个 delta 换一次就是整篇重载，
// 页面闪 + 高度重测把聊天列表拽回底部。内容静默后才挂，且不再退回源码。
const block = (body: string) => '```html\n<html><body>' + body + '\n'

const streaming = (content: string) => (
  <MarkdownStreamingContext.Provider value={true}>
    <ChatMarkdown content={content} />
  </MarkdownStreamingContext.Provider>
)

describe('HTML 预览挂载时机', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('生成中内容还在长时只渲染源码，静默后才挂 iframe', () => {
    const { container, rerender } = render(streaming(block('<p>a</p>')))
    expect(container.querySelector('iframe')).toBeNull()

    rerender(streaming(block('<p>ab</p>')))
    act(() => void vi.advanceTimersByTime(400))
    rerender(streaming(block('<p>abc</p>')))
    act(() => void vi.advanceTimersByTime(400))
    expect(container.querySelector('iframe')).toBeNull() // 还没静默

    act(() => void vi.advanceTimersByTime(800))
    expect(container.querySelector('iframe')?.getAttribute('srcdoc')).toContain('<p>abc</p>')

    // 定稿后又来新 delta（生成中途停顿）：不能退回源码来回跳。
    rerender(streaming(block('<p>abcd</p>')))
    expect(container.querySelector('iframe')).not.toBeNull()
  })

  it('历史消息（非流式）首帧直接挂 iframe', () => {
    const { container } = render(<ChatMarkdown content={block('<p>done</p>')} />)
    expect(container.querySelector('iframe')?.getAttribute('srcdoc')).toContain('<p>done</p>')
  })
})
