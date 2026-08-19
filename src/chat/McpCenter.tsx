// MCP 整页（Chat 窗口「扩展 → MCP」）。全量管理：已安装（启用/删除/连接状态 + 展开编辑
// transport/url/命令/env/headers + 测试连接 + OAuth 授权）、市场（内联注册表浏览）、导入 mcp.json、
// 高级设置（Kivio 内置工具 + 工具运行参数）。插件 MCP 同列表展示，只读（开关在「扩展 → 插件」）。
// 取代原「设置 → MCP」页。

import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react'
import { ChevronDown, FolderOpen, Loader2, RefreshCw, Search, Trash2 } from 'lucide-react'
import { McpIcon } from '../settings/NavIcons'
import { useLang, useT } from '../settings/i18n'
import { open } from '@tauri-apps/plugin-dialog'
import {
  api,
  defaultNativeTools,
  type ChatMcpServer,
  type ChatNativeToolsConfig,
  type ChatToolsConfig,
  type CliImportScan,
  type McpServerState,
  type Settings,
} from '../api/tauri'
import { getSettingsCached, refreshSettings, saveSettingsCached } from '../api/settingsCache'
import { Toggle, Select, Input } from '../settings/components'
import { Button, IconButton } from '../components/Button'
import { McpRegistryBrowser } from './McpRegistryBrowser'
import {
  argsToText,
  CHAT_TOOL_ROUND_PRESETS,
  CHAT_TOOL_TIMEOUT_PRESETS_MS,
  clampMcpIdleTimeoutMs,
  clampSubAgentConcurrency,
  clampToolRounds,
  clampToolTimeoutMs,
  defaultChatTools,
  envToText,
  formatToolRoundsLabel,
  formatToolTimeoutLabel,
  MCP_IDLE_TIMEOUT_PRESETS_MS,
  SUB_AGENT_CONCURRENCY_PRESETS,
  textToArgs,
  textToEnv,
} from '../settings/chatToolsShared'
import { isPluginManagedServer, preservePluginManagedServers } from '../settings/connectorCatalog'

type TestFeedback = { ok: boolean; message: string }

function StatusDot({ state }: { state?: McpServerState }) {
  const t = useT()
  const kind = state?.kind ?? 'disconnected'
  const color =
    kind === 'connected' ? 'bg-emerald-500'
    : kind === 'connecting' ? 'bg-amber-500'
    : kind === 'error' ? 'bg-red-500'
    : 'bg-neutral-300 dark:bg-neutral-600'
  const label = kind === 'connected' ? t.chatMcpStatusConnected : kind === 'connecting' ? t.chatMcpStatusConnecting : kind === 'error' ? t.chatMcpStatusError : t.chatMcpStatusDisconnected
  return (
    <span className="inline-flex items-center gap-1.5 text-[11.5px] text-neutral-500 dark:text-neutral-400">
      <span className={`h-2 w-2 rounded-full ${color}`} />
      {label}
    </span>
  )
}

const TEXTAREA_CLASS =
  'w-full rounded-md border border-neutral-200 bg-white px-2.5 py-2 font-mono text-[12px] text-neutral-800 outline-none focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100'

