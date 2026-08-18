import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Bot,
  Boxes,
  ChevronDown,
  ChevronRight,
  Copy,
  FileClock,
  FolderOpen,
  GitFork,
  Loader2,
  MessageSquare,
  RefreshCw,
  Wrench,
} from 'lucide-react'
import { Button, IconButton } from '../components/Button'
import type { Lang } from '../settings/i18n'
import type { Conversation } from './types'
import { chatApi } from './api'
import {
  flattenPiSessionTree,
  isPiForkableEntry,
  piSessionEntryRole,
  piSessionEntryText,
  piSessionLeafPath,
  type PiSessionMutationResult,
  type PiSessionTreeSnapshot,
} from './piSessionTree'

type PiSessionTreePanelProps = {
  active: boolean
  conversationId: string | null
  lang: Lang
  onConversationChanged: (
    conversationId: string,
    conversation?: Conversation,
    draft?: string,
  ) => void
}

const EMPTY_SNAPSHOT: PiSessionTreeSnapshot = {
  tree: [],
  leafId: null,
  sessionId: '',
  sessionFile: null,
}

function labels(lang: Lang) {
  return lang === 'zh'
    ? {
        refresh: '刷新 Pi 会话树',
        clone: '克隆当前 Pi 分支',
        switch: '切换 Pi 会话文件',
        fork: '从这里创建 Pi 原生 fork',
        empty: '暂无 Pi 会话条目',
        unavailable: '先在这条对话中运行一次 Pi',
        pathPlaceholder: 'Pi session JSONL 路径',
        open: '切换',
        cancel: '取消',
        cancelled: 'Pi 扩展取消了会话操作',
        active: '当前 leaf',
      }
    : {
        refresh: 'Refresh Pi session tree',
        clone: 'Clone current Pi branch',
        switch: 'Switch Pi session file',
        fork: 'Create native Pi fork here',
        empty: 'No Pi session entries',
        unavailable: 'Run Pi once in this conversation first',
        pathPlaceholder: 'Pi session JSONL path',
        open: 'Switch',
        cancel: 'Cancel',
        cancelled: 'A Pi extension cancelled the session operation',
        active: 'Current leaf',
      }
}

function entryIcon(role: string) {
  if (role === 'user') return MessageSquare
  if (role === 'assistant') return Bot
  if (role === 'toolResult' || role === 'bashExecution') return Wrench
  if (role === 'compaction' || role === 'branch_summary') return Boxes
  return FileClock
}

function basename(path: string | null): string {
  if (!path) return ''
  return path.replace(/\\/g, '/').split('/').filter(Boolean).pop() ?? path
}

