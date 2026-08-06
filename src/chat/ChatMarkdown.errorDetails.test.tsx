import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { ChatMarkdown } from './ChatMarkdown'

describe('ChatMarkdown 错误详情', () => {
  it('把受控错误代码块渲染成折叠详情', () => {
    const content = 'Pi 的模型流式响应中途断开。\n\n```kivio-error-details\n原始错误：stream_read_error\n退出码：1\n```'
    const { container } = render(<ChatMarkdown content={content} />)

    const details = container.querySelector('details')
    expect(details).not.toBeNull()
    expect(details).not.toHaveAttribute('open')
    expect(details).toHaveTextContent('原始错误：stream_read_error')
    expect(details).toHaveTextContent('退出码：1')

    fireEvent.click(screen.getByText('错误详情'))
    expect(details).toHaveAttribute('open')
  })

  it('兼容已经保存的旧 details HTML，且不显示原始标签', () => {
    const legacy = 'Pi 通信出错。\n\n<details>\n<summary>错误详情</summary>\n\n```\n原始错误：stream_read_error\n```\n\n</details>'
    const { container } = render(<ChatMarkdown content={legacy} />)

    expect(container.querySelector('details')).not.toBeNull()
    expect(container.textContent).not.toContain('<details>')
    expect(container.textContent).not.toContain('</details>')
  })
})
