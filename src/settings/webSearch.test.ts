import { describe, expect, it } from 'vitest'
import type { WebSearchConfig } from '../api/tauri'
import { isWebSearchConfigured, webSearchKeyField } from './webSearch'

function config(overrides: Partial<WebSearchConfig> = {}): WebSearchConfig {
  return {
    enabled: true,
    provider: 'tavily',
    tavilyApiKey: '',
    exaApiKey: '',
    maxResults: 5,
    searchDepth: 'basic',
    ...overrides,
  }
}

describe('isWebSearchConfigured', () => {
  it('requires the matching credential for each REST provider', () => {
    expect(isWebSearchConfigured(config({ provider: 'tavily', tavilyApiKey: 'tvly' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'brave', braveApiKey: 'bsa' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'serper', serperApiKey: 's' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'bocha', bochaApiKey: 'b' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'zhipu', zhipuApiKey: 'z' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'tinyfish', tinyfishApiKey: 'tf' }))).toBe(true)
    expect(isWebSearchConfigured(config({
      provider: 'tinyfish_mcp',
      tinyfishMcpUrl: 'https://agent.tinyfish.ai/mcp',
      tinyfishMcpAuth: { kind: 'oauth', accessToken: 'tok' },
    }))).toBe(true)
    expect(isWebSearchConfigured(config({
      provider: 'tinyfish_mcp',
      tinyfishMcpUrl: 'https://agent.tinyfish.ai/mcp',
    }))).toBe(false)
    expect(isWebSearchConfigured(config({ provider: 'brave' }))).toBe(false)
  })

  it('treats SearXNG as configured when the instance URL is set', () => {
    expect(isWebSearchConfigured(config({
      provider: 'searxng',
      searxngBaseUrl: 'https://searx.example',
    }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'searxng' }))).toBe(false)
  })
})

describe('webSearchKeyField', () => {
  it('returns null for keyless providers', () => {
    expect(webSearchKeyField('exa_mcp')).toBeNull()
    expect(webSearchKeyField('tinyfish_mcp')).toBeNull()
    expect(webSearchKeyField('searxng')).toBeNull()
    expect(webSearchKeyField('bocha')).toBe('bochaApiKey')
    expect(webSearchKeyField('tinyfish')).toBe('tinyfishApiKey')
  })
})
