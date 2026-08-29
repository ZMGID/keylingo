import { describe, expect, it } from 'vitest'
import { emptyHeroLine } from './emptyHero'

describe('emptyHeroLine', () => {
  it('助手名优先', () => {
    expect(emptyHeroLine({
      lang: 'zh',
      assistantName: '翻译官',
      projectName: 'kivio',
      seed: 'c1',
    })).toBe('翻译官')
  })

  it('项目 / 集用短前缀', () => {
    expect(emptyHeroLine({ lang: 'zh', projectName: 'kivio' })).toBe('在「kivio」')
    expect(emptyHeroLine({ lang: 'en', projectName: 'kivio' })).toBe('In “kivio”')
    expect(emptyHeroLine({ lang: 'zh', setName: '写作' })).toBe('在「写作」')
  })

  it('同一会话种子文案稳定', () => {
    const a = emptyHeroLine({ lang: 'en', seed: 'conv-1' })
    const b = emptyHeroLine({ lang: 'en', seed: 'conv-1' })
    expect(a).toBe(b)
    expect(["What's next?", 'Where to?', 'Say it.', 'Go ahead.']).toContain(a)
  })
})
