// Background tasks 面板：内置 run_command 后台作业 + 外部 CLI（claude）自报的后台任务。
// Running（可停止）/ Finished（可清空）两段，2.5s 轮询；inactive 时停轮询（同兄弟面板惯例）。
import { useEffect, useRef, useState } from 'react'
import { Bot, Square, TerminalSquare } from 'lucide-react'
import { api, type BackgroundTaskInfo } from '../../api/tauri'
import { i18n, type Lang } from '../../settings/i18n'
import { partitionTasks } from '../backgroundTasks'

const POLL_MS = 2500

function formatElapsed(secs: number): string {
  if (secs < 60) return `${secs}s`
  const m = Math.floor(secs / 60)
  const s = secs % 60
  return `${m}m${s.toString().padStart(2, '0')}s`
}

function TaskGlyph({ kind }: { kind: string }) {
  const isAgent = kind === 'local_agent' || kind === 'remote_agent'
  const Icon = isAgent ? Bot : TerminalSquare
  return <Icon size={14} strokeWidth={1.8} className="mt-0.5 shrink-0 text-neutral-400" />
}

type BackgroundTasksPanelProps = {
  active: boolean
  lang: Lang
  /** 面板按对话隔离：只展示当前对话自己的任务。null = 还没有对话（新建未发送）。 */
  conversationId: string | null
}

export function BackgroundTasksPanel({ active, lang, conversationId }: BackgroundTasksPanelProps) {
  const t = i18n[lang]
  const [tasks, setTasks] = useState<BackgroundTaskInfo[]>([])
  const stopping = useRef<Set<string>>(new Set())

  useEffect(() => {
    // 换对话立刻清掉上一个对话的列表，别等下一次轮询。
    setTasks([])
    if (!active || !conversationId) return
    let cancelled = false
    const tick = async () => {
      if (document.hidden) return
      try {
        const next = await api.chatListBackgroundTasks(conversationId)
        if (!cancelled) setTasks(next)
      } catch {
        if (!cancelled) setTasks([])
      }
    }
    void tick()
    const timer = window.setInterval(tick, POLL_MS)
    return () => {
      cancelled = true
      window.clearInterval(timer)
    }
  }, [active, conversationId])

  const { running, finished } = partitionTasks(tasks)

  const stop = async (task: BackgroundTaskInfo) => {
    if (stopping.current.has(task.id)) return
    stopping.current.add(task.id)
    try {
      if (task.source === 'builtin') {
        await api.chatKillBackgroundCommand(task.id)
      } else {
        await api.chatStopExternalBackgroundTask(task.conversationId ?? '', task.id)
      }
      setTasks((prev) =>
        prev.map((item) => (item.id === task.id ? { ...item, status: 'stopped' as const } : item)),
      )
    } catch {
      // leave it; next poll reflects the real state
    } finally {
      stopping.current.delete(task.id)
    }
  }

  const clearFinished = async () => {
    if (!conversationId) return
    try {
      await api.chatClearFinishedBackgroundTasks(conversationId)
      setTasks((prev) => prev.filter((item) => item.status === 'running'))
    } catch {
      // next poll reflects the real state
    }
  }

  const statusLabel = (status: BackgroundTaskInfo['status']) =>
    status === 'completed'
      ? t.chatBgStatusCompleted
      : status === 'failed'
        ? t.chatBgStatusFailed
        : t.chatBgStatusStopped

  if (tasks.length === 0) {
    return (
      <div className="grid flex-1 place-items-center px-6 text-center text-[12.5px] text-neutral-400 dark:text-neutral-500">
        {t.chatBgEmpty}
      </div>
    )
  }

  return (
    <div className="chat-popover-scroll min-h-0 flex-1 overflow-y-auto px-2 py-2">
      {running.length > 0 && (
        <div className="px-1 py-1.5 text-[12px] font-medium text-neutral-500 dark:text-neutral-400">
          {t.chatBgRunning} · {running.length}
        </div>
      )}
      {running.map((task) => (
        <div
          key={task.id}
          className="flex items-start gap-2 rounded-xl px-2 py-2 hover:bg-neutral-100/70 dark:hover:bg-neutral-800/60"
        >
          <TaskGlyph kind={task.kind} />
          <div className="min-w-0 flex-1">
            <div
              className="truncate font-mono text-[12.5px] text-neutral-800 dark:text-neutral-100"
              title={task.title}
            >
              {task.title}
            </div>
            <div className="mt-0.5 text-[11px] text-neutral-400">
              {task.pid != null ? `pid ${task.pid} · ` : ''}
              {formatElapsed(task.elapsedSecs)}
            </div>
          </div>
          <button
            type="button"
            onClick={() => void stop(task)}
            className="grid size-7 shrink-0 place-items-center rounded-md text-neutral-400 hover:bg-red-50 hover:text-red-600 dark:hover:bg-red-950/30 dark:hover:text-red-300"
            title={t.chatKill}
            aria-label={t.chatKillNamed.replace('{name}', task.title)}
          >
            <Square size={13} strokeWidth={2} fill="currentColor" />
          </button>
        </div>
      ))}
      {finished.length > 0 && (
        <div className="flex items-center justify-between px-1 py-1.5">
          <span className="text-[12px] font-medium text-neutral-500 dark:text-neutral-400">
            {t.chatBgFinished} · {finished.length}
          </span>
          <button
            type="button"
            onClick={() => void clearFinished()}
            className="rounded-md px-1.5 py-0.5 text-[11px] text-neutral-400 hover:bg-neutral-100 hover:text-neutral-600 dark:hover:bg-neutral-800 dark:hover:text-neutral-300"
          >
            {t.chatBgClear}
          </button>
        </div>
      )}
      {finished.map((task) => (
        <div key={task.id} className="rounded-xl px-2 py-2 opacity-75">
          <div className="flex items-start gap-2">
            <TaskGlyph kind={task.kind} />
            <div className="min-w-0 flex-1">
              <div
                className="truncate font-mono text-[12.5px] text-neutral-600 dark:text-neutral-300"
                title={task.title}
              >
                {task.title}
              </div>
              <div
                className="mt-0.5 truncate text-[11px] text-neutral-400"
                title={task.summary ?? undefined}
              >
                {statusLabel(task.status)}
                {task.exitCode != null && task.exitCode !== 0 ? ` · exit ${task.exitCode}` : ''}
                {task.summary ? ` · ${task.summary}` : ''}
              </div>
            </div>
          </div>
        </div>
      ))}
    </div>
  )
}