export function McpCenter() {
  const t = useT()
  const lang = useLang()
  const [settings, setSettings] = useState<Settings | null>(null)
  const [states, setStates] = useState<Record<string, McpServerState>>({})
  const [view, setView] = useState<'installed' | 'store' | 'import' | 'advanced'>('installed')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [testingId, setTestingId] = useState<string | null>(null)
  const [oauthId, setOauthId] = useState<string | null>(null)
  const [testFeedback, setTestFeedback] = useState<Record<string, TestFeedback>>({})
  const [cliScan, setCliScan] = useState<CliImportScan | null>(null)
  const [cliScanning, setCliScanning] = useState(false)
  const [cliSelected, setCliSelected] = useState<Set<string>>(new Set())
  const [cliImportDone, setCliImportDone] = useState('')
  const settingsRef = useRef<Settings | null>(null)

  const chatTools = settings?.chatTools ?? defaultChatTools()
  const servers = chatTools.servers
  const nativeTools = chatTools.nativeTools ?? defaultNativeTools()

  const NATIVE_TOOLS: Array<{ key: keyof ChatNativeToolsConfig; label: string; defaultOn?: boolean }> = [
    { key: 'readFile', label: t.chatMcpNativeReadFile },
    { key: 'writeFile', label: t.chatMcpNativeWriteFile },
    { key: 'editFile', label: t.chatMcpNativeEditFile },
    { key: 'runCommand', label: t.chatMcpNativeRunCommand },
    { key: 'runPython', label: t.chatMcpNativeRunPython },
    { key: 'skillRuntime', label: t.chatMcpNativeSkillRuntime, defaultOn: true },
    { key: 'webSearch', label: t.chatMcpNativeWebSearch },
    { key: 'webFetch', label: t.chatMcpNativeWebFetch },
  ]

  const loadSettings = useCallback(async () => {
    try {
      const loaded = await getSettingsCached()
      settingsRef.current = loaded
      setSettings(loaded)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadSettings()
  }, [loadSettings])

  // 连接状态：订阅推送 + 已启用服务器初次快照
  useEffect(() => {
    let unlisten: (() => void) | undefined
    void api.onMcpServerState((payload) => {
      setStates((prev) => ({ ...prev, [payload.serverId]: payload.state }))
    }).then((fn) => {
      unlisten = fn
    })
    return () => unlisten?.()
  }, [])

  useEffect(() => {
    servers.forEach((server) => {
      if (!server.enabled) return
      void api
        .chatMcpServerStatus(server.id)
        .then((status) => setStates((prev) => ({ ...prev, [server.id]: status.state })))
        .catch(() => {})
    })
  }, [servers])

  // 非服务器 chatTools 字段（内置工具 / 运行参数）：本地立即生效 + 持久化，保住后端刷新的 servers。
  const persistChatTools = useCallback((updates: Partial<ChatToolsConfig>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const next: Settings = { ...prev, chatTools: { ...(prev.chatTools ?? defaultChatTools()), ...updates } }
      settingsRef.current = next
      return next
    })
    void (async () => {
      try {
        const fresh = await refreshSettings()
        const merged: Settings = {
          ...fresh,
          chatTools: { ...(fresh.chatTools ?? defaultChatTools()), ...updates },
        }
        const saved = await saveSettingsCached(merged)
        settingsRef.current = saved
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      }
    })()
  }, [])

  const updateNativeTools = useCallback((updates: Partial<ChatNativeToolsConfig>) => {
    const base = settingsRef.current?.chatTools?.nativeTools ?? defaultNativeTools()
    persistChatTools({ nativeTools: { ...defaultNativeTools(), ...base, ...updates } })
  }, [persistChatTools])

  // 变更服务器：先读后端 fresh（保住后端 OAuth 刷新过的 token），再按 id 施加改动后整存。
  const mutateServers = useCallback(async (fn: (servers: ChatMcpServer[]) => ChatMcpServer[]) => {
    try {
      const fresh = await refreshSettings()
      const prevServers = fresh.chatTools?.servers ?? []
      const nextServers = preservePluginManagedServers(prevServers, fn(prevServers))
      const merged: Settings = {
        ...fresh,
        chatTools: { ...(fresh.chatTools ?? defaultChatTools()), servers: nextServers },
      }
      const saved = await saveSettingsCached(merged)
      settingsRef.current = saved
      setSettings(saved)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  const updateServer = useCallback((id: string, updates: Partial<ChatMcpServer>) => {
    void mutateServers((list) => list.map((s) => (s.id === id ? { ...s, ...updates } : s)))
  }, [mutateServers])

  // 启用即连：先落设置（后端按 settings 判定 eligible），再后台预热该 server。
  // 状态点随 onMcpServerState 推送变绿/红，无需手动「测试连接」或发起对话。
  const toggleServerEnabled = useCallback((id: string, enabled: boolean) => {
    const current = settingsRef.current?.chatTools?.servers?.find((s) => s.id === id)
    if (current && isPluginManagedServer(current)) return
    void mutateServers((list) => list.map((s) => (s.id === id ? { ...s, enabled } : s))).then(() => {
      if (enabled) void api.chatMcpWarmup([id])
    })
  }, [mutateServers])

  const handleInstall = useCallback((server: ChatMcpServer) => {
    void mutateServers((list) => [...list, server])
  }, [mutateServers])

  const handleImportJson = useCallback(async () => {
    try {
      const selected = await open({ directory: false, multiple: false, filters: [{ name: 'MCP JSON', extensions: ['json'] }] })
      if (typeof selected !== 'string') return
      const result = await api.chatMcpImportJson(selected)
      if (!result.success) {
        setError(result.error || t.chatMcpImportJsonFailed)
        return
      }
      await mutateServers((list) => [...list, ...result.servers])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [mutateServers, t])

  // 从本地 CLI（Claude Code / Codex / OpenCode / Pi）扫描已配置的 MCP 服务器。
  const handleCliScan = useCallback(async () => {
    setCliScanning(true)
    setCliImportDone('')
    setError('')
    try {
      const scan = await api.chatCliImportScan()
      setCliScan(scan)
      // 默认不勾，由用户自己选。
      setCliSelected(new Set())
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setCliScanning(false)
    }
  }, [])

  const toggleCliSelected = useCallback((id: string) => {
    setCliSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  // 导入选中项 = 复制成 Kivio 自己的 ChatMcpServer（enabled:false，走现有安装流程）。
  const handleCliImportSelected = useCallback(async () => {
    if (!cliScan) return
    const all = [...cliScan.claude.servers, ...cliScan.codex.servers, ...cliScan.opencode.servers, ...cliScan.pi.servers]
    const chosen = all.filter((server) => cliSelected.has(server.id))
    if (chosen.length === 0) return
    await mutateServers((list) => [...list, ...chosen])
    setCliImportDone(t.chatMcpCliImportDone.replace('{n}', String(chosen.length)))
    setCliScan(null)
    setCliSelected(new Set())
    setView('installed')
  }, [cliScan, cliSelected, mutateServers, t])

  const handleTest = useCallback(async (server: ChatMcpServer) => {
    setTestingId(server.id)
    setTestFeedback((prev) => {
      const next = { ...prev }
      delete next[server.id]
      return next
    })
    try {
      const result = await api.chatMcpTestServer(server, chatTools.toolTimeoutMs)
      setTestFeedback((prev) => ({
        ...prev,
        [server.id]: result.success
          ? { ok: true, message: t.chatMcpTestConnected.replace('{n}', String(result.tools.length)) }
          : { ok: false, message: result.error || t.chatMcpTestFailed },
      }))
    } catch (err) {
      setTestFeedback((prev) => ({ ...prev, [server.id]: { ok: false, message: err instanceof Error ? err.message : String(err) } }))
    } finally {
      setTestingId(null)
    }
  }, [chatTools.toolTimeoutMs, t])

  // OAuth 授权 remote(streamable_http) MCP：复用连接器 PKCE+DCR，把返回的 auth+Authorization 拼回本条。
  const handleOauth = useCallback(async (server: ChatMcpServer) => {
    const url = (server.url || '').trim()
    if (!url) return
    setOauthId(server.id)
    try {
      const authed = await api.connectorOauthConnect({ url, name: server.name })
      const authorization = authed.headers?.Authorization
      const nextHeaders = authorization ? { ...(server.headers || {}), Authorization: authorization } : (server.headers || {})
      await mutateServers((list) => list.map((s) => (s.id === server.id ? { ...s, auth: authed.auth, headers: nextHeaders } : s)))
      await handleTest({ ...server, auth: authed.auth, headers: nextHeaders })
    } catch (err) {
      setTestFeedback((prev) => ({ ...prev, [server.id]: { ok: false, message: err instanceof Error ? err.message : String(err) } }))
    } finally {
      setOauthId(null)
    }
  }, [handleTest, mutateServers])

  const listedServers = servers.filter((s) => !s.connectorId || isPluginManagedServer(s))

  const renderRuntimeSelect = (
    label: string,
    value: string,
    onChange: (value: string) => void,
    options: Array<{ value: string; label: string }>,
    desc?: string,
  ) => (
    <div className="flex h-full flex-col">
      <div className="mb-2">
        <div className="text-[13px] font-medium text-neutral-800 dark:text-neutral-100">{label}</div>
        {desc && <p className="mt-0.5 text-[12px] text-neutral-500 dark:text-neutral-400">{desc}</p>}
      </div>
      <div className="mt-auto">
        <Select className="w-full" value={value} onChange={onChange} options={options} />
      </div>
    </div>
  )

  return (
    <div className="assistant-center-root flex h-full min-h-0 flex-col text-neutral-900 dark:text-neutral-100">

      <main className="custom-scrollbar min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex h-full min-h-0 w-full max-w-[1040px] flex-col px-9 pb-10 pt-7">
          <div className="border-b border-neutral-200 pb-5 dark:border-neutral-800">
            <h1 className="flex items-center gap-2.5 text-[28px] font-semibold tracking-normal text-neutral-950 dark:text-neutral-50">
              <McpIcon size={24} className="text-neutral-500" />
              MCP
            </h1>
            <div className="mt-3.5 flex min-w-0 items-center gap-4">
              <p className="min-w-0 flex-1 text-[14px] leading-relaxed text-neutral-500 dark:text-neutral-400">
                {t.chatMcpSubtitle}
              </p>
              <IconButton size="lg" label={t.chatMcpRefresh} onClick={() => void loadSettings()} data-tauri-drag-region="false">
                <RefreshCw size={17} />
              </IconButton>
            </div>
          </div>

          <div className="mt-5 flex items-center gap-1 border-b border-neutral-200 dark:border-neutral-800">
            {([['installed', t.chatMcpTabInstalled], ['store', t.chatMcpTabStore], ['import', t.chatMcpTabImport], ['advanced', t.chatMcpTabAdvanced]] as const).map(([id, label]) => (
              <button
                key={id}
                type="button"
                onClick={() => setView(id)}
                data-tauri-drag-region="false"
                className={`relative px-3 py-2 text-[13px] font-medium transition-colors ${
                  view === id ? 'text-neutral-900 dark:text-neutral-100' : 'text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200'
                }`}
              >
                {label}
                {id === 'installed' && listedServers.length > 0 && (
                  <span className="ml-1.5 text-[11px] tabular-nums text-neutral-400">{listedServers.length}</span>
                )}
                {view === id && <span className="chat-motion-tab-underline absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-[#2f6ff0] dark:bg-[#5c8df7]" />}
              </button>
            ))}
          </div>

          {error && (
            <div className="mt-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-[12px] text-red-700 dark:border-red-900/60 dark:bg-red-950/30 dark:text-red-300">
              {error}
            </div>
          )}

          {view === 'store' ? (
            <div key="store" className="chat-motion-tab-in mt-5 flex min-h-[420px] flex-col">
              <McpRegistryBrowser existingServers={servers} onInstall={handleInstall} />
            </div>
          ) : view === 'import' ? (
            <div key="import" className="chat-motion-tab-in mt-5 space-y-4">
              <div className="rounded-md border border-neutral-200 p-3 dark:border-neutral-800">
                <div className="mb-1.5 text-[13px] font-medium text-neutral-800 dark:text-neutral-100">{t.chatMcpImportJsonTitle}</div>
                <p className="mb-2 text-[12px] text-neutral-500 dark:text-neutral-400">{t.chatMcpImportJsonDesc}</p>
                <Button onClick={() => void handleImportJson()} data-tauri-drag-region="false">
                  <FolderOpen size={14} />
                  {t.chatMcpImportJsonPick}
                </Button>
              </div>

              <div className="rounded-md border border-neutral-200 p-3 dark:border-neutral-800">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="mb-1.5 text-[13px] font-medium text-neutral-800 dark:text-neutral-100">{t.chatMcpImportCliTitle}</div>
                    <p className="text-[12px] text-neutral-500 dark:text-neutral-400">
                      {t.chatMcpImportCliDesc}
                    </p>
                  </div>
                  <Button onClick={() => void handleCliScan()} disabled={cliScanning} data-tauri-drag-region="false">
                    {cliScanning ? <Loader2 size={14} className="animate-spin" /> : <Search size={14} />}
                    {t.chatMcpScan}
                  </Button>
                </div>

                {cliScan && (
                  <div className="mt-3 space-y-3">
                    {([['claude', 'Claude Code'], ['codex', 'Codex'], ['opencode', 'OpenCode'], ['pi', 'Pi']] as const).map(([key, label]) => {
                      const group = cliScan[key]
                      return (
                        <div key={key}>
                          <div className="mb-1.5 flex items-center gap-2 text-[12px] font-medium text-neutral-600 dark:text-neutral-300">
                            {label}
                            {!group.available && <span className="text-[11px] font-normal text-neutral-400">{t.chatMcpCliNotDetected}</span>}
                          </div>
                          {group.available && (
                            group.servers.length === 0 ? (
                              <div className="rounded-md border border-dashed border-neutral-200 px-3 py-2 text-[11.5px] text-neutral-400 dark:border-neutral-800">
                                {t.chatMcpCliNoServers}
                              </div>
                            ) : (
                              <div className="overflow-hidden rounded-md border border-neutral-200 dark:border-neutral-800 [&>*+*]:border-t [&>*+*]:border-neutral-100 dark:[&>*+*]:border-neutral-800/70">
                                {group.servers.map((server) => {
                                  const isHttp = server.transport === 'streamable_http'
                                  return (
                                    <label
                                      key={server.id}
                                      className="flex cursor-pointer items-center gap-2.5 px-3 py-2"
                                      data-tauri-drag-region="false"
                                    >
                                      <input
                                        type="checkbox"
                                        checked={cliSelected.has(server.id)}
                                        onChange={() => toggleCliSelected(server.id)}
                                        className="size-3.5 shrink-0 accent-[#2f6ff0]"
                                      />
                                      <div className="min-w-0 flex-1">
                                        <div className="flex items-center gap-2">
                                          <span className="truncate text-[12.5px] font-medium text-neutral-800 dark:text-neutral-100">{server.name}</span>
                                          <span className="shrink-0 rounded bg-neutral-100 px-1.5 py-0.5 text-[10px] text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400">{isHttp ? 'http' : 'stdio'}</span>
                                        </div>
                                        <div className="truncate font-mono text-[10.5px] text-neutral-400">
                                          {isHttp ? server.url : [server.command, ...server.args].filter(Boolean).join(' ')}
                                        </div>
                                      </div>
                                    </label>
                                  )
                                })}
                              </div>
                            )
                          )}
                        </div>
                      )
                    })}
                    <Button onClick={() => void handleCliImportSelected()} disabled={cliSelected.size === 0} data-tauri-drag-region="false">
                      {t.chatMcpCliImportSelected.replace('{n}', String(cliSelected.size))}
                    </Button>
                  </div>
                )}

                {cliImportDone && (
                  <div className="mt-2 text-[12px] text-emerald-600 dark:text-emerald-400">{cliImportDone}</div>
                )}
              </div>
            </div>
          ) : view === 'advanced' ? (
            <div key="advanced" className="chat-motion-tab-in mt-5 space-y-6">
              <section>
                <div className="mb-2 text-[13px] font-semibold text-neutral-800 dark:text-neutral-100">{t.chatMcpNativeToolsTitle}</div>
                <p className="mb-3 text-[12px] text-neutral-500 dark:text-neutral-400">
                  {t.chatMcpNativeToolsDesc}
                </p>
                <div className="overflow-hidden rounded-md border border-neutral-200 dark:border-neutral-800 [&>*+*]:border-t [&>*+*]:border-neutral-100 dark:[&>*+*]:border-neutral-800/70">
                  {NATIVE_TOOLS.map((tool) => (
                    <div key={tool.key} className="flex items-center justify-between px-4 py-2.5">
                      <span className="text-[13px] text-neutral-800 dark:text-neutral-100">{tool.label}</span>
                      <Toggle
                        checked={tool.defaultOn ? nativeTools[tool.key] !== false : nativeTools[tool.key] === true}
                        onChange={(checked) => updateNativeTools({ [tool.key]: checked } as Partial<ChatNativeToolsConfig>)}
                      />
                    </div>
                  ))}
                </div>
              </section>

              <section>
                <div className="mb-3 text-[13px] font-semibold text-neutral-800 dark:text-neutral-100">{t.chatMcpToolRuntimeTitle}</div>
                <div className="flex items-center justify-between rounded-md border border-neutral-200 px-4 py-3 dark:border-neutral-800">
                  <span className="text-[13px] text-neutral-800 dark:text-neutral-100">{t.chatMcpEnableMcp}</span>
                  <Toggle checked={chatTools.enabled} onChange={(enabled) => persistChatTools({ enabled })} />
                </div>
                <div className="mt-4 grid grid-cols-[repeat(auto-fit,minmax(190px,1fr))] items-stretch gap-x-4 gap-y-5">
                  {renderRuntimeSelect(t.chatMcpApprovalPolicy, chatTools.approvalPolicy || 'auto', (approvalPolicy) => persistChatTools({ approvalPolicy }), [
                    { value: 'readonly_auto_sensitive_confirm', label: t.chatMcpApprovalOnce },
                    { value: 'always_confirm', label: t.chatMcpApprovalAlways },
                    { value: 'auto', label: t.chatMcpApprovalAuto },
                  ])}
                  {renderRuntimeSelect(
                    t.chatMcpMaxToolRounds,
                    chatTools.maxToolRounds === null ? 'unlimited' : String(clampToolRounds(chatTools.maxToolRounds)),
                    (value) => persistChatTools({ maxToolRounds: value === 'unlimited' ? null : clampToolRounds(value) }),
                    [
                      ...CHAT_TOOL_ROUND_PRESETS.map((rounds) => ({ value: String(rounds), label: formatToolRoundsLabel(rounds, lang) })),
                      { value: 'unlimited', label: t.chatMcpUnlimited },
                    ],
                  )}
                  {renderRuntimeSelect(
                    t.chatMcpSubagentConcurrency,
                    String(clampSubAgentConcurrency(chatTools.subAgentConcurrency)),
                    (value) => persistChatTools({ subAgentConcurrency: clampSubAgentConcurrency(value) }),
                    SUB_AGENT_CONCURRENCY_PRESETS.map((n) => ({ value: String(n), label: String(n) })),
                  )}
                  {renderRuntimeSelect(
                    t.chatMcpToolTimeout,
                    String(clampToolTimeoutMs(chatTools.toolTimeoutMs)),
                    (value) => persistChatTools({ toolTimeoutMs: clampToolTimeoutMs(value) }),
                    CHAT_TOOL_TIMEOUT_PRESETS_MS.map((ms) => ({ value: String(ms), label: formatToolTimeoutLabel(ms, lang) })),
                    t.chatMcpToolTimeoutDesc,
                  )}
                  {renderRuntimeSelect(
                    t.chatMcpIdleTimeout,
                    String(clampMcpIdleTimeoutMs(chatTools.mcpIdleTimeoutMs)),
                    (value) => persistChatTools({ mcpIdleTimeoutMs: clampMcpIdleTimeoutMs(value) }),
                    MCP_IDLE_TIMEOUT_PRESETS_MS.map((ms) => ({ value: String(ms), label: formatToolTimeoutLabel(ms, lang) })),
                    t.chatMcpIdleTimeoutDesc,
                  )}
                </div>
              </section>
            </div>
          ) : (
            <div key="installed" className="chat-motion-tab-in mt-5">
              {loading ? (
                <div className="space-y-2">
                  {Array.from({ length: 3 }, (_, i) => (
                    <div key={i} className="rounded-xl border border-neutral-200/80 px-4 py-3 dark:border-neutral-800/70">
                      <div className="kv-skeleton h-4 w-1/4 rounded" />
                      <div className="kv-skeleton mt-2 h-3 w-1/2 rounded" />
                    </div>
                  ))}
                </div>
              ) : listedServers.length === 0 ? (
                <div className="grid min-h-[220px] place-items-center rounded-md border border-dashed border-neutral-200 px-6 text-center text-[13px] text-neutral-400 dark:border-neutral-800">
                  {t.chatMcpNoServers}
                </div>
              ) : (
                <div className="space-y-2">
                  {listedServers.map((server, idx) => {
                    const expanded = expandedId === server.id
                    const isHttp = server.transport === 'streamable_http'
                    const pluginManaged = isPluginManagedServer(server)
                    const feedback = testFeedback[server.id]
                    return (
                      <div
                        key={server.id}
                        style={{ '--chat-motion-delay': `${Math.min(idx, 8) * 24}ms` } as CSSProperties}
                        className="chat-motion-fade-up overflow-hidden rounded-xl border border-neutral-200 bg-white shadow-sm transition-[border-color,box-shadow] duration-[var(--kv-dur-fast)] hover:border-neutral-300 dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700"
                      >
                        <div className="flex items-center gap-3 px-4 py-3">
                          <button
                            type="button"
                            className="flex min-w-0 flex-1 items-center gap-2 text-left"
                            onClick={() => setExpandedId(expanded ? null : server.id)}
                            data-tauri-drag-region="false"
                          >
                            <ChevronDown size={15} className={`shrink-0 text-neutral-400 transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] ${expanded ? 'rotate-180' : ''}`} />
                            <div className="min-w-0">
                              <div className="flex items-center gap-2">
                                <span className="truncate text-[13.5px] font-medium">{server.name}</span>
                                <span className="shrink-0 rounded bg-neutral-100 px-1.5 py-0.5 text-[10px] text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400">{isHttp ? 'http' : 'stdio'}</span>
                                {pluginManaged && (
                                  <span className="shrink-0 rounded bg-sky-50 px-1.5 py-0.5 text-[10px] text-sky-700 dark:bg-sky-950/40 dark:text-sky-300">{t.chatSkillSourcePlugin}</span>
                                )}
                              </div>
                              <div className="mt-0.5 flex items-center gap-3">
                                {server.enabled ? <StatusDot state={states[server.id]} /> : <span className="text-[11.5px] text-neutral-400">{t.chatMcpDisabled}</span>}
                                <span className="truncate font-mono text-[10.5px] text-neutral-400">{isHttp ? server.url : [server.command, ...server.args].filter(Boolean).join(' ')}</span>
                              </div>
                            </div>
                          </button>
                          <span title={pluginManaged ? t.chatMcpPluginNote : undefined}>
                            <Toggle
                              checked={server.enabled}
                              disabled={pluginManaged}
                              onChange={(enabled) => toggleServerEnabled(server.id, enabled)}
                              ariaLabel={pluginManaged ? t.chatMcpPluginNote : undefined}
                            />
                          </span>
                          {pluginManaged ? null : (
                            <IconButton size="sm" variant="danger" label={t.chatDelete} onClick={() => void mutateServers((list) => list.filter((s) => s.id !== server.id))} data-tauri-drag-region="false">
                              <Trash2 size={14} />
                            </IconButton>
                          )}
                        </div>

                        {expanded && (
                          <div className="chat-motion-search-reveal space-y-3 border-t border-neutral-100 px-4 py-3 dark:border-neutral-800/70">
                            {pluginManaged ? (
                              <>
                                <p className="text-[12px] leading-relaxed text-neutral-500 dark:text-neutral-400">
                                  {t.chatMcpPluginNote}
                                </p>
                                <div className="font-mono text-[12px] text-neutral-500 dark:text-neutral-400">
                                  {isHttp ? server.url : [server.command, ...server.args].filter(Boolean).join(' ')}
                                </div>
                                <div className="flex flex-wrap items-center gap-2">
                                  <Button size="sm" onClick={() => void handleTest(server)} disabled={testingId === server.id} data-tauri-drag-region="false">
                                    {testingId === server.id ? <Loader2 size={12} className="animate-spin" /> : t.chatMcpTestConnection}
                                  </Button>
                                </div>
                                {feedback && (
                                  <div className={`text-[12px] ${feedback.ok ? 'text-emerald-600 dark:text-emerald-400' : 'text-red-600 dark:text-red-400'}`}>{feedback.message}</div>
                                )}
                              </>
                            ) : (
                              <>
                            <div>
                              <label className="mb-1 block text-[11.5px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatMcpName}</label>
                              <Input value={server.name} onChange={(name) => updateServer(server.id, { name })} />
                            </div>
                            <div>
                              <label className="mb-1 block text-[11.5px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatMcpTransport}</label>
                              <Select
                                value={server.transport === 'streamable_http' ? 'streamable_http' : 'stdio'}
                                onChange={(transport) => updateServer(server.id, { transport })}
                                options={[{ value: 'stdio', label: t.chatMcpTransportStdio }, { value: 'streamable_http', label: t.chatMcpTransportHttp }]}
                              />
                            </div>
                            {isHttp ? (
                              <div>
                                <label className="mb-1 block text-[11.5px] font-medium text-neutral-600 dark:text-neutral-300">URL</label>
                                <Input mono value={server.url} onChange={(url) => updateServer(server.id, { url })} />
                              </div>
                            ) : (
                              <>
                                <div>
                                  <label className="mb-1 block text-[11.5px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatMcpCommand}</label>
                                  <Input mono value={server.command} onChange={(command) => updateServer(server.id, { command })} placeholder="npx" />
                                </div>
                                <div>
                                  <label className="mb-1 block text-[11.5px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatMcpArgsLabel}</label>
                                  <textarea className={TEXTAREA_CLASS} rows={2} value={argsToText(server.args)} onChange={(e) => updateServer(server.id, { args: textToArgs(e.target.value) })} data-tauri-drag-region="false" />
                                </div>
                              </>
                            )}
                            <div>
                              <label className="mb-1 block text-[11.5px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatMcpEnvLabel}</label>
                              <textarea className={TEXTAREA_CLASS} rows={2} value={envToText(server.env)} onChange={(e) => updateServer(server.id, { env: textToEnv(e.target.value) })} data-tauri-drag-region="false" />
                            </div>
                            <div>
                              <label className="mb-1 block text-[11.5px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatMcpHeadersLabel}</label>
                              <textarea className={TEXTAREA_CLASS} rows={2} value={envToText(server.headers)} onChange={(e) => updateServer(server.id, { headers: textToEnv(e.target.value) })} data-tauri-drag-region="false" />
                            </div>
                            <div className="flex flex-wrap items-center gap-2">
                              <Button size="sm" onClick={() => void handleTest(server)} disabled={testingId === server.id} data-tauri-drag-region="false">
                                {testingId === server.id ? <Loader2 size={12} className="animate-spin" /> : t.chatMcpTestConnection}
                              </Button>
                              {isHttp && (
                                <Button size="sm" variant="ghost" onClick={() => void handleOauth(server)} disabled={oauthId === server.id} data-tauri-drag-region="false">
                                  {oauthId === server.id ? <Loader2 size={12} className="animate-spin" /> : t.chatMcpOauthAuthorize}
                                </Button>
                              )}
                            </div>
                            {feedback && (
                              <div className={`text-[12px] ${feedback.ok ? 'text-emerald-600 dark:text-emerald-400' : 'text-red-600 dark:text-red-400'}`}>{feedback.message}</div>
                            )}
                              </>
                            )}
                          </div>
                        )}
                      </div>
                    )
                  })}
                </div>
              )}
            </div>
          )}
        </div>
      </main>
    </div>
  )
}
