import { describe, expect, it } from 'vitest'
import {
  _matchGlyphForTest as matchGlyph,
  _matchProviderGlyphForTest as matchProviderGlyph,
  _providerIconMapKeysForTest as providerIconMapKeys,
  PROVIDER_BRANDS,
} from './ModelIcon'

describe('ModelIcon model→brand mapping', () => {
  it('matches common model families', () => {
    const cases = [
      'gpt-4o', 'o3-mini', 'claude-3-5-sonnet', 'gemini-2.0-flash', 'gemma-2',
      'deepseek-chat', 'qwen-max', 'grok-3', 'kimi-k2', 'moonshot-v1-8k',
      'glm-4', 'mistral-large', 'llama-3.1-70b', 'yi-large', 'doubao-pro',
      'ernie-4.0', 'minimax-abab6', 'command-r', 'phi-3-medium', 'step-1v',
    ]
    for (const id of cases) {
      expect(matchGlyph(id), `${id} should resolve a brand`).not.toBeNull()
    }
  })

  it('is case-insensitive', () => {
    expect(matchGlyph('GPT-4O')).toBe(matchGlyph('gpt-4o'))
  })

  it('does not misfire on substrings (gemma ≠ gemini, yi only as token)', () => {
    expect(matchGlyph('mayis-model')).toBeNull() // "yi" inside a word must not match
  })

  it('returns null for unknown models', () => {
    expect(matchGlyph('totally-made-up-model')).toBeNull()
    expect(matchGlyph('')).toBeNull()
  })
})

describe('ProviderIcon provider→brand mapping', () => {
  it('resolves by base URL alone, so a renamed provider still gets its brand', () => {
    // 名字一律换成认不出来的，确保命中的只可能是 URL —— 否则这条测不出「改名后还认得」。
    const cases: Array<[string, string]> = [
      ['https://api.siliconflow.cn/v1', 'SiliconCloud'],
      ['https://openrouter.ai/api/v1', 'OpenRouter'],
      ['https://integrate.api.nvidia.com/v1', 'Nvidia'],
      ['https://open.bigmodel.cn/api/paas/v4', 'ChatGLM'],
      ['https://ollama.com/v1', 'Ollama'],
      ['https://generativelanguage.googleapis.com/v1beta', 'Google'],
    ]
    for (const [url, brand] of cases) {
      expect(matchProviderGlyph(`${url} 小白`), url).toBe(PROVIDER_BRANDS[brand])
    }
  })

  it('resolves by name when the base URL says nothing', () => {
    expect(matchProviderGlyph('https://llm.internal.corp/v1 英伟达')).toBe(PROVIDER_BRANDS.Nvidia)
  })

  it('falls back to null for a provider nobody knows', () => {
    expect(matchProviderGlyph('https://llm.internal.corp/v1 星辰')).toBeNull()
  })

  it('every auto-match key exists in the picker registry (both share one table)', () => {
    const keys = providerIconMapKeys()
    // 空表会让循环体一次都不跑，那这条就成了恒真断言。
    expect(keys.length).toBeGreaterThan(20)
    for (const key of keys) {
      expect(PROVIDER_BRANDS[key], `PROVIDER_ICON_MAP 里的 ${key} 在 PROVIDER_BRANDS 里不存在`)
        .toBeTruthy()
    }
  })
})
