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
})
