import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react'
import {
  Archive,
  ArchiveRestore,
  Check,
  FolderKanban,
  Layers,
  Loader2,
  MessagesSquare,
  MoreHorizontal,
  Pin,
  PinOff,
  RefreshCw,
  Search,
  Star,
  Trash2,
} from 'lucide-react'
import { save } from '@tauri-apps/plugin-dialog'
import { chatApi } from './api'
import type {
  ChatProject,
  ChatSet,
  ConversationLibraryDensity,
  ConversationLibraryGroup,
  ConversationLibraryOrder,
  ConversationLibraryQuery,
  ConversationLibraryShelf,
  ConversationLibrarySort,
  ConversationSearchHit,
} from './types'
import { conversationMarkdownFilename } from './conversationExport'
import { IconButton, Button } from '../components/Button'
import { Select, Toggle } from '../settings/components'
import { useT, type Lang } from '../settings/i18n'
import {
  conversationOwnerLabel,
  dayBucket,
  dayBucketLabel,
  formatRelativeTime,
  libraryTimestamp,
  shortModelName,
  type DayBucket,
} from './sessionLibrary/format'
import { HighlightText } from './searchHighlight'

const PAGE_SIZE = 80

/** 内容区宽度档位（测的是对话库根节点，不是浏览器视口）。 */
type LayoutTier = {
  /** 整页内容宽 */
  page: number
  /** 结果表宽（不含书架） */
  table: number
}

/**
 * 列策略：必须按表格真实宽度，不能用 sm/md 视口断点
 * （聊天侧栏 + 书架会让视口宽 ≠ 表格宽）。
 */
function columnFlags(tableWidth: number, density: ConversationLibraryDensity) {
  // 行固定占位约：checkbox7 + gap + title + time14 + menu8 + padding ≈ 120，再加副列
  if (density === 'compact' || tableWidth < 480) {
    return { model: false, owner: false, msgs: false, preview: false }
  }
  if (tableWidth < 560) {
    return { model: false, owner: false, msgs: false, preview: density === 'comfortable' }
  }
  if (tableWidth < 720) {
    return { model: false, owner: true, msgs: false, preview: true }
  }
  if (tableWidth < 900) {
    return { model: true, owner: true, msgs: false, preview: true }
  }
  return { model: true, owner: true, msgs: true, preview: true }
}

type ShelfId = ConversationLibraryShelf

interface SessionCenterProps {
  lang: Lang
  currentConversationId?: string
  generatingConversationIds?: ReadonlySet<string>
  onSelectConversation: (id: string, conversation?: ConversationSearchHit) => void
  onConversationDeleted?: (id: string) => void
  onForceDropConversation?: (id: string) => void
  onConversationsChanged?: () => void
}

interface LibraryState {
  items: ConversationSearchHit[]
  total: number
  loading: boolean
  loadingMore: boolean
  error: string
}