export function PiSessionTreePanel({
  active,
  conversationId,
  lang,
  onConversationChanged,
}: PiSessionTreePanelProps) {
  const copy = labels(lang)
  const [snapshot, setSnapshot] = useState<PiSessionTreeSnapshot>(EMPTY_SNAPSHOT)
  const [forkableIds, setForkableIds] = useState<Set<string>>(new Set())
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [mutating, setMutating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [switchOpen, setSwitchOpen] = useState(false)
  const [switchPath, setSwitchPath] = useState('')
  const loadEpochRef = useRef(0)
  const conversationIdRef = useRef(conversationId)
  if (conversationIdRef.current !== conversationId) loadEpochRef.current += 1
  conversationIdRef.current = conversationId
  const load = useCallback(async () => {
    const requestedConversationId = conversationId
    const epoch = ++loadEpochRef.current
    if (!requestedConversationId) {
      setSnapshot(EMPTY_SNAPSHOT)
      setForkableIds(new Set())
      return
    }
    setLoading(true)
    setError(null)
    try {
      const nextSnapshot = await chatApi.piSessionTree(requestedConversationId)
      const forkMessages = await chatApi.piForkMessages(requestedConversationId)
      if (
        loadEpochRef.current !== epoch
        || conversationIdRef.current !== requestedConversationId
      ) return
      setSnapshot(nextSnapshot)
      setForkableIds(new Set(forkMessages.map((message) => message.entryId)))
      const leafPath = piSessionLeafPath(nextSnapshot.tree, nextSnapshot.leafId)
      setExpanded((previous) => new Set([...previous, ...leafPath]))
      setSelectedId((previous) => previous ?? nextSnapshot.leafId)
      setSwitchPath(nextSnapshot.sessionFile ?? '')
    } catch (err) {
      if (
        loadEpochRef.current !== epoch
        || conversationIdRef.current !== requestedConversationId
      ) return
      setError(typeof err === 'string' ? err : (err as Error).message || copy.unavailable)
      setSnapshot(EMPTY_SNAPSHOT)
      setForkableIds(new Set())
    } finally {
      if (loadEpochRef.current === epoch) setLoading(false)
    }
  }, [conversationId, copy.unavailable])

  useEffect(() => {
    loadEpochRef.current += 1
    setSnapshot(EMPTY_SNAPSHOT)
    setForkableIds(new Set())
    setExpanded(new Set())
    setSelectedId(null)
    setError(null)
    if (active) void load()
  }, [active, conversationId, load])

  const visibleNodes = useMemo(
    () => flattenPiSessionTree(snapshot.tree, expanded),
    [expanded, snapshot.tree],
  )

  const runMutation = useCallback(async (action: () => Promise<PiSessionMutationResult>) => {
    setMutating(true)
    setError(null)
    try {
      const result = await action()
      if (result.cancelled) {
        setError(copy.cancelled)
        return
      }
      setSwitchOpen(false)
      if (result.conversationId) {
        onConversationChanged(
          result.conversationId,
          result.conversation ?? undefined,
          result.text ?? undefined,
        )
      } else {
        await load()
      }
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || 'Pi session operation failed')
    } finally {
      setMutating(false)
    }
  }, [copy.cancelled, load, onConversationChanged])

  const handleSwitch = useCallback(async () => {
    if (!conversationId || !switchPath.trim()) return
    setMutating(true)
    setError(null)
    try {
      const result = await chatApi.piSessionSwitch(conversationId, switchPath.trim())
      setSwitchOpen(false)
      onConversationChanged(result.conversationId)
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || 'Pi session navigation failed')
    } finally {
      setMutating(false)
    }
  }, [conversationId, onConversationChanged, switchPath])

  const toggleExpanded = useCallback((id: string) => {
    setExpanded((previous) => {
      const next = new Set(previous)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  const sessionLabel = basename(snapshot.sessionFile) || snapshot.sessionId

  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-neutral-200/70 px-2 dark:border-neutral-700/50">
        <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-neutral-700 dark:text-neutral-200" title={snapshot.sessionFile ?? snapshot.sessionId}>
          {sessionLabel || 'Pi'}
        </span>
        <IconButton label={copy.switch} size="xs" variant="ghost" onClick={() => setSwitchOpen((value) => !value)} disabled={!conversationId || mutating}>
          <FolderOpen size={13} />
        </IconButton>
        <IconButton
          label={copy.clone}
          size="xs"
          variant="ghost"
          disabled={!conversationId || !snapshot.sessionId || mutating}
          onClick={() => conversationId && void runMutation(() => chatApi.piSessionClone(conversationId))}
        >
          <Copy size={13} />
        </IconButton>
        <IconButton label={copy.refresh} size="xs" variant="ghost" onClick={() => void load()} disabled={loading || mutating}>
          <RefreshCw size={13} className={loading ? 'animate-spin' : undefined} />
        </IconButton>
      </div>

      {switchOpen && (
        <div className="flex shrink-0 items-center gap-1 border-b border-neutral-200/70 px-2 py-2 dark:border-neutral-700/50">
          <input
            value={switchPath}
            onChange={(event) => setSwitchPath(event.target.value)}
            placeholder={copy.pathPlaceholder}
            className="min-w-0 flex-1 rounded border border-neutral-300 bg-transparent px-2 py-1 text-[12px] outline-none focus:border-neutral-500 dark:border-neutral-600 dark:focus:border-neutral-400"
          />
          <Button
            size="sm"
            onClick={() => void handleSwitch()}
            disabled={!conversationId || !switchPath.trim() || mutating}
          >
            {copy.open}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setSwitchOpen(false)}>
            {copy.cancel}
          </Button>
        </div>
      )}

      {error && (
        <div className="shrink-0 border-b border-amber-200 bg-amber-50 px-3 py-2 text-[11px] text-amber-800 dark:border-amber-900/60 dark:bg-amber-950/30 dark:text-amber-200">
          {error}
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto py-1">
        {(loading || mutating) && visibleNodes.length === 0 ? (
          <div className="flex h-24 items-center justify-center text-neutral-400">
            <Loader2 size={16} className="animate-spin" />
          </div>
        ) : visibleNodes.length === 0 ? (
          <div className="px-3 py-8 text-center text-[12px] text-neutral-400">{copy.empty}</div>
        ) : (
          visibleNodes.map(({ node, depth, hasChildren }) => {
            const entry = node.entry
            const id = typeof entry.id === 'string' ? entry.id : ''
            const role = piSessionEntryRole(entry)
            const text = piSessionEntryText(entry)
            const Icon = entryIcon(role)
            const current = id === snapshot.leafId
            const selected = id === selectedId
            const forkable = isPiForkableEntry(entry, forkableIds)
            return (
              <div
                key={id || `${role}-${entry.timestamp ?? depth}`}
                className={`group flex h-8 items-center gap-1 pr-1 text-[12px] ${
                  selected ? 'bg-neutral-500/10 dark:bg-neutral-400/10' : 'hover:bg-neutral-500/5'
                }`}
                style={{ paddingLeft: `${6 + depth * 14}px` }}
              >
                <button
                  type="button"
                  className="flex h-6 w-5 shrink-0 items-center justify-center text-neutral-400 disabled:opacity-30"
                  disabled={!hasChildren || !id}
                  onClick={() => id && toggleExpanded(id)}
                  aria-label={expanded.has(id) ? 'Collapse' : 'Expand'}
                >
                  {hasChildren && (expanded.has(id) ? <ChevronDown size={13} /> : <ChevronRight size={13} />)}
                </button>
                <button
                  type="button"
                  className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
                  onClick={() => setSelectedId(id || null)}
                  title={text}
                >
                  <Icon size={13} className={current ? 'text-emerald-600 dark:text-emerald-400' : 'text-neutral-400'} />
                  <span className="min-w-0 flex-1 truncate text-neutral-700 dark:text-neutral-200">{text}</span>
                  {current && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-500" title={copy.active} />}
                </button>
                {forkable && (
                  <IconButton
                    label={copy.fork}
                    size="xs"
                    variant="ghost"
                    className="opacity-0 group-hover:opacity-100 focus:opacity-100"
                    disabled={mutating || !conversationId}
                    onClick={() => conversationId && void runMutation(() => chatApi.piSessionFork(conversationId, id))}
                  >
                    <GitFork size={12} />
                  </IconButton>
                )}
              </div>
            )
          })
        )}
      </div>

      {selectedId && (
        <div className="shrink-0 border-t border-neutral-200/70 px-3 py-1.5 font-mono text-[10px] text-neutral-400 dark:border-neutral-700/50">
          {selectedId}
        </div>
      )}
    </section>
  )
}
