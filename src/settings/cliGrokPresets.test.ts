import { describe, expect, it } from 'vitest'
import {
  buildGrokConfigToml,
  parseGrokConfigToml,
  setGrokStructuredFields,
  validateGrokConfigToml,
} from './cliGrokPresets'

describe('cliGrokPresets', () => {
  it('builds a minimal config.toml with model routing', () => {
    const toml = buildGrokConfigToml({
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-test',
      model: 'grok-4.5',
      displayName: '小白',
      apiBackend: 'responses',
      contextWindow: '500000',
    })
    expect(toml).toContain('[models]')
    expect(toml).toContain('default = "grok-4.5"')
    expect(toml).toContain('[model."grok-4.5"]')
    expect(toml).toContain('base_url = "https://relay.example/v1"')
    expect(toml).toContain('api_key = "sk-test"')
    expect(toml).toContain('name = "小白"')
    expect(toml).toContain('context_window = 500000')
  })

  it('round-trips parse ↔ build for cc-switch style snippets', () => {
    const source = `[models]
default = "grok-4.5"

[model."grok-4.5"]
model = "grok-4.5"
base_url = "https://xb1520.com/v1"
name = "小白"
api_backend = "responses"
context_window = 500_000
api_key = "sk-756a"
`
    const fields = parseGrokConfigToml(source)
    expect(fields).toMatchObject({
      model: 'grok-4.5',
      baseUrl: 'https://xb1520.com/v1',
      displayName: '小白',
      apiBackend: 'responses',
      contextWindow: '500000',
      apiKey: 'sk-756a',
    })
    const rebuilt = buildGrokConfigToml(fields)
    const again = parseGrokConfigToml(rebuilt)
    expect(again.baseUrl).toBe(fields.baseUrl)
    expect(again.model).toBe(fields.model)
    expect(again.apiKey).toBe(fields.apiKey)
  })

  it('setGrokStructuredFields preserves non-model sections', () => {
    const source = `[models]
default = "old"

[marketplace]
official_marketplace_auto_installed = true

[model."old"]
model = "old"
base_url = "https://old.example/v1"
api_key = "old-key"
`
    const next = setGrokStructuredFields(source, {
      baseUrl: 'https://new.example/v1',
      apiKey: 'new-key',
      model: 'grok-4.5',
    })
    expect(next).toContain('[marketplace]')
    expect(next).toContain('official_marketplace_auto_installed = true')
    expect(next).toContain('default = "grok-4.5"')
    expect(next).toContain('https://new.example/v1')
    expect(next).not.toContain('old-key')
  })

  it('validateGrokConfigToml rejects empty / incomplete snippets', () => {
    expect(validateGrokConfigToml('')).toBe('empty')
    expect(validateGrokConfigToml('[models]\ndefault = "x"\n')).toBe('missing-base-url')
    expect(
      validateGrokConfigToml(buildGrokConfigToml({
        baseUrl: 'https://x.com/v1',
        apiKey: 'k',
        model: 'm',
        displayName: '',
        apiBackend: 'responses',
        contextWindow: '',
      })),
    ).toBeNull()
  })
})
