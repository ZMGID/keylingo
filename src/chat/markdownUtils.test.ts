import { describe, expect, it } from 'vitest'
import { normalizeMarkdownForRender } from './markdownUtils'

describe('normalizeMarkdownForRender', () => {
  it('inserts row breaks between inline GFM table rows', () => {
    const input = '| a | b | | c | d |'
    expect(normalizeMarkdownForRender(input)).toBe('| a | b |\n| c | d |')
  })

  it('leaves already multiline tables unchanged', () => {
    const input = '| a | b |\n| c | d |'
    expect(normalizeMarkdownForRender(input)).toBe(input)
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
})
