import { describe, expect, it } from 'vitest'
import { isValidElement } from 'react'
import { highlightCode } from './ChatMarkdown'

// 护栏：scanTokens 改成「空白整段吞 + 无分类文本按区间 slice」之后，最容易出的错是
// 丢字符、重字符或错位。把高亮结果的文本拼回去必须逐字等于原文。
function flatten(nodes: ReturnType<typeof highlightCode>): string {
  return nodes
    .map((node) => {
      if (typeof node === 'string') return node
      if (isValidElement<{ children?: string }>(node)) return node.props.children ?? ''
      return ''
    })
    .join('')
}

const SAMPLES: Array<[string, string]> = [
  ['json', '{\n  "type": "bash",\n  "command": "git status",\n  "n": 12\n}\n'],
  ['bash', '#!/bin/sh\ngit status   # 注释\nfor f in *.ts; do echo "$f"; done\n'],
  ['ts', 'export function add(a: number, b = 1) {\n  // 加\n  return a + b\n}\n'],
  ['python', 'def f(x):\n    """doc"""\n    return x or None\n'],
  ['css', '.a { color: #fff; margin: 0 auto; }\n'],
  ['html', '<div class="x">文字</div>\n'],
  ['', '一段没有语言标注的\t纯文本   带空白\n\n和空行\n'],
  ['ts', '   \n\t\n   '], // 纯空白
  ['ts', ''],
]

describe('highlightCode', () => {
  it('拼回的文本逐字等于原文', () => {
    for (const [lang, code] of SAMPLES) {
      expect(flatten(highlightCode(code, lang)), `lang=${lang}`).toBe(code)
    }
  })

  it('关键字仍然被分类，不是整段纯文本', () => {
    const nodes = highlightCode('export function add() {}', 'ts')
    expect(nodes.some((n) => isValidElement(n))).toBe(true)
  })

  it('同样的输入返回同一个缓存实例', () => {
    const a = highlightCode('const x = 1', 'ts')
    const b = highlightCode('const x = 1', 'ts')
    expect(b).toBe(a)
  })
})
