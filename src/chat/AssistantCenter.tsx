import { useCallback, useEffect, useMemo, useState, type CSSProperties } from 'react'
import {
  ArrowLeft,
  BookOpen,
  Check,
  Copy,
  Minus,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Save,
  Search,
  Sparkles,
  Trash2,
  Wrench,
} from 'lucide-react'
import { api, type ModelProvider } from '../api/tauri'
import { getSettingsCached } from '../api/settingsCache'
import { Button, IconButton } from '../components/Button'
import { isProviderEnabled } from '../settings/utils'
import { Select } from '../settings/components'
import { useT } from '../settings/i18n'
import { builtinAssistantGlyph } from './assistantIcons'
import { AgentIcon } from '../settings/NavIcons'
import { chatApi } from './api'
import type { ChatAssistant, SkillMeta } from './types'

interface AssistantCenterProps {
  skills: SkillMeta[]
  currentAssistantId?: string | null
  onStartAssistantChat: (assistant: ChatAssistant) => void
  onStartBuilder?: () => void
  onApplyAssistant?: (assistantId: string | null) => void
}

type AssistantDraft = ChatAssistant
type CenterView = 'list' | 'detail' | 'edit'
type SuiteTab = 'plaza' | 'installed' | 'mine'

const assistantColors = ['#6A8FBD', '#2f6ff0', '#4F9D7A', '#8A6FBD', '#B7791F', '#5E8C6A']

function nowSeconds() {
  return Math.floor(Date.now() / 1000)
}

function listFromMaybe<T>(snake?: T[], camel?: T[]): T[] {
  return Array.isArray(snake) ? snake : Array.isArray(camel) ? camel : []
}

function assistantMcpIds(assistant?: ChatAssistant | null): string[] {
  return listFromMaybe(assistant?.mcp_server_ids, assistant?.mcpServerIds)
}

function assistantSkillIds(assistant?: ChatAssistant | null): string[] {
  return listFromMaybe(assistant?.skill_ids, assistant?.skillIds)
}

function toggleId(list: string[], id: string): string[] {
  return list.includes(id) ? list.filter((item) => item !== id) : [...list, id]
}

function normalizeStringList(values?: string[], limit = 64): string[] {
  const out: string[] = []
  for (const value of values ?? []) {
    const item = value.trim()
    if (!item || out.includes(item)) continue
    out.push(item)
    if (out.length >= limit) break
  }
  return out
}

function normalizeAssistantForDraft(assistant: ChatAssistant): AssistantDraft {
  return {
    ...assistant,
    description: assistant.description ?? '',
    icon: assistant.icon ?? 'bot',
    color: assistant.color ?? '#6A8FBD',
    source: assistant.source ?? (assistant.built_in ?? assistant.builtIn ? 'builtin' : 'user'),
    system_prompt: assistant.system_prompt ?? assistant.systemPrompt ?? '',
    provider_id: assistant.provider_id ?? assistant.providerId ?? '',
    model: assistant.model ?? '',
    mcp_server_ids: assistantMcpIds(assistant),
    skill_ids: assistantSkillIds(assistant),
    enabled: assistant.enabled ?? true,
    installed: assistant.installed ?? true,
    archived: assistant.archived ?? false,
    built_in: assistant.built_in ?? assistant.builtIn ?? false,
    created_at: assistant.created_at ?? assistant.createdAt ?? nowSeconds(),
    updated_at: assistant.updated_at ?? assistant.updatedAt ?? nowSeconds(),
  }
}

function createBlankAssistant(): AssistantDraft {
  const now = nowSeconds()
  return {
    id: `asst_${crypto.randomUUID()}`,
    name: '新助手',
    description: '',
    icon: 'bot',
    color: '#6A8FBD',
    source: 'user',
    system_prompt: '',
    provider_id: '',
    model: '',
    mcp_server_ids: [],
    skill_ids: [],
    enabled: true,
    installed: true,
    archived: false,
    built_in: false,
    created_at: now,
    updated_at: now,
  }
}

function draftPayload(draft: AssistantDraft): ChatAssistant {
  return {
    ...draft,
    name: draft.name.trim(),
    description: draft.description?.trim() ?? '',
    icon: draft.icon?.trim() || 'bot',
    color: draft.color?.trim() || '#6A8FBD',
    source: draft.source || (draft.built_in ?? draft.builtIn ? 'builtin' : 'user'),
    system_prompt: (draft.system_prompt ?? draft.systemPrompt ?? '').trim(),
    provider_id: (draft.provider_id ?? draft.providerId ?? '').trim(),
    model: draft.provider_id ? (draft.model ?? '').trim() : '',
    mcp_server_ids: normalizeStringList(assistantMcpIds(draft)),
    skill_ids: normalizeStringList(assistantSkillIds(draft)),
    enabled: draft.enabled ?? true,
    installed: draft.installed ?? true,
    archived: false,
    built_in: draft.built_in ?? draft.builtIn ?? false,
    created_at: draft.created_at,
    updated_at: nowSeconds(),
  }
}

