import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ExternalLink,
  FileSpreadsheet,
  Loader2,
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

interface PluginCenterProps {
  /** 让 Kivio AI 按规范文档安装：父级开新对话并发送 install brief */
  onRequestAiInstall?: (pluginId: string) => void | Promise<void>
}

type TabId = 'plaza' | 'installed'

function PluginCard({
  plugin,
  busy,
  installBusy,
  onAiInstall,
  onToggleEnabled,
  onUninstall,
}: {
  plugin: PluginStatus
  busy: boolean
  installBusy: boolean
  onAiInstall: (id: string) => void
  onToggleEnabled: (id: string, enabled: boolean) => void
  onUninstall: (id: string) => void
}) {
  return (
    <article className="chat-motion-fade-up flex min-w-0 flex-col gap-3 rounded-xl border border-neutral-200 bg-white p-5 shadow-sm transition-[border-color,box-shadow] duration-[var(--kv-dur-fast)] hover:border-neutral-300 dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700">
      <div className="flex min-w-0 items-start gap-3">
        <span className="grid size-9 shrink-0 place-items-center rounded-md border border-neutral-200 bg-neutral-50 text-neutral-600 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-300">
          {plugin.id === 'officecli' ? (
            <FileSpreadsheet size={18} strokeWidth={1.75} />
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
                {plugin.enabled ? '已启用' : '已检测到 · 未启用'}
              </span>
            ) : (
              <span className="rounded-md bg-amber-50 px-1.5 py-0.5 text-[11px] font-medium text-amber-700 dark:bg-amber-950/40 dark:text-amber-300">
                未检测到
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
                <span className="text-emerald-600 dark:text-emerald-400"> · 已注册</span>
              ) : plugin.enabled && (plugin.mcpCount ?? 0) > 0 ? (
                <span className="text-amber-600 dark:text-amber-400"> · 待注册</span>
              ) : null}
            </span>
            <span className="text-neutral-300 dark:text-neutral-600">·</span>
            <span>
              <span className="font-semibold tabular-nums text-neutral-800 dark:text-neutral-200">
                {plugin.skillCount ?? plugin.skillIds?.length ?? 0}
              </span>
              {' '}Skill
              {plugin.skillActive ? (
                <span className="text-emerald-600 dark:text-emerald-400"> · 已注入</span>
              ) : plugin.enabled && (plugin.skillCount ?? 0) > 0 ? (
                <span className="text-amber-600 dark:text-amber-400"> · 待注入</span>
              ) : null}
            </span>
            {(plugin.skillIds?.length ?? 0) > 0 && (
              <span className="text-neutral-400 dark:text-neutral-500">
                （{plugin.skillIds.join(', ')}）
              </span>
            )}
            {plugin.mcpServerId && plugin.mcpActive && (
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
              {plugin.source === 'system' ? ' · 系统 PATH' : plugin.source === 'kivio' ? ' · Kivio 托管' : ''}
            </p>
          )}
        </div>

        {plugin.installed && (
          <div className="flex shrink-0 flex-col items-end gap-1 pt-0.5">
            <span className="text-[11px] text-neutral-400">启用</span>
            <Toggle
              checked={plugin.enabled}
              disabled={busy}
              onChange={(next) => onToggleEnabled(plugin.id, next)}
              ariaLabel={`启用 ${plugin.name}`}
            />
          </div>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          disabled={installBusy || busy}
          onClick={() => onAiInstall(plugin.id)}
          title="开新对话，由 Kivio AI 按安装规范下载安装"
        >
          {installBusy ? <Loader2 size={14} className="animate-spin" /> : <Sparkles size={14} />}
          {plugin.installed ? '让 AI 重装 / 升级' : '让 AI 安装'}
        </Button>
        {plugin.installed && (
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => onUninstall(plugin.id)}
            title="彻底卸载：移除 Kivio 配置、MCP、官方二进制与相关 skills"
            className="text-red-600 hover:bg-red-50 hover:text-red-700 dark:text-red-400 dark:hover:bg-red-950/40 dark:hover:text-red-300"
          >
            <Trash2 size={14} />
            卸载
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={() => void api.openExternal(plugin.repo)}>
          <ExternalLink size={14} />
          GitHub
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void api.openExternal(plugin.homepage)}>
          官网
        </Button>
      </div>

      {!plugin.installed && (
        <p className="text-[12px] leading-relaxed text-neutral-400 dark:text-neutral-500">
          安装由 <strong className="font-medium text-neutral-600 dark:text-neutral-300">Kivio AI</strong>{' '}
          按规范文档执行（终端下载/配置），不是后台静默下载。装好后回来点「刷新」并打开启用开关。
        </p>
      )}
      {plugin.installed && !plugin.enabled && (
        <p className="text-[12px] leading-relaxed text-neutral-400 dark:text-neutral-500">
          已检测到命令，但未启用。打开右侧
          <strong className="font-medium text-neutral-600 dark:text-neutral-300">启用</strong>
          后，Kivio 会自动：
          {plugin.hasSkill ? ' 注入 Skill' : ''}
          {plugin.hasMcp ? ' + 注册 stdio MCP（`officecli mcp`，不是 mcp claude）' : ''}
          + 系统提示。README 里的 `officecli mcp claude/cursor` 不用在 Kivio 里跑。
        </p>
      )}
      {plugin.enabled && (
        <p className="text-[12px] leading-relaxed text-neutral-500 dark:text-neutral-400">
          已启用
          {plugin.skillActive ? ' · Skill 就绪' : ''}
          {plugin.mcpActive ? ' · MCP 已写入设置（stdio）' : plugin.hasMcp ? ' · MCP 未注册成功请关开重试' : ''}
          。新开对话或下一轮 Agent 即可使用；可用终端 `{plugin.binary}` 或 MCP 工具。
        </p>
      )}
    </article>
  )
}

/** 插件中心：检测状态 + 启用开关；安装交给 Kivio AI。 */
export function PluginCenter({ onRequestAiInstall }: PluginCenterProps) {
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
      setError('插件管理需在 Kivio 应用内使用')
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
  }, [])

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

  const handleAiInstall = useCallback(
    async (id: string) => {
      if (!onRequestAiInstall) {
        setError('当前界面未接入 AI 安装入口')
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
    [onRequestAiInstall],
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
        `彻底卸载插件「${name}」？\n\n` +
          '将删除：\n' +
          '· Kivio 中的启用状态、MCP 注册、插件数据\n' +
          '· 本机 officecli 可执行文件与官方安装目录\n' +
          '· 官方写入的 skills / 相关配置（若存在）\n\n' +
          '此操作不可撤销。需要时请重新「让 AI 安装」。',
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
        setStatusMsg(result.message || `已从 Kivio 卸载 ${name}`)
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
        await refresh()
      } finally {
        setBusyId(null)
      }
    },
    [plugins, refresh],
  )

  const installed = useMemo(() => plugins.filter((p) => p.installed), [plugins])

  const filtered = useMemo(() => {
    return tab === 'plaza' ? plugins : installed
  }, [plugins, installed, tab])

  return (
    <div className="assistant-center-root flex h-full min-h-0 flex-col text-neutral-900 dark:text-neutral-100">

      <main className="custom-scrollbar min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[1040px] px-9 pb-10 pt-7">
          <div className="border-b border-neutral-200 pb-5 dark:border-neutral-800">
            <div className="flex min-w-0 items-center gap-2">
              <h1 className="flex items-center gap-2.5 text-[28px] font-semibold tracking-normal text-neutral-950 dark:text-neutral-50">
                <Puzzle size={24} className="text-neutral-500" />
                插件
              </h1>
              <IconButton size="lg" label="刷新检测" onClick={() => void refresh()} disabled={refreshing}>
                <RefreshCw size={16} className={refreshing ? 'animate-spin' : ''} />
              </IconButton>
            </div>
            <p className="mt-3.5 max-w-2xl text-[14px] leading-relaxed text-neutral-500 dark:text-neutral-400">
              安装交给 <strong className="font-medium text-neutral-700 dark:text-neutral-300">Kivio AI</strong>
              ：点「让 AI 安装」会开新对话，并要求 AI
              <strong className="font-medium text-neutral-700 dark:text-neutral-300">先拉取官方 README</strong>
              （安装/用法权威来源），再按其中步骤用终端安装。装好后你回来
              <strong className="font-medium text-neutral-700 dark:text-neutral-300">启用</strong>
              （Skill / MCP 随启用统一开关）。
            </p>
          </div>

          <div className="mt-6 flex flex-wrap items-center gap-3">
            <div className="flex items-center gap-1 rounded-lg bg-neutral-100 p-0.5 dark:bg-neutral-800/80">
              {(
                [
                  ['plaza', '插件广场', plugins.length],
                  ['installed', '已检测', installed.length],
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
            <span className="font-medium text-neutral-700 dark:text-neutral-300">流程：</span>
            让 AI 安装（先读官方 README → 按文档安装）→ 刷新检测 PATH → 打开「启用」→ Agent 才有命令 / Skill / MCP。
            关闭启用则全部卸下。
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
                <div key={i} className="rounded-xl border border-neutral-200/80 p-5 dark:border-neutral-800/70">
                  <div className="kv-skeleton h-4 w-1/4 rounded" />
                  <div className="kv-skeleton mt-2.5 h-3 w-3/4 rounded" />
                  <div className="kv-skeleton mt-3 h-7 w-40 rounded" />
                </div>
              ))}
            </div>
          ) : filtered.length === 0 ? (
            <div className="mt-16 flex flex-col items-center justify-center text-center">
              <div className="flex h-14 w-14 items-center justify-center rounded-md bg-neutral-100 text-neutral-400 dark:bg-neutral-800 dark:text-neutral-500">
                <Puzzle size={28} strokeWidth={1.5} />
              </div>
              <p className="mt-4 text-[15px] font-medium text-neutral-700 dark:text-neutral-200">
                {tab === 'installed' ? '还没有检测到已安装插件' : '没有匹配的插件'}
              </p>
            </div>
          ) : (
            <div key={tab} className="chat-motion-tab-in mt-6 grid gap-4">
              {filtered.map((plugin) => (
                <PluginCard
                  key={plugin.id}
                  plugin={plugin}
                  busy={busyId === plugin.id}
                  installBusy={installBusyId === plugin.id}
                  onAiInstall={(id) => void handleAiInstall(id)}
                  onToggleEnabled={(id, enabled) => void handleToggle(id, enabled)}
                  onUninstall={(id) => void handleUninstall(id)}
                />
              ))}
            </div>
          )}
        </div>
      </main>
    </div>
  )
}