export function SessionCenter({
  lang,
  currentConversationId,
  generatingConversationIds = new Set(),
  onSelectConversation,
  onConversationDeleted,
  onForceDropConversation,
  onConversationsChanged,
}: SessionCenterProps) {
  const t = useT()
  const searchInputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const rootRef = useRef<HTMLDivElement>(null)
  const bodyRef = useRef<HTMLDivElement>(null)
  const tableHostRef = useRef<HTMLDivElement>(null)
  const lastClickedIdRef = useRef<string | null>(null)

  const [layout, setLayout] = useState<LayoutTier>({ page: 1200, table: 900 })
  const [shelf, setShelf] = useState<ShelfId>('all')
  const [projectId, setProjectId] = useState<string | null>(null)
  const [setId, setSetId] = useState<string | null>(null)
  const [searchInput, setSearchInput] = useState('')
  const [debouncedQ, setDebouncedQ] = useState('')
  const [sort, setSort] = useState<ConversationLibrarySort>('updated')
  const [order, setOrder] = useState<ConversationLibraryOrder>('desc')
  const [groupBy, setGroupBy] = useState<ConversationLibraryGroup>('day')
  const [density, setDensity] = useState<ConversationLibraryDensity>('comfortable')
  const [fullText, setFullText] = useState(true)

  /** 窄屏书架：chip 横条；宽屏左侧纵栏 */
  const shelfAsChips = layout.page < 720
  const cols = useMemo(() => columnFlags(layout.table, density), [layout.table, density])

  useLayoutEffect(() => {
    const measure = () => {
      const page = rootRef.current?.clientWidth ?? 1200
      // 优先表格宿主；芯片模式下表格占满 body
      const table =
        tableHostRef.current?.clientWidth ||
        bodyRef.current?.clientWidth ||
        page
      setLayout((prev) =>
        Math.abs(prev.page - page) < 1 && Math.abs(prev.table - table) < 1
          ? prev
          : { page, table },
      )
    }
    measure()
    const ro = new ResizeObserver(() => measure())
    const nodes = [rootRef.current, bodyRef.current, tableHostRef.current].filter(
      (n): n is HTMLDivElement => Boolean(n),
    )
    for (const n of nodes) ro.observe(n)
    // 下一帧再测一次：首屏 aside 挂载后表格宽度才稳定
    const raf = requestAnimationFrame(measure)
    return () => {
      cancelAnimationFrame(raf)
      ro.disconnect()
    }
  }, [shelfAsChips])

  const [projects, setProjects] = useState<ChatProject[]>([])
  const [sets, setSets] = useState<ChatSet[]>([])
  const [state, setState] = useState<LibraryState>({
    items: [],
    total: 0,
    loading: true,
    loadingMore: false,
    error: '',
  })
  const [selected, setSelected] = useState<Set<string>>(() => new Set())
  const [busy, setBusy] = useState(false)
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null)
  const [renameId, setRenameId] = useState<string | null>(null)
  const [renameDraft, setRenameDraft] = useState('')
  const [moveOpen, setMoveOpen] = useState<'set' | 'project' | null>(null)

  // debounce search
  useEffect(() => {
    const timer = window.setTimeout(() => setDebouncedQ(searchInput.trim()), 250)
    return () => window.clearTimeout(timer)
  }, [searchInput])

  // keyboard: / focus search, Esc clear selection / menu
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === '/' && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const tag = (e.target as HTMLElement)?.tagName
        if (tag === 'INPUT' || tag === 'TEXTAREA' || (e.target as HTMLElement)?.isContentEditable) return
        e.preventDefault()
        searchInputRef.current?.focus()
      }
      if (e.key === 'Escape') {
        setSelected(new Set())
        setMenu(null)
        setMoveOpen(null)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const buildQuery = useCallback(
    (offset: number): ConversationLibraryQuery => ({
      offset,
      limit: PAGE_SIZE,
      sort,
      order,
      q: debouncedQ || undefined,
      fullText,
      shelf,
      projectId: projectId,
      setId: setId,
    }),
    [debouncedQ, fullText, order, projectId, setId, shelf, sort],
  )

  const itemsLenRef = useRef(0)
  itemsLenRef.current = state.items.length
  const loadingMoreRef = useRef(false)
  /** 书架/筛选切换时丢弃过期响应，避免旧页回写造成闪一下。 */
  const loadSeqRef = useRef(0)

  const loadPage = useCallback(
    async (opts?: { append?: boolean }) => {
      const append = opts?.append ?? false
      if (append) {
        if (loadingMoreRef.current) return
        loadingMoreRef.current = true
      } else {
        loadSeqRef.current += 1
      }
      const seq = loadSeqRef.current
      setState((s) => ({
        ...s,
        // 已有列表时只标 loading，不先清空，避免书架切换白闪
        loading: !append,
        loadingMore: append,
        error: '',
      }))
      try {
        const offset = append ? itemsLenRef.current : 0
        const [page, projectData, setData] = await Promise.all([
          chatApi.queryConversations(buildQuery(offset)),
          append ? Promise.resolve(null) : chatApi.getProjects(),
          append ? Promise.resolve(null) : chatApi.getSets(),
        ])
        if (seq !== loadSeqRef.current) return
        if (!append) {
          if (projectData) setProjects(projectData)
          if (setData) setSets(setData)
        }
        setState((s) => ({
          items: append ? [...s.items, ...page.items] : page.items,
          total: page.total,
          loading: false,
          loadingMore: false,
          error: '',
        }))
        if (!append) setSelected(new Set())
      } catch (err) {
        if (seq !== loadSeqRef.current) return
        setState((s) => ({
          ...s,
          loading: false,
          loadingMore: false,
          error: err instanceof Error ? err.message : String(err),
        }))
      } finally {
        if (append) loadingMoreRef.current = false
      }
    },
    [buildQuery],
  )

  // reset & reload when filters change
  useEffect(() => {
    void loadPage({ append: false })
  }, [loadPage])

  const hasMore = state.items.length < state.total

  const onScrollList = useCallback(() => {
    const el = listRef.current
    if (!el || state.loadingMore || !hasMore || state.loading) return
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 240) {
      void loadPage({ append: true })
    }
  }, [hasMore, loadPage, state.loading, state.loadingMore])

  const notify = useCallback(() => {
    onConversationsChanged?.()
  }, [onConversationsChanged])

  const toggleSelect = useCallback((id: string, shiftKey: boolean) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (shiftKey && lastClickedIdRef.current) {
        const ids = state.items.map((c) => c.id)
        const a = ids.indexOf(lastClickedIdRef.current)
        const b = ids.indexOf(id)
        if (a >= 0 && b >= 0) {
          const [lo, hi] = a < b ? [a, b] : [b, a]
          for (let i = lo; i <= hi; i++) next.add(ids[i])
          return next
        }
      }
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
    lastClickedIdRef.current = id
  }, [state.items])

  const selectAllVisible = useCallback(() => {
    setSelected(new Set(state.items.map((c) => c.id)))
  }, [state.items])

  const clearSelection = useCallback(() => setSelected(new Set()), [])

  const selectedIds = useMemo(() => [...selected], [selected])

  const runBulk = useCallback(
    async (action: () => Promise<void>) => {
      if (selectedIds.length === 0 || busy) return
      setBusy(true)
      try {
        await action()
        notify()
        await loadPage({ append: false })
      } catch (err) {
        window.alert(err instanceof Error ? err.message : String(err))
      } finally {
        setBusy(false)
        setMoveOpen(null)
      }
    },
    [busy, loadPage, notify, selectedIds.length],
  )

  const bulkPin = (pinned: boolean) =>
    void runBulk(async () => {
      await chatApi.bulkUpdateConversations(selectedIds, { pinned })
    })

  const bulkArchive = (archived: boolean) =>
    void runBulk(async () => {
      await chatApi.bulkUpdateConversations(selectedIds, { archived })
      if (archived && currentConversationId && selected.has(currentConversationId)) {
        onConversationDeleted?.(currentConversationId)
      }
    })

  const bulkMoveProject = (pid: string | null) =>
    void runBulk(async () => {
      await chatApi.bulkUpdateConversations(selectedIds, { projectId: pid })
    })

  const bulkMoveSet = (sid: string | null) =>
    void runBulk(async () => {
      await chatApi.bulkUpdateConversations(selectedIds, { setId: sid })
    })

  const bulkDelete = () => {
    if (selectedIds.length === 0) return
    if (!window.confirm(t.chatLibBulkDeleteConfirm.replace('{n}', String(selectedIds.length)))) return
    void runBulk(async () => {
      for (const id of selectedIds) {
        if (generatingConversationIds.has(id)) onForceDropConversation?.(id)
      }
      const { warnings } = await chatApi.bulkDeleteConversations(selectedIds)
      if (currentConversationId && selected.has(currentConversationId)) {
        onConversationDeleted?.(currentConversationId)
      }
      if (warnings.length > 0) {
        window.alert(t.chatDeleteConversationPartial + warnings.slice(0, 8).join('\n'))
      }
    })
  }

  const bulkExport = () =>
    void runBulk(async () => {
      for (const id of selectedIds) {
        const row = state.items.find((c) => c.id === id)
        const title = row?.title || id
        const path = await save({
          defaultPath: conversationMarkdownFilename(title),
          filters: [{ name: 'Markdown', extensions: ['md'] }],
        })
        if (!path) continue
        await chatApi.exportConversationMarkdown(id, path, lang)
      }
    })

  const patchOne = useCallback(
    async (id: string, patch: { pinned?: boolean; archived?: boolean; title?: string; projectId?: string | null; setId?: string | null }) => {
      setBusy(true)
      try {
        await chatApi.updateConversation(id, patch)
        if (patch.archived && currentConversationId === id) onConversationDeleted?.(id)
        notify()
        await loadPage({ append: false })
      } catch (err) {
        window.alert(err instanceof Error ? err.message : String(err))
      } finally {
        setBusy(false)
        setMenu(null)
        setRenameId(null)
      }
    },
    [currentConversationId, loadPage, notify, onConversationDeleted],
  )

  const deleteOne = useCallback(
    async (id: string) => {
      if (!window.confirm(t.chatDeleteConversationConfirm)) return
      if (generatingConversationIds.has(id)) onForceDropConversation?.(id)
      setBusy(true)
      try {
        const warnings = await chatApi.deleteConversation(id)
        if (warnings.length > 0) {
          window.alert(t.chatDeleteConversationPartial + warnings.join('\n'))
        }
        if (currentConversationId === id) onConversationDeleted?.(id)
        notify()
        await loadPage({ append: false })
      } catch (err) {
        window.alert(t.chatDeleteConversationFailed + (err instanceof Error ? err.message : String(err)))
      } finally {
        setBusy(false)
        setMenu(null)
      }
    },
    [
      currentConversationId,
      generatingConversationIds,
      loadPage,
      notify,
      onConversationDeleted,
      onForceDropConversation,
      t,
    ],
  )

  const grouped = useMemo(() => {
    if (groupBy === 'none') {
      return [{ key: 'all', label: '', items: state.items }]
    }
    const map = new Map<string, { label: string; items: ConversationSearchHit[] }>()
    const push = (key: string, label: string, c: ConversationSearchHit) => {
      let g = map.get(key)
      if (!g) {
        g = { label, items: [] }
        map.set(key, g)
      }
      g.items.push(c)
    }
    for (const c of state.items) {
      if (groupBy === 'day' || groupBy === 'week') {
        // 按日/周分组与当前排序键对齐：最近创建看 created_at，其余看 updated_at。
        const b: DayBucket = dayBucket(libraryTimestamp(c, sort))
        // week grouping collapses day buckets into fewer: today/yesterday stay, rest week/older
        const key = groupBy === 'week' && (b === 'today' || b === 'yesterday') ? b : b === 'today' || b === 'yesterday' ? b : b
        push(key, dayBucketLabel(key, t), c)
      } else if (groupBy === 'project') {
        const pid = c.project_id ?? c.projectId ?? c.folder ?? '__none__'
        const label =
          conversationOwnerLabel(c, projects, sets, t) ||
          (pid === '__none__' ? t.chatLibUncategorized : String(pid))
        push(String(pid), label.startsWith(t.chatSetPrefix) ? t.chatLibUncategorized : label, c)
      } else if (groupBy === 'set') {
        const sid = c.set_id ?? c.setId ?? '__none__'
        const name = sets.find((s) => s.id === sid)?.name
        push(String(sid), name ? `${t.chatSetPrefix} · ${name}` : t.chatLibUncategorized, c)
      }
    }
    return [...map.entries()].map(([key, v]) => ({ key, label: v.label, items: v.items }))
  }, [groupBy, projects, sets, sort, state.items, t])

  const shelves: Array<{ id: ShelfId; label: string; icon: typeof Star }> = [
    { id: 'all', label: t.chatLibShelfAll, icon: MessagesSquare },
    { id: 'starred', label: t.chatLibShelfStarred, icon: Star },
    { id: 'uncategorized', label: t.chatLibUncategorized, icon: Layers },
    { id: 'recent7d', label: t.chatLibShelfRecent7d, icon: RefreshCw },
    { id: 'archived', label: t.chatLibShelfArchived, icon: Archive },
  ]

  const pickShelf = (id: ShelfId) => {
    setShelf(id)
    setProjectId(null)
    setSetId(null)
  }

  const pickProject = (id: string | null) => {
    setProjectId(id)
    setSetId(null)
    setShelf('all')
  }

  const pickSet = (id: string | null) => {
    setSetId(id)
    setProjectId(null)
    setShelf('all')
  }

  const menuConv = menu ? state.items.find((c) => c.id === menu.id) : undefined
  const compactPad = layout.page < 640

  // 与 SkillStore / Knowledge / MCP 中心一致：自绘 Select，不用原生 <select>
  const sortOptions = useMemo(
    () => [
      { value: 'updated:desc', label: t.chatLibSortUpdatedDesc },
      { value: 'updated:asc', label: t.chatLibSortUpdatedAsc },
      { value: 'created:desc', label: t.chatLibSortCreatedDesc },
      { value: 'title:asc', label: t.chatLibSortTitleAsc },
      { value: 'messages:desc', label: t.chatLibSortMessagesDesc },
    ],
    [t],
  )
  const groupOptions = useMemo(
    () => [
      { value: 'none', label: t.chatLibGroupNone },
      { value: 'day', label: t.chatLibGroupByDay },
      { value: 'week', label: t.chatLibGroupByWeek },
      { value: 'project', label: t.chatLibGroupByProject },
      { value: 'set', label: t.chatLibGroupBySet },
    ],
    [t],
  )
  const densityOptions = useMemo(
    () => [
      { value: 'comfortable', label: t.chatLibDensityComfortable },
      { value: 'compact', label: t.chatLibDensityCompact },
    ],
    [t],
  )

  const shelfNavVertical = (
    <>
      <nav className="flex flex-col gap-0.5">
        <div className="mb-1 px-2 text-[11px] font-medium uppercase tracking-wide text-neutral-400">
          {t.chatLibShelf}
        </div>
        {shelves.map(({ id, label, icon: Icon }) => {
          const active = shelf === id && !projectId && !setId
          return (
            <button
              key={id}
              type="button"
              onClick={() => pickShelf(id)}
              className={`flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] transition-colors ${
                active
                  ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-white/[0.08] dark:text-neutral-50'
                  : 'text-neutral-600 hover:bg-neutral-50 dark:text-neutral-300 dark:hover:bg-white/[0.05]'
              }`}
            >
              <Icon size={14} className="shrink-0 opacity-70" />
              <span className="truncate">{label}</span>
            </button>
          )
        })}
      </nav>

      {sets.length > 0 && (
        <nav className="flex flex-col gap-0.5">
          <div className="mb-1 flex items-center gap-1 px-2 text-[11px] font-medium uppercase tracking-wide text-neutral-400">
            <Layers size={11} /> {t.chatTabSets}
          </div>
          {sets.map((s) => {
            const active = setId === s.id
            return (
              <button
                key={s.id}
                type="button"
                onClick={() => pickSet(active ? null : s.id)}
                className={`truncate rounded-md px-2 py-1.5 text-left text-[13px] transition-colors ${
                  active
                    ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-white/[0.08] dark:text-neutral-50'
                    : 'text-neutral-600 hover:bg-neutral-50 dark:text-neutral-300 dark:hover:bg-white/[0.05]'
                }`}
              >
                {s.name}
              </button>
            )
          })}
        </nav>
      )}

      {projects.length > 0 && (
        <nav className="flex flex-col gap-0.5">
          <div className="mb-1 flex items-center gap-1 px-2 text-[11px] font-medium uppercase tracking-wide text-neutral-400">
            <FolderKanban size={11} /> {t.chatTabProjects}
          </div>
          {projects.map((p) => {
            const active = projectId === p.id
            return (
              <button
                key={p.id}
                type="button"
                onClick={() => pickProject(active ? null : p.id)}
                className={`truncate rounded-md px-2 py-1.5 text-left text-[13px] transition-colors ${
                  active
                    ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-white/[0.08] dark:text-neutral-50'
                    : 'text-neutral-600 hover:bg-neutral-50 dark:text-neutral-300 dark:hover:bg-white/[0.05]'
                }`}
              >
                {p.name}
              </button>
            )
          })}
        </nav>
      )}
    </>
  )

  const chipBtn = (active: boolean) =>
    `shrink-0 rounded-full px-2.5 py-1 text-[12px] transition-colors ${
      active
        ? 'bg-neutral-900 font-medium text-white dark:bg-neutral-100 dark:text-neutral-900'
        : 'bg-neutral-100 text-neutral-600 hover:bg-neutral-200 dark:bg-white/[0.06] dark:text-neutral-300 dark:hover:bg-white/[0.1]'
    }`

  return (
    <div
      ref={rootRef}
      className="assistant-center-root flex h-full min-h-0 flex-col text-neutral-900 dark:text-neutral-100"
    >
      {/* Header */}
      <div
        className={`shrink-0 border-b border-neutral-200 pb-3 pt-5 dark:border-white/[0.07] ${
          compactPad ? 'px-3' : 'px-6 pb-4 pt-6'
        }`}
      >
        <div className="mx-auto flex w-full max-w-[1200px] items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <h1
              className={`flex min-w-0 items-center gap-2 font-semibold tracking-normal text-neutral-950 dark:text-neutral-50 ${
                compactPad ? 'text-[20px]' : 'gap-2.5 text-[26px]'
              }`}
            >
              <MessagesSquare size={compactPad ? 18 : 22} className="shrink-0 text-neutral-500" />
              <span className="truncate">{t.chatNavSessions}</span>
            </h1>
            {!compactPad && (
              <p className="mt-2 max-w-2xl text-[13.5px] leading-relaxed text-neutral-500 dark:text-neutral-400">
                {t.chatSessionSubtitle}
              </p>
            )}
          </div>
          <div className="flex shrink-0 items-center gap-2 pt-0.5">
            <span className="hidden text-[12px] tabular-nums text-neutral-400 sm:inline">
              {state.loading
                ? t.chatLoading
                : t.chatLibCount.replace('{n}', String(state.total))}
            </span>
            <IconButton size="md" label={t.chatSessionRefresh} onClick={() => void loadPage({ append: false })} disabled={state.loading || busy}>
              <RefreshCw size={15} className={state.loading ? 'animate-spin' : ''} />
            </IconButton>
          </div>
        </div>

        {/* Toolbar：Input + Select + Toggle，与技能商店/知识库/设置同一套组件 */}
        <div className="mx-auto mt-3 flex w-full max-w-[1200px] flex-col gap-2">
          <div className="relative w-full min-w-0">
            <Search size={14} className="pointer-events-none absolute left-2.5 top-1/2 z-[1] -translate-y-1/2 text-neutral-400" />
            <input
              ref={searchInputRef}
              type="search"
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
              placeholder={t.chatSessionSearchPlaceholder}
              data-tauri-drag-region="false"
              className="kv-input w-full min-w-0 !pl-8"
            />
          </div>
          <div className="flex min-w-0 flex-wrap items-center gap-2.5">
            <Select
              className="w-[148px] shrink-0"
              value={`${sort}:${order}`}
              onChange={(value) => {
                const [s, o] = value.split(':') as [ConversationLibrarySort, ConversationLibraryOrder]
                setSort(s)
                setOrder(o)
              }}
              options={sortOptions}
              title={t.chatLibSort}
            />
            <Select
              className="w-[132px] shrink-0"
              value={groupBy}
              onChange={(value) => setGroupBy(value as ConversationLibraryGroup)}
              options={groupOptions}
            />
            <Select
              className="w-[100px] shrink-0"
              value={density}
              onChange={(value) => setDensity(value as ConversationLibraryDensity)}
              options={densityOptions}
            />
            <div className="flex h-[30px] shrink-0 items-center gap-2 border-l border-neutral-200 pl-3 dark:border-white/[0.08]">
              <Toggle checked={fullText} onChange={setFullText} ariaLabel={t.chatLibFullText} />
              <span className="whitespace-nowrap text-[12.5px] text-neutral-600 dark:text-neutral-300">
                {t.chatLibFullText}
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* Body */}
      <div
        ref={bodyRef}
        className={`mx-auto flex min-h-0 w-full max-w-[1200px] flex-1 ${
          shelfAsChips ? 'flex-col' : 'flex-row'
        } ${compactPad ? 'gap-2 px-2 pb-2 pt-2' : 'gap-0 px-4 pb-4 pt-3'}`}
      >
        {/* 窄屏：书架 / 集 / 项目 → 横向 chip，不占死 200px */}
        {shelfAsChips ? (
          <div className="custom-scrollbar shrink-0 overflow-x-auto pb-1">
            <div className="flex min-w-min items-center gap-1.5">
              {shelves.map(({ id, label }) => (
                <button
                  key={id}
                  type="button"
                  onClick={() => pickShelf(id)}
                  className={chipBtn(shelf === id && !projectId && !setId)}
                >
                  {label}
                </button>
              ))}
              {sets.length > 0 && (
                <>
                  <span className="mx-0.5 h-4 w-px shrink-0 bg-neutral-200 dark:bg-white/[0.1]" />
                  {sets.map((s) => (
                    <button
                      key={s.id}
                      type="button"
                      onClick={() => pickSet(setId === s.id ? null : s.id)}
                      className={chipBtn(setId === s.id)}
                    >
                      {s.name}
                    </button>
                  ))}
                </>
              )}
              {projects.length > 0 && (
                <>
                  <span className="mx-0.5 h-4 w-px shrink-0 bg-neutral-200 dark:bg-white/[0.1]" />
                  {projects.map((p) => (
                    <button
                      key={p.id}
                      type="button"
                      onClick={() => pickProject(projectId === p.id ? null : p.id)}
                      className={chipBtn(projectId === p.id)}
                    >
                      {p.name}
                    </button>
                  ))}
                </>
              )}
            </div>
          </div>
        ) : (
          <aside
            className={`custom-scrollbar flex shrink-0 flex-col gap-4 overflow-y-auto border-r border-neutral-200 pr-3 dark:border-white/[0.07] ${
              layout.page < 900 ? 'w-[148px]' : 'w-[180px]'
            }`}
          >
            {shelfNavVertical}
          </aside>
        )}

        {/* Main list */}
        <div
          ref={tableHostRef}
          className={`flex min-h-0 min-w-0 flex-1 flex-col ${shelfAsChips ? '' : 'pl-3'}`}
        >
          {selected.size > 0 && (
            <div className="mb-2 flex flex-wrap items-center gap-1.5 rounded-lg border border-neutral-200 bg-neutral-50 px-2.5 py-2 dark:border-white/[0.08] dark:bg-white/[0.04]">
              <span className="text-[12.5px] font-medium text-neutral-700 dark:text-neutral-200">
                {t.chatLibSelected.replace('{n}', String(selected.size))}
              </span>
              <Button size="sm" disabled={busy} onClick={() => bulkPin(true)}>
                <Star size={12} />
                {!compactPad && <span>{t.chatLibStar}</span>}
              </Button>
              <Button size="sm" disabled={busy} onClick={() => bulkPin(false)}>
                <PinOff size={12} />
                {!compactPad && <span>{t.chatLibUnstar}</span>}
              </Button>
              <div className="relative">
                <Button size="sm" disabled={busy} onClick={() => setMoveOpen(moveOpen === 'set' ? null : 'set')}>
                  <Layers size={12} />
                  {!compactPad && <span>{t.chatLibMoveToSet}</span>}
                </Button>
                {moveOpen === 'set' && (
                  <div className="absolute left-0 top-full z-20 mt-1 max-h-56 w-48 overflow-y-auto rounded-md border border-neutral-200 bg-white py-1 shadow-lg dark:border-white/[0.09] dark:bg-[#2a2a2c]">
                    <button type="button" className="block w-full px-3 py-1.5 text-left text-[12.5px] hover:bg-neutral-50 dark:hover:bg-white/[0.06]" onClick={() => bulkMoveSet(null)}>
                      {t.chatLibClearOwner}
                    </button>
                    {sets.map((s) => (
                      <button key={s.id} type="button" className="block w-full truncate px-3 py-1.5 text-left text-[12.5px] hover:bg-neutral-50 dark:hover:bg-white/[0.06]" onClick={() => bulkMoveSet(s.id)}>
                        {s.name}
                      </button>
                    ))}
                  </div>
                )}
              </div>
              <div className="relative">
                <Button size="sm" disabled={busy} onClick={() => setMoveOpen(moveOpen === 'project' ? null : 'project')}>
                  <FolderKanban size={12} />
                  {!compactPad && <span>{t.chatLibMoveToProject}</span>}
                </Button>
                {moveOpen === 'project' && (
                  <div className="absolute left-0 top-full z-20 mt-1 max-h-56 w-48 overflow-y-auto rounded-md border border-neutral-200 bg-white py-1 shadow-lg dark:border-white/[0.09] dark:bg-[#2a2a2c]">
                    <button type="button" className="block w-full px-3 py-1.5 text-left text-[12.5px] hover:bg-neutral-50 dark:hover:bg-white/[0.06]" onClick={() => bulkMoveProject(null)}>
                      {t.chatLibClearOwner}
                    </button>
                    {projects.map((p) => (
                      <button key={p.id} type="button" className="block w-full truncate px-3 py-1.5 text-left text-[12.5px] hover:bg-neutral-50 dark:hover:bg-white/[0.06]" onClick={() => bulkMoveProject(p.id)}>
                        {p.name}
                      </button>
                    ))}
                  </div>
                )}
              </div>
              {shelf === 'archived' ? (
                <Button size="sm" disabled={busy} onClick={() => bulkArchive(false)}>
                  <ArchiveRestore size={12} />
                  {!compactPad && <span>{t.chatLibUnarchive}</span>}
                </Button>
              ) : (
                <Button size="sm" disabled={busy} onClick={() => bulkArchive(true)}>
                  <Archive size={12} />
                  {!compactPad && <span>{t.chatLibArchive}</span>}
                </Button>
              )}
              <Button size="sm" disabled={busy} onClick={bulkExport}>
                {t.chatLibExport}
              </Button>
              <Button size="sm" disabled={busy} onClick={bulkDelete} className="text-red-600">
                <Trash2 size={12} />
                {!compactPad && <span>{t.chatLibDelete}</span>}
              </Button>
              <button type="button" className="ml-auto text-[12px] text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200" onClick={clearSelection}>
                {t.chatLibClearSelection}
              </button>
            </div>
          )}

          {state.error && (
            <div className="mb-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-[13px] text-red-700 dark:border-red-900/50 dark:bg-red-950/30 dark:text-red-300">
              {state.error}
            </div>
          )}

          <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-neutral-200 bg-white dark:border-white/[0.08] dark:bg-[var(--bg-input)]">
          <div
            ref={listRef}
            onScroll={onScrollList}
            className="custom-scrollbar min-h-0 flex-1 overflow-x-hidden overflow-y-auto"
          >
            {/* Column header — 列显隐跟表格宽度，标题强制单行 truncate；禁止视口 sm: 再撑开。
               圆角裁在外层 overflow-hidden：sticky + backdrop-blur 在滚动容器上裁不掉顶角。 */}
            <div className="sticky top-0 z-10 flex min-w-0 items-center gap-2 border-b border-neutral-100 bg-neutral-50/95 px-3 py-2 text-[11px] font-medium uppercase tracking-wide text-neutral-400 backdrop-blur dark:border-white/[0.06] dark:bg-[var(--bg-hover)]/95">
              <label className="flex w-7 shrink-0 items-center justify-center">
                <input
                  type="checkbox"
                  checked={state.items.length > 0 && selected.size === state.items.length}
                  onChange={(e) => (e.target.checked ? selectAllVisible() : clearSelection())}
                  className="rounded"
                />
              </label>
              <span className="min-w-0 flex-1 truncate">{t.chatLibColTitle}</span>
              {cols.model && (
                <span className="w-24 shrink-0 truncate">{t.chatLibColModel}</span>
              )}
              {cols.owner && (
                <span className="w-24 shrink-0 truncate">{t.chatLibColOwner}</span>
              )}
              {cols.msgs && (
                <span className="w-12 shrink-0 text-right">{t.chatLibColMsgs}</span>
              )}
              <span className="w-14 shrink-0 text-right">{t.chatLibColTime}</span>
              <span className="w-8 shrink-0" />
            </div>

            {state.loading && state.items.length === 0 ? (
              <div className="flex flex-col gap-2 p-4">
                {Array.from({ length: 8 }, (_, i) => (
                  <div key={i} className="kv-skeleton h-11 rounded-md" />
                ))}
              </div>
            ) : state.items.length === 0 ? (
              <div className="flex flex-col items-center justify-center px-6 py-20 text-center">
                <div className="flex h-14 w-14 items-center justify-center rounded-md bg-neutral-100 text-neutral-400 dark:bg-white/[0.06]">
                  <MessagesSquare size={28} strokeWidth={1.5} />
                </div>
                <p className="mt-4 text-[15px] font-medium text-neutral-700 dark:text-neutral-200">
                  {debouncedQ ? t.chatSessionEmptySearch : t.chatSessionEmpty}
                </p>
                <p className="mt-1.5 max-w-sm text-[13px] text-neutral-500">{t.chatLibEmptyHint}</p>
              </div>
            ) : (
              grouped.map((group) => (
                <div key={group.key}>
                  {group.label && (
                    <div className="sticky top-[33px] z-[5] border-b border-neutral-100 bg-neutral-50/90 px-3 py-1.5 text-[11px] font-medium text-neutral-500 backdrop-blur dark:border-white/[0.06] dark:bg-[var(--bg-hover)]/90">
                      {group.label}
                      <span className="ml-1.5 tabular-nums text-neutral-400">{group.items.length}</span>
                    </div>
                  )}
                  {group.items.map((c) => {
                    const isSel = selected.has(c.id)
                    const isCurrent = c.id === currentConversationId
                    const owner = conversationOwnerLabel(c, projects, sets, t)
                    const rowPad = density === 'compact' || !cols.preview ? 'py-1.5' : 'py-2.5'
                    return (
                      <div
                        key={c.id}
                        role="button"
                        tabIndex={0}
                        onClick={(e) => {
                          if ((e.target as HTMLElement).closest('[data-row-chrome]')) return
                          onSelectConversation(c.id, c)
                        }}
                        onKeyDown={(e: ReactKeyboardEvent) => {
                          if (e.key === 'Enter') onSelectConversation(c.id, c)
                        }}
                        className={`group flex cursor-pointer items-center gap-2 border-b border-neutral-50 px-3 ${rowPad} transition-colors hover:bg-neutral-50 dark:border-white/[0.04] dark:hover:bg-white/[0.04] ${
                          isSel ? 'bg-sky-50/80 dark:bg-[var(--accent-soft)]' : ''
                        } ${isCurrent ? 'ring-1 ring-inset ring-sky-200 dark:ring-white/15' : ''}`}
                      >
                        <label data-row-chrome className="flex w-7 shrink-0 items-center justify-center" onClick={(e) => e.stopPropagation()}>
                          <input
                            type="checkbox"
                            checked={isSel}
                            onChange={(e) => toggleSelect(c.id, (e.nativeEvent as MouseEvent).shiftKey)}
                            className="rounded"
                          />
                        </label>
                        <div className="min-w-0 flex-1 overflow-hidden">
                          <div className="flex min-w-0 items-center gap-1.5">
                            {c.pinned && <Star size={12} className="shrink-0 fill-amber-400 text-amber-500" />}
                            {c.archived && <Archive size={12} className="shrink-0 text-neutral-400" />}
                            {renameId === c.id ? (
                              <input
                                data-row-chrome
                                autoFocus
                                value={renameDraft}
                                onChange={(e) => setRenameDraft(e.target.value)}
                                onClick={(e) => e.stopPropagation()}
                                onKeyDown={(e) => {
                                  if (e.key === 'Enter') void patchOne(c.id, { title: renameDraft.trim() || c.title })
                                  if (e.key === 'Escape') setRenameId(null)
                                }}
                                onBlur={() => {
                                  if (renameDraft.trim() && renameDraft.trim() !== c.title) {
                                    void patchOne(c.id, { title: renameDraft.trim() })
                                  } else setRenameId(null)
                                }}
                                className="min-w-0 flex-1 rounded border border-neutral-300 bg-white px-1.5 py-0.5 text-[13px] dark:border-[var(--border-input)] dark:bg-[var(--bg-hover)]"
                              />
                            ) : (
                              <span className="min-w-0 truncate text-[13.5px] font-medium text-neutral-900 dark:text-neutral-50">
                                {debouncedQ
                                  ? <HighlightText text={c.title || t.chatLibUntitled} query={debouncedQ} />
                                  : (c.title || t.chatLibUntitled)}
                              </span>
                            )}
                          </div>
                          {(() => {
                            // 有搜索词时优先展示命中片段（正文/思考），否则回退 index 预览。
                            const snippet = (c.match_snippet ?? c.matchSnippet ?? '').trim()
                            const line = debouncedQ
                              ? (snippet && snippet !== (c.title || '') ? snippet : (c.preview || snippet))
                              : c.preview
                            // 搜索命中片段即使紧凑布局也要露出，否则正文匹配看不见高亮。
                            if (!(cols.preview || debouncedQ) || !line) return null
                            return (
                              <p className="mt-0.5 line-clamp-2 text-[12px] leading-snug text-neutral-500 dark:text-neutral-400">
                                {debouncedQ
                                  ? <HighlightText text={line} query={debouncedQ} />
                                  : line}
                              </p>
                            )
                          })()}
                        </div>
                        {cols.model && (
                          <span className="w-24 shrink-0 truncate text-[12px] text-neutral-500">
                            {shortModelName(c.model)}
                          </span>
                        )}
                        {cols.owner && (
                          <span className="w-24 shrink-0 truncate text-[12px] text-neutral-500">
                            {owner || '—'}
                          </span>
                        )}
                        {cols.msgs && (
                          <span className="w-12 shrink-0 text-right text-[12px] tabular-nums text-neutral-400">
                            {c.message_count}
                          </span>
                        )}
                        <span className="w-14 shrink-0 text-right text-[12px] tabular-nums text-neutral-400">
                          {formatRelativeTime(libraryTimestamp(c, sort), t)}
                        </span>
                        <button
                          data-row-chrome
                          type="button"
                          className="grid size-8 shrink-0 place-items-center rounded-md text-neutral-400 opacity-60 hover:bg-neutral-100 hover:text-neutral-700 group-hover:opacity-100 dark:text-neutral-500 dark:hover:bg-white/[0.08] dark:hover:text-neutral-200"
                          onClick={(e) => {
                            e.stopPropagation()
                            const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
                            const menuW = 180
                            const x = Math.min(rect.right - menuW, window.innerWidth - menuW - 8)
                            setMenu({ id: c.id, x: Math.max(8, x), y: rect.bottom + 4 })
                          }}
                        >
                          <MoreHorizontal size={16} />
                        </button>
                      </div>
                    )
                  })}
                </div>
              ))
            )}

            {state.loadingMore && (
              <div className="flex items-center justify-center gap-2 py-3 text-[12px] text-neutral-400">
                <Loader2 size={14} className="animate-spin" /> {t.chatLoading}
              </div>
            )}
            {!state.loading && !state.loadingMore && hasMore && (
              <button
                type="button"
                className="w-full py-2.5 text-center text-[12.5px] text-neutral-500 hover:bg-neutral-50 dark:hover:bg-white/[0.04]"
                onClick={() => void loadPage({ append: true })}
              >
                {t.chatLibLoadMore}
              </button>
            )}
          </div>
          </div>
        </div>
      </div>

      {/* Row context menu */}
      {menu && menuConv && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setMenu(null)} />
          <div
            className="fixed z-50 w-[180px] rounded-lg border border-neutral-200 bg-white py-1 shadow-xl dark:border-white/[0.09] dark:bg-[#2a2a2c]"
            style={{ left: menu.x, top: menu.y }}
          >
            <MenuItem
              label={menuConv.pinned ? t.chatLibUnstar : t.chatLibStar}
              icon={menuConv.pinned ? <PinOff size={13} /> : <Pin size={13} />}
              onClick={() => void patchOne(menuConv.id, { pinned: !menuConv.pinned })}
            />
            <MenuItem
              label={t.chatRename}
              onClick={() => {
                setRenameId(menuConv.id)
                setRenameDraft(menuConv.title)
                setMenu(null)
              }}
            />
            <MenuItem
              label={menuConv.archived ? t.chatLibUnarchive : t.chatLibArchive}
              icon={menuConv.archived ? <ArchiveRestore size={13} /> : <Archive size={13} />}
              onClick={() => void patchOne(menuConv.id, { archived: !menuConv.archived })}
            />
            <MenuItem
              label={t.chatLibExport}
              onClick={() => {
                setSelected(new Set([menuConv.id]))
                setMenu(null)
                void (async () => {
                  const path = await save({
                    defaultPath: conversationMarkdownFilename(menuConv.title),
                    filters: [{ name: 'Markdown', extensions: ['md'] }],
                  })
                  if (!path) return
                  await chatApi.exportConversationMarkdown(menuConv.id, path, lang)
                })()
              }}
            />
            <div className="my-1 border-t border-neutral-100 dark:border-white/[0.07]" />
            <MenuItem
              label={t.chatLibDelete}
              danger
              icon={<Trash2 size={13} />}
              onClick={() => void deleteOne(menuConv.id)}
            />
          </div>
        </>
      )}
    </div>
  )
}

function MenuItem({
  label,
  onClick,
  icon,
  danger,
}: {
  label: string
  onClick: () => void
  icon?: React.ReactNode
  danger?: boolean
}) {
  // icon optional
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13px] hover:bg-neutral-50 dark:hover:bg-white/[0.06] ${
        danger ? 'text-red-600 dark:text-red-400' : 'text-neutral-700 dark:text-neutral-200'
      }`}
    >
      {icon}
      <span className="flex-1">{label}</span>
      {!icon && null}
      {false && <Check size={12} />}
    </button>
  )
}
