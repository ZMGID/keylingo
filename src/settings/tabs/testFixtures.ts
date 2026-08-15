import type { Settings as SettingsData, ModelProvider } from '../../api/tauri'

/**
 * 设置页 tab 组件测试用的最小 Settings。
 *
 * Settings 有 60+ 字段，逐个填只会让测试脆弱（加一个字段就要改所有 fixture）。
 * 这里只给被测 tab 真正读到的字段，其余按 Partial 断言掉 —— 组件只读自己那几个键，
 * 缺失字段若被误读会立刻 undefined 报错，反而比填假值更能暴露问题。
 */
export function makeSettings(overrides: Partial<SettingsData> = {}): SettingsData {
  return {
    hotkey: 'CommandOrControl+Shift+K',
    chatHotkey: 'CommandOrControl+Shift+J',
    closeChatHotkey: 'CommandOrControl+Shift+W',
    theme: 'system',
    themeColor: 'default',
    translucentSidebar: true,
    targetLang: 'auto',
    autoPaste: false,
    launchAtStartup: false,
    launchMinimizedToTray: false,
    translatorProviderId: 'p1',
    translatorModel: 'gpt-4o',
    chatProviderId: 'p1',
    chatModel: 'gpt-4o',
    retryEnabled: true,
    retryAttempts: 3,
    providers: [],
    defaultModels: {
      chat: { providerId: '', model: '' },
      vision: { providerId: '', model: '' },
      titleSummary: { providerId: '', model: '' },
      compression: { providerId: '', model: '' },
      imageGeneration: { providerId: '', model: '' },
      advisor: { providerId: '', model: '' },
    },
    chatTools: { enabled: false, servers: [] },
    screenshotTranslation: {
      enabled: true,
      hotkey: 'CommandOrControl+Shift+A',
      textHotkey: 'CommandOrControl+Shift+T',
      replaceHotkey: 'CommandOrControl+Shift+R',
      providerId: 'p1',
      model: 'gpt-4o',
    },
    ...overrides,
  } as SettingsData
}

export function makeProvider(overrides: Partial<ModelProvider> = {}): ModelProvider {
  return {
    id: 'p1',
    name: 'OpenAI',
    baseUrl: 'https://api.openai.com/v1',
    apiKeys: ['sk-test'],
    availableModels: ['gpt-4o'],
    enabledModels: ['gpt-4o'],
    enabled: true,
    ...overrides,
  } as ModelProvider
}
