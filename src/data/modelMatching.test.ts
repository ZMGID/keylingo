import { describe, expect, it } from 'vitest'
import { matchModel, matchModelExact, resolveModelInfo } from './modelMatching'

describe('matchModel', () => {
  it('returns null for blank model names', () => {
    expect(matchModel('')).toBeNull()
    expect(matchModel('   ')).toBeNull()
  })

  it('matches known models by exact id', () => {
    const info = matchModel('gpt-4o')
    expect(info).not.toBeNull()
    expect(info?.displayName).toBeTruthy()
    expect(info?.contextWindow).toBeGreaterThan(0)
  })

  it('strips OpenRouter-style provider prefix before matching', () => {
    const direct = matchModel('gpt-4o')
    const prefixed = matchModel('openai/gpt-4o')
    expect(prefixed).toEqual(direct)
  })

  it('returns null for unknown models', () => {
    expect(matchModel('totally-unknown-model-xyz-9999')).toBeNull()
  })

  it('matches dash-versioned ids against dot-keyed db entries', () => {
    // Provider ids use dashes (claude-sonnet-4-6); db keys use dots (claude-sonnet-4.6).
    // Without separator normalization these fall back to the older major-version entry.
    expect(matchModel('claude-sonnet-4-6')?.displayName).toBe('Claude Sonnet 4.6')
    expect(matchModel('claude-opus-4-8')?.displayName).toBe('Claude Opus 4.8')
    expect(matchModel('claude-opus-4-7')?.displayName).toBe('Claude Opus 4.7')
    expect(matchModel('claude-haiku-4-5')?.displayName).toBe('Claude Haiku 4.5')
  })

  it('still resolves the bare major-version model to its own entry', () => {
    expect(matchModel('claude-sonnet-4')?.displayName).toBe('Claude Sonnet 4')
    expect(matchModel('claude-opus-4')?.displayName).toBe('Claude Opus 4')
  })

  it('matches dated dash-versioned ids by longest normalized prefix', () => {
    expect(matchModel('claude-opus-4-8-20260101')?.displayName).toBe('Claude Opus 4.8')
  })

  it('does not collapse an unknown minor version onto its base entry', () => {
    // Unknown 5.7 variants must not fall back to the base "gpt-5" entry.
    expect(matchModel('gpt-5.7-nebula')).toBeNull()
    // Known 5.6 variants resolve to their exact entries rather than the base family.
    expect(matchModel('gpt-5.6-luna')?.displayName).toBe('GPT-5.6 Luna')
    expect(matchModel('gpt-5.6-sol')?.displayName).toBe('GPT-5.6 Sol')
    expect(matchModel('gpt-5.6-terra')?.displayName).toBe('GPT-5.6 Terra')
    expect(matchModel('gpt-5.5')?.displayName).toBe('GPT-5.5')
    expect(matchModel('gpt-5')?.displayName).toBe('GPT-5')
  })

  it('recognizes image generation model naming patterns', () => {
    const info = matchModel('dall-e-3')
    expect(info?.capabilities?.imageGeneration).toBe(true)
  })

  it('matches the latest official Kimi model ids', () => {
    const k3 = matchModel('kimi-k3')
    expect(k3?.displayName).toBe('Kimi K3')
    expect(k3?.contextWindow).toBe(1_048_576)
    expect(k3?.maxOutput).toBe(1_048_576)
    expect(k3?.temperature).toBeUndefined()

    expect(matchModel('kimi-k2.7-code')?.displayName).toBe('Kimi K2.7 Code')
    expect(matchModel('kimi-k2.7-code-highspeed')?.displayName)
      .toBe('Kimi K2.7 Code HighSpeed')
    expect(matchModel('kimi-k2.7-code-highspeed')?.pricing?.output).toBe(8)
  })

  it('matches Claude Mythos 5 official metadata', () => {
    const info = matchModel('claude-mythos-5')
    expect(info?.displayName).toBe('Claude Mythos 5')
    expect(info?.contextWindow).toBe(1_000_000)
    expect(info?.maxOutput).toBe(128_000)
    expect(info?.pricing?.input).toBe(10)
  })

  it('matches Claude Opus 5 without collapsing onto Opus 4.x', () => {
    const info = matchModel('claude-opus-5')
    expect(info?.displayName).toBe('Claude Opus 5')
    expect(info?.contextWindow).toBe(1_000_000)
    expect(info?.maxOutput).toBe(128_000)
    expect(info?.pricing?.input).toBe(5)
    expect(info?.pricing?.output).toBe(25)
    expect(info?.reasoningEfforts).toEqual(['low', 'medium', 'high', 'xhigh', 'max'])
    // OpenRouter-style provider prefix
    expect(matchModel('anthropic/claude-opus-5')?.displayName).toBe('Claude Opus 5')
    // Must not degrade to Opus 4
    expect(matchModel('claude-opus-5')?.displayName).not.toBe('Claude Opus 4')
  })

  it('matches latest Gemini Flash family ids', () => {
    expect(matchModel('gemini-3.6-flash')?.displayName).toBe('Gemini 3.6 Flash')
    expect(matchModel('gemini-3.6-flash')?.pricing?.output).toBe(7.5)
    expect(matchModel('gemini-3.5-flash-lite')?.displayName).toBe('Gemini 3.5 Flash-Lite')
    expect(matchModel('gemini-3.5-flash-lite')?.pricing?.input).toBe(0.3)
    expect(matchModel('gemini-3.1-flash-lite')?.displayName).toBe('Gemini 3.1 Flash-Lite')
    expect(matchModel('gemini-3-flash-preview')?.displayName).toBe('Gemini 3 Flash Preview')
    // Longer lite id must not collapse onto gemini-3.5-flash
    expect(matchModel('gemini-3.5-flash-lite')?.displayName).not.toBe('Gemini 3.5 Flash')
  })

  it('matches Grok 4.6 official metadata without collapsing onto 4.5', () => {
    const info = matchModel('grok-4.6')
    expect(info?.displayName).toBe('Grok 4.6')
    expect(info?.contextWindow).toBe(500_000)
    expect(info?.maxOutput).toBe(128_000)
    expect(info?.capabilities?.vision).toBe(true)
    expect(info?.capabilities?.functionCalling).toBe(true)
    expect(info?.capabilities?.reasoning).toBe(true)
    expect(info?.capabilities?.webSearch).toBe(true)
    expect(info?.reasoningEfforts).toEqual(['low', 'medium', 'high', 'xhigh'])
    expect(info?.pricing?.input).toBe(2)
    expect(info?.pricing?.output).toBe(6)
    expect(info?.pricing?.cachedInput).toBe(0.5)
    expect(matchModel('x-ai/grok-4.6')?.displayName).toBe('Grok 4.6')
    expect(matchModel('grok-4-6')?.displayName).toBe('Grok 4.6')
    expect(matchModel('grok-4.6')?.displayName).not.toBe('Grok 4.5')
    expect(matchModel('grok-4.5')?.displayName).toBe('Grok 4.5')
  })

  it('matches Cursor Composer model ids without collapsing versions', () => {
    // Cursor docs pricing catalog: composer-2.5 base + fast sub-row; supportsImage=true.
    const c25 = matchModel('composer-2.5')
    expect(c25?.displayName).toBe('Composer 2.5')
    expect(c25?.contextWindow).toBe(200_000)
    expect(c25?.maxOutput).toBeUndefined()
    expect(c25?.capabilities?.vision).toBe(true)
    expect(c25?.capabilities?.functionCalling).toBe(true)
    expect(c25?.capabilities?.reasoning).toBe(true)
    expect(c25?.capabilities?.streaming).toBe(true)
    expect(c25?.capabilities?.webSearch).toBe(false)
    expect(c25?.capabilities?.imageGeneration).toBe(false)
    expect(c25?.pricing?.input).toBe(0.5)
    expect(c25?.pricing?.output).toBe(2.5)
    expect(c25?.pricing?.cachedInput).toBe(0.2)
    expect(c25?.reasoningEfforts).toEqual([])
    // Explicit fast sub-row id from the same catalog.
    expect(matchModel('composer-2.5-fast')?.pricing?.input).toBe(3)
    expect(matchModel('composer-2.5-fast')?.pricing?.output).toBe(15)
    expect(matchModel('composer-2.5-fast')?.capabilities?.vision).toBe(true)
    // Param-style ACP ids still resolve to the base entry (not the -fast sub-row).
    expect(matchModel('composer-2.5[fast=true]')?.displayName).toBe('Composer 2.5')
    // Version continuation: composer-2 must not steal composer-2.5.
    expect(matchModel('composer-2')?.displayName).toBe('Composer 2')
    expect(matchModel('composer-1.5')?.displayName).toBe('Composer 1.5')
    expect(matchModel('composer-1')?.displayName).toBe('Composer 1')
    expect(matchModel('composer-1')?.pricing?.input).toBe(1.25)
    expect(matchModel('composer-1')?.capabilities?.vision).toBe(true)
  })
})



