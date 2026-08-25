import { describe, expect, it } from 'vitest'
import type { ChatMcpServer, WebSearchConfig } from '../api/tauri'
import {
  buildInstalledMcpList,
  classifyInstalledServer,
  EXA_MCP_ID,
  filterInstalledMcpList,
  TINYFISH_MCP_ID,
  webSearchLoadedMcps,
} from './mcpInstalledList'

function server(partial: Partial<ChatMcpServer> & Pick<ChatMcpServer, 'id' | 'name'>): ChatMcpServer {
  return {
    enabled: true,
    transport: 'stdio',
    url: '',
    command: 'npx',
    args: [],
    env: {},
    headers: {},
    enabledTools: [],
    ...partial,
  }
}

function webSearch(partial: Partial<WebSearchConfig> = {}): WebSearchConfig {
  return {
    enabled: true,
    provider: 'tavily',
    tavilyApiKey: '',
    exaApiKey: '',
    maxResults: 5,
    searchDepth: 'basic',
    ...partial,
  }
}

describe('classifyInstalledServer', () => {
  it('keeps marketplace rows as user, connectors and plugins as their own kinds', () => {
    expect(classifyInstalledServer(server({ id: 'local', name: 'Local' }))).toBe('user')
    expect(
      classifyInstalledServer(server({ id: 'connector-notion', name: 'Notion', connectorId: 'notion' })),
    ).toBe('connector')
    expect(
      classifyInstalledServer(
        server({ id: 'plugin-officecli', name: 'OfficeCLI', connectorId: 'plugin:officecli' }),
      ),
    ).toBe('plugin')
  })
})

describe('webSearchLoadedMcps', () => {
  it('surfaces TinyFish when it is the search provider even before a token lands', () => {
    const loaded = webSearchLoadedMcps(
      webSearch({
        provider: 'tinyfish_mcp',
        tinyfishMcpUrl: 'https://agent.tinyfish.ai/mcp',
      }),
    )
    expect(loaded).toHaveLength(1)
    expect(loaded[0].id).toBe(TINYFISH_MCP_ID)
    expect(loaded[0].enabled).toBe(true)
  })

  it('surfaces TinyFish when OAuth is stored even if another search provider is selected', () => {
    const loaded = webSearchLoadedMcps(
      webSearch({
        provider: 'tavily',
        tinyfishMcpUrl: 'https://agent.tinyfish.ai/mcp',
        tinyfishMcpAuth: { kind: 'oauth', accessToken: 'tok' },
      }),
    )
    expect(loaded[0]?.id).toBe(TINYFISH_MCP_ID)
    expect(loaded[0]?.enabled).toBe(false)
    expect(loaded[0]?.headers.Authorization).toBe('Bearer tok')
  })

  it('does not invent an Exa row from the default URL', () => {
    expect(
      webSearchLoadedMcps(webSearch({ provider: 'tavily', exaMcpUrl: 'https://mcp.exa.ai/mcp' })),
    ).toEqual([])
  })

  it('surfaces Exa MCP only when that provider is selected', () => {
    const loaded = webSearchLoadedMcps(
      webSearch({ provider: 'exa_mcp', exaMcpUrl: 'https://mcp.exa.ai/mcp' }),
    )
    expect(loaded[0]?.id).toBe(EXA_MCP_ID)
  })
})

describe('buildInstalledMcpList', () => {
  it('shows a disabled plugin MCP as off and still locked to the plugin page', () => {
    const list = buildInstalledMcpList([
      server({
        id: 'plugin-cua-driver',
        name: 'Cua Driver',
        connectorId: 'plugin:cua-driver',
        enabled: false,
      }),
    ])
    expect(list).toHaveLength(1)
    expect(list[0]?.kind).toBe('plugin')
    expect(list[0]?.manageLocked).toBe(true)
    expect(list[0]?.server.enabled).toBe(false)
  })

  it('includes connector and plugin servers instead of hiding them', () => {
    const list = buildInstalledMcpList([
      server({ id: 'plugin-cua', name: 'Cua Driver', connectorId: 'plugin:cua-driver' }),
      server({
        id: 'connector-notion',
        name: 'Notion',
        connectorId: 'notion',
        url: 'https://mcp.notion.com/mcp',
        transport: 'streamable_http',
      }),
    ])
    expect(list.map((entry) => entry.kind).sort()).toEqual(['connector', 'plugin'])
  })

  it('adds TinyFish from web search when it is not already a chatTools server', () => {
    const list = buildInstalledMcpList(
      [server({ id: 'plugin-cua', name: 'Cua', connectorId: 'plugin:cua-driver' })],
      webSearch({
        provider: 'tinyfish_mcp',
        tinyfishMcpUrl: 'https://agent.tinyfish.ai/mcp',
        tinyfishMcpAuth: { kind: 'oauth', accessToken: 'tok' },
      }),
    )
    const tinyfish = list.find((entry) => entry.kind === 'websearch')
    expect(tinyfish?.server.id).toBe(TINYFISH_MCP_ID)
    expect(tinyfish?.manageLocked).toBe(true)
  })

  it('does not duplicate TinyFish when the same URL is already installed', () => {
    const list = buildInstalledMcpList(
      [
        server({
          id: 'mine',
          name: 'TinyFish',
          url: 'https://agent.tinyfish.ai/mcp/',
          transport: 'streamable_http',
        }),
      ],
      webSearch({
        provider: 'tinyfish_mcp',
        tinyfishMcpUrl: 'https://agent.tinyfish.ai/mcp',
        tinyfishMcpAuth: { kind: 'oauth', accessToken: 'tok' },
      }),
    )
    expect(list.filter((entry) => entry.kind === 'websearch')).toEqual([])
    expect(list).toHaveLength(1)
  })
})

describe('filterInstalledMcpList', () => {
  it('matches name and url', () => {
    const list = buildInstalledMcpList(
      [
        server({ id: 'a', name: 'OfficeCLI', command: 'officecli.exe' }),
        server({
          id: 'b',
          name: 'Remote',
          url: 'https://agent.tinyfish.ai/mcp',
          transport: 'streamable_http',
        }),
      ],
      undefined,
    )
    expect(filterInstalledMcpList(list, 'tinyfish').map((entry) => entry.server.id)).toEqual(['b'])
    expect(filterInstalledMcpList(list, 'office').map((entry) => entry.server.id)).toEqual(['a'])
  })
})
