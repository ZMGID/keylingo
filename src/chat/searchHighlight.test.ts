import { describe, expect, it } from 'vitest'
import { splitHighlightParts } from './searchHighlight'

describe('splitHighlightParts', () => {
  it('returns whole text when query empty', () => {
    expect(splitHighlightParts('hello', '')).toEqual([{ text: 'hello', match: false }])
  })

  it('highlights case-insensitive matches and keeps original casing', () => {
    expect(splitHighlightParts('Foo CDN77 bar cdn77', 'cdn77')).toEqual([
      { text: 'Foo ', match: false },
      { text: 'CDN77', match: true },
      { text: ' bar ', match: false },
      { text: 'cdn77', match: true },
    ])
  })

  it('handles no match', () => {
    expect(splitHighlightParts('hello', 'xyz')).toEqual([{ text: 'hello', match: false }])
  })

  it('highlights CJK needles without splitting surrounding characters', () => {
    expect(splitHighlightParts('请检查知识库配置', '知识库')).toEqual([
      { text: '请检查', match: false },
      { text: '知识库', match: true },
      { text: '配置', match: false },
    ])
  })

  it('trims the query before matching', () => {
    expect(splitHighlightParts('alpha beta', '  beta  ')).toEqual([
      { text: 'alpha ', match: false },
      { text: 'beta', match: true },
    ])
  })

})