describe('matchModelExact', () => {
  it('matches exact and provider-prefixed model ids', () => {
    expect(matchModelExact('gpt-4o')).toEqual(matchModelExact('openai/gpt-4o'))
    expect(matchModelExact('claude-sonnet-4-6')?.displayName).toBe('Claude Sonnet 4.6')
  })

  it('does not infer catalog metadata for private aliases', () => {
    expect(matchModelExact('company-gpt-4o-special')).toBeNull()
  })
})

describe('resolveModelInfo', () => {
  it('merges database defaults with user overrides', () => {
    const resolved = resolveModelInfo('gpt-4o', {
      'gpt-4o': {
        displayName: 'Custom GPT-4o',
      },
    })
    expect(resolved.displayName).toBe('Custom GPT-4o')
    expect(resolved.contextWindow).toBeGreaterThan(0)
  })

  it('returns override-only info when database has no match', () => {
    const resolved = resolveModelInfo('custom-local-model', {
      'custom-local-model': {
        displayName: 'Local',
        contextWindow: 8192,
      },
    })
    expect(resolved.displayName).toBe('Local')
    expect(resolved.contextWindow).toBe(8192)
  })

  it('leaves temperature absent when neither the database nor overrides define it', () => {
    expect(resolveModelInfo('gpt-4o').temperature).toBeUndefined()
  })

  it('uses a numeric temperature override', () => {
    const resolved = resolveModelInfo('gpt-4o', {
      'gpt-4o': { temperature: 0.4 },
    })
    expect(resolved.temperature).toBe(0.4)
    expect(resolved.omitTemperature).toBeUndefined()
  })

  it('uses omitTemperature as an explicit blank tombstone', () => {
    const resolved = resolveModelInfo('gpt-4o', {
      'gpt-4o': { temperature: 0.4, omitTemperature: true },
    })
    expect(resolved.temperature).toBeUndefined()
    expect(resolved.omitTemperature).toBe(true)
  })
})

describe('embedding models', () => {
  it('resolves BAAI/bge-m3 (provider-prefixed) with embedding info', () => {
    const info = matchModel('BAAI/bge-m3')
    expect(info?.capabilities?.embedding).toBe(true)
    expect(info?.dimensions).toBe(1024)
    expect(info?.multilingual).toBe(true)
    expect(info?.contextWindow).toBe(8192)
  })

  it('knows OpenAI embedding dimensions', () => {
    expect(matchModel('text-embedding-3-small')?.dimensions).toBe(1536)
    expect(matchModel('text-embedding-3-large')?.dimensions).toBe(3072)
  })

  it('matches models/-prefixed Gemini embedding id', () => {
    const info = matchModel('models/gemini-embedding-001')
    expect(info?.capabilities?.embedding).toBe(true)
    expect(info?.dimensions).toBe(3072)
  })

  it('carries embedding fields through resolveModelInfo', () => {
    const info = resolveModelInfo('jina-embeddings-v3')
    expect(info.capabilities?.embedding).toBe(true)
    expect(info.dimensions).toBe(1024)
    expect(info.multilingual).toBe(true)
  })
})
