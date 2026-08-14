import { describe, expect, it } from 'vitest'
import {
  buildNativeCliProvider,
  dshApiKeyEnv,
  emptyNativeModel,
  nativeProviderIdFromName,
  readNativeCliProvider,
  resolveOpenCodeModelMetadata,
  resolvePiModelMetadata,
} from './cliNativeProviderConfigs'

describe('cliNativeProviderConfigs', () => {
  it('builds the documented OpenCode provider and auth shapes', () => {
    const result = buildNativeCliProvider('opencode', 'Relay', {
      nativeProviderId: 'relay',
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-test',
      api: '@ai-sdk/openai-compatible',
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
      sourceConfigJson: '',
    })
    expect(JSON.parse(result.configJson!)).toEqual({
      npm: '@ai-sdk/openai-compatible',
      name: 'Relay',
      options: { baseURL: 'https://relay.example/v1' },
      models: {
        'gpt-test': {
          name: 'GPT Test',
          reasoning: false,
          attachment: false,
          tool_call: true,
          limit: { context: 128000, output: 16384 },
          modalities: { input: ['text'], output: ['text'] },
        },
      },
    })
    expect(JSON.parse(result.authJson!)).toEqual({ type: 'api', key: 'sk-test' })
  })

  it('fills OpenCode model metadata from an exact catalog match', () => {
    const model = emptyNativeModel('opencode', 'grok-4.5')
    expect(resolveOpenCodeModelMetadata(model)).toMatchObject({
      matched: true,
      displayName: 'Grok 4.5',
      reasoning: true,
      vision: true,
      toolCall: true,
      contextWindow: 500000,
      maxTokens: 128000,
    })

    const built = buildNativeCliProvider('opencode', 'Grok Relay', {
      nativeProviderId: 'grok-relay',
      baseUrl: 'https://relay.example/v1',
      apiKey: '',
      api: '@ai-sdk/openai-compatible',
      models: [model],
      defaultModel: model.id,
      defaultThinkingLevel: 'off',
      sourceConfigJson: '',
    })
    expect(built.authJson).toBe('')
    expect(JSON.parse(built.configJson!).models['grok-4.5']).toMatchObject({
      name: 'Grok 4.5',
      reasoning: true,
      attachment: true,
      tool_call: true,
      limit: { context: 500000, output: 128000 },
      modalities: { input: ['text', 'image'], output: ['text'] },
    })
  })

  it('keeps unknown OpenCode provider and model fields while updating managed fields', () => {
    const built = buildNativeCliProvider('opencode', 'Relay', {
      nativeProviderId: 'relay',
      baseUrl: '',
      apiKey: '',
      api: '@ai-sdk/anthropic',
      models: [emptyNativeModel('opencode', 'private-model')],
      defaultModel: 'private-model',
      defaultThinkingLevel: 'off',
      sourceConfigJson: JSON.stringify({
        timeout: 30000,
        options: { customFlag: true, baseURL: 'https://old.example' },
        models: {
          'private-model': {
            variants: { high: { reasoningEffort: 'high' } },
            headers: { 'x-model': 'private' },
          },
        },
      }),
    })
    const config = JSON.parse(built.configJson!)
    expect(config.timeout).toBe(30000)
    expect(config.options).toEqual({ customFlag: true })
    expect(config.models['private-model']).toMatchObject({
      variants: { high: { reasoningEffort: 'high' } },
      headers: { 'x-model': 'private' },
      limit: { context: 128000, output: 16384 },
    })
  })

  it('round-trips Pi provider fields and credential type', () => {
    const built = buildNativeCliProvider('pi', 'Relay', {
      nativeProviderId: '',
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
      sourceConfigJson: '',
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

  it('builds and round-trips a dsh llm-pi-ai provider without storing the key in config JSON', () => {
    const built = buildNativeCliProvider('dsh', 'Relay', {
      nativeProviderId: 'relay-one',
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-dsh',
      api: 'openai-responses',
      models: [{
        id: 'gpt-test',
        name: 'GPT Test',
        reasoning: true,
        vision: false,
        contextWindow: '256000',
        maxTokens: '32768',
      }],
      defaultModel: 'gpt-test',
      defaultThinkingLevel: 'off',
      sourceConfigJson: '',
    })
    const config = JSON.parse(built.configJson!)
    expect(config).toMatchObject({
      displayName: 'Relay',
      apiKeyEnv: 'KIVIO_DSH_RELAY_ONE_API_KEY',
      api: 'openai-responses',
      baseURL: 'https://relay.example/v1',
      models: [{
        id: 'gpt-test',
        name: 'GPT Test',
        contextWindow: 256000,
        maxTokens: 32768,
        input: ['text'],
      }],
    })
    expect(config.models[0].reasoningEfforts).toMatchObject({ off: null, high: 'high' })
    expect(JSON.stringify(config)).not.toContain('sk-dsh')
    expect(built.authJson).toBe('')
    expect(dshApiKeyEnv('relay-one')).toBe('KIVIO_DSH_RELAY_ONE_API_KEY')

    const read = readNativeCliProvider('dsh', {
      id: 'p-dsh',
      name: 'Relay',
      nativeProviderId: 'relay-one',
      env: [{ key: 'KIVIO_DSH_RELAY_ONE_API_KEY', value: 'sk-dsh' }],
      ...built,
    })
    expect(read).toMatchObject({
      nativeProviderId: 'relay-one',
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-dsh',
      api: 'openai-responses',
      defaultModel: 'gpt-test',
      models: [{
        id: 'gpt-test',
        reasoning: true,
        vision: false,
        contextWindow: '256000',
        maxTokens: '32768',
      }],
    })
  })

  it('marks adaptive-generation Claude models with forceAdaptiveThinking on anthropic-messages', () => {
    const model = (id: string) => ({
      id,
      name: '',
      reasoning: true,
      vision: true,
      contextWindow: '200000',
      maxTokens: '32768',
    })
    const built = buildNativeCliProvider('pi', 'Claude Relay', {
      nativeProviderId: '',
      baseUrl: 'https://relay.example',
      apiKey: 'sk-pi',
      api: 'anthropic-messages',
      models: [model('claude-opus-4-8'), model('claude-sonnet-4-5'), model('claude-fable-5')],
      defaultModel: 'claude-opus-4-8',
      defaultThinkingLevel: 'high',
      sourceConfigJson: '',
    })
    const models = JSON.parse(built.configJson!).models as Array<Record<string, unknown>>
    // opus ≥4.6 / fable ≥5 是 adaptive 世代 → model 级 compat；sonnet-4-5 是旧世代 → 不打。
    expect(models[0]).toMatchObject({ id: 'claude-opus-4-8', compat: { forceAdaptiveThinking: true } })
    expect(models[1].compat).toBeUndefined()
    expect(models[2]).toMatchObject({ id: 'claude-fable-5', compat: { forceAdaptiveThinking: true } })
  })

  it('does not mark adaptive compat on non-anthropic wire formats', () => {
    const built = buildNativeCliProvider('pi', 'Relay', {
      nativeProviderId: '',
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-completions',
      models: [{
        id: 'claude-opus-4-8',
        name: '',
        reasoning: true,
        vision: true,
        contextWindow: '200000',
        maxTokens: '32768',
      }],
      defaultModel: 'claude-opus-4-8',
      defaultThinkingLevel: 'high',
      sourceConfigJson: '',
    })
    const models = JSON.parse(built.configJson!).models as Array<Record<string, unknown>>
    // compat.forceAdaptiveThinking 是 Anthropic Messages 专属；OpenAI 线不发 thinking 块。
    expect(models[0].compat).toBeUndefined()
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
      nativeProviderId: '',
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-responses',
      models: [model],
      defaultModel: 'grok-4.5',
      defaultThinkingLevel: 'high',
      sourceConfigJson: '',
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
      nativeProviderId: '',
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-responses',
      models: [model],
      defaultModel: model.id,
      defaultThinkingLevel: 'max',
      sourceConfigJson: '',
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
      nativeProviderId: '',
      baseUrl: 'https://relay.example/v1',
      apiKey: 'sk-pi',
      api: 'openai-responses',
      models: [{
        ...emptyNativeModel('pi', 'grok-4.5'),
        contextWindow: '500000',
      }],
      defaultModel: 'grok-4.5',
      defaultThinkingLevel: 'high',
      sourceConfigJson: '',
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

  it('slugs display names like the backend (keeps . and _)', () => {
    expect(nativeProviderIdFromName('My.Relay')).toBe('my.relay')
    expect(nativeProviderIdFromName('hello_world')).toBe('hello_world')
    expect(nativeProviderIdFromName('Relay One')).toBe('relay-one')
  })
})
