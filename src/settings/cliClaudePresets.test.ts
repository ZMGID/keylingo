import { describe, expect, it } from 'vitest'
import {
  applyClaudePreset,
  CLAUDE_TOKEN,
  CLAUDE_TOKEN_ALT,
  readClaudeApiKey,
  writeClaudeApiKey,
  type EnvPair,
} from './cliClaudePresets'

describe('cliClaudePresets key compat', () => {
  it('readClaudeApiKey prefers AUTH_TOKEN over API_KEY', () => {
    const env: EnvPair[] = [
      { key: CLAUDE_TOKEN_ALT, value: 'sk-alt' },
      { key: CLAUDE_TOKEN, value: 'sk-auth' },
    ]
    expect(readClaudeApiKey(env)).toBe('sk-auth')
  })

  it('readClaudeApiKey falls back to API_KEY', () => {
    const env: EnvPair[] = [{ key: CLAUDE_TOKEN_ALT, value: 'sk-only-alt' }]
    expect(readClaudeApiKey(env)).toBe('sk-only-alt')
  })

  it('writeClaudeApiKey normalizes to AUTH_TOKEN and drops API_KEY', () => {
    const env: EnvPair[] = [
      { key: CLAUDE_TOKEN_ALT, value: 'old' },
      { key: 'ANTHROPIC_BASE_URL', value: 'https://x' },
    ]
    const next = writeClaudeApiKey(env, 'sk-new')
    expect(readClaudeApiKey(next)).toBe('sk-new')
    expect(next.some((p) => p.key === CLAUDE_TOKEN)).toBe(true)
    expect(next.some((p) => p.key === CLAUDE_TOKEN_ALT)).toBe(false)
    expect(next.find((p) => p.key === 'ANTHROPIC_BASE_URL')?.value).toBe('https://x')
  })

  it('applyClaudePreset preserves key from either token field', () => {
    const fromAlt: EnvPair[] = [
      { key: CLAUDE_TOKEN_ALT, value: 'sk-keep' },
      { key: 'ANTHROPIC_BASE_URL', value: 'https://old' },
    ]
    const next = applyClaudePreset('deepseek', fromAlt)
    expect(readClaudeApiKey(next)).toBe('sk-keep')
    expect(next.some((p) => p.key === CLAUDE_TOKEN_ALT)).toBe(false)
    expect(next.find((p) => p.key === CLAUDE_TOKEN)?.value).toBe('sk-keep')
  })
})
