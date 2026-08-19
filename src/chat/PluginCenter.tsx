import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ExternalLink,
  FileSpreadsheet,
  Loader2,
  Monitor,
  Puzzle,
  RefreshCw,
  Sparkles,
  Terminal,
  Trash2,
} from 'lucide-react'
import {
  api,
  isTauriRuntime,
  type PluginStatus,
} from '../api/tauri'
import { refreshSettings } from '../api/settingsCache'
import { Button, IconButton } from '../components/Button'
import { Toggle } from '../settings/components'
import { useT } from '../settings/i18n'

interface PluginCenterProps {
  /** 让 Kivio AI 按规范文档安装：父级开新对话并发送 install brief */
  onRequestAiInstall?: (pluginId: string) => void | Promise<void>
}

type TabId = 'plaza' | 'installed'

function PluginCard({
  plugin,
  busy,
  installBusy,
  onRunInstall,
  onAiInstall,
  onToggleEnabled,
  onUninstall,
}: {
  plugin: PluginStatus
  busy: boolean
  installBusy: boolean
  onRunInstall: (id: string) => void
  onAiInstall: (id: string) => void
  onToggleEnabled: (id: string, enabled: boolean) => void
  onUninstall: (id: string) => void
}) {
  const t = useT()
  const canInstall = plugin.canInstall === true

  return (
    <article className="chat-motion-fade-up flex min-w-0 flex-col gap-3 rounded-xl border border-neutral-200 bg-white p-5 shadow-sm transition-[border-color,box-shadow] duration-[var(--kv-dur-fast)] hover:border-neutral-300 dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700">
      <div className="flex min-w-0 items-start gap-3">
        <span className="grid size-9 shrink-0 place-items-center rounded-md border border-neutral-200 bg-neutral-50 text-neutral-600 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-300">
          {plugin.id === 'officecli' ? (
            <FileSpreadsheet size={18} strokeWidth={1.75} />
          ) : plugin.id === 'cua-driver' ? (
            <Monitor size={18} strokeWidth={1.75} />
          ) : (
            <Terminal size={18} strokeWidth={1.75} />
          )}
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <h3 className="truncate text-[16px] font-semibold text-neutral-900 dark:text-neutral-50">
              {plugin.name}
            </h3>
            <span className="rounded-md bg-neutral-100 px-1.5 py-0.5 text-[11px] font-medium text-neutral-600 dark:bg-neutral-800 dark:text-neutral-300">
              CLI
            </span>
            {plugin.installed ? (
              <span
                className={`rounded-md px-1.5 py-0.5 text-[11px] font-medium ${
                  plugin.enabled
                    ? 'bg-emerald-50 text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300'
                    : 'bg-sky-50 text-sky-700 dark:bg-sky-950/40 dark:text-sky-300'
                }`}
              >
                {plugin.enabled ? t.chatPluginEnabled : t.chatPluginDetectedNotEnabled}
              </span>
            ) : (
              <span className="rounded-md bg-amber-50 px-1.5 py-0.5 text-[11px] font-medium text-amber-700 dark:bg-amber-950/40 dark:text-amber-300">
                {t.chatPluginNotDetected}
              </span>
            )}
            {plugin.version && (
              <span className="text-[11px] text-neutral-400">v{plugin.version}</span>
            )}
          </div>
          <p className="mt-1.5 text-[13px] leading-relaxed text-neutral-500 dark:text-neutral-400">
            {plugin.description}
          </p>
          {/* 配置数量：与专家套件「0 MCP · 3 技能」同一信息层级 */}
          <div className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12px] text-neutral-500 dark:text-neutral-400">
            <span>
              <span className="font-semibold tabular-nums text-neutral-800 dark:text-neutral-200">
                {plugin.mcpCount ?? (plugin.hasMcp ? 1 : 0)}
              </span>
              {' '}MCP
              {plugin.mcpActive ? (
                <span className="text-emerald-600 dark:text-emerald-400">{t.chatPluginMcpRegistered}</span>
              ) : plugin.enabled && (plugin.mcpCount ?? 0) > 0 ? (
                <span className="text-amber-600 dark:text-amber-400">{t.chatPluginMcpPending}</span>
              ) : null}
            </span>
            <span className="text-neutral-300 dark:text-neutral-600">·</span>
            <span>
              <span className="font-semibold tabular-nums text-neutral-800 dark:text-neutral-200">
                {plugin.skillCount ?? plugin.skillIds?.length ?? 0}
              </span>
              {' '}Skill
              {plugin.skillActive ? (
                <span className="text-emerald-600 dark:text-emerald-400">{t.chatPluginSkillInjected}</span>
              ) : plugin.enabled && (plugin.skillCount ?? 0) > 0 ? (
                <span className="text-amber-600 dark:text-amber-400">{t.chatPluginSkillPending}</span>
              ) : null}
            </span>
            {(plugin.skillIds?.length ?? 0) > 0 && (
              <span className="text-neutral-400 dark:text-neutral-500">
                {t.chatPluginSkillIdsWrap.replace('{names}', plugin.skillIds.join(', '))}
              </span>
            )}
            {plugin.mcpServerId && (
              <span className="font-mono text-[11px] text-neutral-400">
                {plugin.mcpServerId}
              </span>
            )}
          </div>
          <div className="mt-2 flex flex-wrap gap-1.5">
            {plugin.tags.map((tag) => (
              <span
                key={tag}
                className="rounded-md border border-neutral-200 px-1.5 py-0.5 text-[11px] text-neutral-500 dark:border-neutral-700 dark:text-neutral-400"
              >
                {tag}
              </span>
            ))}
          </div>
          {plugin.path && (
            <p className="mt-2 truncate font-mono text-[11px] text-neutral-400" title={plugin.path}>
              {plugin.path}
              {plugin.source === 'system'
                ? t.chatPluginSourceSystemPath
                : plugin.source === 'kivio'
                  ? t.chatPluginSourceKivio
                  : ''}
            </p>
          )}
        </div>

        {plugin.installed && (
          <div className="flex shrink-0 flex-col items-end gap-1 pt-0.5">
            <span className="text-[11px] text-neutral-400">{t.chatPluginEnable}</span>
            <Toggle
              checked={plugin.enabled}
              disabled={busy}
              onChange={(next) => onToggleEnabled(plugin.id, next)}
              ariaLabel={t.chatPluginEnableNamed.replace('{name}', plugin.name)}
            />
          </div>
        )}
      </div>

      {!plugin.installed && !canInstall ? (
        <p className="text-[12px] leading-relaxed text-neutral-400 dark:text-neutral-500">
          {t.chatPluginInstallUnavailable}
        </p>
      ) : null}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          disabled={!canInstall || installBusy || busy}
          onClick={() => onRunInstall(plugin.id)}
          title={t.chatPluginRunInstallTitle}
        >
          {installBusy ? <Loader2 size={14} className="animate-spin" /> : <Terminal size={14} />}
          {installBusy
            ? t.chatPluginInstalling
            : plugin.installed
              ? t.chatPluginRunReinstall
              : t.chatPluginRunInstall}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void api.openExternal(plugin.repo)}>
          <ExternalLink size={14} />
          GitHub
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={installBusy || busy}
          onClick={() => onAiInstall(plugin.id)}
          title={t.chatPluginAiInstallTitle}
        >
          {installBusy ? <Loader2 size={14} className="animate-spin" /> : <Sparkles size={14} />}
          {plugin.installed ? t.chatPluginAiReinstall : t.chatPluginAiInstall}
        </Button>
        {plugin.installed && (
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => onUninstall(plugin.id)}
            title={t.chatPluginUninstallTitle}
            className="text-red-600 hover:bg-red-50 hover:text-red-700 dark:text-red-400 dark:hover:bg-red-950/40 dark:hover:text-red-300"
          >
            <Trash2 size={14} />
            {t.chatPluginUninstall}
          </Button>
        )}
      </div>
      {plugin.installed && !plugin.enabled && (
        <p className="text-[12px] leading-relaxed text-neutral-400 dark:text-neutral-500">
          {t.chatPluginDetectedNotEnabledLead}
          <strong className="font-medium text-neutral-600 dark:text-neutral-300">{t.chatPluginEnable}</strong>
          {t.chatPluginDetectedNotEnabledTail}
          {plugin.hasSkill ? t.chatPluginAutoInjectSkill : ''}
          {plugin.hasMcp ? t.chatPluginAutoRegisterMcp : ''}
          {t.chatPluginAutoSystemPrompt}
        </p>
      )}
      {plugin.enabled && (
        <p className="text-[12px] leading-relaxed text-neutral-500 dark:text-neutral-400">
          {t.chatPluginEnabled}
          {plugin.skillActive ? t.chatPluginSkillReady : ''}
          {plugin.mcpActive ? t.chatPluginMcpWritten : plugin.hasMcp ? t.chatPluginMcpRegisterRetry : ''}
          {t.chatPluginEnabledTail}
        </p>
      )}
    </article>
  )
}

