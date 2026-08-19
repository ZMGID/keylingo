import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Copy, FolderOpen, GitFork, Loader2, RefreshCw } from 'lucide-react'
import { Button, IconButton } from '../components/Button'
import type { Lang } from '../settings/i18n'
import type { ChatMessage, Conversation } from './types'
import { chatApi } from './api'
import {
  buildConversationTrajectory,
  filterTrajectorySteps,
  summarizeTrajectory,
  type TrajectoryKind,
  type TrajectoryStep,
} from './trajectory'
import { mapPiForkEntriesToMessages, type PiForkMessage, type PiSessionMutationResult } from './piSessionTree'

type TrajectoryPanelProps = {
  active: boolean
  conversation: Conversation | null
  messages: ChatMessage[]
  lang: Lang
  piNativeEnabled: boolean
  onFocusMessage: (messageId: string) => void
  onConversationChanged: (
    conversationId: string,
    conversation?: Conversation,
    draft?: string,
  ) => void
}

const KIND_LABEL: Record<TrajectoryKind, { zh: string; en: string }> = {
  user: { zh: '用户', en: 'User' },
  assistant: { zh: '助手', en: 'Assistant' },
  tool: { zh: '工具', en: 'Tool' },
  compacted: { zh: '压缩', en: 'Compacted' },
}

function labels(lang: Lang) {
  return lang === 'zh'
    ? {
        search: '搜索轨迹',
        empty: '这条对话还没有轨迹',
        turns: '轮',
        steps: '步',
        calls: '次调用',
        clone: '克隆当前分支',
        switch: '切换已绑定会话',
        fork: '从这里分叉',
        refresh: '刷新原生会话',
        pathPlaceholder: '已绑定的 session JSONL 路径',
        open: '打开',
        cancel: '取消',
        cancelled: '扩展取消了这次会话操作',
        loadFailed: '无法加载 Pi 原生会话',
      }
    : {
        search: 'Search trajectory',
        empty: 'This conversation has no trajectory yet',
        turns: 'turns',
        steps: 'steps',
        calls: 'calls',
        clone: 'Clone current branch',
        switch: 'Switch bound session',
        fork: 'Fork from here',
        refresh: 'Refresh native session',
        pathPlaceholder: 'Bound session JSONL path',
        open: 'Open',
        cancel: 'Cancel',
        cancelled: 'An extension cancelled this session operation',
        loadFailed: 'Could not load the Pi native session',
      }
}

function kindChipClass(kind: TrajectoryKind, error?: boolean): string {
  if (error) return 'bg-rose-500/15 text-rose-700 dark:text-rose-300'
  if (kind === 'user') return 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300'
  if (kind === 'assistant') return 'bg-violet-500/15 text-violet-700 dark:text-violet-300'
  if (kind === 'tool') return 'bg-amber-500/15 text-amber-800 dark:text-amber-300'
  return 'bg-neutral-500/10 text-neutral-500'
}

function kindDotClass(kind: TrajectoryKind, error?: boolean): string {
  if (error) return 'bg-rose-500'
  if (kind === 'user') return 'bg-emerald-500'
  if (kind === 'assistant') return 'bg-violet-500'
  if (kind === 'tool') return 'bg-amber-500'
  return 'bg-neutral-400'
}

