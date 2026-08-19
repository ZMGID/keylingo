import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import {
  Box,
  ChevronDown,
  Download,
  ExternalLink,
  FolderOpen,
  Plus,
  RefreshCw,
  Search,
  Sliders,
  Sparkles,
  Trash2,
  X,
} from 'lucide-react'
import { open } from '@tauri-apps/plugin-dialog'
import { homeDir } from '@tauri-apps/api/path'
import { ChatMarkdown } from './ChatMarkdown'
import {
  api,
  defaultNativeTools,
  isTauriRuntime,
  type ChatToolsConfig,
  type Settings,
  type SkillDetail,
  type SkillMeta,
} from '../api/tauri'
import { getSettingsCached, refreshSettings, saveSettingsCached } from '../api/settingsCache'
import { Select, Toggle } from '../settings/components'
import { useT, type I18n } from '../settings/i18n'
import { Button, IconButton } from '../components/Button'
import { SkillStoreBrowser } from './SkillStoreBrowser'
import { SkillIcon } from '../settings/NavIcons'

interface SkillCenterProps {
  /** Skill 启用状态 / 列表变化后通知 Chat 刷新其技能列表 */
  onSkillsChanged?: () => void
  /** 当前对话工作目录：扫描项目 `.kivio/skills` 与 `.agents/skills` */
  projectCwd?: string
}

/** 本地 CLI 技能来源：只扫各家「自己的」目录。`~/.agents/skills` 是共享目录，Kivio 会直接扫描，不必再导入。 */
const CLI_SKILL_SOURCES = [
  { key: 'claude', label: 'Claude Code', dirs: ['.claude/skills'] },
  { key: 'codex', label: 'Codex', dirs: ['.codex/skills'] },
  { key: 'opencode', label: 'OpenCode', dirs: ['.config/opencode/skills', '.opencode/skills'] },
  { key: 'pi', label: 'Pi', dirs: ['.pi/agent/skills'] },
] as const

type CliSkillKey = (typeof CLI_SKILL_SOURCES)[number]['key']
type CliSkillGroups = Record<CliSkillKey, SkillMeta[]>

function defaultChatTools(): ChatToolsConfig {
  return {
    enabled: false,
    servers: [],
    skillScanPaths: [],
    skillAutoMatch: true,
    skillFallbackMode: 'progressive',
    disabledSkillIds: [],
    maxToolRounds: null,
    toolTimeoutMs: 60_000,
    mcpIdleTimeoutMs: 600_000,
    approvalPolicy: 'readonly_auto_sensitive_confirm',
    nativeTools: defaultNativeTools(),
  }
}

function isBuiltinSkill(skill: SkillMeta): boolean {
  return skill.source === 'builtin'
}

function isPluginSkill(skill: SkillMeta): boolean {
  return skill.source === 'plugin'
}

function isProjectSkill(skill: SkillMeta): boolean {
  return skill.source === 'project'
}

function isGlobalAgentsSkill(skill: SkillMeta): boolean {
  return skill.source === 'agents'
}

function canDeleteSkill(skill: SkillMeta): boolean {
  return skill.source === 'user'
}

function skillSourceLabel(skill: SkillMeta, t: I18n): string {
  if (skill.source === 'builtin') return t.chatSkillSourceBuiltin
  if (skill.source === 'plugin') return t.chatSkillSourcePlugin
  if (skill.source === 'external') return t.chatSkillSourceWorkspace
  if (skill.source === 'project') return t.chatSkillSourceProject
  if (skill.source === 'agents') return t.chatSkillSourceGlobal
  return t.chatSkillSourcePersonal
}

function skillMatches(skill: SkillMeta, query: string): boolean {
  if (!query) return true
  return (
    skill.name.toLowerCase().includes(query) ||
    (skill.description ?? '').toLowerCase().includes(query)
  )
}

