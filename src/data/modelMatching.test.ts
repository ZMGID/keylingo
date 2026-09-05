import { describe, expect, it } from 'vitest'
import { matchModel, matchModelExact, resolveModelInfo } from './modelMatching'

describe('matchModel', () => {
  it('resolves all eight OpenCode free variants without inheriting paid limits or prices', () => {
    for (const id of ['big-pickle', 'deepseek-v4-flash-free', 'ling-3.0-flash-fin-free', 'mimo-v2.5-free', 'muse-spark-1.2-contributor-free', 'muse-spark-1.3-contributor-free', 'nemotron-3-ultra-free', 'nemotron-3.5-lightning-free']) {
      const info = matchModelExact(id)
      expect(info, id).not.toBeNull()
      expect(info?.pricing?.input).toBe(0)
      expect(info?.pricing?.output).toBe(0)
      expect(matchModel(`opencode/${id}`)).toEqual(info)
    }
    expect(matchModelExact('muse-spark-1.3-contributor-free')?.maxOutput).toBe(131072)
    expect(matchModelExact('muse-spark-1.3-contributor')?.pricing?.input).toBeGreaterThan(0)
    expect(matchModelExact('deepseek-v4-flash-free')?.contextWindow).toBe(200000)
    expect(matchModelExact('nemotron-3.5-lightning-free')?.reasoningEfforts).toEqual([])
  })

  it('resolves Kimi Code short IDs only with provider context and preserves overrides', () => {
    const provider = { baseUrl: 'https://api.kimi.com/coding/v1' }
    expect(resolveModelInfo('k3', undefined, provider).displayName).toBe('Kimi K3')
    expect(resolveModelInfo('k3-256k', undefined, provider).contextWindow).toBe(262144)
    expect(resolveModelInfo('k3', undefined, provider).reasoningEfforts).toEqual(['low', 'high', 'max'])
    expect(resolveModelInfo('k3', undefined, provider).pricing?.input).toBeUndefined()
    expect(resolveModelInfo('k3', { k3: { contextWindow: 1048576 } }, provider).contextWindow).toBe(1048576)
    expect(matchModel('k3')).toBeNull()
    expect(resolveModelInfo('k3', undefined, { baseUrl: 'https://other.example/v1' })).toEqual({})
  })

  it('resolves September models through provider and effort aliases without losing variants', () => {
    expect(matchModel('gemini-3.8-flash-high')).toEqual(matchModelExact('gemini-3.8-flash'))
    expect(matchModel('openai/gpt-6-astra')?.contextWindow).toBe(1050000)
    expect(matchModel('anthropic/claude-fable-5-1')?.displayName).toBe('Claude Fable 5.1')
    expect(matchModel('claude-mythos-5-1')?.displayName).toBe('Claude Mythos 5.1')
    expect(matchModel('meta/muse-spark-1.3-contributor')?.pricing?.input).toBe(0.1)
    expect(matchModel('meta/muse-spark-1.3')?.pricing?.input).toBe(1.25)
    expect(matchModel('gemini-3.8-flash-high')?.reasoningEfforts).toEqual(['low', 'medium', 'high'])
    expect(matchModelExact('gpt-6')).toBeNull()
  })

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
    expect(matchModel('claude-opus-4-5')?.displayName).toBe('Claude Opus 4.5')
    expect(matchModel('claude-opus-4-5')?.reasoningEfforts).toEqual(['low', 'medium', 'high'])
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

  it('matches current Grok Imagine image ids without collapsing variants', () => {
    expect(matchModel('grok-imagine-image')?.displayName).toBe('Grok Imagine Image')
    expect(matchModel('grok-imagine-image')?.capabilities?.imageGeneration).toBe(true)
    expect(matchModel('grok-imagine-image-2.0')?.displayName).toBe('Grok Imagine Image 2.0')
    expect(matchModel('grok-imagine-image-2.0')?.displayName).not.toBe('Grok Imagine Image')
    expect(matchModel('x-ai/grok-imagine-image-2.0')?.displayName).toBe('Grok Imagine Image 2.0')
    expect(matchModel('grok-imagine-image-quality')?.displayName).toBe('Grok Imagine Image Quality')
    expect(matchModel('grok-imagine-image-quality')?.displayName).not.toBe('Grok Imagine Image')
    expect(matchModel('x-ai/grok-imagine-image-quality')?.displayName).toBe('Grok Imagine Image Quality')
  })

  it('matches current third-party image models used by Mixer', () => {
    expect(matchModel('gemini-3.1-flash-lite-image')?.capabilities?.imageGeneration).toBe(true)
    expect(matchModel('google/gemini-3.1-flash-lite-image')?.displayName).toBe(
      'Gemini 3.1 Flash Lite Image',
    )
    expect(matchModel('nano-banana-2')?.displayName).toBe('Nano Banana 2')
    expect(matchModel('nano-banana-2')?.displayName).not.toBe('Nano Banana')
    expect(matchModel('qwen/qwen-image-3')?.displayName).toBe('Qwen Image 3')
    expect(matchModel('qwen-image-3-pro')?.displayName).toBe('Qwen Image 3 Pro')
    expect(matchModel('qwen-image-3')?.displayName).not.toBe('Qwen Image')
    expect(matchModel('krea-2-large')?.capabilities?.imageGeneration).toBe(true)
    expect(matchModel('meta/muse-image')?.displayName).toBe('Muse Image')
    expect(matchModel('mai-image-2.5-pro')?.displayName).toBe('MAI Image 2.5 Pro')
    expect(matchModel('riverflow-v2.5-pro')?.displayName).toBe('Riverflow V2.5 Pro')
    expect(matchModel('black-forest-labs/flux.2-klein-4b')?.displayName).toBe('FLUX.2 Klein 4B')
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
    expect(matchModel('gemini-3.7-flash')?.displayName).toBe('Gemini 3.7 Flash')
    expect(matchModel('gemini-3.7-flash')?.contextWindow).toBe(1_048_576)
    expect(matchModel('gemini-3.7-flash')?.maxOutput).toBe(65_536)
    expect(matchModel('gemini-3.7-flash')?.pricing?.input).toBe(0.75)
    expect(matchModel('gemini-3.7-flash')?.pricing?.output).toBe(3.75)
    expect(matchModel('gemini-3.6-flash')?.displayName).toBe('Gemini 3.6 Flash')
    expect(matchModel('gemini-3.6-flash')?.pricing?.output).toBe(7.5)
    expect(matchModel('gemini-3.5-flash-lite')?.displayName).toBe('Gemini 3.5 Flash-Lite')
    expect(matchModel('gemini-3.5-flash-lite')?.pricing?.input).toBe(0.3)
    expect(matchModel('gemini-3.1-flash-lite')?.displayName).toBe('Gemini 3.1 Flash-Lite')
    expect(matchModel('gemini-3-flash-preview')?.displayName).toBe('Gemini 3 Flash Preview')
    // Longer lite id must not collapse onto gemini-3.5-flash
    expect(matchModel('gemini-3.5-flash-lite')?.displayName).not.toBe('Gemini 3.5 Flash')
    expect(matchModel('gemini-3.7-flash')?.displayName).not.toBe('Gemini 3.6 Flash')
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

  it('matches Tencent Hy family ids without collapsing preview onto GA', () => {
    const hy4 = matchModel('hy4-preview')
    expect(hy4?.displayName).toBe('Hy4 Preview')
    expect(hy4?.contextWindow).toBe(1_048_576)
    expect(hy4?.maxOutput).toBe(65_536)
    expect(hy4?.temperature).toBe(0.9)
    expect(hy4?.capabilities?.vision).toBe(false)
    expect(hy4?.capabilities?.functionCalling).toBe(true)
    expect(hy4?.capabilities?.reasoning).toBe(true)
    expect(hy4?.reasoningEfforts).toEqual(['high'])
    expect(hy4?.pricing?.input).toBe(0.834)
    expect(hy4?.pricing?.output).toBe(2.501)
    expect(hy4?.pricing?.cachedInput).toBe(0.042)
    expect(matchModel('hy4')?.displayName).toBe('Hy4 Preview')
    expect(matchModel('tencent/hy4-preview')).toEqual(hy4)

    const hy3 = matchModel('hy3')
    expect(hy3?.displayName).toBe('Hy3')
    expect(hy3?.contextWindow).toBe(262_144)
    expect(hy3?.maxOutput).toBe(131_072)
    expect(hy3?.reasoningEfforts).toEqual(['low', 'high'])
    expect(hy3?.pricing?.input).toBe(0.132)
    expect(matchModel('tencent/hy3')?.displayName).toBe('Hy3')

    expect(matchModel('hy3-preview')?.displayName).toBe('Hy3 Preview')
    expect(matchModel('hy3-preview')?.displayName).not.toBe('Hy3')
    expect(matchModel('hy-mt2-pro')?.displayName).toBe('Hy-MT2 Pro')
    expect(matchModel('hy-mt2-1.8b')?.displayName).toBe('Hy-MT2 1.8B')
    expect(matchModel('hy-vision-2.0-instruct')?.capabilities?.vision).toBe(true)
    expect(matchModel('hunyuan-t1-vision-20250916')?.displayName).toBe('HY-Vision 1.5 Thinking')
    expect(matchModel('hy-image-v3')?.capabilities?.imageGeneration).toBe(true)
  })

  it('matches Doubao Seed family ids without collapsing 2.1 onto 2.0', () => {
    const pro21 = matchModel('doubao-seed-2.1-pro')
    expect(pro21?.displayName).toBe('Doubao Seed 2.1 Pro')
    expect(pro21?.contextWindow).toBe(262_144)
    expect(pro21?.maxOutput).toBe(262_144)
    expect(pro21?.capabilities?.vision).toBe(true)
    expect(pro21?.capabilities?.functionCalling).toBe(true)
    expect(pro21?.capabilities?.reasoning).toBe(true)
    expect(pro21?.reasoningEfforts).toEqual(['low', 'medium', 'high'])
    expect(pro21?.pricing?.input).toBe(0.9)
    expect(pro21?.pricing?.output).toBe(4.5)
    expect(matchModel('doubao-seed-2-1-pro-260628')?.displayName).toBe('Doubao Seed 2.1 Pro')
    expect(matchModel('seed-2.1-pro')?.displayName).toBe('Doubao Seed 2.1 Pro')

    expect(matchModel('doubao-seed-2.1-turbo')?.displayName).toBe('Doubao Seed 2.1 Turbo')
    expect(matchModel('bytedance-seed/seed-2.1-turbo')?.displayName).toBe('Doubao Seed 2.1 Turbo')
    expect(matchModel('doubao-seed-evolving')?.displayName).toBe('Doubao Seed Evolving')

    const pro20 = matchModel('doubao-seed-2.0-pro')
    expect(pro20?.displayName).toBe('Doubao Seed 2.0 Pro')
    expect(pro20?.capabilities?.vision).toBe(true)
    expect(pro20?.pricing?.input).toBe(0.5)
    expect(pro20?.displayName).not.toBe('Doubao Seed 2.1 Pro')
    expect(matchModel('doubao-seed-2.1-pro')?.displayName).not.toBe('Doubao Seed 2.0 Pro')

    expect(matchModel('doubao-seed-2.0-code')?.displayName).toBe('Doubao Seed 2.0 Code')
    expect(matchModel('bytedance-seed/seed-2.0-code')?.displayName).toBe('Doubao Seed 2.0 Code')
    expect(matchModel('doubao-seed-2.0-lite')?.pricing?.input).toBe(0.25)
    expect(matchModel('doubao-seed-2.0-mini')?.pricing?.output).toBe(0.4)
    expect(matchModel('seed-1.6-flash')?.displayName).toBe('Doubao Seed 1.6 Flash')
    expect(matchModel('doubao-seed-1.6')?.displayName).toBe('Doubao Seed 1.6')
    expect(matchModel('seed-1.6-flash')?.displayName).not.toBe('Doubao Seed 1.6')
    expect(matchModel('seedream-5.0-pro')?.capabilities?.imageGeneration).toBe(true)
  })

  it('matches Ox Alpha as GLM multimodal with a 1M window', () => {
    const info = matchModel('ox-alpha')
    expect(info?.displayName).toBe('Ox Alpha')
    expect(info?.contextWindow).toBe(1_048_576)
    expect(info?.maxOutput).toBe(16_000)
    expect(info?.capabilities?.vision).toBe(true)
    expect(info?.capabilities?.functionCalling).toBe(true)
    expect(info?.capabilities?.reasoning).toBe(true)
    expect(info?.capabilities?.streaming).toBe(true)
    expect(info?.reasoningEfforts).toEqual(['high', 'max'])
    expect(info?.pricing?.input).toBe(0)
    expect(info?.pricing?.output).toBe(0)
    expect(matchModel('stealth/ox-alpha')).toEqual(info)
  })

  it('matches GLM-5.3 family without collapsing onto 5.2 or 5', () => {
    const glm53 = matchModel('glm-5.3')
    expect(glm53?.displayName).toBe('GLM-5.3')
    expect(glm53?.contextWindow).toBe(1_000_000)
    expect(glm53?.maxOutput).toBe(131_072)
    expect(glm53?.capabilities?.vision).toBe(false)
    expect(glm53?.reasoningEfforts).toEqual(['low', 'high', 'max'])
    expect(glm53?.pricing?.input).toBe(1.4)
    expect(glm53?.pricing?.output).toBe(4.4)
    expect(matchModel('z-ai/glm-5.3')?.displayName).toBe('GLM-5.3')
    expect(matchModel('glm-5-3')?.displayName).toBe('GLM-5.3')

    const flash = matchModel('glm-5.3-flash')
    expect(flash?.displayName).toBe('GLM-5.3 Flash')
    expect(flash?.capabilities?.vision).toBe(true)
    expect(flash?.maxOutput).toBe(131_072)
    expect(flash?.pricing?.input).toBe(0.075)
    expect(flash?.pricing?.output).toBe(0.25)
    expect(matchModel('glm-5.3-flash')?.displayName).not.toBe('GLM-5.3')

    const glm47 = matchModel('glm-4.7')
    expect(glm47?.displayName).toBe('GLM-4.7')
    expect(glm47?.contextWindow).toBe(200_000)
    expect(glm47?.maxOutput).toBe(131_072)
    expect(glm47?.capabilities?.vision).toBe(false)
    expect(glm47?.reasoningEfforts).toEqual([])
    expect(glm47?.pricing?.input).toBe(0.6)
    expect(matchModel('glm-4.7-flash')?.displayName).toBe('GLM-4.7 Flash')
    expect(matchModel('glm-4.7-flashx')?.displayName).toBe('GLM-4.7 FlashX')
    expect(matchModel('glm-4.7-flash')?.displayName).not.toBe('GLM-4.7')
    expect(matchModel('glm-4.6v')?.capabilities?.vision).toBe(true)
    expect(matchModel('glm-4.6')?.maxOutput).toBe(131_072)
    expect(matchModel('glm-4.5-air')?.displayName).toBe('GLM-4.5 Air')
    expect(matchModel('glm-4.5-airx')?.displayName).toBe('GLM-4.5 AirX')
    expect(matchModel('glm-4.5-airx')?.displayName).not.toBe('GLM-4.5 Air')
    expect(matchModel('glm-4.5-x')?.displayName).toBe('GLM-4.5 X')
    expect(matchModel('glm-4.5-x')?.pricing?.input).toBe(2.2)
    expect(matchModel('glm-4.5-flash')?.displayName).toBe('GLM-4.5 Flash')
    expect(matchModel('glm-4.5-flash')?.displayName).not.toBe('GLM-4.5')
    expect(matchModel('glm-image')?.capabilities?.imageGeneration).toBe(true)

    expect(matchModel('glm-5.2')?.maxOutput).toBe(131_072)
    expect(matchModel('glm-5.2')?.capabilities?.vision).toBe(false)
    expect(matchModel('glm-5')?.displayName).toBe('GLM-5')
    expect(matchModel('glm-5')?.contextWindow).toBe(200_000)
    expect(matchModel('glm-5-turbo')?.displayName).toBe('GLM-5 Turbo')
    expect(matchModel('glm-5v-turbo')?.displayName).toBe('GLM-5V Turbo')
    expect(matchModel('glm-5v-turbo')?.capabilities?.vision).toBe(true)
    expect(matchModel('glm-5.3')?.displayName).not.toBe('GLM-5')
    expect(matchModel('glm-5.3')?.displayName).not.toBe('GLM-5.2')
  })

  it('matches Qwen 3.8 family without collapsing onto 3.7', () => {
    const max = matchModel('qwen3.8-max')
    expect(max?.displayName).toBe('Qwen3.8 Max')
    expect(max?.contextWindow).toBe(1_000_000)
    expect(max?.maxOutput).toBe(131_072)
    expect(max?.capabilities?.vision).toBe(true)
    expect(max?.pricing?.input).toBe(2)
    expect(max?.pricing?.output).toBe(6)
    expect(matchModel('qwen/qwen3.8-max')?.displayName).toBe('Qwen3.8 Max')

    expect(matchModel('qwen3.8-flash')?.displayName).toBe('Qwen3.8 Flash')
    expect(matchModel('qwen3.8-flash')?.maxOutput).toBe(131_072)
    expect(matchModel('qwen3.8-27b')?.displayName).toBe('Qwen3.8 27B')
    expect(matchModel('qwen3.7-flash')?.displayName).toBe('Qwen3.7 Flash')
    expect(matchModel('qwen3.8-max')?.displayName).not.toBe('Qwen3.7 Max')
    expect(matchModel('qwen3.8-flash')?.displayName).not.toBe('Qwen3.7 Flash')
    expect(matchModel('qwen3.5-plus')?.displayName).toBe('Qwen3.5 Plus')
    expect(matchModel('qwen3.5-flash')?.displayName).toBe('Qwen3.5 Flash')
    expect(matchModel('qwen3.5-flash')?.displayName).not.toBe('Qwen3.5 Plus')
  })

  it('matches DeepSeek V4 official windows and the vision-exp sibling', () => {
    const flash = matchModel('deepseek-v4-flash')
    expect(flash?.displayName).toBe('DeepSeek V4 Flash')
    expect(flash?.contextWindow).toBe(1_048_576)
    expect(flash?.maxOutput).toBe(384_000)
    expect(flash?.capabilities?.vision).toBe(false)
    expect(flash?.capabilities?.reasoning).toBe(true)
    expect(flash?.pricing?.input).toBe(0.44)
    expect(matchModel('deepseek-v4-pro')?.maxOutput).toBe(384_000)
    expect(matchModel('deepseek-v4-flash-vision-exp')?.capabilities?.vision).toBe(true)
    expect(matchModel('deepseek-v4-flash-vision-exp')?.displayName).not.toBe('DeepSeek V4 Flash')
  })

  it('matches MiniMax M2.7 as a thinking model without collapsing onto highspeed', () => {
    const m27 = matchModel('minimax-m2.7')
    expect(m27?.displayName).toBe('MiniMax M2.7')
    expect(m27?.capabilities?.reasoning).toBe(true)
    expect(m27?.pricing?.input).toBe(0.3)
    expect(matchModel('minimax-m2.7-highspeed')?.displayName).toBe('MiniMax M2.7 Highspeed')
    expect(matchModel('MiniMax-M2.7-highspeed')?.pricing?.output).toBe(2.4)
    expect(matchModel('minimax-m2.7-highspeed')?.displayName).not.toBe('MiniMax M2.7')
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
