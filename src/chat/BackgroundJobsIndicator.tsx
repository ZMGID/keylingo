import { useEffect, useState } from 'react'
import { TerminalSquare } from 'lucide-react'
import { api, type BackgroundTaskInfo } from '../api/tauri'
import { useT } from '../settings/i18n'
import { chatTitlebarIconButtonClass } from './platform'
import { partitionTasks } from './backgroundTasks'

const POLL_MS = 2500

/**
 * Header status light for background tasks (built-in `run_command background:true`
 * jobs + the external CLI's self-reported background tasks). Renders nothing when
 * the list is empty; green + ping while anything is running. Clicking opens the
 * right dock's Tasks tab — the full panel (stop / clear / finished list) lives
 * there (`dock/BackgroundTasksPanel.tsx`), not in a popover.
 */
export function BackgroundJobsIndicator({
  conversationId,
  onOpen,
}: {
  /** 状态灯按对话隔离，与右栏任务页同一份数据口径。null = 还没有对话。 */
  conversationId: string | null
  onOpen: () => void
}) {
  const t = useT()
  const [tasks, setTasks] = useState<BackgroundTaskInfo[]>([])

  useEffect(() => {
    setTasks([])
    if (!conversationId) return
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
  }, [conversationId])

  if (tasks.length === 0) return null

  const { running } = partitionTasks(tasks)
  const label =
    running.length > 0 ? t.chatBgJobsRunning.replace('{n}', String(running.length)) : t.chatBgJobs

  return (
    <button
      type="button"
      onClick={onOpen}
      data-tauri-drag-region="false"
      className={`relative ${chatTitlebarIconButtonClass} ${
        running.length > 0
          ? 'text-emerald-600 hover:text-emerald-700 dark:text-emerald-500 dark:hover:text-emerald-400'
          : 'text-neutral-400 hover:text-neutral-600 dark:text-neutral-500 dark:hover:text-neutral-300'
      }`}
      title={label}
      aria-label={label}
    >
      <TerminalSquare size={16} strokeWidth={1.8} />
      {running.length > 0 && (
        <span className="absolute -right-0.5 -top-0.5 flex size-2">
          <span className="absolute inline-flex size-full animate-ping rounded-full bg-emerald-400 opacity-75" />
          <span className="relative inline-flex size-2 rounded-full bg-emerald-500" />
        </span>
      )}
    </button>
  )
}