export function TrajectoryPanel({
  active,
  conversation,
  messages,
  lang,
  piNativeEnabled,
  onFocusMessage,
  onConversationChanged,
}: TrajectoryPanelProps) {
  const copy = labels(lang)
  const conversationId = conversation?.id ?? null
  const steps = useMemo(
    () => (active ? buildConversationTrajectory(messages) : []),
    [active, messages],
  )
  const stats = useMemo(
    () => (active ? summarizeTrajectory(steps, conversation) : { turns: 0, steps: 0, calls: 0, durationLabel: null }),
    [active, conversation, steps],
  )
  const [query, setQuery] = useState('')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [forkMessages, setForkMessages] = useState<PiForkMessage[]>([])
  const [sessionFile, setSessionFile] = useState<string | null>(null)
  const [sessionId, setSessionId] = useState('')
  const [loadingPi, setLoadingPi] = useState(false)
  const [mutating, setMutating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [switchOpen, setSwitchOpen] = useState(false)
  const [switchPath, setSwitchPath] = useState('')
  const rowRefs = useRef(new Map<string, HTMLButtonElement>())
  const loadEpochRef = useRef(0)
  const conversationIdRef = useRef(conversationId)
  if (conversationIdRef.current !== conversationId) loadEpochRef.current += 1
  conversationIdRef.current = conversationId
  const visibleSteps = useMemo(() => filterTrajectorySteps(steps, query), [query, steps])
  const forkByMessageId = useMemo(
    () => mapPiForkEntriesToMessages(
      messages.filter((message) => message.role === 'user').map((message) => ({
        id: message.id,
        content: message.content,
      })),
      forkMessages,
    ),
    [forkMessages, messages],
  )

  const loadPi = useCallback(async () => {
    const requestedConversationId = conversationId
    const epoch = ++loadEpochRef.current
    if (!requestedConversationId || !piNativeEnabled) {
      setForkMessages([])
      setSessionFile(null)
      setSessionId('')
      return
    }
    setLoadingPi(true)
    setError(null)
    try {
      const [snapshot, forkMessages] = await Promise.all([
        chatApi.piSessionTree(requestedConversationId),
        chatApi.piForkMessages(requestedConversationId),
      ])
      if (loadEpochRef.current !== epoch || conversationIdRef.current !== requestedConversationId) return
      setForkMessages(forkMessages)
      setSessionFile(snapshot.sessionFile)
      setSessionId(snapshot.sessionId)
      setSwitchPath(snapshot.sessionFile ?? '')
    } catch {
      if (loadEpochRef.current !== epoch || conversationIdRef.current !== requestedConversationId) return
      setForkMessages([])
      setSessionFile(null)
      setSessionId('')
      setSwitchPath('')
      setError(copy.loadFailed)
    } finally {
      if (loadEpochRef.current === epoch) setLoadingPi(false)
    }
  }, [conversationId, copy.loadFailed, piNativeEnabled])

  useEffect(() => {
    setQuery('')
    setSelectedId(null)
    setError(null)
    setSwitchOpen(false)
    setForkMessages([])
    setSessionFile(null)
    setSessionId('')
    setSwitchPath('')
    if (active && piNativeEnabled) void loadPi()
  }, [active, conversationId, loadPi, piNativeEnabled])

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
        await loadPi()
      }
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || 'Session operation failed')
    } finally {
      setMutating(false)
    }
  }, [copy.cancelled, loadPi, onConversationChanged])

  const handleSwitch = useCallback(async () => {
    if (!conversationId || !switchPath.trim()) return
    setMutating(true)
    setError(null)
    try {
      const result = await chatApi.piSessionSwitch(conversationId, switchPath.trim())
      setSwitchOpen(false)
      onConversationChanged(result.conversationId)
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message || 'Session navigation failed')
    } finally {
      setMutating(false)
    }
  }, [conversationId, onConversationChanged, switchPath])

  const handleSelect = useCallback((step: TrajectoryStep) => {
    setSelectedId(step.id)
    onFocusMessage(step.messageId)
  }, [onFocusMessage])

  const handleTimelineClick = useCallback((step: TrajectoryStep) => {
    setSelectedId(step.id)
    rowRefs.current.get(step.id)?.scrollIntoView({ block: 'nearest' })
    onFocusMessage(step.messageId)
  }, [onFocusMessage])

  const statLine = [
    stats.durationLabel,
    `${stats.turns} ${copy.turns}`,
    `${stats.steps} ${copy.steps}`,
    `${stats.calls} ${copy.calls}`,
  ].filter(Boolean).join(' · ')

  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-col gap-2 border-b border-neutral-200/70 px-3 py-2 dark:border-neutral-700/50">
        <div className="flex items-center gap-2">
          <p className="min-w-0 flex-1 truncate text-[11px] text-neutral-500 dark:text-neutral-400" title={sessionFile ?? sessionId}>
            {statLine || copy.empty}
          </p>
          {piNativeEnabled && (
            <>
              <IconButton label={copy.switch} size="xs" variant="ghost" onClick={() => setSwitchOpen((value) => !value)} disabled={!conversationId || mutating}>
                <FolderOpen size={13} />
              </IconButton>
              <IconButton
                label={copy.clone}
                size="xs"
                variant="ghost"
                disabled={!conversationId || !sessionId || mutating}
                onClick={() => conversationId && void runMutation(() => chatApi.piSessionClone(conversationId))}
              >
                <Copy size={13} />
              </IconButton>
              <IconButton label={copy.refresh} size="xs" variant="ghost" onClick={() => void loadPi()} disabled={loadingPi || mutating}>
                <RefreshCw size={13} className={loadingPi ? 'animate-spin' : undefined} />
              </IconButton>
            </>
          )}
        </div>
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={copy.search}
          aria-label={copy.search}
          className="h-7 w-full rounded-md border border-neutral-200 bg-transparent px-2 text-[12px] outline-none placeholder:text-neutral-400 focus:border-neutral-400 dark:border-neutral-700 dark:focus:border-neutral-500"
        />
        {steps.length > 0 && (
          <div className="flex flex-wrap gap-[3px]" role="list" aria-label={lang === 'zh' ? '轨迹时间线' : 'Trajectory timeline'}>
            {steps.map((step) => (
              <button
                key={step.id}
                type="button"
                role="listitem"
                title={`${KIND_LABEL[step.kind][lang]} ${step.title === step.kind ? '' : step.title} ${step.preview}`.trim()}
                className={`h-2 w-2 rounded-[2px] ${kindDotClass(step.kind, step.error)} ${
                  selectedId === step.id ? 'ring-1 ring-neutral-800 ring-offset-1 dark:ring-neutral-100' : 'opacity-80 hover:opacity-100'
                }`}
                onClick={() => handleTimelineClick(step)}
              />
            ))}
          </div>
        )}
      </div>

      {switchOpen && (
        <div className="flex shrink-0 items-center gap-1 border-b border-neutral-200/70 px-2 py-2 dark:border-neutral-700/50">
          <input
            value={switchPath}
            onChange={(event) => setSwitchPath(event.target.value)}
            placeholder={copy.pathPlaceholder}
            className="min-w-0 flex-1 rounded border border-neutral-300 bg-transparent px-2 py-1 text-[12px] outline-none focus:border-neutral-500 dark:border-neutral-600 dark:focus:border-neutral-400"
          />
          <Button size="sm" onClick={() => void handleSwitch()} disabled={!conversationId || !switchPath.trim() || mutating}>
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

      <div className="custom-scrollbar min-h-0 flex-1 overflow-y-auto px-2 py-2">
        {visibleSteps.length === 0 ? (
          <div className="px-2 py-8 text-center text-[12px] text-neutral-400">
            {loadingPi ? <Loader2 size={16} className="mx-auto animate-spin" /> : copy.empty}
          </div>
        ) : (
          <ol className="relative m-0 flex list-none flex-col gap-0.5 pl-3">
            <span className="absolute bottom-2 left-[11px] top-2 w-px bg-neutral-200 dark:bg-neutral-700" aria-hidden />
            {visibleSteps.map((step) => {
              const selected = selectedId === step.id
              const forkEntryId = step.kind === 'user' ? forkByMessageId.get(step.messageId) : undefined
              return (
                <li key={step.id} className="group relative flex items-start">
                  <span className={`absolute left-[-9px] top-[11px] z-[1] h-1.5 w-1.5 rounded-full ${kindDotClass(step.kind, step.error)}`} />
                  <button
                    ref={(node) => {
                      if (node) rowRefs.current.set(step.id, node)
                      else rowRefs.current.delete(step.id)
                    }}
                    type="button"
                    className={`flex min-w-0 flex-1 items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors ${
                      selected ? 'bg-neutral-500/10 dark:bg-neutral-400/10' : 'hover:bg-neutral-500/5'
                    }`}
                    onClick={() => handleSelect(step)}
                  >
                    <span className={`mt-0.5 shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium leading-none ${kindChipClass(step.kind, step.error)}`}>
                      {KIND_LABEL[step.kind][lang]}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[12px] text-neutral-800 dark:text-neutral-100">
                        {step.kind === 'tool' ? step.title : step.preview}
                      </span>
                      {step.kind === 'tool' && (
                        <span className="mt-0.5 block truncate text-[11px] text-neutral-500 dark:text-neutral-400">
                          {step.preview}
                          {step.result ? ` → ${step.result}` : ''}
                        </span>
                      )}
                    </span>
                  </button>
                  {forkEntryId && conversationId && (
                    <IconButton
                      label={copy.fork}
                      size="xs"
                      variant="ghost"
                      className="mt-1 opacity-0 group-hover:opacity-100 focus:opacity-100"
                      disabled={mutating}
                      onClick={() => void runMutation(() => chatApi.piSessionFork(conversationId, forkEntryId))}
                    >
                      <GitFork size={12} />
                    </IconButton>
                  )}
                </li>
              )
            })}
          </ol>
        )}
      </div>
    </section>
  )
}
