import { describe, expect, it } from 'vitest'
import {
  buildNativeCliProvider,
  emptyNativeModel,
  readNativeCliProvider,
  resolvePiModelMetadata,
} from './cliNativeProviderConfigs'

describe('cliNativeProviderConfigs', () => {
  it('builds the documented OpenCode provider and auth shapes', () => {
    const result = buildNativeCliProvider('opencode', 'Relay', {
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-test',
      api: 'openai-completions',
      models: [{
        id: 'gpt-test',
        name: 'GPT Test',
        reasoning: false,
        vision: false,
        contextWindow: '',
        maxTokens: '',
      }],
      defaultModel: 'gpt-test',
      defaultThinkingLevel: 'off',
    })
    expect(JSON.parse(result.configJson!)).toEqual({
      npm: '@ai-sdk/openai-compatible',
      name: 'Relay',
      options: { baseURL: 'https://relay.example/v1' },
      models: { 'gpt-test': { name: 'GPT Test' } },
    })
    expect(JSON.parse(result.authJson!)).toEqual({ type: 'api', key: 'sk-test' })
  })

  it('round-trips Pi provider fields and credential type', () => {
    const built = buildNativeCliProvider('pi', 'Relay', {
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-responses',
      models: [{
        id: 'gpt-test',
        name: '',
        reasoning: true,
        vision: false,
        contextWindow: '256000',
        maxTokens: '32768',
      }],
      defaultModel: 'gpt-test',
      defaultThinkingLevel: 'high',
    })
    expect(JSON.parse(built.authJson!)).toEqual({ type: 'api_key', key: 'sk-pi' })
    expect(JSON.parse(built.configJson!)).toMatchObject({
      models: [{
        id: 'gpt-test',
        reasoning: true,
        contextWindow: 256000,
        maxTokens: 32768,
      }],
    })
    expect(built.defaultReasoning).toBe('high')
    const read = readNativeCliProvider('pi', {
      id: 'p-1',
      name: 'Relay',
      ...built,
    })
    expect(read).toMatchObject({
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-responses',
      defaultModel: 'gpt-test',
      defaultThinkingLevel: 'high',
      models: [{
        id: 'gpt-test',
        name: '',
        reasoning: true,
        vision: false,
        contextWindow: '256000',
        maxTokens: '32768',
      }],
    })
  })

  it('fills known Pi model metadata from the local catalog using only the model id', () => {
    const model = emptyNativeModel('pi', 'grok-4.5')
    expect(resolvePiModelMetadata(model)).toMatchObject({
      matched: true,
      displayName: 'Grok 4.5',
      reasoning: true,
      vision: true,
      contextWindow: 500000,
      maxTokens: 128000,
    })

    const built = buildNativeCliProvider('pi', 'Grok Relay', {
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-responses',
      models: [model],
      defaultModel: 'grok-4.5',
      defaultThinkingLevel: 'high',
    })
    expect(JSON.parse(built.configJson!).models).toEqual([{
      id: 'grok-4.5',
      name: 'Grok 4.5',
      reasoning: true,
      input: ['text', 'image'],
      contextWindow: 500000,
      maxTokens: 128000,
    }])
  })

  it('emits Pi sparse thinking mappings including xhigh and max', () => {
    const model = emptyNativeModel('pi', 'deepseek-v4-flash')
    expect(resolvePiModelMetadata(model).thinkingLevels).toEqual([
      'off',
      'low',
      'high',
      'xhigh',
      'max',
    ])

    const built = buildNativeCliProvider('pi', 'DeepSeek Relay', {
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-responses',
      models: [model],
      defaultModel: model.id,
      defaultThinkingLevel: 'max',
    })
    expect(JSON.parse(built.configJson!).models[0].thinkingLevelMap).toEqual({
      minimal: null,
      low: 'low',
      medium: null,
      high: 'high',
      xhigh: 'xhigh',
      max: 'max',
    })
    expect(readNativeCliProvider('pi', {
      id: 'deepseek',
      name: 'DeepSeek Relay',
      ...built,
    }).defaultThinkingLevel).toBe('max')
  })

  it('uses Pi defaults for an unknown model and keeps manual overrides optional', () => {
    const automatic = resolvePiModelMetadata(emptyNativeModel('pi', 'private-model'))
    expect(automatic).toMatchObject({
      matched: false,
      reasoning: false,
      vision: false,
      contextWindow: 128000,
      maxTokens: 16384,
    })

    expect(resolvePiModelMetadata({
      ...emptyNativeModel('pi', 'private-model'),
      reasoning: true,
      contextWindow: '256000',
    })).toMatchObject({
      matched: false,
      reasoning: true,
      contextWindow: 256000,
      maxTokens: 16384,
    })
  })

  it('does not fuzzy-match a private Pi model alias', () => {
    expect(resolvePiModelMetadata(emptyNativeModel('pi', 'company-gpt-4o-special'))).toMatchObject({
      matched: false,
      reasoning: false,
      vision: false,
      contextWindow: 128000,
      maxTokens: 16384,
      thinkingLevels: ['off'],
    })
  })

  it('persists manual override intent separately from generated Pi metadata', () => {
    const built = buildNativeCliProvider('pi', 'Grok Relay', {
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-responses',
      models: [{
        ...emptyNativeModel('pi', 'grok-4.5'),
        contextWindow: '500000',
      }],
      defaultModel: 'grok-4.5',
      defaultThinkingLevel: 'high',
    })
    expect(JSON.parse(built.modelMetadataJson!)).toEqual({
      version: 1,
      models: { 'grok-4.5': { contextWindow: '500000' } },
    })
    expect(readNativeCliProvider('pi', {
      id: 'grok',
      name: 'Grok Relay',
      ...built,
    }).models[0].contextWindow).toBe('500000')
  })

  it('can migrate a previous env-only entry into the native form', () => {
    const read = readNativeCliProvider('opencode', {
      id: 'old',
      name: 'Old',
      env: [
        { key: 'OPENAI_BASE_URL', value: 'https://old.example/v1' },
        { key: 'OPENAI_API_KEY', value: 'sk-old' },
      ],
    })
    expect(read.baseUrl).toBe('https://old.example/v1')
    expect(read.apiKey).toBe('sk-old')
  })
})