/** 插件中心：安装 / 启用开关控制 MCP / Skill。 */
export function PluginCenter({ onRequestAiInstall }: PluginCenterProps) {
  const t = useT()
  const [tab, setTab] = useState<TabId>('plaza')
  const [plugins, setPlugins] = useState<PluginStatus[]>([])
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState('')
  const [statusMsg, setStatusMsg] = useState('')
  const [busyId, setBusyId] = useState<string | null>(null)
  const [installBusyId, setInstallBusyId] = useState<string | null>(null)

  // 打开页面：只读缓存态（无子进程，~2ms 秒开）。缓存来自 meta.json，对已装/启用的插件已准确。
  const loadCached = useCallback(async () => {
    if (!isTauriRuntime()) {
      setPlugins([])
      setLoading(false)
      setError(t.chatPluginRequiresApp)
      return
    }
    setError('')
    try {
      const cached = await api.pluginsListCached()
      setPlugins(cached)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setPlugins([])
    } finally {
      setLoading(false)
    }
  }, [t])

  // 完整探测（which/--version 子进程，较慢）：仅手动刷新 / 启用 / 卸载后调用，覆盖为精确状态。
  const refresh = useCallback(async () => {
    if (!isTauriRuntime()) return
    setRefreshing(true)
    setError('')
    try {
      const list = await api.pluginsList()
      setPlugins(list)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setRefreshing(false)
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadCached()
  }, [loadCached])

  const patchStatus = useCallback((status: PluginStatus) => {
    setPlugins((prev) => prev.map((p) => (p.id === status.id ? status : p)))
  }, [])

  const handleRunInstall = useCallback(
    async (id: string) => {
      setInstallBusyId(id)
      setError('')
      setStatusMsg('')
      try {
        const result = await api.pluginsRunOfficialInstall(id)
        await refresh()
        setStatusMsg(result.message || '')
      } catch (err) {
        setStatusMsg('')
        setError(err instanceof Error ? err.message : String(err))
        await refresh()
      } finally {
        setInstallBusyId(null)
      }
    },
    [refresh],
  )

  const handleAiInstall = useCallback(
    async (id: string) => {
      if (!onRequestAiInstall) {
        setError(t.chatPluginAiInstallUnavailable)
        return
      }
      setInstallBusyId(id)
      setError('')
      try {
        await onRequestAiInstall(id)
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      } finally {
        setInstallBusyId(null)
      }
    },
    [onRequestAiInstall, t],
  )

  const handleToggle = useCallback(
    async (id: string, enabled: boolean) => {
      setBusyId(id)
      setError('')
      try {
        const result = await api.pluginsSetEnabled(id, enabled)
        patchStatus(result.status)
        try {
          await refreshSettings()
        } catch {
          /* ignore */
        }
        setStatusMsg(result.message || '')
        setError('')
      } catch (err) {
        setStatusMsg('')
        setError(err instanceof Error ? err.message : String(err))
        await refresh()
      } finally {
        setBusyId(null)
      }
    },
    [patchStatus, refresh],
  )

  const handleUninstall = useCallback(
    async (id: string) => {
      const plugin = plugins.find((p) => p.id === id)
      const name = plugin?.name ?? id
      const ok = window.confirm(
        t.chatPluginUninstallConfirmTitle.replace('{name}', name) +
          '\n\n' +
          t.chatPluginUninstallWillDelete +
          '\n' +
          t.chatPluginUninstallItemState +
          '\n' +
          t.chatPluginUninstallItemBinary +
          '\n' +
          t.chatPluginUninstallItemSkills +
          '\n\n' +
          t.chatPluginUninstallIrreversible,
      )
      if (!ok) return
      setBusyId(id)
      setError('')
      setStatusMsg('')
      try {
        const result = await api.pluginsUninstall(id)
        // 卸载后列表仍可能「已检测到」系统命令；用刷新拿最新 status
        await refresh()
        try {
          await refreshSettings()
        } catch {
          /* ignore */
        }
        setStatusMsg(result.message || t.chatPluginUninstalledNamed.replace('{name}', name))
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
        await refresh()
      } finally {
        setBusyId(null)
      }
    },
    [plugins, refresh, t],
  )

  const installed = useMemo(() => plugins.filter((p) => p.installed), [plugins])

  const filtered = useMemo(() => {
    return tab === 'plaza' ? plugins : installed
  }, [plugins, installed, tab])

  const body = (
    <>
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <p className="max-w-2xl text-[13px] leading-relaxed text-neutral-500 dark:text-neutral-400">
          {t.chatPluginIntro}
        </p>
        <IconButton size="md" label={t.chatPluginRefreshDetection} onClick={() => void refresh()} disabled={refreshing}>
          <RefreshCw size={15} className={refreshing ? 'animate-spin' : ''} />
        </IconButton>
      </div>

      <div className="mt-1 flex flex-wrap items-center gap-3">
        <div className="flex items-center gap-1 rounded-lg bg-neutral-100 p-0.5 dark:bg-neutral-800/80">
          {(
            [
              ['plaza', t.chatPluginPlaza, plugins.length],
              ['installed', t.chatPluginDetectedTab, installed.length],
            ] as const
          ).map(([id, label, count]) => (
            <button
              key={id}
              type="button"
              onClick={() => setTab(id)}
              className={`rounded-md px-3 py-1.5 text-[13px] transition-colors ${
                tab === id
                  ? 'bg-white font-medium text-neutral-900 shadow-sm dark:bg-neutral-900 dark:text-neutral-50'
                  : 'text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200'
              }`}
            >
              {label}
              <span className="ml-1.5 text-neutral-400">{count}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="mt-4 rounded-md border border-dashed border-neutral-200 bg-neutral-50/80 px-4 py-3 text-[12.5px] leading-relaxed text-neutral-500 dark:border-neutral-800 dark:bg-neutral-900/40 dark:text-neutral-400">
        <span className="font-medium text-neutral-700 dark:text-neutral-300">{t.chatPluginFlowLabel}</span>
        {t.chatPluginFlowDesc}
      </div>

      {error && (
        <div className="mt-4 rounded-md border border-red-200 bg-red-50 px-4 py-3 text-[13px] text-red-700 dark:border-red-900/50 dark:bg-red-950/30 dark:text-red-300">
          {error}
        </div>
      )}
      {statusMsg && !error && (
        <div className="mt-4 rounded-md border border-emerald-200 bg-emerald-50 px-4 py-3 text-[13px] text-emerald-800 dark:border-emerald-900/50 dark:bg-emerald-950/30 dark:text-emerald-200">
          {statusMsg}
        </div>
      )}

      {loading && plugins.length === 0 ? (
        <div className="mt-6 grid gap-4">
          {Array.from({ length: 2 }, (_, i) => (
            <div key={i} className="rounded-xl border border-neutral-200/80 p-5 dark:border-neutral-700/70">
              <div className="kv-skeleton h-4 w-1/4 rounded" />
              <div className="kv-skeleton mt-2.5 h-3 w-3/4 rounded" />
              <div className="kv-skeleton mt-3 h-7 w-40 rounded" />
            </div>
          ))}
        </div>
      ) : filtered.length === 0 ? (
        <div className="mt-10 flex flex-col items-center justify-center text-center">
          <div className="flex h-14 w-14 items-center justify-center rounded-md bg-neutral-100 text-neutral-400 dark:bg-neutral-800 dark:text-neutral-500">
            <Puzzle size={28} strokeWidth={1.5} />
          </div>
          <p className="mt-4 text-[15px] font-medium text-neutral-700 dark:text-neutral-200">
            {tab === 'installed' ? t.chatPluginEmptyInstalled : t.chatPluginEmptyNoMatch}
          </p>
        </div>
      ) : (
        <div key={tab} className="chat-motion-tab-in mt-6 grid gap-4">
          {filtered.map((plugin) => (
            <PluginCard
              key={plugin.id}
              plugin={plugin}
              busy={busyId === plugin.id || installBusyId === plugin.id}
              installBusy={installBusyId === plugin.id}
              onRunInstall={(id) => void handleRunInstall(id)}
              onAiInstall={(id) => void handleAiInstall(id)}
              onToggleEnabled={(id, enabled) => void handleToggle(id, enabled)}
              onUninstall={(id) => void handleUninstall(id)}
            />
          ))}
        </div>
      )}
    </>
  )

  return <div className="min-w-0 text-neutral-900 dark:text-neutral-100">{body}</div>
}
