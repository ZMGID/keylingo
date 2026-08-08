import { describe, expect, it } from 'vitest'
import {
  buildKimiConfigToml,
  isValidKimiProviderId,
  kimiModelAlias,
  parseKimiConfigToml,
  setKimiStructuredFields,
  validateKimiConfigToml,
} from './cliKimiPresets'

describe('cliKimiPresets', () => {
  it('builds providers + models + default_model in the documented shape', () => {
    const toml = buildKimiConfigToml({
      providerId: 'relay',
      type: 'openai_responses',
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-test',
      models: [
        { model: 'gpt-5', displayName: 'GPT 5', contextWindow: '200000' },
        { model: 'gpt-mini', displayName: '', contextWindow: '' },
      ],
      defaultModel: 'gpt-5',
    })
    expect(toml).toContain('default_model = "relay/gpt-5"')
    expect(toml).toContain('[providers.relay]')
    expect(toml).toContain('type = "openai_responses"')
    expect(toml).toContain('base_url = "https://relay.example/v1"')
    expect(toml).toContain('api_key = "sk-test"')
    expect(toml).toContain('[models."relay/gpt-5"]')
    expect(toml).toContain('provider = "relay"')
    expect(toml).toContain('model = "gpt-5"')
    expect(toml).toContain('max_context_size = 200000')
    expect(toml).toContain('display_name = "GPT 5"')
    expect(toml).toContain('[models."relay/gpt-mini"]')
    expect(toml).toContain('max_context_size = 128000')
  })

  it('round-trips parse ↔ build', () => {
    const source = `default_model = "xb/gpt-5.6"

[providers.xb]
type = "openai"
base_url = "https://xb1520.com/v1"
api_key = "sk-756a"

[models."xb/gpt-5.6"]
provider = "xb"
model = "gpt-5.6"
max_context_size = 272_000
display_name = "GPT 5.6"
`
    const fields = parseKimiConfigToml(source)
    expect(fields).toMatchObject({
      providerId: 'xb',
      type: 'openai',
      baseUrl: 'https://xb1520.com/v1',
      apiKey: 'sk-756a',
      defaultModel: 'gpt-5.6',
    })
    expect(fields.models).toEqual([
      { model: 'gpt-5.6', displayName: 'GPT 5.6', contextWindow: '272000' },
    ])
    const rebuilt = buildKimiConfigToml(fields)
    const again = parseKimiConfigToml(rebuilt)
    expect(again).toMatchObject({
      providerId: 'xb',
      type: 'openai',
      baseUrl: 'https://xb1520.com/v1',
      apiKey: 'sk-756a',
      defaultModel: 'gpt-5.6',
    })
    expect(again.models[0]?.model).toBe('gpt-5.6')
  })

  it('setKimiStructuredFields rebuilds a clean fragment', () => {
    const source = buildKimiConfigToml({
      providerId: 'old',
      type: 'openai',
      baseUrl: 'https://old.example/v1',
      apiKey: 'old-key',
      models: [{ model: 'm1', displayName: '', contextWindow: '128000' }],
      defaultModel: 'm1',
    })
    const next = setKimiStructuredFields(source, {
      providerId: 'new',
      baseUrl: 'https://new.example/v1',
      apiKey: 'new-key',
      type: 'anthropic',
      models: [{ model: 'claude', displayName: 'Claude', contextWindow: '200000' }],
      defaultModel: 'claude',
    })
    expect(next).toContain('[providers.new]')
    expect(next).toContain('type = "anthropic"')
    expect(next).toContain('https://new.example/v1')
    expect(next).not.toContain('old-key')
    expect(next).toContain(kimiModelAlias('new', 'claude'))
  })

  it('validates required fields', () => {
    expect(validateKimiConfigToml('')).toBe('empty')
    expect(validateKimiConfigToml('[providers.x]\ntype = "openai"\n')).toBe('missing-base-url')
    expect(
      validateKimiConfigToml(buildKimiConfigToml({
        providerId: 'relay',
        type: 'openai',
        baseUrl: 'https://x.com/v1',
        apiKey: '',
        models: [{ model: 'm', displayName: '', contextWindow: '128000' }],
        defaultModel: 'm',
      })),
    ).toBe('missing-api-key')
    expect(
      validateKimiConfigToml(buildKimiConfigToml({
        providerId: 'relay',
        type: 'openai',
        baseUrl: 'https://x.com/v1',
        apiKey: 'k',
        models: [{ model: 'm', displayName: '', contextWindow: '128000' }],
        defaultModel: 'm',
      })),
    ).toBeNull()
    expect(
      validateKimiConfigToml(buildKimiConfigToml({
        providerId: 'relay',
        type: 'anthropic',
        baseUrl: '',
        apiKey: 'k',
        models: [{ model: 'claude', displayName: '', contextWindow: '200000' }],
        defaultModel: 'claude',
      })),
    ).toBeNull()
  })

  it('accepts official-looking provider ids', () => {
    expect(isValidKimiProviderId('relay')).toBe(true)
    expect(isValidKimiProviderId('my-relay')).toBe(true)
    expect(isValidKimiProviderId('xb 1520')).toBe(true)
    expect(isValidKimiProviderId('')).toBe(false)
    expect(isValidKimiProviderId('-bad')).toBe(false)
  })
})
