// 连接器面板：目录化 + 一键授权的外部数据源 UX。
// 每个连接器最终物化成一条带 Authorization header 的 ChatMcpServer，写入
// chatTools.servers（connectorId 非空），由现有 MCP 管线自动收集工具。
// token 类（GitHub PAT / Composio key / 自定义 token）直接前端物化；
// OAuth 类（Notion / 自定义 OAuth）走后端 connector_oauth_connect（PKCE + DCR + loopback）。

import { useCallback, useEffect, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { Check, FolderOpen, Loader2, Trash2, X, Plus } from 'lucide-react'
import { api, type ChatMcpServer, type ChatToolsConfig } from '../api/tauri'
import { i18n, type Lang } from './i18n'
import { SettingsGroup, Input, Select } from './components'
import { CONNECTOR_CATALOG, isPluginManagedServer, type ConnectorCatalogEntry } from './connectorCatalog'
import {
  NotionBrandIcon,
  GithubBrandIcon,
  ComposioBrandIcon,
  CustomConnectorIcon,
  LinearBrandIcon,
  SentryBrandIcon,
  AtlassianBrandIcon,
  ObsidianBrandIcon,
} from './ConnectorBrandIcons'
import { ConnectorDetailModal } from './ConnectorDetailModal'
import { Button } from '../components/Button'

// catalog 项 iconKey → 品牌图标组件查找表；未命中（含自定义连接器）回退到通用 link 图标。
const CONNECTOR_ICON_BY_KEY: Record<
  string,
  (props: { size?: number; className?: string }) => JSX.Element
> = {
  notion: NotionBrandIcon,
  github: GithubBrandIcon,
  composio: ComposioBrandIcon,
  linear: LinearBrandIcon,
  sentry: SentryBrandIcon,
  atlassian: AtlassianBrandIcon,
  obsidian: ObsidianBrandIcon,
}

function connectorIconFor(iconKey: string | undefined) {
  if (iconKey && CONNECTOR_ICON_BY_KEY[iconKey]) return CONNECTOR_ICON_BY_KEY[iconKey]
  return CustomConnectorIcon
}

type Props = {
  servers: ChatMcpServer[]
  updateChatTools: (updates: Partial<ChatToolsConfig>) => void
  obsidianVaultPath: string
  onObsidianVaultPathChange: (path: string) => void
  lang: Lang
  testServer: (server: ChatMcpServer) => Promise<{ ok: boolean; message: string; tools: { name: string }[] } | null>
}

const OBSIDIAN_CATALOG_ID = 'obsidian'

// 目录项 → 已物化 server 的 id 约定。
function connectorServerId(catalogId: string): string {
  return `connector-${catalogId}`
}

function slugify(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    || 'custom'
}

export function ConnectorsPanel({
  servers,
  updateChatTools,
  obsidianVaultPath,
  onObsidianVaultPathChange,
  lang,
  testServer,
}: Props) {
  const t = i18n[lang]

  const obsidianEntry = CONNECTOR_CATALOG.find((e) => e.id === OBSIDIAN_CATALOG_ID)
  const obsidianConnected = obsidianVaultPath.trim().length > 0

  // Obsidian vault 选择器状态。
  const [vaultInputFor, setVaultInputFor] = useState(false)
  const [vaultDraft, setVaultDraft] = useState('')
  const [vaultOptions, setVaultOptions] = useState<{ name: string; path: string }[]>([])
  const [vaultLoading, setVaultLoading] = useState(false)

  // 哪个目录项正展开 token 输入框。
  const [tokenInputFor, setTokenInputFor] = useState<string | null>(null)
  const [tokenDraft, setTokenDraft] = useState('')
  const [busyId, setBusyId] = useState<string | null>(null)
  // 每条 server 的工具数（连接后或测试后填充）。
  const [toolCounts, setToolCounts] = useState<Record<string, number>>({})

  // 自定义连接器表单状态。
  const [showCustomForm, setShowCustomForm] = useState(false)
  const [customName, setCustomName] = useState('')
  const [customUrl, setCustomUrl] = useState('')
  const [customAuth, setCustomAuth] = useState<'none' | 'token' | 'oauth'>('token')
  const [customToken, setCustomToken] = useState('')

  // OAuth 授权进行中的目录项 id（卡片显示「授权中…」）。
  const [oauthBusyFor, setOauthBusyFor] = useState<string | null>(null)
  // OAuth 错误提示（按目录项 id 暂存）。
  const [oauthError, setOauthError] = useState<{ id: string; message: string } | null>(null)

  // 详情弹层：按已连接 server id 或目录项 id 标识。
  const [detail, setDetail] = useState<
    { kind: 'server'; serverId: string } | { kind: 'entry'; entryId: string } | null
  >(null)

  const connectedById = new Map(
    servers
      .filter((s) => s.connectorId && s.enabled && !isPluginManagedServer(s))
      .map((s) => [s.connectorId as string, s]),
  )

  const writeServer = useCallback(
    (server: ChatMcpServer) => {
      const next = servers.filter((s) => s.id !== server.id)
      next.push(server)
      updateChatTools({ servers: next })
    },
    [servers, updateChatTools],
  )

  const removeServer = useCallback(
    (serverId: string) => {
      updateChatTools({ servers: servers.filter((s) => s.id !== serverId) })
    },
    [servers, updateChatTools],
  )

  const loadVaultOptions = useCallback(async () => {
    setVaultLoading(true)
    try {
      const vaults = await api.listObsidianVaults()
      setVaultOptions(vaults)
      if (!vaultDraft && vaults.length > 0) {
        setVaultDraft(vaults[0].path)
      }
    } catch {
      setVaultOptions([])
    } finally {
      setVaultLoading(false)
    }
  }, [vaultDraft])

  useEffect(() => {
    if (!vaultInputFor) return
    void loadVaultOptions()
  }, [vaultInputFor, loadVaultOptions])

  const connectVault = useCallback(
    (path: string) => {
      const trimmed = path.trim()
      if (!trimmed) return
      onObsidianVaultPathChange(trimmed)
      setVaultInputFor(false)
      setVaultDraft('')
    },
    [onObsidianVaultPathChange],
  )

  const browseVaultFolder = useCallback(async () => {
    try {
      const selected = await open({ directory: true, multiple: false })
      if (typeof selected === 'string') {
        setVaultDraft(selected)
      }
    } catch (err) {
      console.error('Failed to pick vault directory:', err)
    }
  }, [])

  // token 类目录项 → 物化 + 可选测试连接。
  const connectTokenConnector = useCallback(
    async (entry: ConnectorCatalogEntry, token: string) => {
      const trimmed = token.trim()
      if (!trimmed) return
      const id = connectorServerId(entry.id)
      const server: ChatMcpServer = {
        id,
        name: entry.name,
        connectorId: entry.id,
        enabled: true,
        transport: 'streamable_http',
        url: entry.url ?? '',
        command: '',
        args: [],
        env: {},
        headers: { Authorization: `Bearer ${trimmed}` },
        cwd: null,
        enabledTools: [],
        auth: { kind: 'token', accessToken: trimmed },
      }
      writeServer(server)
      setTokenInputFor(null)
      setTokenDraft('')
      setBusyId(id)
      try {
        const result = await testServer(server)
        if (result?.ok) {
          setToolCounts((prev) => ({ ...prev, [id]: result.tools.length }))
        }
      } finally {
        setBusyId(null)
      }
    },
    [testServer, writeServer],
  )

  // OAuth 类目录项 → 跑后端 connector_oauth_connect（浏览器授权）→ 合并返回的 server。
  const connectOauthConnector = useCallback(
    async (entry: ConnectorCatalogEntry) => {
      setOauthError(null)
      setOauthBusyFor(entry.id)
      try {
        const server = await api.connectorOauthConnect({ catalogId: entry.id, url: entry.url })
        writeServer(server)
        // 授权后顺手测一下连接，填充工具数。
        setBusyId(server.id)
        try {
          const result = await testServer(server)
          if (result?.ok) {
            setToolCounts((prev) => ({ ...prev, [server.id]: result.tools.length }))
          }
        } finally {
          setBusyId(null)
        }
      } catch (err) {
        setOauthError({ id: entry.id, message: String(err) })
      } finally {
        setOauthBusyFor(null)
      }
    },
    [testServer, writeServer],
  )

  const addCustomConnector = useCallback(async () => {
    const name = customName.trim()
    const url = customUrl.trim()
    if (!name || !url) return

    // OAuth：跑后端 connector_oauth_connect（浏览器授权），返回物化好的 server。
    if (customAuth === 'oauth') {
      setOauthError(null)
      setOauthBusyFor('custom')
      try {
        const server = await api.connectorOauthConnect({ url, name })
        writeServer(server)
        setCustomName('')
        setCustomUrl('')
        setCustomToken('')
        setCustomAuth('token')
        setShowCustomForm(false)
        setBusyId(server.id)
        try {
          const result = await testServer(server)
          if (result?.ok) {
            setToolCounts((prev) => ({ ...prev, [server.id]: result.tools.length }))
          }
        } finally {
          setBusyId(null)
        }
      } catch (err) {
        setOauthError({ id: 'custom', message: String(err) })
      } finally {
        setOauthBusyFor(null)
      }
      return
    }

    const slug = slugify(name)
    const connectorId = `custom-${slug}`
    const id = connectorServerId(connectorId)
    const headers: Record<string, string> = {}
    let auth: ChatMcpServer['auth']
    if (customAuth === 'token' && customToken.trim()) {
      headers.Authorization = `Bearer ${customToken.trim()}`
      auth = { kind: 'token', accessToken: customToken.trim() }
    }
    const server: ChatMcpServer = {
      id,
      name,
      connectorId,
      enabled: true,
      transport: 'streamable_http',
      url,
      command: '',
      args: [],
      env: {},
      headers,
      cwd: null,
      enabledTools: [],
      auth,
    }
    writeServer(server)
    setCustomName('')
    setCustomUrl('')
    setCustomToken('')
    setCustomAuth('token')
    setShowCustomForm(false)
    setBusyId(id)
    try {
      const result = await testServer(server)
      if (result?.ok) {
        setToolCounts((prev) => ({ ...prev, [id]: result.tools.length }))
      }
    } finally {
      setBusyId(null)
    }
  }, [customAuth, customName, customToken, customUrl, testServer, writeServer])

  // 已连接卡（MCP server；Obsidian 单独渲染）。插件 MCP 走「插件」页，不在这里出现。
  const connectedServers = servers.filter(
    (s) => s.connectorId && s.enabled && !isPluginManagedServer(s),
  )
  // 目录中尚未连接的项（Obsidian 已配置路径时从可用列表移除）。
  const availableEntries = CONNECTOR_CATALOG.filter((e) => {
    if (e.id === OBSIDIAN_CATALOG_ID) return !obsidianConnected
    return !connectedById.has(e.id)
  })

  const renderConnectedCard = (server: ChatMcpServer) => {
    const entry = CONNECTOR_CATALOG.find((e) => e.id === server.connectorId)
    const count = toolCounts[server.id]
    const busy = busyId === server.id
    const Icon = connectorIconFor(entry?.iconKey)
    return (
      <div
        key={server.id}
        className="kv-panel cursor-pointer transition hover:border-[var(--accent)]"
        role="button"
        tabIndex={0}
        onClick={() => setDetail({ kind: 'server', serverId: server.id })}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            setDetail({ kind: 'server', serverId: server.id })
          }
        }}
      >
        <div className="flex items-start gap-3">
          <Icon size={22} className="mt-0.5 shrink-0 opacity-90" />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-[var(--text)]">{server.name}</div>
            <div className="kv-row-desc line-clamp-2 text-[12px]">
              {entry?.description[lang] ?? server.url}
            </div>
            {entry?.composio && (
              <div className="kv-row-desc text-[12px] opacity-70">{t.connectorsThirdParty}</div>
            )}
          </div>
          <div className="flex shrink-0 flex-col items-end gap-1">
            <div className="flex items-center gap-1 text-[12px] text-emerald-600 dark:text-emerald-400">
              {busy ? (
                <Loader2 size={12} className="animate-spin opacity-70" />
              ) : (
                <Check size={12} />
              )}
              <span>
                {busy
                  ? t.connectorsConnecting
                  : typeof count === 'number'
                    ? t.connectorsToolsFound.replace('{n}', String(count))
                    : t.connectorsConnected}
              </span>
            </div>
            <Button
              variant="danger"
              size="sm"
              onClick={(e) => {
                e.stopPropagation()
                removeServer(server.id)
              }}
              data-tauri-drag-region="false"
            >
              <Trash2 size={10} />
              {t.connectorsDisconnect}
            </Button>
          </div>
        </div>
      </div>
    )
  }

  const renderObsidianConnectedCard = () => {
    if (!obsidianEntry || !obsidianConnected) return null
    const Icon = ObsidianBrandIcon
    return (
      <div
        key="obsidian-vault"
        className="kv-panel cursor-pointer transition hover:border-[var(--accent)]"
        role="button"
        tabIndex={0}
        onClick={() => setDetail({ kind: 'entry', entryId: OBSIDIAN_CATALOG_ID })}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            setDetail({ kind: 'entry', entryId: OBSIDIAN_CATALOG_ID })
          }
        }}
      >
        <div className="flex items-start gap-3">
          <Icon size={22} className="mt-0.5 shrink-0 opacity-90" />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-[var(--text)]">{obsidianEntry.name}</div>
            <div className="kv-row-desc line-clamp-2 text-[12px]">{obsidianEntry.description[lang]}</div>
            <div className="kv-row-desc mt-1 truncate font-mono text-[11px] opacity-80">
              {obsidianVaultPath}
            </div>
          </div>
          <div className="flex shrink-0 flex-col items-end gap-1">
            <div className="flex items-center gap-1 text-[12px] text-emerald-600 dark:text-emerald-400">
              <Check size={12} />
              <span>{t.connectorsConnected}</span>
            </div>
            <Button
              variant="danger"
              size="sm"
              onClick={(e) => {
                e.stopPropagation()
                onObsidianVaultPathChange('')
              }}
              data-tauri-drag-region="false"
            >
              <Trash2 size={10} />
              {t.connectorsDisconnect}
            </Button>
          </div>
        </div>
      </div>
    )
  }

  const renderAvailableCard = (entry: ConnectorCatalogEntry) => {
    const isVault = entry.authKind === 'vault'
    const isTokenInput = tokenInputFor === entry.id
    const isVaultInput = isVault && vaultInputFor
    const isOauth = entry.authKind === 'oauth'
    const oauthBusy = oauthBusyFor === entry.id
    const errorMessage = oauthError?.id === entry.id ? oauthError.message : null
    const Icon = connectorIconFor(entry.iconKey)
    return (
      <div
        key={entry.id}
        className="kv-panel cursor-pointer transition hover:border-[var(--accent)]"
        role="button"
        tabIndex={0}
        onClick={() => {
          setDetail({ kind: 'entry', entryId: entry.id })
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            setDetail({ kind: 'entry', entryId: entry.id })
          }
        }}
      >
        <div className="flex items-start gap-3">
          <Icon size={22} className="mt-0.5 shrink-0 opacity-90" />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-[var(--text)]">{entry.name}</div>
            <div className="kv-row-desc line-clamp-2 text-[12px]">{entry.description[lang]}</div>
            {entry.composio && (
              <div className="kv-row-desc text-[12px] opacity-70">{t.connectorsThirdParty}</div>
            )}
            {errorMessage && (
              <div className="kv-row-desc text-[12px] text-red-500 dark:text-red-400">
                {t.connectorsOauthFailed}: {errorMessage}
              </div>
            )}
          </div>
          {isOauth ? (
            <Button
              variant="primary"
              size="sm"
              className="shrink-0"
              disabled={oauthBusy}
              onClick={(e) => {
                e.stopPropagation()
                void connectOauthConnector(entry)
              }}
              data-tauri-drag-region="false"
            >
              {oauthBusy ? (
                <>
                  <Loader2 size={10} className="animate-spin" />
                  {t.connectorsOauthAuthorizing}
                </>
              ) : (
                t.connectorsOauthAuthorize
              )}
            </Button>
          ) : isVault ? (
            !isVaultInput && (
              <Button
                variant="primary"
                size="sm"
                className="shrink-0"
                onClick={(e) => {
                  e.stopPropagation()
                  setVaultDraft(obsidianVaultPath)
                  setVaultInputFor(true)
                }}
                data-tauri-drag-region="false"
              >
                {t.connectorsConnect}
              </Button>
            )
          ) : (
            !isTokenInput && (
              <Button
                variant="primary"
                size="sm"
                className="shrink-0"
                onClick={(e) => {
                  e.stopPropagation()
                  setTokenDraft('')
                  setTokenInputFor(entry.id)
                }}
                data-tauri-drag-region="false"
              >
                {t.connectorsConnect}
              </Button>
            )
          )}
        </div>
        {isVaultInput && (
          <div
            className="mt-2 space-y-2"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="kv-row-desc text-[12px] opacity-80">{t.connectorsVaultPathHint}</div>
            {vaultLoading ? (
              <div className="flex items-center gap-2 text-[12px] opacity-70">
                <Loader2 size={12} className="animate-spin" />
                {t.connectorsConnecting}
              </div>
            ) : vaultOptions.length > 0 ? (
              <Select
                value={vaultDraft}
                onChange={setVaultDraft}
                options={vaultOptions.map((v) => ({
                  value: v.path,
                  label: `${v.name} — ${v.path}`,
                }))}
              />
            ) : (
              <div className="kv-row-desc text-[12px] opacity-70">{t.connectorsVaultEmpty}</div>
            )}
            <div className="flex flex-wrap items-center gap-2">
              <Input
                value={vaultDraft}
                onChange={setVaultDraft}
                mono
                placeholder={t.connectorsVaultSelect}
              />
              <Button
                size="sm"
                onClick={() => void browseVaultFolder()}
                data-tauri-drag-region="false"
              >
                <FolderOpen size={10} />
                {t.connectorsVaultBrowse}
              </Button>
              <Button
                variant="primary"
                size="sm"
                disabled={!vaultDraft.trim()}
                onClick={() => connectVault(vaultDraft)}
                data-tauri-drag-region="false"
              >
                {t.connectorsVaultSave}
              </Button>
              <Button
                size="sm"
                onClick={() => {
                  setVaultInputFor(false)
                  setVaultDraft('')
                }}
                data-tauri-drag-region="false"
              >
                <X size={10} />
              </Button>
            </div>
          </div>
        )}
        {isTokenInput && (
          <div
            className="mt-2 flex items-center gap-2"
            onClick={(e) => e.stopPropagation()}
          >
            <Input
              value={tokenDraft}
              onChange={setTokenDraft}
              type="password"
              mono
              placeholder={entry.tokenHint?.[lang] ?? t.connectorsTokenPlaceholder}
            />
            <Button
              variant="primary"
              size="sm"
              disabled={!tokenDraft.trim()}
              onClick={(e) => {
                e.stopPropagation()
                void connectTokenConnector(entry, tokenDraft)
              }}
              data-tauri-drag-region="false"
            >
              {t.connectorsTokenSubmit}
            </Button>
            <Button
              size="sm"
              onClick={(e) => {
                e.stopPropagation()
                setTokenInputFor(null)
                setTokenDraft('')
              }}
              data-tauri-drag-region="false"
            >
              <X size={10} />
            </Button>
          </div>
        )}
      </div>
    )
  }

  return (
    <>
      <SettingsGroup title={t.connectorsSectionConnected}>
        {connectedServers.length === 0 && !obsidianConnected ? (
          <div className="kv-row-desc py-2">{t.connectorsEmptyConnected}</div>
        ) : (
          <div className="grid grid-cols-1 gap-3 py-2 md:grid-cols-2">
            {renderObsidianConnectedCard()}
            {connectedServers.map(renderConnectedCard)}
          </div>
        )}
        {t.connectorsSaveHint ? (
          <div className="kv-row-desc py-1 opacity-70">{t.connectorsSaveHint}</div>
        ) : null}
      </SettingsGroup>

      <SettingsGroup title={t.connectorsSectionAvailable}>
        <div className="grid grid-cols-1 gap-3 py-2 md:grid-cols-2">
          {availableEntries.map(renderAvailableCard)}
        </div>
        <div className="pt-1">
          <Button
            size="sm"
            onClick={() => setShowCustomForm((v) => !v)}
            data-tauri-drag-region="false"
          >
            {showCustomForm ? <X size={10} /> : <Plus size={10} />}
            {t.connectorsAddCustomToggle}
          </Button>
        </div>
        {showCustomForm && (
          <div className="kv-panel mt-2 space-y-2">
            <Input value={customName} onChange={setCustomName} placeholder={t.connectorsCustomName} />
            <Input value={customUrl} onChange={setCustomUrl} mono placeholder={t.connectorsCustomUrl} />
            <div className="flex items-center gap-2">
              <Select
                value={customAuth}
                onChange={(v) =>
                  setCustomAuth(v === 'none' ? 'none' : v === 'oauth' ? 'oauth' : 'token')
                }
                options={[
                  { value: 'token', label: t.connectorsAuthToken },
                  { value: 'oauth', label: t.connectorsAuthOauth },
                  { value: 'none', label: t.connectorsAuthNone },
                ]}
              />
              {customAuth === 'token' && (
                <Input
                  value={customToken}
                  onChange={setCustomToken}
                  type="password"
                  mono
                  placeholder={t.connectorsTokenPlaceholder}
                />
              )}
            </div>
            {oauthError?.id === 'custom' && (
              <div className="kv-row-desc text-[12px] text-red-500 dark:text-red-400">
                {t.connectorsOauthFailed}: {oauthError.message}
              </div>
            )}
            <Button
              variant="primary"
              size="sm"
              disabled={!customName.trim() || !customUrl.trim() || oauthBusyFor === 'custom'}
              onClick={() => void addCustomConnector()}
              data-tauri-drag-region="false"
            >
              {oauthBusyFor === 'custom' ? (
                <>
                  <Loader2 size={10} className="animate-spin" />
                  {t.connectorsOauthAuthorizing}
                </>
              ) : (
                t.connectorsCustomAdd
              )}
            </Button>
          </div>
        )}
      </SettingsGroup>

      {detail &&
        (() => {
          // 解析当前详情对应的 server / catalog 项。
          const server =
            detail.kind === 'server'
              ? servers.find((s) => s.id === detail.serverId) ?? null
              : null
          const entry =
            detail.kind === 'entry'
              ? CONNECTOR_CATALOG.find((e) => e.id === detail.entryId)
              : CONNECTOR_CATALOG.find((e) => e.id === server?.connectorId)
          // server 不存在（已被断开）时关闭。
          if (detail.kind === 'server' && !server) {
            setDetail(null)
            return null
          }
          const connectBusy =
            (entry && oauthBusyFor === entry.id) || (server ? busyId === server.id : false)
          const vaultPath =
            entry?.authKind === 'vault' && obsidianConnected ? obsidianVaultPath : undefined
          return (
            <ConnectorDetailModal
              lang={lang}
              entry={entry}
              server={server}
              vaultPath={vaultPath}
              onDisconnectVault={() => {
                onObsidianVaultPathChange('')
                setDetail(null)
              }}
              fallbackName={server?.name ?? entry?.name ?? ''}
              fallbackUrl={server?.url ?? entry?.url}
              connectBusy={!!connectBusy}
              onClose={() => setDetail(null)}
              onUpdateServer={writeServer}
              onDisconnect={(serverId) => {
                removeServer(serverId)
                setDetail(null)
              }}
              onConnect={() => {
                if (!entry) return
                if (entry.authKind === 'vault') {
                  setDetail(null)
                  setVaultDraft(obsidianVaultPath)
                  setVaultInputFor(true)
                } else if (entry.authKind === 'oauth') {
                  void connectOauthConnector(entry)
                } else {
                  // token 类：关闭弹层、在可用卡片上展开 token 输入框。
                  setDetail(null)
                  setTokenDraft('')
                  setTokenInputFor(entry.id)
                }
              }}
            />
          )
        })()}
    </>
  )
}