/** 自带样式的开关：明暗对比清晰，不依赖设置面板的 CSS 变量作用域 */
function SkillCard({
  skill,
  enabled,
  index,
  onToggleEnabled,
  onPreview,
  onDelete,
  manageLocked = false,
}: {
  skill: SkillMeta
  enabled: boolean
  /** 卡片入场 stagger 序号 */
  index: number
  onToggleEnabled: (skillId: string, enabled: boolean) => void
  onPreview: (skillId: string) => void
  /** 删除个人/导入技能（仅 source==='user' 显示）；不传则不显示删除 */
  onDelete?: (skill: SkillMeta) => void
  /** 插件附属：开关在插件页，此处只展示 */
  manageLocked?: boolean
}) {
  const t = useT()
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => onPreview(skill.id)}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault()
          onPreview(skill.id)
        }
      }}
      data-tauri-drag-region="false"
      title={t.chatSkillViewFull}
      style={{ '--chat-motion-delay': `${Math.min(index, 8) * 24}ms` } as CSSProperties}
      className={`chat-motion-fade-up group flex h-full min-w-0 cursor-pointer flex-col rounded-xl border p-3.5 text-left transition-[border-color,box-shadow,transform,background-color] duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-neutral-900/15 dark:focus-visible:ring-white/20 ${
        enabled
          ? 'border-neutral-200 bg-white shadow-sm hover:-translate-y-0.5 hover:border-neutral-300 hover:shadow-md dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700'
          : 'border-neutral-200/80 bg-neutral-50/60 hover:-translate-y-0.5 hover:border-neutral-300 hover:bg-white hover:shadow-md dark:border-neutral-800/70 dark:bg-neutral-900/30 dark:hover:border-neutral-700 dark:hover:bg-neutral-950/40'
      }`}
    >
      <div className="flex items-start justify-between gap-2">
        <span
          className={`grid size-10 shrink-0 place-items-center rounded-lg border transition-colors duration-[var(--kv-dur-fast)] ${
            enabled
              ? 'border-neutral-200 bg-white text-neutral-600 dark:border-neutral-700 dark:bg-neutral-950 dark:text-neutral-300'
              : 'border-neutral-200/80 bg-neutral-100/80 text-neutral-400 group-hover:text-neutral-500 dark:border-neutral-800 dark:bg-neutral-900 dark:text-neutral-600'
          }`}
        >
          <Box size={18} />
        </span>
        {manageLocked ? (
          <span
            className="shrink-0 pt-0.5 text-[11px] text-neutral-400 dark:text-neutral-500"
            title={t.chatSkillPluginManageHint}
          >
            {enabled ? t.chatSkillPluginEnabled : t.chatSkillPluginDisabled}
          </span>
        ) : (
          <span
            className="shrink-0"
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => event.stopPropagation()}
          >
            <Toggle checked={enabled} onChange={(next) => onToggleEnabled(skill.id, next)} ariaLabel={t.chatSkillEnableNamed.replace('{name}', skill.name)} />
          </span>
        )}
      </div>
      <div className="mt-2.5 min-w-0 flex-1">
        <div className={`truncate text-[13.5px] font-semibold leading-tight ${
          enabled ? 'text-neutral-950 dark:text-neutral-50' : 'text-neutral-600 dark:text-neutral-400'
        }`}>
          {skill.name}
        </div>
        <p className="mt-1 line-clamp-2 text-[12px] leading-[1.45] text-neutral-500 dark:text-neutral-400">
          {skill.description || t.chatSkillNoDescription}
        </p>
      </div>
      <div className="mt-2.5 flex min-h-6 items-center gap-1 border-t border-neutral-100 pt-2 text-[11px] text-neutral-400 dark:border-neutral-800/70 dark:text-neutral-500">
        <span className="truncate">{skillSourceLabel(skill, t)}</span>
        {onDelete && canDeleteSkill(skill) && !manageLocked ? (
          <span
            className="ml-auto shrink-0 opacity-0 transition-opacity duration-[var(--kv-dur-fast)] focus-within:opacity-100 group-hover:opacity-100"
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => event.stopPropagation()}
          >
            <IconButton
              size="sm"
              className="danger"
              onClick={() => onDelete(skill)}
              label={t.chatSkillDeleteNamed.replace('{name}', skill.name)}
              title={t.chatSkillDelete}
            >
              <Trash2 size={14} strokeWidth={1.75} />
            </IconButton>
          </span>
        ) : null}
      </div>
    </div>
  )
}

function SkillSection({
  title,
  note,
  emptyText,
  skills,
  disabledSkillIds,
  onToggleEnabled,
  onPreview,
  onDelete,
  collapsible = false,
  defaultCollapsed = false,
  manageLocked = false,
  lockedActiveIds,
}: {
  title: string
  note?: string
  emptyText: string
  skills: SkillMeta[]
  disabledSkillIds: string[]
  onToggleEnabled: (skillId: string, enabled: boolean) => void
  onPreview: (skillId: string) => void
  onDelete?: (skill: SkillMeta) => void
  collapsible?: boolean
  defaultCollapsed?: boolean
  manageLocked?: boolean
  lockedActiveIds?: Set<string>
}) {
  const [collapsed, setCollapsed] = useState(collapsible && defaultCollapsed)
  const t = useT()
  const enabledCount = skills.filter((skill) =>
    manageLocked ? Boolean(lockedActiveIds?.has(skill.id)) : !disabledSkillIds.includes(skill.id),
  ).length
  return (
    <section className="space-y-2.5">
      <div
        className={`flex min-w-0 items-center gap-3 px-1 ${collapsible ? 'cursor-pointer select-none' : ''}`}
        onClick={collapsible ? () => setCollapsed((v) => !v) : undefined}
      >
        {collapsible && (
          <ChevronDown
            size={16}
            className={`shrink-0 text-neutral-400 transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] ${collapsed ? '-rotate-90' : ''}`}
          />
        )}
        <h3 className="text-[15px] font-semibold text-neutral-700 dark:text-neutral-200">{title}</h3>
        <span className="text-[14px] font-medium text-neutral-400">{skills.length}</span>
        {collapsed && skills.length > 0 && (
          <span className="text-[12.5px] text-neutral-400">{t.chatSkillsEnabledCount.replace('{n}', String(enabledCount))}</span>
        )}
        {note && <span className="ml-auto truncate text-[12.5px] text-neutral-400">{note}</span>}
      </div>
      {collapsed ? null : skills.length === 0 ? (
        <div className="grid min-h-[72px] place-items-center rounded-md border border-dashed border-neutral-200 text-[13px] text-neutral-400 dark:border-neutral-800">
          {emptyText}
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {skills.map((skill, index) => (
            <SkillCard
              key={skill.id}
              skill={skill}
              index={index}
              enabled={
                manageLocked
                  ? Boolean(lockedActiveIds?.has(skill.id))
                  : !disabledSkillIds.includes(skill.id)
              }
              onToggleEnabled={onToggleEnabled}
              onPreview={onPreview}
              onDelete={onDelete}
              manageLocked={manageLocked}
            />
          ))}
        </div>
      )}
    </section>
  )
}

