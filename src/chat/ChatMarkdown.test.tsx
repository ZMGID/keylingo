import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { ChatMarkdown } from './ChatMarkdown'
import { clearSettledMarkdownCache, settledMarkdownCacheSize } from './settledMarkdownCache'

describe('ChatMarkdown settled cache', () => {
  it('reuses a bounded settled normalization entry across remounts', () => {
    clearSettledMarkdownCache()
    const content = 'A settled answer with **markdown**.'
    const first = render(<ChatMarkdown content={content} />)
    first.unmount()
    render(<ChatMarkdown content={content} />)
    expect(settledMarkdownCacheSize()).toBe(1)
  })
})

// 回归测试：ChatMarkdown 因 props 变化（如 artifacts/citations 换引用）重渲时，公式节点
// 仍应保留在文档中，避免切换对话时出现「原始 LaTeX → 公式」的闪烁。
describe('ChatMarkdown 公式稳定性', () => {
  it('artifacts 换引用重渲时，公式节点不被 remount', () => {
    const { container, rerender } = render(
      <ChatMarkdown content={'目标函数 $Z_1$ 最小化'} artifacts={[]} />,
    )
    const before = container.querySelector('.katex')
    expect(before).not.toBeNull()

    // 模拟切模型/思考等级时上层重渲传入的新 artifacts 引用（内容不变）。
    rerender(<ChatMarkdown content={'目标函数 $Z_1$ 最小化'} artifacts={[]} />)
    const after = container.querySelector('.katex')

    expect(after).toBe(before) // 同一个 DOM 节点 = 未 remount
  })
})

describe('ChatMarkdown artifact 图片', () => {
  it('Streamdown 清洗前保留相对图片路径，并映射到 artifact data URL', () => {
    const { container } = render(
      <ChatMarkdown
        content={'![结果图](assets/chart.png)'}
        artifacts={[{
          name: 'assets/chart.png',
          mimeType: 'image/png',
          dataUrl: 'data:image/png;base64,AAAA',
        }]}
      />,
    )

    expect(container.querySelector('img')).toHaveAttribute('src', 'data:image/png;base64,AAAA')
  })

  it('renders consecutive markdown images as inline tiles', () => {
    const { container } = render(
      <ChatMarkdown
        content={'![a](one.png)\n\n![b](two.png)'}
        artifacts={[
          { name: 'one.png', mimeType: 'image/png', dataUrl: 'data:image/png;base64,AAAA' },
          { name: 'two.png', mimeType: 'image/png', dataUrl: 'data:image/png;base64,BBBB' },
        ]}
      />,
    )
    const images = container.querySelectorAll('[data-chat-inline-image]')
    expect(images).toHaveLength(2)
    expect(images[0]?.getAttribute('style') ?? '').toContain('128px')
    expect(container.querySelectorAll('[data-chat-md-image]')).toHaveLength(2)
  })
})
