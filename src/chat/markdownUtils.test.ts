import { describe, expect, it } from 'vitest'
import { normalizeMarkdownForRender } from './markdownUtils'

describe('normalizeMarkdownForRender', () => {
  it('splits an inline GFM table into rows and supplies the missing delimiter', () => {
    const input = '| a | b | | c | d |'
    expect(normalizeMarkdownForRender(input)).toBe('| a | b |\n| --- | --- |\n| c | d |')
  })

  it('splits every row of a squashed table, not just the last one', () => {
    // 旧实现的贪婪正则只切出一刀，4 行表格变 2 行 → 仍然不是表格 → 糊成一段话。
    const input = '| 操作 | 作用 | | 拖拽 | 旋转 | | 滚轮 | 缩放 | | 点击 | 进入 |'
    expect(normalizeMarkdownForRender(input)).toBe(
      ['| 操作 | 作用 |', '| --- | --- |', '| 拖拽 | 旋转 |', '| 滚轮 | 缩放 |', '| 点击 | 进入 |'].join('\n'),
    )
  })

  it('keeps a squashed table that already carries its delimiter row', () => {
    const input = '| a | b | | --- | --- | | 1 | 2 |'
    expect(normalizeMarkdownForRender(input)).toBe('| a | b |\n| --- | --- |\n| 1 | 2 |')
  })

  it('leaves already multiline tables unchanged', () => {
    // 关键回归：断行若用 \s 会跨换行匹配，给正常表格插入空行把它拆成两块。
    const input = '| a | b |\n| --- | --- |\n| 1 | 2 |'
    expect(normalizeMarkdownForRender(input)).toBe(input)
  })

  it('stays stable while a squashed table streams in', () => {
    // 末行还没写完时不能让整张表判定失败（否则表格会闪一下消失）。
    expect(normalizeMarkdownForRender('| a | b | | c')).toBe('| a | b |\n| --- | --- |\n| c')
  })

  it('does not touch prose that merely contains pipes', () => {
    for (const input of ['用 A|B 或 C | D 都行', '这是 || 逻辑或', 'a | b', '| 只有一行 |']) {
      expect(normalizeMarkdownForRender(input)).toBe(input)
    }
  })

  it('converts \\[...\\] block math to $$...$$', () => {
    expect(normalizeMarkdownForRender('\\[E = mc^2\\]')).toBe('\n$$\nE = mc^2\n$$\n')
  })

  it('converts \\(...\\) inline math to $...$ and trims the body', () => {
    expect(normalizeMarkdownForRender('a \\( x^2 \\) b')).toBe('a $x^2$ b')
  })

  it('leaves an unclosed \\[ untouched (streaming)', () => {
    const input = 'start \\[ E = mc^2'
    expect(normalizeMarkdownForRender(input)).toBe(input)
  })

  it('does not touch delimiters inside code', () => {
    const input = '`\\[x\\]` and\n```\n\\(y\\)\n```'
    expect(normalizeMarkdownForRender(input)).toBe(input)
  })

  it('does not reach inside an unterminated fence while it streams', () => {
    // 流式中代码块还没收尾：里面的表格/公式写法必须原样留着。
    const input = 'see:\n```md\n| a | b | | c | d |\n\\[x\\]'
    expect(normalizeMarkdownForRender(input)).toBe(input)
  })
})
