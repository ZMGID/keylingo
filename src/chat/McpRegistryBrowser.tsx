// MCP 注册表内联浏览（无 modal 外壳，供 McpCenter「市场」tab 用）。浏览/搜索/翻页三源，
// 一键把服务器交给 onInstall。needs_config 的条目在卡片下方内联展开填参，不弹二级窗口。
// 数据层见 ../settings/mcpRegistry.ts。

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { Check, ExternalLink, Loader2, Search } from 'lucide-react'
import type { ChatMcpServer } from '../api/tauri'
import { api } from '../api/tauri'
import { Button, IconButton } from '../components/Button'
import { Input } from '../settings/components'
import { useT } from '../settings/i18n'
import {
  applyMcpRegistryInstallConfig,
  MCP_REGISTRY_SOURCE_OPTIONS,
  mcpRegistryConfigInputKey,
  resolveMcpRegistryInstallDraft,
  searchMcpRegistry,
  withUniqueMcpServerId,
  type McpRegistryCard,
  type McpRegistryInstallDraft,
  type McpRegistrySource,
} from '../settings/mcpRegistry'

type Props = {
  existingServers: ChatMcpServer[]
  onInstall: (server: ChatMcpServer) => void
}

type ConfigState = { card: McpRegistryCard; draft: McpRegistryInstallDraft; values: Record<string, string> }

const PAGE_LIMIT = 24

