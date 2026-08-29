import { describe, expect, it } from 'vitest'
import type { WebSearchConfig } from '../api/tauri'
import {
  isProviderConfigured,
  isWebSearchConfigured,
  providerSupportsFetch,
  resolvedFetchProvider,
  webSearchKeyField,
} from './webSearch'

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
    expect(isWebSearchConfigured(config({ provider: 'grok', grokApiKey: 'xai' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'deepseek', deepseekApiKey: 'sk' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'brave', braveApiKey: 'bsa' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'serper', serperApiKey: 's' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'bocha', bochaApiKey: 'b' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'zhipu', zhipuApiKey: 'z' }))).toBe(true)
    expect(isWebSearchConfigured(config({ provider: 'kimi', kimiApiKey: 'sk' }))).toBe(true)
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
    expect(webSearchKeyField('deepseek')).toBe('deepseekApiKey')
    expect(webSearchKeyField('grok')).toBe('grokApiKey')
    expect(webSearchKeyField('kimi')).toBe('kimiApiKey')
  })
})

describe('resolvedFetchProvider', () => {
  it('follows the search provider when it has an extract API', () => {
    expect(resolvedFetchProvider(config({ provider: 'exa' }))).toBe('exa')
    expect(providerSupportsFetch('exa')).toBe(true)
  })

  it('returns null when search has no extract API and fetch is not overridden', () => {
    expect(resolvedFetchProvider(config({ provider: 'brave' }))).toBeNull()
    expect(providerSupportsFetch('brave')).toBe(false)
  })

  it('uses an explicit fetch provider independent of search', () => {
    expect(resolvedFetchProvider(config({
      provider: 'exa',
      fetchProvider: 'tavily',
    }))).toBe('tavily')
    expect(resolvedFetchProvider(config({
      provider: 'brave',
      fetchProvider: 'tavily',
    }))).toBe('tavily')
  })
})

describe('isProviderConfigured', () => {
  it('checks the given vendor even when it is not the search default', () => {
    expect(isProviderConfigured(
      config({ provider: 'exa', tavilyApiKey: 'tvly' }),
      'tavily',
    )).toBe(true)
    expect(isProviderConfigured(
      config({ provider: 'exa', tavilyApiKey: 'tvly' }),
      'exa',
    )).toBe(false)
  })
})
