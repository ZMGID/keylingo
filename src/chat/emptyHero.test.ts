import { describe, expect, it } from 'vitest'
import { emptyHeroGreetings, emptyHeroJab, emptyHeroJabPool, emptyHeroLine, emptyHeroPinnedLine } from './emptyHero'

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
    expect(emptyHeroGreetings('en')).toContain(a)
  })

  it('问候有多条，钉住文案不走轮换池', () => {
    expect(emptyHeroGreetings('zh').length).toBeGreaterThan(1)
    expect(emptyHeroGreetings('en').length).toBe(emptyHeroGreetings('zh').length)
    expect(emptyHeroPinnedLine({ lang: 'zh' })).toBeNull()
    expect(emptyHeroPinnedLine({ lang: 'zh', assistantName: '翻译官' })).toBe('翻译官')
  })

  it('吐槽按连点档位走，避开刚说过的', () => {
    expect(emptyHeroJabPool('zh', 1)[0]).toBe('？')
    expect(emptyHeroJabPool('zh', 3)[0]).toBe('挺闲的')
    expect(emptyHeroJabPool('zh', 6)[0]).toBe('急了')
    expect(emptyHeroJabPool('zh', 9)[0]).toBe('绷不住了')
    expect(emptyHeroJabPool('en', 6).length).toBe(emptyHeroJabPool('zh', 6).length)
    expect(emptyHeroJab('zh', 1, null, () => 0)).toBe('？')
    expect(emptyHeroJab('zh', 1, '？', () => 0)).toBe('哦')
    expect(emptyHeroJab('en', 6, null, () => 0)).toBe('Mad?')
  })
})
