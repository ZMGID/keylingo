import type { ChatMcpServer, WebSearchConfig, WebSearchMcpAuth } from '../api/tauri'
import { isPluginManagedServer } from '../settings/connectorCatalog'

export const TINYFISH_MCP_ID = 'tinyfish-mcp'
export const EXA_MCP_ID = 'exa-mcp'

export type McpInstalledKind = 'user' | 'connector' | 'plugin' | 'websearch'

export type McpInstalledEntry = {
  server: ChatMcpServer
  kind: McpInstalledKind
  /** 开关在插件 / 连接器 / 网络搜索页，这里只读。 */
  manageLocked: boolean
}

export function mcpUrlKey(url: string): string {
  return url.trim().replace(/\/+$/, '').toLowerCase()
}

export function classifyInstalledServer(server: ChatMcpServer): McpInstalledKind {
  if (isPluginManagedServer(server)) return 'plugin'
  if (server.connectorId) return 'connector'
  return 'user'
}

function httpMcpServer(
  partial: Pick<ChatMcpServer, 'id' | 'name' | 'url'> & Partial<ChatMcpServer>,
): ChatMcpServer {
  return {
    enabled: true,
    transport: 'streamable_http',
    command: '',
    args: [],
    env: {},
    headers: {},
    cwd: null,
    enabledTools: [],
    ...partial,
  }
}

function bearerHeaders(auth: WebSearchMcpAuth | null | undefined): Record<string, string> {
  const token = auth?.accessToken?.trim()
  if (!token) return {}
  const value = /^bearer /i.test(token) ? token : `Bearer ${token}`
  return { Authorization: value }
}

function tinyfishMcpLoaded(webSearch: WebSearchConfig): boolean {
  if (webSearch.provider === 'tinyfish_mcp') return true
  const auth = webSearch.tinyfishMcpAuth
  return Boolean(auth?.accessToken?.trim() || auth?.refreshToken?.trim())
}

/** 网络搜索实际在跑的 MCP（未写入 chatTools.servers 的那一份）。 */
export function webSearchLoadedMcps(webSearch: WebSearchConfig | undefined): ChatMcpServer[] {
  if (!webSearch) return []
  const out: ChatMcpServer[] = []

  if (tinyfishMcpLoaded(webSearch)) {
    const url = (webSearch.tinyfishMcpUrl ?? '').trim() || 'https://agent.tinyfish.ai/mcp'
    const auth = webSearch.tinyfishMcpAuth ?? undefined
    out.push(
      httpMcpServer({
        id: TINYFISH_MCP_ID,
        name: 'TinyFish',
        url,
        enabled: webSearch.provider === 'tinyfish_mcp',
        headers: bearerHeaders(auth),
        auth,
      }),
    )
  }

  const exaUrl = (webSearch.exaMcpUrl ?? '').trim()
  if (webSearch.provider === 'exa_mcp' && exaUrl) {
    out.push(
      httpMcpServer({
        id: EXA_MCP_ID,
        name: 'Exa MCP',
        url: exaUrl,
        enabled: true,
      }),
    )
  }

  return out
}

export function buildInstalledMcpList(
  servers: ChatMcpServer[],
  webSearch?: WebSearchConfig,
): McpInstalledEntry[] {
  const entries: McpInstalledEntry[] = servers.map((server) => {
    const kind = classifyInstalledServer(server)
    return { server, kind, manageLocked: kind !== 'user' }
  })
  const existingUrls = new Set(servers.map((server) => mcpUrlKey(server.url)).filter(Boolean))
  for (const server of webSearchLoadedMcps(webSearch)) {
    if (existingUrls.has(mcpUrlKey(server.url))) continue
    entries.push({ server, kind: 'websearch', manageLocked: true })
  }
  return entries
}

export function filterInstalledMcpList(
  entries: McpInstalledEntry[],
  query: string,
): McpInstalledEntry[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return entries
  return entries.filter(({ server }) => {
    const haystack = [
      server.name,
      server.url,
      server.command,
      server.connectorId ?? '',
      ...server.args,
    ]
      .join(' ')
      .toLowerCase()
    return haystack.includes(needle)
  })
}

export function entriesOfKind(
  entries: McpInstalledEntry[],
  kind: McpInstalledKind,
): McpInstalledEntry[] {
  return entries.filter((entry) => entry.kind === kind)
}