function assistantMatches(assistant: ChatAssistant, query: string) {
  if (!query) return true
  const text = [assistant.name, assistant.description, assistant.system_prompt ?? assistant.systemPrompt]
    .filter(Boolean)
    .join('\n')
    .toLowerCase()
  return text.includes(query)
}

function providerModels(provider?: ModelProvider): string[] {
  if (!provider) return []
  return provider.enabledModels.length > 0 ? provider.enabledModels : provider.availableModels
}

function suiteStats(assistant: ChatAssistant) {
  return {
    mcp: assistantMcpIds(assistant).length,
    skills: assistantSkillIds(assistant).length,
  }
}

function AssistantSuiteCard({
  assistant,
  index,
  onOpen,
  onStartChat,
  onToggleInstalled,
}: {
  assistant: ChatAssistant
  index: number
  onOpen: (assistant: ChatAssistant) => void
  onStartChat: (assistant: ChatAssistant) => void
  onToggleInstalled: (assistant: ChatAssistant) => void
}) {
  const t = useT()
  const stats = suiteStats(assistant)
  const builtIn = assistant.built_in ?? assistant.builtIn ?? false
  const inFavorites = (assistant.installed ?? true) !== false

  return (
    <article
      role="button"
      tabIndex={0}
      onClick={() => onOpen(assistant)}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault()
          onOpen(assistant)
        }
      }}
      data-tauri-drag-region="false"
      aria-label={t.chatAssistantOpenNamed.replace('{name}', assistant.name)}
      style={{ '--chat-motion-delay': `${Math.min(index, 8) * 24}ms` } as CSSProperties}
      className="chat-motion-fade-up group flex h-full min-w-0 cursor-pointer flex-col rounded-xl border border-neutral-200 bg-white p-3.5 text-left shadow-sm transition-[border-color,box-shadow,transform] duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] hover:-translate-y-0.5 hover:border-neutral-300 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-neutral-900/15 dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700 dark:focus-visible:ring-white/20"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2.5">
          <span
            className="grid size-9 shrink-0 place-items-center rounded-lg border border-neutral-200 bg-white text-[15px] font-semibold dark:border-neutral-700 dark:bg-neutral-950"
            style={{ color: assistant.color || '#6A8FBD' }}
          >
            {builtinAssistantGlyph(assistant.id, 18) ?? (assistant.name.trim().slice(0, 1) || t.chatAssistantAvatarFallback)}
          </span>
          <span className="truncate text-[13.5px] font-semibold leading-tight text-neutral-950 dark:text-neutral-50">
            {assistant.name}
          </span>
        </div>
        {!builtIn && (
          <span className="shrink-0 rounded-full bg-neutral-100 px-2 py-0.5 text-[10.5px] font-medium text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400">
            {t.chatAssistantCustom}
          </span>
        )}
      </div>
      <p className="mt-1.5 line-clamp-2 min-h-[2.4em] flex-1 text-[12px] leading-[1.45] text-neutral-500 dark:text-neutral-400">
        {assistant.description || t.chatAssistantNoDescription}
      </p>
      <div className="mt-2.5 flex items-center justify-between gap-2 border-t border-neutral-100 pt-2.5 dark:border-neutral-800/70">
        <div className="flex min-w-0 items-center gap-1.5 text-[11px] tabular-nums text-neutral-400 dark:text-neutral-500">
          <span className="shrink-0">{stats.mcp} MCP</span>
          <span className="shrink-0 opacity-50">·</span>
          <span className="shrink-0">{t.chatAssistantSkillCount.replace('{n}', String(stats.skills))}</span>
        </div>
        <div
          className="flex shrink-0 items-center gap-1"
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => event.stopPropagation()}
        >
          {inFavorites ? (
            <>
              <IconButton
                size="sm"
                onClick={() => onStartChat(assistant)}
                label={t.chatAssistantStartChatNamed.replace('{name}', assistant.name)}
                title={t.chatAssistantStartChat}
              >
                <Play size={14} />
              </IconButton>
              <IconButton
                size="sm"
                onClick={() => onToggleInstalled(assistant)}
                label={t.chatAssistantRemoveFromFavoritesNamed.replace('{name}', assistant.name)}
                title={t.chatAssistantRemoveFromFavorites}
              >
                <Minus size={14} />
              </IconButton>
            </>
          ) : (
            <Button
              size="sm"
              onClick={() => onToggleInstalled(assistant)}
              title={t.chatAssistantAddToFavorites}
            >
              {t.chatAssistantAdd}
            </Button>
          )}
        </div>
      </div>
    </article>
  )
}

