import { describe, expect, it } from 'vitest'
import { render } from '@testing-library/react'
import { ChatMarkdown } from './ChatMarkdown'

/** CommonMark 原样的 flanking 规则会让这些中文写法把星号原样吐出来（remark-cjk-friendly 修的就是这个）。 */
describe('中文强调', () => {
  it.each([
    ['**一句话总结：**最近 AI 圈', '一句话总结：'],
    ['一份**“截至 2026 年”初步报告**，共 8 条', '“截至 2026 年”初步报告'],
    ['**结论（重要）**如下', '结论（重要）'],
  ])('%s', (md, bold) => {
    const { container } = render(<ChatMarkdown content={md} />)
    expect(container.querySelector('strong')?.textContent).toBe(bold)
    expect(container.textContent).not.toContain('**')
  })

  it('代码里的星号不动', () => {
    const { container } = render(<ChatMarkdown content={'`**代码：**里的`不该变'} />)
    expect(container.querySelector('code')?.textContent).toBe('**代码：**里的')
    expect(container.querySelector('strong')).toBeNull()
  })
})