export function McpRegistryBrowser({ existingServers, onInstall }: Props) {
  const t = useT()
  const [source, setSource] = useState<McpRegistrySource>('official')
  const [queryInput, setQueryInput] = useState('')
  const [query, setQuery] = useState('')
  const [items, setItems] = useState<McpRegistryCard[]>([])
  const [cursor, setCursor] = useState<string | undefined>(undefined)
  const [loading, setLoading] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState('')
  const [config, setConfig] = useState<ConfigState | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [installedIds, setInstalledIds] = useState<ReadonlySet<string>>(new Set())

  useEffect(() => {
    const timer = setTimeout(() => setQuery(queryInput.trim()), 400)
    return () => clearTimeout(timer)
  }, [queryInput])

  const reqSeq = useRef(0)
  useEffect(() => {
    const reqId = ++reqSeq.current
    setLoading(true)
    setError('')
    setItems([])
    setCursor(undefined)
    setConfig(null)
    searchMcpRegistry({ source, query: query || undefined, limit: PAGE_LIMIT })
      .then((result) => {
        if (reqId !== reqSeq.current) return
        setItems(result.items)
        setCursor(result.nextCursor)
      })
      .catch((err) => {
        if (reqId !== reqSeq.current) return
        setError(err instanceof Error ? err.message : String(err))
      })
      .finally(() => {
        if (reqId === reqSeq.current) setLoading(false)
      })
  }, [source, query])

  const loadMore = useCallback(() => {
    if (!cursor || loadingMore) return
    const reqId = reqSeq.current
    setLoadingMore(true)
    searchMcpRegistry({ source, query: query || undefined, cursor, limit: PAGE_LIMIT })
      .then((result) => {
        if (reqId !== reqSeq.current) return
        setItems((prev) => [...prev, ...result.items])
        setCursor(result.nextCursor)
      })
      .catch((err) => {
        if (reqId !== reqSeq.current) return
        setError(err instanceof Error ? err.message : String(err))
      })
      .finally(() => {
        if (reqId === reqSeq.current) setLoadingMore(false)
      })
  }, [cursor, loadingMore, source, query])

  const commitServer = useCallback(
    (draft: McpRegistryInstallDraft, cardId: string) => {
      const unique = withUniqueMcpServerId(draft, existingServers)
      onInstall(unique.server)
      setInstalledIds((prev) => new Set(prev).add(cardId))
    },
    [existingServers, onInstall],
  )

  const handleInstall = useCallback(
    async (card: McpRegistryCard) => {
      setBusyId(card.id)
      setError('')
      try {
        const resolved = await resolveMcpRegistryInstallDraft(card)
        const draft = resolved.installDraft ?? resolved.manualDraft
        if (!draft) {
          setError(t.chatMcpAutoInstallFailed)
          return
        }
        if (draft.status === 'needs_config') {
          const values: Record<string, string> = {}
          for (const input of draft.requiredConfig) values[mcpRegistryConfigInputKey(input)] = ''
          setConfig({ card: resolved, draft, values })
          return
        }
        commitServer(draft, card.id)
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      } finally {
        setBusyId(null)
      }
    },
    [commitServer, t],
  )

  const configReady = useMemo(() => {
    if (!config) return false
    return config.draft.requiredConfig
      .filter((input) => input.required)
      .every((input) => (config.values[mcpRegistryConfigInputKey(input)] ?? '').trim())
  }, [config])

  const submitConfig = useCallback(() => {
    if (!config) return
    const ready = applyMcpRegistryInstallConfig(config.draft, config.values)
    commitServer(ready, config.card.id)
    setConfig(null)
  }, [config, commitServer])

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* 工具行：搜索为主（与技能商店同规格），来源切换退居右侧分段控件 */}
      <div className="flex items-center gap-2 pb-4">
        <div className="relative min-w-0 flex-1">
          <Search size={16} className="pointer-events-none absolute left-3.5 top-1/2 -translate-y-1/2 text-neutral-400" />
          <input
            type="text"
            value={queryInput}
            onChange={(e) => setQueryInput(e.target.value)}
            placeholder={t.chatMcpSearchPlaceholder}
            className="h-10 w-full rounded-md border border-neutral-200 bg-white pl-10 pr-4 text-[14px] outline-none placeholder:text-neutral-400 focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
            data-tauri-drag-region="false"
          />
        </div>
        <div className="inline-flex h-10 shrink-0 items-center rounded-md border border-neutral-200 p-1 dark:border-neutral-700">
          {MCP_REGISTRY_SOURCE_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              type="button"
              onClick={() => setSource(opt.value)}
              data-tauri-drag-region="false"
              className={`rounded px-3 py-1 text-[12.5px] font-medium transition-colors duration-[var(--kv-dur-fast)] ${
                source === opt.value
                  ? 'bg-neutral-100 text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100'
                  : 'text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200'
              }`}
            >
              {opt.label}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="mb-3 rounded-md border border-red-300/60 bg-red-50 px-3 py-2 text-[12px] text-red-700 dark:border-red-800/60 dark:bg-red-950/40 dark:text-red-300">
          {error}
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto">
        {loading ? (
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
            {Array.from({ length: 6 }, (_, i) => (
              <div key={i} className="rounded-xl border border-neutral-200/80 p-3.5 dark:border-neutral-800/70">
                <div className="kv-skeleton h-4 w-2/5 rounded" />
                <div className="kv-skeleton mt-2.5 h-3 w-full rounded" />
                <div className="kv-skeleton mt-1.5 h-3 w-3/5 rounded" />
              </div>
            ))}
          </div>
        ) : items.length === 0 ? (
          <div className="flex h-40 items-center justify-center text-[13px] text-neutral-400">{t.chatMcpNoMatch}</div>
        ) : (
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
            {items.map((card, idx) => {
              const installed = installedIds.has(card.id)
              const configuring = config?.card.id === card.id
              return (
                <div
                  key={card.id}
                  style={{ '--chat-motion-delay': `${Math.min(idx % PAGE_LIMIT, 8) * 24}ms` } as CSSProperties}
                  className="chat-motion-fade-up group rounded-xl border border-neutral-200 bg-white p-3.5 shadow-sm transition-[border-color,box-shadow,transform] duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] hover:-translate-y-0.5 hover:border-neutral-300 hover:shadow-md dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700"
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="truncate text-[13.5px] font-semibold leading-tight text-neutral-950 dark:text-neutral-50">{card.displayName}</span>
                        {card.verified && <span className="shrink-0 rounded-full bg-emerald-500/15 px-2 py-0.5 text-[10px] font-medium text-emerald-600 dark:text-emerald-400">{t.chatMcpVerified}</span>}
                        {card.transportHints.map((hint) => (
                          <span key={hint} className="shrink-0 rounded-full bg-neutral-100 px-2 py-0.5 text-[10px] text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400">{hint}</span>
                        ))}
                      </div>
                      <p className="mt-1 line-clamp-2 min-h-[2.4em] text-[12px] leading-[1.45] text-neutral-500 dark:text-neutral-400">
                        {card.description || t.chatMcpNoDescription}
                      </p>
                      <div className="mt-1 truncate font-mono text-[10.5px] text-neutral-400 dark:text-neutral-500">{card.sourceId}</div>
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      {card.detailUrl && (
                        <span className="opacity-0 transition-opacity duration-[var(--kv-dur-fast)] focus-within:opacity-100 group-hover:opacity-100">
                          <IconButton size="sm" variant="ghost" onClick={() => void api.openExternal(card.detailUrl!)} label={t.chatMcpHomepage}>
                            <ExternalLink size={13} />
                          </IconButton>
                        </span>
                      )}
                      {installed ? (
                        <span className="chat-motion-pop inline-flex items-center gap-1 rounded-md bg-emerald-500/15 px-2 py-1 text-[12px] font-medium text-emerald-600 dark:text-emerald-400"><Check size={13} />{t.chatMcpAdded}</span>
                      ) : (
                        <Button size="sm" onClick={() => void handleInstall(card)} disabled={busyId === card.id}>
                          {busyId === card.id ? <Loader2 size={12} className="animate-spin" /> : t.chatMcpAdd}
                        </Button>
                      )}
                    </div>
                  </div>

                  {configuring && config && (
                    <div className="chat-motion-search-reveal mt-3 space-y-2 border-t border-neutral-100 pt-3 dark:border-neutral-800/70">
                      {config.draft.requiredConfig.map((input) => {
                        const key = mcpRegistryConfigInputKey(input)
                        return (
                          <div key={key}>
                            <label className="mb-1 block text-[11.5px] font-medium text-neutral-600 dark:text-neutral-300">
                              {input.label ?? input.name}
                              {input.required && <span className="ml-1 text-red-500">*</span>}
                              <span className="ml-1.5 text-[10px] font-normal text-neutral-400 dark:text-neutral-500">{input.target}</span>
                            </label>
                            <Input
                              value={config.values[key] ?? ''}
                              onChange={(v) => setConfig((prev) => (prev ? { ...prev, values: { ...prev.values, [key]: v } } : prev))}
                              type={input.secret ? 'password' : 'text'}
                              placeholder={input.secret ? '••••••' : ''}
                              mono
                            />
                          </div>
                        )
                      })}
                      <div className="flex justify-end gap-2 pt-1">
                        <Button size="sm" variant="ghost" onClick={() => setConfig(null)}>{t.cancel}</Button>
                        <Button size="sm" onClick={submitConfig} disabled={!configReady}>{t.chatMcpAdd}</Button>
                      </div>
                    </div>
                  )}
                </div>
              )
            })}
          </div>
        )}

        {cursor && !loading && (
          <div className="pt-2">
            <Button size="sm" variant="ghost" onClick={loadMore} disabled={loadingMore} className="w-full">
              {loadingMore ? <Loader2 size={12} className="animate-spin" /> : t.chatMcpLoadMore}
            </Button>
          </div>
        )}
      </div>
    </div>
  )
}