export function AssistantCenter({
  skills,
  currentAssistantId,
  onStartAssistantChat,
  onStartBuilder,
  onApplyAssistant,
}: AssistantCenterProps) {
  const t = useT()
  const [assistants, setAssistants] = useState<ChatAssistant[]>([])
  const [providers, setProviders] = useState<ModelProvider[]>([])
  const [mcpServers, setMcpServers] = useState<Array<{ id: string; name: string }>>([])
  const [selectedId, setSelectedId] = useState<string | null>(currentAssistantId ?? null)
  const [draft, setDraft] = useState<AssistantDraft | null>(null)
  const [query, setQuery] = useState('')
  const [view, setView] = useState<CenterView>('list')
  const [tab, setTab] = useState<SuiteTab>('installed')
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  const loadAssistants = useCallback(async (preferredId?: string | null) => {
    setLoading(true)
    setError('')
    try {
      const data = await chatApi.getAssistants()
      setAssistants(data)
      const nextSelectedId = preferredId ?? currentAssistantId ?? data[0]?.id ?? null
      const selected = data.find((assistant) => assistant.id === nextSelectedId) ?? null
      setSelectedId(selected?.id ?? null)
      setDraft(selected ? normalizeAssistantForDraft(selected) : null)
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || t.chatAssistantLoadFailed)
    } finally {
      setLoading(false)
    }
  }, [currentAssistantId, t])

  const loadProviders = useCallback(async () => {
    try {
      const settings = await getSettingsCached()
      setProviders(settings.providers || [])
      setMcpServers(
        (settings.chatTools?.servers ?? []).map((server) => ({ id: server.id, name: server.name })),
      )
    } catch {
      setProviders([])
      setMcpServers([])
    }
  }, [])

  useEffect(() => {
    void loadAssistants(currentAssistantId)
    void loadProviders()
  }, [currentAssistantId, loadAssistants, loadProviders])

  // 对话搭建落库后会发 chat-assistants-changed,刷新列表让新专家实时出现。
  useEffect(() => {
    const unlistenPromise = api.onChatAssistantsChanged(() => void loadAssistants())
    return () => {
      void unlistenPromise.then((unlisten) => unlisten())
    }
  }, [loadAssistants])

  const selectedAssistant = useMemo(
    () => assistants.find((assistant) => assistant.id === selectedId) ?? null,
    [assistants, selectedId],
  )

  const filteredAssistants = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase()
    return assistants.filter((assistant) => {
      if (!assistantMatches(assistant, normalizedQuery)) return false
      const builtIn = assistant.built_in ?? assistant.builtIn ?? false
      if (tab === 'plaza' && !builtIn) return false
      if (tab === 'installed' && assistant.installed === false) return false
      if (tab === 'mine' && builtIn) return false
      return true
    })
  }, [assistants, query, tab])

  const enabledProviders = useMemo(
    () => providers.filter(isProviderEnabled),
    [providers],
  )

  const selectedProvider = providers.find((provider) => provider.id === (draft?.provider_id ?? draft?.providerId))
  const models = providerModels(selectedProvider)
  const draftMcpIds = assistantMcpIds(draft)
  const draftSkillIds = assistantSkillIds(draft)
  const canApplyCurrent = Boolean(onApplyAssistant)
  const builtInCount = assistants.filter((assistant) => assistant.built_in ?? assistant.builtIn).length
  const installedCount = assistants.filter((assistant) => assistant.installed !== false).length

  const updateDraft = <K extends keyof AssistantDraft>(key: K, value: AssistantDraft[K]) => {
    setDraft((prev) => (prev ? { ...prev, [key]: value } : prev))
  }

  const openDetail = (assistant: ChatAssistant) => {
    setSelectedId(assistant.id)
    setDraft(normalizeAssistantForDraft(assistant))
    setView('detail')
    setError('')
  }

  const handleCreate = () => {
    const blank = createBlankAssistant()
    setSelectedId(null)
    setDraft(blank)
    setView('edit')
    setError('')
  }

  const saveDraft = async (): Promise<ChatAssistant | null> => {
    if (!draft) return null
    const payload = draftPayload(draft)
    if (!payload.name) {
      setError(t.chatAssistantNameRequired)
      return null
    }
    setSaving(true)
    setError('')
    try {
      const exists = assistants.some((assistant) => assistant.id === payload.id)
      const saved = exists
        ? await chatApi.updateAssistant(payload)
        : await chatApi.createAssistant(payload)
      await loadAssistants(saved.id)
      setSelectedId(saved.id)
      setDraft(normalizeAssistantForDraft(saved))
      return saved
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || t.chatAssistantSaveFailed)
      return null
    } finally {
      setSaving(false)
    }
  }

  const handleDuplicate = async (assistant?: ChatAssistant | null) => {
    const target = assistant ?? draft
    if (!target || !assistants.some((item) => item.id === target.id)) return
    setSaving(true)
    setError('')
    try {
      const copy = await chatApi.duplicateAssistant(target.id)
      await loadAssistants(copy.id)
      setSelectedId(copy.id)
      setDraft(normalizeAssistantForDraft(copy))
      setView('edit')
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || t.chatAssistantDuplicateFailed)
    } finally {
      setSaving(false)
    }
  }

  const handleDelete = async () => {
    if (!draft) return
    const exists = assistants.some((assistant) => assistant.id === draft.id)
    if (!exists) {
      setDraft(null)
      setSelectedId(null)
      setView('list')
      return
    }
    if (!window.confirm(t.chatAssistantDeleteConfirm.replace('{name}', draft.name))) return
    setSaving(true)
    setError('')
    try {
      await chatApi.deleteAssistant(draft.id)
      await loadAssistants(null)
      setView('list')
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || t.chatAssistantDeleteFailed)
    } finally {
      setSaving(false)
    }
  }

  const handleStartChat = async (assistant?: ChatAssistant | null) => {
    if (assistant) {
      onStartAssistantChat(assistant)
      return
    }
    const saved = await saveDraft()
    if (saved) onStartAssistantChat(saved)
  }

  const handleApplyAssistant = async (assistant?: ChatAssistant | null) => {
    const target = assistant ?? await saveDraft()
    if (target) onApplyAssistant?.(target.id)
  }

  // 常用（收藏夹）：切换专家是否「加入常用」。加入后才在对话栏的专家选择器里可选、可调用。
  const handleToggleInstalled = async (assistant: ChatAssistant) => {
    const next = (assistant.installed ?? true) === false
    try {
      await chatApi.updateAssistant({ ...assistant, installed: next })
      await loadAssistants(assistant.id)
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || t.chatOperationFailed)
    }
  }

  const renderList = () => (
    <div className="space-y-4">
      <div className="assistant-center-tabs flex min-w-0 items-center gap-1 border-b border-neutral-200 pb-2 dark:border-neutral-800">
          {[
            ['installed', t.chatAssistantTabInstalled, installedCount],
            ['plaza', t.chatAssistantTabPlaza, builtInCount],
            ['mine', t.chatAssistantTabMine, assistants.length - builtInCount],
          ].map(([value, label, count]) => (
            <button
              key={value}
              type="button"
              onClick={() => setTab(value as SuiteTab)}
              className={`flex h-8 items-center gap-2 rounded-md px-2.5 text-[13px] font-medium transition-colors ${
                tab === value
                  ? 'bg-neutral-100 text-neutral-950 dark:bg-neutral-800 dark:text-neutral-50'
                  : 'text-neutral-500 hover:bg-neutral-50 hover:text-neutral-800 dark:text-neutral-400 dark:hover:bg-neutral-900 dark:hover:text-neutral-200'
              }`}
            >
              {label}
              <span className="rounded-full bg-white px-1.5 py-0.5 text-[11px] text-neutral-500 dark:bg-neutral-950 dark:text-neutral-400">
                {count}
              </span>
            </button>
          ))}
      </div>

      {/* 工具行：搜索为主 + 创建动作（与技能商店/MCP 市场的工具行同规格） */}
      <div className="flex items-center gap-2">
        <div className="relative min-w-0 flex-1">
          <Search
            size={16}
            className="pointer-events-none absolute left-3.5 top-1/2 -translate-y-1/2 text-neutral-400"
          />
          <input
            type="text"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t.chatAssistantSearch}
            className="h-10 w-full rounded-md border border-neutral-200 bg-white pl-10 pr-4 text-[14px] outline-none placeholder:text-neutral-400 focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
            data-tauri-drag-region="false"
          />
        </div>
        {onStartBuilder && (
          <Button
            onClick={() => onStartBuilder()}
            title={t.chatAssistantBuildViaChat}
            data-tauri-drag-region="false"
          >
            <Sparkles size={16} />
            {t.chatAssistantAiCreate}
          </Button>
        )}
        <Button variant="primary" onClick={handleCreate} data-tauri-drag-region="false">
          <Plus size={16} />
          {t.chatAssistantCreate}
        </Button>
      </div>

      {loading ? (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4">
          {Array.from({ length: 6 }, (_, i) => (
            <div key={i} className="flex flex-col rounded-xl border border-neutral-200/80 p-3.5 dark:border-neutral-800/70">
              <div className="kv-skeleton h-4 w-2/5 rounded" />
              <div className="kv-skeleton mt-2.5 h-3 w-full rounded" />
              <div className="kv-skeleton mt-1.5 h-3 w-3/4 rounded" />
              <div className="kv-skeleton mt-3 h-7 w-full rounded" />
            </div>
          ))}
        </div>
      ) : filteredAssistants.length === 0 ? (
        <div className="grid min-h-[220px] place-items-center rounded-md border border-dashed border-neutral-200 text-[13px] text-neutral-400 dark:border-neutral-800">
          {tab === 'installed' && !query.trim()
            ? t.chatAssistantEmptyFavorites
            : t.chatAssistantNoMatch}
        </div>
      ) : (
        <div key={tab} className="chat-motion-tab-in grid grid-cols-1 gap-3 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4">
          {filteredAssistants.map((assistant, index) => (
            <AssistantSuiteCard
              key={assistant.id}
              assistant={assistant}
              index={index}
              onOpen={openDetail}
              onStartChat={(item) => void handleStartChat(item)}
              onToggleInstalled={(item) => void handleToggleInstalled(item)}
            />
          ))}
        </div>
      )}
    </div>
  )

  const renderDetail = () => {
    const assistant = selectedAssistant
    if (!assistant) return renderList()
    const usedMcpIds = assistantMcpIds(assistant)
    const usedSkillIds = assistantSkillIds(assistant)
    const mcpNames = usedMcpIds.map((id) => mcpServers.find((s) => s.id === id)?.name ?? id)
    const skillNames = usedSkillIds.map((id) => skills.find((s) => s.id === id)?.name ?? id)
    const systemPrompt = assistant.system_prompt ?? assistant.systemPrompt ?? ''
    return (
      <div className="space-y-7">
        <div className="flex flex-col gap-4 border-b border-neutral-200 pb-5 dark:border-neutral-800 lg:flex-row lg:items-start lg:justify-between">
          <div className="flex min-w-0 gap-4">
            <IconButton
              size="md"
              onClick={() => setView('list')}
              className="mt-1"
              label={t.chatAssistantBackToList}
            >
              <ArrowLeft size={18} />
            </IconButton>
            <div
              className="grid size-16 shrink-0 place-items-center rounded-md text-[26px] font-semibold text-white"
              style={{ backgroundColor: assistant.color || '#6A8FBD' }}
            >
              {builtinAssistantGlyph(assistant.id, 32) ?? (assistant.name.trim().slice(0, 1) || t.chatAssistantAvatarFallbackDetail)}
            </div>
            <div className="min-w-0">
              <h2 className="truncate text-[28px] font-semibold tracking-normal text-neutral-950 dark:text-neutral-50">
                {assistant.name}
              </h2>
              <div className="mt-1 text-[13px] font-medium text-neutral-500">
                {(assistant.installed ?? true) === false ? t.chatAssistantNotInFavorites : t.chatAssistantInFavorites}
              </div>
              <p className="mt-6 max-w-5xl text-[16px] leading-8 text-neutral-700 dark:text-neutral-300">
                {assistant.description || t.chatAssistantDetailNoDescription}
              </p>
            </div>
          </div>
          <div className="flex shrink-0 flex-wrap gap-2">
            {(assistant.installed ?? true) === false ? (
              <Button variant="primary" onClick={() => void handleToggleInstalled(assistant)}>
                <Plus size={15} />
                {t.chatAssistantAddToFavorites}
              </Button>
            ) : (
              <Button variant="ghost" onClick={() => void handleToggleInstalled(assistant)}>
                <Minus size={15} />
                {t.chatAssistantRemoveFromFavorites}
              </Button>
            )}
            <Button
              onClick={() => {
                setDraft(normalizeAssistantForDraft(assistant))
                setView('edit')
              }}
            >
              <Pencil size={15} />
              {t.chatAssistantEdit}
            </Button>
            <IconButton
              size="sm"
              onClick={() => void handleDuplicate(assistant)}
              label={t.chatAssistantDuplicate}
            >
              <Copy size={15} />
            </IconButton>
            {canApplyCurrent && (
              <Button
                variant="ghost"
                onClick={() => void handleApplyAssistant(assistant)}
              >
                <Check size={15} />
                {t.chatAssistantApplyToChat}
              </Button>
            )}
            <Button
              variant="primary"
              onClick={() => void handleStartChat(assistant)}
            >
              <Play size={15} />
              {t.chatAssistantStartChat}
            </Button>
          </div>
        </div>

        <section className="space-y-3">
          <h3 className="text-[17px] font-semibold text-neutral-950 dark:text-neutral-50">{t.chatSystemPrompt}</h3>
          <div className="rounded-md border border-neutral-200 px-4 py-3 text-[13px] leading-relaxed whitespace-pre-wrap text-neutral-700 dark:border-neutral-800 dark:text-neutral-300">
            {systemPrompt || t.chatAssistantNoSystemPrompt}
          </div>
        </section>

        <div className="grid gap-6 md:grid-cols-2">
          <section className="space-y-3">
            <h3 className="flex items-center gap-2 text-[17px] font-semibold text-neutral-950 dark:text-neutral-50">
              <Wrench size={16} className="text-neutral-400" />
              MCP <span className="text-neutral-400">({mcpNames.length})</span>
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {mcpNames.length === 0 ? (
                <span className="text-[13px] text-neutral-400">{t.chatAssistantNoMcp}</span>
              ) : mcpNames.map((name) => (
                <span key={name} className="rounded-md bg-neutral-100 px-2.5 py-1 text-[12px] text-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
                  {name}
                </span>
              ))}
            </div>
          </section>

          <section className="space-y-3">
            <h3 className="flex items-center gap-2 text-[17px] font-semibold text-neutral-950 dark:text-neutral-50">
              <BookOpen size={16} className="text-neutral-400" />
              {t.chatAssistantSkills} <span className="text-neutral-400">({skillNames.length})</span>
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {skillNames.length === 0 ? (
                <span className="text-[13px] text-neutral-400">{t.chatAssistantNoSkills}</span>
              ) : skillNames.map((name) => (
                <span key={name} className="rounded-md bg-neutral-100 px-2.5 py-1 text-[12px] text-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
                  {name}
                </span>
              ))}
            </div>
          </section>
        </div>
      </div>
    )
  }

  const renderEdit = () => {
    if (!draft) return renderList()
    return (
      <div className="space-y-6">
        <div className="flex flex-col gap-4 border-b border-neutral-200 pb-4 dark:border-neutral-800 lg:flex-row lg:items-center lg:justify-between">
          <div className="flex min-w-0 items-center gap-3">
            <IconButton
              size="md"
              onClick={() => setView(selectedAssistant ? 'detail' : 'list')}
              label={t.chatAssistantBack}
            >
              <ArrowLeft size={18} />
            </IconButton>
            <div className="min-w-0">
              <h2 className="truncate text-[24px] font-semibold text-neutral-950 dark:text-neutral-50">{t.chatAssistantEditTitle}</h2>
              <p className="mt-1 truncate text-[13px] text-neutral-500">
                {draft.built_in ? t.chatAssistantBuiltinTemplate : t.chatAssistantCustomSuite}
              </p>
            </div>
          </div>
          <div className="flex shrink-0 flex-wrap gap-2">
            <IconButton
              size="sm"
              variant="danger"
              onClick={() => void handleDelete()}
              disabled={saving}
              label={t.chatAssistantDeleteTitle}
              title={t.chatDelete}
            >
              <Trash2 size={15} />
            </IconButton>
            <Button
              variant="ghost"
              onClick={() => void saveDraft().then((saved) => {
                if (saved) setView('detail')
              })}
              disabled={saving}
            >
              <Save size={15} />
              {t.save}
            </Button>
            <Button
              variant="primary"
              onClick={() => void handleStartChat()}
              disabled={saving}
            >
              <Play size={15} />
              {t.chatAssistantStartChat}
            </Button>
          </div>
        </div>

        <div className="space-y-6">
          <section className="space-y-5">
            <div className="grid gap-3 sm:grid-cols-[7rem_minmax(0,1fr)]">
              <label className="block">
                <span className="mb-1.5 block text-[12px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatAssistantIcon}</span>
                <input
                  type="text"
                  value={draft.icon ?? ''}
                  onChange={(event) => updateDraft('icon', event.target.value)}
                  className="h-10 w-full rounded-md border border-neutral-200 bg-white px-3 text-[13px] outline-none focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
                />
              </label>
              <label className="block">
                <span className="mb-1.5 block text-[12px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatAssistantName}</span>
                <input
                  type="text"
                  value={draft.name}
                  onChange={(event) => updateDraft('name', event.target.value)}
                  className="h-10 w-full rounded-md border border-neutral-200 bg-white px-3 text-[15px] font-medium outline-none focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
                />
              </label>
            </div>
            <label className="block">
              <span className="mb-1.5 block text-[12px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatAssistantDescription}</span>
              <input
                type="text"
                value={draft.description ?? ''}
                onChange={(event) => updateDraft('description', event.target.value)}
                className="h-10 w-full rounded-md border border-neutral-200 bg-white px-3 text-[13px] outline-none focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
              />
            </label>
            <label className="block">
              <span className="mb-1.5 block text-[12px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatSystemPrompt}</span>
              <textarea
                value={draft.system_prompt ?? ''}
                onChange={(event) => updateDraft('system_prompt', event.target.value)}
                rows={9}
                className="custom-scrollbar w-full resize-none rounded-md border border-neutral-200 bg-white px-3 py-2.5 text-[13px] leading-relaxed text-neutral-900 outline-none focus:border-neutral-300 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
              />
            </label>
            <div className="grid gap-4 md:grid-cols-2">
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <span className="text-[12px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatAssistantMcpServers}</span>
                  <span className="text-[11px] text-neutral-400">{t.chatAssistantSelectedCount.replace('{n}', String(draftMcpIds.length))}</span>
                </div>
                <div className="custom-scrollbar max-h-56 space-y-1 overflow-y-auto rounded-md border border-neutral-200 p-2 dark:border-neutral-700">
                  {mcpServers.length === 0 ? (
                    <div className="px-1 py-2 text-[12px] text-neutral-400">{t.chatAssistantNoMcpConfigured}</div>
                  ) : mcpServers.map((server) => (
                    <label key={server.id} className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1.5 text-[13px] hover:bg-neutral-50 dark:hover:bg-neutral-800">
                      <input
                        type="checkbox"
                        checked={draftMcpIds.includes(server.id)}
                        onChange={() => updateDraft('mcp_server_ids', toggleId(draftMcpIds, server.id))}
                        className="size-4 accent-neutral-900 dark:accent-neutral-100"
                      />
                      <span className="min-w-0 truncate text-neutral-700 dark:text-neutral-200">{server.name}</span>
                    </label>
                  ))}
                </div>
              </div>
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <span className="text-[12px] font-medium text-neutral-600 dark:text-neutral-300">{t.chatAssistantSkills}</span>
                  <span className="text-[11px] text-neutral-400">{t.chatAssistantSelectedCount.replace('{n}', String(draftSkillIds.length))}</span>
                </div>
                <div className="custom-scrollbar max-h-56 space-y-1 overflow-y-auto rounded-md border border-neutral-200 p-2 dark:border-neutral-700">
                  {skills.length === 0 ? (
                    <div className="px-1 py-2 text-[12px] text-neutral-400">{t.chatAssistantNoAvailableSkills}</div>
                  ) : skills.map((skill) => (
                    <label key={skill.id} className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1.5 text-[13px] hover:bg-neutral-50 dark:hover:bg-neutral-800">
                      <input
                        type="checkbox"
                        checked={draftSkillIds.includes(skill.id)}
                        onChange={() => updateDraft('skill_ids', toggleId(draftSkillIds, skill.id))}
                        className="size-4 accent-neutral-900 dark:accent-neutral-100"
                      />
                      <span className="min-w-0 truncate text-neutral-700 dark:text-neutral-200">{skill.name}</span>
                    </label>
                  ))}
                </div>
              </div>
            </div>
          </section>

          <section className="grid gap-4 lg:grid-cols-3">
            <section className="space-y-3 rounded-md border border-neutral-200 p-3 dark:border-neutral-800">
              <div className="text-[12px] font-semibold text-neutral-700 dark:text-neutral-200">{t.chatAssistantRunSettings}</div>
              <label className="block">
                <span className="mb-1 block text-[11px] text-neutral-500 dark:text-neutral-400">{t.chatAssistantModelProvider}</span>
                <Select
                  value={draft.provider_id ?? ''}
                  onChange={(providerId) => {
                    const provider = providers.find((item) => item.id === providerId)
                    updateDraft('provider_id', providerId)
                    updateDraft('model', providerModels(provider)[0] ?? '')
                  }}
                  options={[
                    { value: '', label: t.chatAssistantFollowChatDefault },
                    ...enabledProviders.map((provider) => ({
                      value: provider.id,
                      label: provider.name,
                    })),
                  ]}
                />
              </label>
              <label className="block">
                <span className="mb-1 block text-[11px] text-neutral-500 dark:text-neutral-400">{t.sectionModel}</span>
                <Select
                  value={draft.model ?? ''}
                  onChange={(model) => updateDraft('model', model)}
                  options={
                    draft.provider_id
                      ? models.map((model) => ({ value: model, label: model }))
                      : [{ value: '', label: t.chatAssistantFollowChatDefault }]
                  }
                />
              </label>
            </section>

            <section className="space-y-3 rounded-md border border-neutral-200 p-3 dark:border-neutral-800">
              <div className="text-[12px] font-semibold text-neutral-700 dark:text-neutral-200">{t.chatAssistantColor}</div>
              <div className="flex flex-wrap gap-1.5">
                {assistantColors.map((color) => (
                  <button
                    key={color}
                    type="button"
                    onClick={() => updateDraft('color', color)}
                    className={`size-6 rounded-full border ${
                      draft.color === color
                        ? 'border-neutral-900 ring-2 ring-neutral-300 dark:border-neutral-100 dark:ring-neutral-600'
                        : 'border-transparent'
                    }`}
                    style={{ backgroundColor: color }}
                    aria-label={t.chatAssistantSelectColorNamed.replace('{name}', color)}
                  />
                ))}
              </div>
            </section>

            <section className="space-y-2 rounded-md border border-neutral-200 p-3 dark:border-neutral-800">
              <div className="text-[12px] font-semibold text-neutral-700 dark:text-neutral-200">{t.chatAssistantCurrentConfig}</div>
              <div className="space-y-1 text-[11px] text-neutral-500 dark:text-neutral-400">
                <div className="truncate">{t.chatAssistantConfigModel.replace('{name}', draft.model || t.chatAssistantFollowChatDefault)}</div>
                <div className="truncate">{t.chatAssistantConfigMcp.replace('{n}', String(draftMcpIds.length))}</div>
                <div className="truncate">{t.chatAssistantConfigSkills.replace('{n}', String(draftSkillIds.length))}</div>
              </div>
              {providers.length === 0 && (
                <div className="rounded-md bg-amber-50 px-2 py-1.5 text-[11px] text-amber-700 dark:bg-amber-950/30 dark:text-amber-300">
                  {t.chatAssistantNoProviders}
                </div>
              )}
            </section>
          </section>
        </div>
      </div>
    )
  }

  return (
    <div className="assistant-center-root flex h-full min-h-0 flex-col text-neutral-900 dark:text-neutral-100">
      {/* 顶栏：与聊天主区同底色、无分隔，可拖拽，右侧避开窗口按钮 */}

      {/* 内容区：直接坐在白底上，与聊天主区无缝 */}
      <main className="custom-scrollbar min-h-0 flex-1 overflow-y-auto px-6 py-6">
          <div className="mx-auto max-w-7xl space-y-4">
            <header className="border-b border-neutral-200 pb-5 dark:border-neutral-800">
              <h1 className="flex items-center gap-2.5 truncate text-[28px] font-semibold tracking-normal text-neutral-950 dark:text-neutral-50">
                <AgentIcon size={24} className="shrink-0 text-neutral-500" />
                {t.chatAssistantTitle}
              </h1>
              <div className="mt-3.5 flex min-w-0 items-center gap-4">
                <p className="min-w-0 flex-1 text-[14px] leading-relaxed text-neutral-500 dark:text-neutral-400">
                  {t.chatAssistantSubtitle}
                </p>
                <IconButton
                  size="lg"
                  onClick={() => void loadAssistants(selectedId)}
                  label={t.chatAssistantRefresh}
                  title={t.chatAssistantRefreshShort}
                  data-tauri-drag-region="false"
                >
                  <RefreshCw size={17} />
                </IconButton>
              </div>
            </header>

            {error && (
              <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-[12px] text-red-700 dark:border-red-900/60 dark:bg-red-950/30 dark:text-red-300">
                {error}
              </div>
            )}

            {view === 'list' && renderList()}
            {view === 'detail' && renderDetail()}
            {view === 'edit' && renderEdit()}
          </div>
        </main>
    </div>
  )
}
