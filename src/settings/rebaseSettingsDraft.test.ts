import { describe, expect, it } from 'vitest'
import type { ChatMcpServer, Settings } from '../api/tauri'
import { rebaseDraftAgainstCache, rebaseSettingsDraft } from './rebaseSettingsDraft'
import { stableStringify } from './utils'

function settings(partial: Record<string, unknown>): Settings {
  return {
    theme: 'light',
    settingsLanguage: 'zh',
    favoriteModels: [],
    chatProviderId: 'p1',
    chatModel: 'm1',
    defaultModels: {
      chat: { providerId: 'p1', model: 'm1' },
      vision: { providerId: '', model: '' },
      titleSummary: { providerId: '', model: '' },
      compression: { providerId: '', model: '' },
      imageGeneration: { providerId: '', model: '' },
      promptOptimize: { providerId: '', model: '' },
      advisor: { providerId: '', model: '' },
    },
    chatTools: {
      enabled: false,
      servers: [],
      skillScanPaths: [],
      disabledSkillIds: [],
      maxToolRounds: null,
      toolTimeoutMs: 60_000,
      approvalPolicy: 'auto',
      nativeTools: { skillRuntime: false, runCommand: false, webSearch: false },
    },
    lens: { enabled: true, hotkey: '', webSearch: { enabled: false, provider: 'tavily' } },
    screenshotTranslation: { cardWidth: 480 },
    providers: [],
    ...partial,
  } as unknown as Settings
}

function server(partial: Partial<ChatMcpServer> & Pick<ChatMcpServer, 'id'>): ChatMcpServer {
  return {
    name: partial.id,
    enabled: true,
    transport: 'stdio',
    url: '',
    command: 'bin',
    args: [],
    env: {},
    headers: {},
    enabledTools: [],
    ...partial,
  }
}

describe('rebaseSettingsDraft', () => {
  it('returns fresh when the settings draft was not edited', () => {
    const snapshot = settings({})
    const fresh = settings({ theme: 'dark' })
    expect(rebaseSettingsDraft(snapshot, snapshot, fresh)).toBe(fresh)
  })

  it('keeps a theme edit and still takes a plugin MCP disable from fresh', () => {
    const pluginOn = server({
      id: 'plugin-cua-driver',
      connectorId: 'plugin:cua-driver',
      enabled: true,
    })
    const snapshot = settings({ chatTools: { ...settings({}).chatTools, servers: [pluginOn] } })
    const draft = settings({
      theme: 'dark',
      chatTools: { ...snapshot.chatTools, servers: [pluginOn] },
    })
    const fresh = settings({
      chatTools: {
        ...snapshot.chatTools,
        servers: [{ ...pluginOn, enabled: false }],
      },
    })
    const next = rebaseSettingsDraft(snapshot, draft, fresh)
    expect(next.theme).toBe('dark')
    expect(next.chatTools.servers[0]?.enabled).toBe(false)
  })

  it('keeps a user MCP edit in the draft and still adopts plugin enabled from fresh', () => {
    const plugin = server({ id: 'plugin-cua-driver', connectorId: 'plugin:cua-driver', enabled: true })
    const user = server({ id: 'mine', enabled: true, command: 'npx' })
    const snapshot = settings({ chatTools: { ...settings({}).chatTools, servers: [plugin, user] } })
    const draftUser = { ...user, enabled: false }
    const draft = settings({
      chatTools: { ...snapshot.chatTools, servers: [plugin, draftUser] },
    })
    const fresh = settings({
      chatTools: {
        ...snapshot.chatTools,
        servers: [{ ...plugin, enabled: false }, user],
      },
    })
    const next = rebaseSettingsDraft(snapshot, draft, fresh)
    expect(next.chatTools.servers.find((row) => row.id === 'mine')?.enabled).toBe(false)
    expect(next.chatTools.servers.find((row) => row.id === 'plugin-cua-driver')?.enabled).toBe(false)
  })

  it('takes favorites, language, chat default, card width, and plugin native flags from fresh when those slices were not edited', () => {
    const snapshot = settings({})
    const draft = settings({ theme: 'dark' })
    const fresh = settings({
      theme: 'light',
      favoriteModels: ['p:m'],
      settingsLanguage: 'en',
      chatProviderId: 'p2',
      chatModel: 'm2',
      defaultModels: {
        ...snapshot.defaultModels,
        chat: { providerId: 'p2', model: 'm2' },
      },
      screenshotTranslation: { cardWidth: 520 },
      chatTools: {
        ...snapshot.chatTools,
        enabled: true,
        nativeTools: { skillRuntime: true, runCommand: true, webSearch: false },
      },
    })
    const next = rebaseSettingsDraft(snapshot, draft, fresh)
    expect(next.theme).toBe('dark')
    expect(next.favoriteModels).toEqual(['p:m'])
    expect(next.settingsLanguage).toBe('en')
    expect(next.defaultModels.chat).toEqual({ providerId: 'p2', model: 'm2' })
    expect(next.screenshotTranslation?.cardWidth).toBe(520)
    expect(next.chatTools.enabled).toBe(true)
    expect(next.chatTools.nativeTools.skillRuntime).toBe(true)
    expect(next.chatTools.nativeTools.runCommand).toBe(true)
  })

  it('keeps in-progress provider placeholder rows instead of taking sanitized fresh providers', () => {
    const snapshot = settings({ providers: [{ id: 'p1', apiKeys: ['sk'] }] })
    const draft = settings({
      providers: [
        { id: 'p1', apiKeys: ['sk'] },
        { id: 'p2', apiKeys: [''] },
      ],
    })
    const fresh = settings({ providers: [{ id: 'p1', apiKeys: ['sk'] }] })
    const next = rebaseSettingsDraft(snapshot, draft, fresh)
    expect(next.providers).toHaveLength(2)
  })
})

describe('rebaseDraftAgainstCache', () => {
  it('parses the snapshot string used by SettingsShell', () => {
    const snapshot = settings({})
    const draft = settings({ theme: 'dark' })
    const fresh = settings({ favoriteModels: ['a:b'] })
    const next = rebaseDraftAgainstCache(stableStringify(snapshot), draft, fresh)
    expect(next.theme).toBe('dark')
    expect(next.favoriteModels).toEqual(['a:b'])
  })
})
