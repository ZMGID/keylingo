import { describe, expect, it } from 'vitest'
import { render } from '@testing-library/react'
import { ChatMarkdown } from './ChatMarkdown'

describe('表格', () => {
  it('列对齐透传到单元格（remark-gfm 的 text-align 内联样式）', () => {
    const md = ['| 左 | 中 | 右 |', '| :--- | :---: | ---: |', '| a | b | c |'].join('\n')
    const { container } = render(<ChatMarkdown content={md} />)
    const ths = Array.from(container.querySelectorAll('th'))
    expect(ths.map((th) => th.style.textAlign)).toEqual(['left', 'center', 'right'])
    const tds = Array.from(container.querySelectorAll('td'))
    expect(tds.map((td) => td.style.textAlign)).toEqual(['left', 'center', 'right'])
  })

  it('每个单元格一个圆角块：无任何边框线，横竖都靠 border-spacing 空隙分隔', () => {
    const md = ['| a | b |', '| --- | --- |', '| 1 | 2 |'].join('\n')
    const { container } = render(<ChatMarkdown content={md} />)
    // 单元格不许有边框——分隔靠块间空隙，不是线。每格有底色 + 四角圆角。
    for (const cell of container.querySelectorAll('th, td')) {
      const classes = cell.className.split(/\s+/)
      expect(classes.some((c) => c.startsWith('border'))).toBe(false)
      expect(classes.some((c) => c.startsWith('bg-'))).toBe(true)
      expect(classes).toContain('rounded-md')
    }
    // 块之间的空隙来自 border-separate + 双向 border-spacing。
    const table = container.querySelector('table')!
    expect(table.className).toContain('border-separate')
    expect(table.className).toContain('[border-spacing:3px]')
    // 外层容器不许描边/圆角 —— 否则读成「表格里还有个表格」。
    const wrapper = table.parentElement!
    expect(wrapper.className).not.toContain('border')
    expect(wrapper.className).not.toContain('rounded')
  })
})
