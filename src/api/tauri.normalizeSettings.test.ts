import { describe, expect, it } from 'vitest'
import { normalizeSettings, type Settings } from './tauri'
import { defaultChatTools } from '../settings/chatToolsShared'

/**
 * 最小 Settings：只保证 normalizeSettings 能跑通。
 * 用 Partial 断言避开 60+ 字段；缺字段被误读会立刻暴露。
 */
function baseSettings(overrides: Partial<Settings> = {}): Settings {
  return {
    hotkey: 'CommandOrControl+Alt+T',
    chatHotkey: 'CommandOrControl+Shift+K',
    closeChatHotkey: 'CommandOrControl+Shift+W',
    theme: 'system',
    themeColor: 'blue',
    translucentSidebar: true,
    targetLang: 'auto',
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
    chat: {
      streamEnabled: true,
      thinkingEnabled: true,
      maxOutputTokens: 8192,
      defaultLanguage: '',
      systemPrompt: '',
      userDisplayName: '',
      userAvatar: '',
    },
    providers: [],
    chatTools: defaultChatTools(),
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
    },
    ...overrides,
  } as Settings
}

describe('normalizeSettings', () => {
  it('旧设置缺少 translucentSidebar 时默认关闭', () => {
    const input = baseSettings()
    delete (input as Partial<Settings>).translucentSidebar

    expect(normalizeSettings(input).translucentSidebar).toBe(false)
  })

  it('旧设置缺少 launchMinimizedToTray 时默认关闭', () => {
    const input = baseSettings()
    delete (input as Partial<Settings>).launchMinimizedToTray

    expect(normalizeSettings(input).launchMinimizedToTray).toBe(false)
  })

  it('保留 chat.externalCliAgents（回归：重建 chat 时丢掉 → 供应商列表变空）', () => {
    const providers = [
      {
        id: 'p-1',
        name: 'Relay',
        remark: '',
        env: [{ key: 'ANTHROPIC_BASE_URL', value: 'https://x/anthropic' }],
        configToml: '',
        authJson: '',
      },
    ]
    const input = baseSettings({
      chat: {
        streamEnabled: false,
        thinkingEnabled: false,
        maxOutputTokens: 1024,
        defaultLanguage: 'zh',
        systemPrompt: 'hi',
        userDisplayName: 'u',
        userAvatar: '',
        defaultAgentRuntime: {
          kind: 'external',
          externalAgentId: 'claude',
          externalModel: null,
          externalReasoning: null,
        },
        externalCliAgents: {
          claude: {
            disabled: false,
            path: '/bin/claude',
            providers,
            currentProvider: 'p-1',
          },
          codex: {
            providers: [
              {
                id: 'c-1',
                name: 'Codex Relay',
                configToml: 'model = "gpt-5.5"',
                authJson: '{"OPENAI_API_KEY":"sk"}',
              },
            ],
            currentProvider: 'c-1',
          },
        },
      },
    })

    const out = normalizeSettings(input)

    expect(out.chat?.externalCliAgents?.claude?.providers).toEqual(providers)
    expect(out.chat?.externalCliAgents?.claude?.currentProvider).toBe('p-1')
    expect(out.chat?.externalCliAgents?.claude?.path).toBe('/bin/claude')
    expect(out.chat?.externalCliAgents?.codex?.providers?.[0]?.id).toBe('c-1')
    expect(out.chat?.defaultAgentRuntime?.kind).toBe('external')
    expect(out.chat?.defaultAgentRuntime?.externalAgentId).toBe('claude')
    // 其它 chat 字段仍归一
    expect(out.chat?.streamEnabled).toBe(false)
    expect(out.chat?.systemPrompt).toBe('hi')
  })
})