function SkillUrlImport({ onInstalled }: { onInstalled: () => void }) {
  const t = useT()
  const [url, setUrl] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [done, setDone] = useState('')
  const install = useCallback(async () => {
    const value = url.trim()
    if (!value) return
    setBusy(true)
    setError('')
    setDone('')
    try {
      const result = await api.chatSkillsInstallFromUrl(value)
      if (!result.success) throw new Error(result.error || t.chatSkillInstallFailed)
      setDone(t.chatSkillInstalled)
      setUrl('')
      onInstalled()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }, [t, url, onInstalled])
  return (
    <div className="rounded-md border border-neutral-200 p-3 dark:border-neutral-800">
      <div className="mb-1.5 text-[13px] font-medium text-neutral-800 dark:text-neutral-100">{t.chatSkillInstallFromUrl}</div>
      <p className="mb-2 text-[12px] text-neutral-500 dark:text-neutral-400">
        {t.chatSkillUrlImportHint}
      </p>
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://github.com/owner/repo"
          className="h-9 w-full rounded-md border border-neutral-200 bg-white px-2.5 font-mono text-[12.5px] text-neutral-800 outline-none focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
          data-tauri-drag-region="false"
        />
        <Button onClick={() => void install()} disabled={busy || !url.trim()} data-tauri-drag-region="false">
          {busy ? t.chatSkillInstalling : t.chatSkillInstall}
        </Button>
      </div>
      {error && <div className="mt-2 text-[12px] text-red-600 dark:text-red-400">{error}</div>}
      {done && <div className="mt-2 text-[12px] text-emerald-600 dark:text-emerald-400">{done}</div>}
    </div>
  )
}

export function SkillCenter({ onSkillsChanged, projectCwd }: SkillCenterProps) {
  const t = useT()
  const [settings, setSettings] = useState<Settings | null>(null)
  const [skills, setSkills] = useState<SkillMeta[]>([])
  const [skillsLoading, setSkillsLoading] = useState(false)
  const [skillError, setSkillError] = useState('')
  const [query, setQuery] = useState('')
  const [view, setView] = useState<'installed' | 'store' | 'import' | 'advanced'>('installed')
  const [selectedSkillPreview, setSelectedSkillPreview] = useState<SkillDetail | null>(null)
  // 从本地 CLI（Claude Code / Codex / OpenCode）的技能目录导入
  const [cliSkills, setCliSkills] = useState<CliSkillGroups | null>(null)
  const [cliScanning, setCliScanning] = useState(false)
  const [cliSelected, setCliSelected] = useState<Set<string>>(new Set())
  const [cliImporting, setCliImporting] = useState(false)
  const [cliImportDone, setCliImportDone] = useState('')
  const [enabledPluginSkillIds, setEnabledPluginSkillIds] = useState<Set<string>>(() => new Set())

  const settingsRef = useRef<Settings | null>(null)
  const saveTimer = useRef<number | null>(null)

  const chatTools = settings?.chatTools ?? defaultChatTools()
  const disabledSkillIds = chatTools.disabledSkillIds ?? []

  const refreshChatSkills = useCallback(async (scanPaths?: string[]) => {
    setSkillsLoading(true)
    setSkillError('')
    try {
      if (isTauriRuntime()) {
        try {
          const plugins = await api.pluginsListCached()
          const ids = new Set<string>()
          for (const plugin of plugins) {
            if (!plugin.enabled) continue
            for (const skillId of plugin.skillIds ?? []) ids.add(skillId)
          }
          setEnabledPluginSkillIds(ids)
        } catch {
          /* 插件列表失败不挡技能列表 */
        }
      }
      const result = await api.chatSkillsList(
        scanPaths ?? settingsRef.current?.chatTools?.skillScanPaths,
        projectCwd || undefined,
      )
      if (result.success) {
        setSkills(result.skills)
      } else {
        setSkillError(result.error || t.chatSkillListLoadFailed)
      }
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    } finally {
      setSkillsLoading(false)
    }
  }, [projectCwd, t])

  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const loaded = await getSettingsCached()
        if (cancelled) return
        settingsRef.current = loaded
        setSettings(loaded)
        await refreshChatSkills(loaded.chatTools?.skillScanPaths)
      } catch (err) {
        if (!cancelled) setSkillError(err instanceof Error ? err.message : String(err))
      }
    })()
    return () => {
      cancelled = true
      if (saveTimer.current) window.clearTimeout(saveTimer.current)
    }
  }, [refreshChatSkills])

  const flushSave = useCallback(async (next: Settings) => {
    try {
      // 整体保存前现读后端最新态，仅把 servers 取自 fresh：servers[].auth/headers 会被后端
      // OAuth 刷新独立改写（mcp/manager.rs），若用本地快照整体保存会覆盖回旧 token。
      // 其余 chatTools 字段（技能开关/扫描路径等）是本任务编辑值，仍以本地 next 为准。
      const fresh = await refreshSettings()
      const merged: Settings = {
        ...next,
        chatTools: {
          ...(next.chatTools ?? defaultChatTools()),
          servers: fresh.chatTools?.servers ?? next.chatTools?.servers ?? [],
        },
      }
      const saved = await saveSettingsCached(merged)
      settingsRef.current = saved
      onSkillsChanged?.()
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    }
  }, [onSkillsChanged])

  // 更新 chatTools：本地立即生效，再持久化（文本类编辑防抖，开关/下拉立即保存）
  const persistChatTools = useCallback((updates: Partial<ChatToolsConfig>, debounce = false) => {
    setSettings((prev) => {
      if (!prev) return prev
      const next: Settings = {
        ...prev,
        chatTools: { ...(prev.chatTools ?? defaultChatTools()), ...updates },
      }
      settingsRef.current = next
      if (saveTimer.current) {
        window.clearTimeout(saveTimer.current)
        saveTimer.current = null
      }
      if (debounce) {
        saveTimer.current = window.setTimeout(() => {
          saveTimer.current = null
          void flushSave(next)
        }, 500)
      } else {
        void flushSave(next)
      }
      return next
    })
  }, [flushSave])

  const handleToggleSkillEnabled = useCallback((skillId: string, enabled: boolean) => {
    const disabled = settingsRef.current?.chatTools?.disabledSkillIds ?? []
    const next = enabled
      ? disabled.filter((id) => id !== skillId)
      : disabled.includes(skillId)
        ? disabled
        : [...disabled, skillId]
    persistChatTools({ disabledSkillIds: next })
  }, [persistChatTools])

  const handlePreviewSkill = useCallback(async (skillId: string) => {
    setSkillError('')
    try {
      const result = await api.chatSkillsRead(skillId, projectCwd || undefined)
      if (result.success && result.skill) {
        setSelectedSkillPreview(result.skill)
      } else {
        setSkillError(result.error || t.chatSkillReadFailed)
      }
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    }
  }, [projectCwd, t])

  const handleImportSkill = useCallback(async () => {
    try {
      const selected = await open({ directory: true, multiple: false })
      if (typeof selected !== 'string') return
      const result = await api.chatSkillsImport(selected)
      if (!result.success) {
        setSkillError(result.error || t.chatSkillImportFailed)
        return
      }
      await refreshChatSkills()
      onSkillsChanged?.()
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    }
  }, [onSkillsChanged, refreshChatSkills, t])

  const handleDeleteSkill = useCallback(async (skill: SkillMeta) => {
    if (!window.confirm(t.chatSkillDeleteConfirm.replace('{name}', skill.name))) return
    setSkillError('')
    try {
      await api.chatSkillsUninstall(skill.id)
      await refreshChatSkills()
      onSkillsChanged?.()
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    }
  }, [onSkillsChanged, refreshChatSkills, t])

  const handleImportSkillZip = useCallback(async () => {
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        filters: [{ name: 'Skill Zip', extensions: ['zip'] }],
      })
      if (typeof selected !== 'string') return
      const result = await api.chatSkillsImport(selected)
      if (!result.success) {
        setSkillError(result.error || t.chatSkillImportFailed)
        return
      }
      await refreshChatSkills()
      onSkillsChanged?.()
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    }
  }, [onSkillsChanged, refreshChatSkills, t])

  const handleOpenSkillFolder = useCallback(async () => {
    setSkillError('')
    try {
      const result = await api.chatSkillsOpenFolder()
      if (!result.success) {
        setSkillError(result.error || t.chatSkillOpenFolderFailed)
      }
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    }
  }, [t])

  // 扫描各本地 CLI 的技能目录：复用 chat_skills_list 的额外扫描路径（external 源即 CLI 技能），
  // 再按 skill.path 的目录前缀把结果归到对应 CLI 分组。
  const handleCliScan = useCallback(async () => {
    setCliScanning(true)
    setCliImportDone('')
    setSkillError('')
    try {
      const home = (await homeDir()).replace(/[/\\]+$/, '')
      const norm = (p: string) => p.replace(/\\/g, '/').toLowerCase()
      const piAgentDir = (await api.chatPiAgentDir())?.replace(/[/\\]+$/, '')
      const defaultPiDirs = CLI_SKILL_SOURCES
        .filter((source) => source.key === 'pi')
        .flatMap((source) => source.dirs.map((dir) => `${home}/${dir}`))
      const piSkillDirs = Array.from(new Set([
        ...(piAgentDir ? [`${piAgentDir}/skills`] : []),
        ...defaultPiDirs,
      ].map((dir) => dir.replace(/\\/g, '/'))))
      // 每个 CLI 目录 → 归一化前缀（用于把扫描结果分组）
      const sources = CLI_SKILL_SOURCES.map((source) => ({
        key: source.key,
        prefixes: source.key === 'pi'
          ? piSkillDirs.map(norm)
          : source.dirs.map((dir) => norm(`${home}/${dir}`)),
      }))
      const scanDirs = [
        ...CLI_SKILL_SOURCES.filter((source) => source.key !== 'pi')
          .flatMap((source) => source.dirs.map((dir) => `${home}/${dir}`)),
        ...piSkillDirs,
      ]
      const result = await api.chatSkillsList(scanDirs)
      if (!result.success) {
        setSkillError(result.error || t.chatSkillScanFailed)
        setCliSkills({ claude: [], codex: [], opencode: [], pi: [] })
        return
      }
      const scanned = result.skills.filter((skill) => skill.source === 'external' && skill.path)
      const groups: CliSkillGroups = { claude: [], codex: [], opencode: [], pi: [] }
      for (const skill of scanned) {
        const path = norm(skill.path as string)
        const source = sources.find((s) => s.prefixes.some((prefix) => path.startsWith(prefix)))
        if (source) groups[source.key].push(skill)
      }
      setCliSkills(groups)
      setCliSelected(new Set())
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    } finally {
      setCliScanning(false)
    }
  }, [t])

  const toggleCliSelected = useCallback((id: string) => {
    setCliSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  // 导入选中技能 = 逐项从 skill.path（.../<id>/SKILL.md）推出文件夹后复制进 Kivio 用户技能目录。
  const handleCliImportSelected = useCallback(async () => {
    if (!cliSkills) return
    const all = [...cliSkills.claude, ...cliSkills.codex, ...cliSkills.opencode, ...cliSkills.pi]
    const chosen = all.filter((skill) => cliSelected.has(skill.id) && skill.path)
    if (chosen.length === 0) return
    setCliImporting(true)
    setCliImportDone('')
    setSkillError('')
    let imported = 0
    try {
      for (const skill of chosen) {
        const folder = (skill.path as string).replace(/[/\\]+SKILL\.md$/i, '')
        const result = await api.chatSkillsImport(folder)
        if (result.success) imported += 1
        else setSkillError(result.error || t.chatSkillImportNamedFailed.replace('{name}', skill.name))
      }
      await refreshChatSkills()
      onSkillsChanged?.()
      if (imported > 0) {
        setCliImportDone(t.chatSkillCliImportDone.replace('{n}', String(imported)))
        setCliSkills(null)
        setCliSelected(new Set())
      }
    } catch (err) {
      setSkillError(err instanceof Error ? err.message : String(err))
    } finally {
      setCliImporting(false)
    }
  }, [cliSkills, cliSelected, onSkillsChanged, refreshChatSkills, t])

  const normalizedQuery = query.trim().toLowerCase()
  const builtinSkills = useMemo(
    () => skills.filter((skill) => isBuiltinSkill(skill) && skillMatches(skill, normalizedQuery)),
    [skills, normalizedQuery],
  )
  const pluginSkills = useMemo(
    () => skills.filter((skill) => isPluginSkill(skill) && skillMatches(skill, normalizedQuery)),
    [skills, normalizedQuery],
  )
  const projectSkills = useMemo(
    () => skills.filter((skill) => isProjectSkill(skill) && skillMatches(skill, normalizedQuery)),
    [skills, normalizedQuery],
  )
  const globalSkills = useMemo(
    () => skills.filter((skill) => isGlobalAgentsSkill(skill) && skillMatches(skill, normalizedQuery)),
    [skills, normalizedQuery],
  )
  const personalSkills = useMemo(
    () =>
      skills.filter(
        (skill) =>
          !isBuiltinSkill(skill)
          && !isPluginSkill(skill)
          && !isProjectSkill(skill)
          && !isGlobalAgentsSkill(skill)
          && skillMatches(skill, normalizedQuery),
      ),
    [skills, normalizedQuery],
  )

  return (
    <div className="assistant-center-root flex h-full min-h-0 flex-col text-neutral-900 dark:text-neutral-100">
      {/* 顶栏：与聊天主区同底色、无分隔；可拖拽，右侧避开窗口按钮 */}

      {/* 内容区：直接坐在白底上，与聊天主区无缝 */}
      <main className="custom-scrollbar min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto w-full max-w-[1040px] px-9 pb-10 pt-7">
            {/* 头部：标题 + 副标题 + 图标动作 */}
            <div className="border-b border-neutral-200 pb-5 dark:border-neutral-800">
              <h1 className="flex items-center gap-2.5 text-[28px] font-semibold tracking-normal text-neutral-950 dark:text-neutral-50">
                <SkillIcon size={24} className="text-neutral-500" />
                Skill
              </h1>
              <div className="mt-3.5 flex min-w-0 items-center gap-4">
              <p className="min-w-0 flex-1 text-[14px] leading-relaxed text-neutral-500 dark:text-neutral-400">
                {t.chatSkillPageSubtitle}
              </p>
              <div className="flex shrink-0 items-center gap-0.5">
                <IconButton
                  size="lg"
                  label={t.chatSkillImportFolder}
                  onClick={() => void handleImportSkill()}
                  data-tauri-drag-region="false"
                >
                  <FolderOpen size={17} />
                </IconButton>
                <IconButton
                  size="lg"
                  label={t.chatSkillImportZip}
                  onClick={() => void handleImportSkillZip()}
                  data-tauri-drag-region="false"
                >
                  <Download size={17} />
                </IconButton>
                <IconButton
                  size="lg"
                  label={t.chatSkillOpenSkillFolder}
                  onClick={() => void handleOpenSkillFolder()}
                  data-tauri-drag-region="false"
                >
                  <ExternalLink size={17} />
                </IconButton>
                <IconButton
                  size="lg"
                  label={t.chatSkillRefreshList}
                  onClick={() => void refreshChatSkills()}
                  disabled={skillsLoading}
                  data-tauri-drag-region="false"
                >
                  <RefreshCw size={17} className={skillsLoading ? 'animate-spin' : ''} />
                </IconButton>
              </div>
            </div>
          </div>

          {/* Tab 行 */}
          <div className="mt-5 flex items-center gap-1 border-b border-neutral-200 dark:border-neutral-800">
            {([['installed', t.chatSkillTabInstalled], ['store', t.chatSkillTabStore], ['import', t.chatSkillTabImport], ['advanced', t.chatSkillTabAdvanced]] as const).map(([id, label]) => (
              <button
                key={id}
                type="button"
                onClick={() => setView(id)}
                data-tauri-drag-region="false"
                className={`relative px-3 py-2 text-[13px] font-medium transition-colors ${
                  view === id
                    ? 'text-neutral-900 dark:text-neutral-100'
                    : 'text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200'
                }`}
              >
                {label}
                {view === id && (
                  <span className="chat-motion-tab-underline absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-[#2f6ff0] dark:bg-[#5c8df7]" />
                )}
              </button>
            ))}
          </div>

          {view === 'store' ? (
            <div key="store" className="chat-motion-tab-in mt-5 flex min-h-[420px] flex-col">
              <SkillStoreBrowser onInstalled={() => void refreshChatSkills()} />
            </div>
          ) : view === 'import' ? (
            <div key="import" className="chat-motion-tab-in mt-5 space-y-4">
              <div className="flex flex-wrap gap-2">
                <Button onClick={() => void handleImportSkill()} data-tauri-drag-region="false">
                  <FolderOpen size={14} />
                  {t.chatSkillImportFolder}
                </Button>
                <Button onClick={() => void handleImportSkillZip()} data-tauri-drag-region="false">
                  <Download size={14} />
                  {t.chatSkillImportZip}
                </Button>
                <Button onClick={() => void handleOpenSkillFolder()} data-tauri-drag-region="false">
                  <ExternalLink size={14} />
                  {t.chatSkillOpenSkillFolder}
                </Button>
              </div>
              <SkillUrlImport onInstalled={() => void refreshChatSkills()} />
              <div className="rounded-md border border-neutral-200 p-3 dark:border-neutral-800">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="mb-1.5 text-[13px] font-medium text-neutral-800 dark:text-neutral-100">{t.chatSkillImportFromCli}</div>
                    <p className="text-[12px] text-neutral-500 dark:text-neutral-400">
                      {t.chatSkillCliImportHint}
                    </p>
                  </div>
                  <Button onClick={() => void handleCliScan()} disabled={cliScanning} data-tauri-drag-region="false">
                    <Search size={14} />
                    {cliScanning ? t.chatSkillScanning : t.chatSkillScan}
                  </Button>
                </div>

                {cliSkills && (() => {
                  const total = cliSkills.claude.length + cliSkills.codex.length + cliSkills.opencode.length + cliSkills.pi.length
                  return (
                    <div className="mt-3 space-y-3">
                      {total === 0 ? (
                        <div className="rounded-md border border-dashed border-neutral-200 px-3 py-2 text-[11.5px] text-neutral-400 dark:border-neutral-800">
                          {t.chatSkillCliNoSkillsFound}
                        </div>
                      ) : (
                        <>
                          {CLI_SKILL_SOURCES.map((source) => {
                            const group = cliSkills[source.key]
                            if (group.length === 0) return null
                            return (
                              <div key={source.key}>
                                <div className="mb-1.5 text-[12px] font-medium text-neutral-600 dark:text-neutral-300">{source.label}</div>
                                <div className="overflow-hidden rounded-md border border-neutral-200 dark:border-neutral-800 [&>*+*]:border-t [&>*+*]:border-neutral-100 dark:[&>*+*]:border-neutral-800/70">
                                  {group.map((skill) => (
                                    <label
                                      key={skill.id}
                                      className="flex cursor-pointer items-center gap-2.5 px-3 py-2"
                                      data-tauri-drag-region="false"
                                    >
                                      <input
                                        type="checkbox"
                                        checked={cliSelected.has(skill.id)}
                                        onChange={() => toggleCliSelected(skill.id)}
                                        className="size-3.5 shrink-0 accent-[#2f6ff0]"
                                      />
                                      <div className="min-w-0 flex-1">
                                        <div className="truncate text-[12.5px] font-medium text-neutral-800 dark:text-neutral-100">{skill.name}</div>
                                        <div className="truncate text-[11px] text-neutral-400">{skill.description || t.chatSkillNoDescription}</div>
                                      </div>
                                    </label>
                                  ))}
                                </div>
                              </div>
                            )
                          })}
                          <Button
                            onClick={() => void handleCliImportSelected()}
                            disabled={cliImporting || cliSelected.size === 0}
                            data-tauri-drag-region="false"
                          >
                            {cliImporting ? t.chatSkillImporting : t.chatSkillImportSelected.replace('{n}', String(cliSelected.size))}
                          </Button>
                        </>
                      )}
                    </div>
                  )
                })()}

                {cliImportDone && (
                  <div className="mt-2 text-[12px] text-emerald-600 dark:text-emerald-400">{cliImportDone}</div>
                )}
              </div>
              {skillError && (
                <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-[12px] text-red-700 dark:border-red-900/60 dark:bg-red-950/30 dark:text-red-300">
                  {skillError}
                </div>
              )}
            </div>
          ) : view === 'advanced' ? (
          <section key="advanced" className="chat-motion-tab-in mt-5 overflow-hidden rounded-md border border-neutral-200 dark:border-neutral-800">
            <div className="flex w-full items-center gap-2 px-4 py-3">
              <Sliders size={15} className="shrink-0 text-neutral-400" />
              <span className="text-[13px] font-semibold text-neutral-800 dark:text-neutral-100">{t.chatSkillTabAdvanced}</span>
              <span className="text-[12px] text-neutral-400">{t.chatSkillAdvancedSubtitle}</span>
            </div>
            <div>
              <div className="space-y-5 border-t border-neutral-200 px-4 py-4 dark:border-neutral-800">
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0">
                    <div className="text-[13px] font-medium text-neutral-800 dark:text-neutral-100">{t.chatSkillAutoMatch}</div>
                    <p className="mt-0.5 text-[12px] text-neutral-500 dark:text-neutral-400">
                      {t.chatSkillAutoMatchHint}
                    </p>
                  </div>
                  <Toggle
                    checked={chatTools.skillAutoMatch !== false}
                    onChange={(skillAutoMatch) => persistChatTools({ skillAutoMatch })}
                    ariaLabel={t.chatSkillAutoMatch}
                  />
                </div>

                <div className="min-w-0">
                  <div className="min-w-0">
                    <div className="mb-1.5 text-[13px] font-medium text-neutral-800 dark:text-neutral-100">
                      {t.chatSkillFallbackMode}
                    </div>
                    <Select
                      value={chatTools.skillFallbackMode || 'progressive'}
                      onChange={(value) => persistChatTools({ skillFallbackMode: value })}
                      options={[
                        { value: 'progressive', label: t.chatSkillFallbackProgressive },
                        { value: 'skill_md_only', label: t.chatSkillFallbackSkillMdOnly },
                        { value: 'legacy_full_body', label: t.chatSkillFallbackLegacyFullBody },
                      ]}
                    />
                  </div>
                </div>

                <div className="min-w-0">
                  <div className="mb-1.5 text-[13px] font-medium text-neutral-800 dark:text-neutral-100">{t.chatSkillExtraScanPaths}</div>
                  <div className="space-y-1.5">
                    {chatTools.skillScanPaths.map((path, index) => (
                      <div key={`${path}-${index}`} className="flex items-center gap-1.5">
                        <input
                          type="text"
                          value={path}
                          onChange={(event) => {
                            const next = [...chatTools.skillScanPaths]
                            next[index] = event.target.value
                            persistChatTools({ skillScanPaths: next }, true)
                          }}
                          placeholder="/path/to/skills"
                          className="h-9 w-full rounded-md border border-neutral-200 bg-white px-2.5 font-mono text-[12.5px] text-neutral-800 outline-none focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
                          data-tauri-drag-region="false"
                        />
                        <IconButton
                          size="lg"
                          variant="danger"
                          label={t.chatSkillRemovePath}
                          onClick={() => {
                            const next = chatTools.skillScanPaths.filter((_, i) => i !== index)
                            persistChatTools({ skillScanPaths: next })
                            void refreshChatSkills(next)
                          }}
                          data-tauri-drag-region="false"
                        >
                          <Trash2 size={14} />
                        </IconButton>
                      </div>
                    ))}
                    <Button
                      onClick={async () => {
                        const selected = await open({ directory: true, multiple: false })
                        if (typeof selected === 'string') {
                          const next = [...chatTools.skillScanPaths, selected]
                          persistChatTools({ skillScanPaths: next })
                          void refreshChatSkills(next)
                        }
                      }}
                      data-tauri-drag-region="false"
                    >
                      <Plus size={13} />
                      {t.chatSkillAddScanPath}
                    </Button>
                  </div>
                </div>
              </div>
            </div>
          </section>
          ) : (
          <div key="installed" className="chat-motion-tab-in">
          {/* 搜索 */}
          <div className="relative mt-6">
            <Search size={16} className="pointer-events-none absolute left-3.5 top-1/2 -translate-y-1/2 text-neutral-400" />
            <input
              type="text"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t.chatSkillSearchPlaceholder}
              className="h-10 w-full rounded-md border border-neutral-200 bg-white pl-10 pr-4 text-[14px] outline-none placeholder:text-neutral-400 focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
              data-tauri-drag-region="false"
            />
          </div>

          {skillError && (
            <div className="mt-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-[12px] text-red-700 dark:border-red-900/60 dark:bg-red-950/30 dark:text-red-300">
              {skillError}
            </div>
          )}

          {/* 技能列表 */}
          <div className="mt-6 space-y-5">
            {skillsLoading && skills.length === 0 ? (
              <div className="grid min-h-[220px] place-items-center text-[13px] text-neutral-400">{t.chatSkillLoading}</div>
            ) : (
              <>
                {(projectSkills.length > 0 || Boolean(projectCwd)) && (
                  <SkillSection
                    title={t.chatSkillSectionProject}
                    note={t.chatSkillProjectNote}
                    emptyText={normalizedQuery ? t.chatSkillNoMatchingSkills : t.chatSkillNoProjectSkills}
                    skills={projectSkills}
                    disabledSkillIds={disabledSkillIds}
                    onToggleEnabled={handleToggleSkillEnabled}
                    onPreview={handlePreviewSkill}
                  />
                )}
                <SkillSection
                  title={t.chatSkillSectionWorkspacePersonal}
                  note={t.chatSkillPersonalNote}
                  emptyText={normalizedQuery ? t.chatSkillNoMatchingSkills : t.chatSkillNoImportedSkills}
                  skills={personalSkills}
                  disabledSkillIds={disabledSkillIds}
                  onToggleEnabled={handleToggleSkillEnabled}
                  onPreview={handlePreviewSkill}
                  onDelete={handleDeleteSkill}
                />
                <SkillSection
                  title={t.chatSkillSectionPlugin}
                  note={t.chatSkillPluginNote}
                  emptyText={normalizedQuery ? t.chatSkillNoMatchingPlugin : t.chatSkillNoPluginSkills}
                  skills={pluginSkills}
                  disabledSkillIds={disabledSkillIds}
                  onToggleEnabled={handleToggleSkillEnabled}
                  onPreview={handlePreviewSkill}
                  manageLocked
                  lockedActiveIds={enabledPluginSkillIds}
                />
                {(globalSkills.length > 0 || normalizedQuery) && (
                  <SkillSection
                    title={t.chatSkillSectionGlobal}
                    note={t.chatSkillGlobalNote}
                    emptyText={t.chatSkillNoMatchingSkills}
                    skills={globalSkills}
                    disabledSkillIds={disabledSkillIds}
                    onToggleEnabled={handleToggleSkillEnabled}
                    onPreview={handlePreviewSkill}
                  />
                )}
                <SkillSection
                  title={t.chatSkillSectionBuiltin}
                  note={t.chatSkillBuiltinNote}
                  emptyText={normalizedQuery ? t.chatSkillNoMatchingBuiltin : t.chatSkillNoBuiltin}
                  skills={builtinSkills}
                  disabledSkillIds={disabledSkillIds}
                  onToggleEnabled={handleToggleSkillEnabled}
                  onPreview={handlePreviewSkill}
                  collapsible
                  defaultCollapsed
                />
              </>
            )}
          </div>
          </div>
          )}
        </div>
      </main>

      {/* 预览弹窗 */}
      {selectedSkillPreview && (
        <div
          className="chat-motion-fade fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
          data-tauri-drag-region="false"
          onClick={() => setSelectedSkillPreview(null)}
        >
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="skill-preview-title"
            className="chat-motion-modal-in flex max-h-[80vh] w-full max-w-[640px] flex-col gap-3 overflow-hidden rounded-2xl border border-neutral-200 bg-white p-5 shadow-2xl dark:border-neutral-700 dark:bg-neutral-900"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="flex items-start gap-2">
              <Sparkles size={16} className="mt-0.5 shrink-0 text-[#2f6ff0] dark:text-[#5c8df7]" />
              <div className="min-w-0 flex-1">
                <h3 id="skill-preview-title" className="truncate text-[15px] font-semibold text-neutral-900 dark:text-neutral-100">
                  {selectedSkillPreview.name}
                </h3>
                <p className="mt-0.5 text-[12.5px] text-neutral-500 dark:text-neutral-400">
                  {selectedSkillPreview.description}
                </p>
              </div>
              <IconButton
                size="sm"
                label={t.chatWinClose}
                onClick={() => setSelectedSkillPreview(null)}
                data-tauri-drag-region="false"
              >
                <X size={14} />
              </IconButton>
            </div>
            {selectedSkillPreview.recommendedTools.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                {selectedSkillPreview.recommendedTools.map((tool) => (
                  <span
                    key={tool}
                    className="rounded-md bg-neutral-100 px-2 py-0.5 text-[11.5px] text-neutral-600 dark:bg-neutral-800 dark:text-neutral-300"
                  >
                    {tool}
                  </span>
                ))}
              </div>
            )}
            <div className="custom-scrollbar max-h-[52vh] overflow-y-auto rounded-lg border border-neutral-200 bg-neutral-50 p-3 dark:border-neutral-800 dark:bg-neutral-950/50">
              <ChatMarkdown content={selectedSkillPreview.body} />
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
