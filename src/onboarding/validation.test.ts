import { describe, expect, it } from 'vitest'
import type { Settings } from '../api/tauri'
import {
  canCompleteOnboarding,
  isProviderModelBindingUsable,
  providerHasUsableConfig,
  validateProviderStep,
  webSearchConfigured,
} from './validation'

function baseSettings(overrides: Partial<Settings> = {}): Settings {
  return {
    hotkey: 'CommandOrControl+Alt+T',
    theme: 'system',
    themeColor: 'neutral',
    targetLang: 'auto',
    source: 'openai',
    autoPaste: true,
    launchAtStartup: false,
    translatorProviderId: '',
    translatorModel: '',
    chatProviderId: '',
    chatModel: '',
    defaultModels: {
      chat: { providerId: '', model: '' },
      vision: { providerId: '', model: '' },
      titleSummary: { providerId: '', model: '' },
      compression: { providerId: '', model: '' },
      imageGeneration: { providerId: '', model: '' },
      advisor: { providerId: '', model: '' },
    },
    providers: [],
    chatTools: {
      servers: [],
      nativeTools: {},
      approvalPolicy: 'auto',
    },
    retryEnabled: true,
    retryAttempts: 3,
    screenshotTranslation: {
      enabled: true,
      hotkey: 'CommandOrControl+Shift+A',
      textHotkey: 'CommandOrControl+Shift+T',
      providerId: '',
      model: '',
    },
    lens: {
      enabled: true,
      hotkey: 'CommandOrControl+Shift+G',
      providerId: '',
      model: '',
    },
    onboardingStatus: 'pending',
    ...overrides,
  } as Settings
}

const testProvider = {
  id: 'p1',
  name: 'Test',
  apiKeys: ['sk-test'],
  baseUrl: 'https://api.example.com/v1',
  availableModels: ['gpt-4o'],
  enabledModels: ['gpt-4o'],
  enabled: true,
  apiFormat: 'openai_chat' as const,
}

const configuredBindings = {
  providers: [testProvider],
  screenshotTranslation: {
    enabled: true,
    hotkey: 'CommandOrControl+Shift+A',
    textHotkey: 'CommandOrControl+Shift+T',
    providerId: 'p1',
    model: 'gpt-4o',
  },
  lens: {
    enabled: true,
    hotkey: 'CommandOrControl+Shift+G',
    providerId: 'p1',
    model: 'gpt-4o',
  },
  defaultModels: {
    chat: { providerId: 'p1', model: 'gpt-4o' },
    vision: { providerId: '', model: '' },
    titleSummary: { providerId: '', model: '' },
    compression: { providerId: '', model: '' },
    imageGeneration: { providerId: '', model: '' },
    advisor: { providerId: '', model: '' },
  },
}

describe('onboarding validation', () => {
  it('detects usable provider config', () => {
    const settings = baseSettings({
      providers: [testProvider],
    })
    expect(providerHasUsableConfig(settings)).toBe(true)
    expect(validateProviderStep(settings).ok).toBe(false)
  })

  it('requires quick translate, lens, and chat model bindings', () => {
    const settings = baseSettings(configuredBindings)
    expect(canCompleteOnboarding(settings)).toBe(true)
  })

  it('rejects bindings that point to providers without keys', () => {
    const settings = baseSettings({
      ...configuredBindings,
      providers: [{
        ...testProvider,
        apiKeys: [],
      }],
    })
    expect(isProviderModelBindingUsable(settings, 'p1', 'gpt-4o')).toBe(false)
    expect(canCompleteOnboarding(settings)).toBe(false)
  })

  it('rejects bindings that point to providers without a base URL', () => {
    const settings = baseSettings({
      ...configuredBindings,
      providers: [{
        ...testProvider,
        baseUrl: '   ',
      }],
    })
    expect(isProviderModelBindingUsable(settings, 'p1', 'gpt-4o')).toBe(false)
    expect(canCompleteOnboarding(settings)).toBe(false)
  })

  it('detects configured web search keys', () => {
    const settings = baseSettings({
      lens: {
        enabled: true,
        hotkey: 'CommandOrControl+Shift+G',
        webSearch: {
          enabled: true,
          provider: 'tavily',
          tavilyApiKey: 'tvly-test',
          exaApiKey: '',
          maxResults: 5,
          searchDepth: 'basic',
        },
      },
    })
    expect(webSearchConfigured(settings)).toBe(true)
  })

  it('detects configured Brave and SearXNG search sources', () => {
    expect(webSearchConfigured(baseSettings({
      lens: {
        enabled: true,
        hotkey: 'CommandOrControl+Shift+G',
        webSearch: {
          enabled: true,
          provider: 'brave',
          tavilyApiKey: '',
          exaApiKey: '',
          braveApiKey: 'bsa-test',
          maxResults: 5,
          searchDepth: 'basic',
        },
      },
    }))).toBe(true)
    expect(webSearchConfigured(baseSettings({
      lens: {
        enabled: true,
        hotkey: 'CommandOrControl+Shift+G',
        webSearch: {
          enabled: true,
          provider: 'searxng',
          tavilyApiKey: '',
          exaApiKey: '',
          searxngBaseUrl: 'https://searx.example',
          maxResults: 5,
          searchDepth: 'basic',
        },
      },
    }))).toBe(true)
  })
})
