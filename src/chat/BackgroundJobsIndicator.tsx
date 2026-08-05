import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Square, TerminalSquare } from 'lucide-react'
import { api, type BackgroundCommandInfo } from '../api/tauri'
import { useT } from '../settings/i18n'
import { chatTitlebarIconButtonClass } from './platform'

const POLL_MS = 2500

function formatElapsed(secs: number): string {
  if (secs < 60) return `${secs}s`
  const m = Math.floor(secs / 60)
  const s = secs % 60
  return `${m}m${s.toString().padStart(2, '0')}s`
}

/**
 * Header indicator for running background commands (the chat agent's
 * `run_command background:true` jobs). Polls the registry; renders nothing when
 * nothing is running, so it only appears when there's actually something to see.
 *
 * The dropdown is portaled to <body> with position:fixed because the chat pane
 * uses container-type (CSS containment), which would otherwise clip/cover an
 * absolutely-positioned popover inside the header.
 */
export function BackgroundJobsIndicator() {
  const t = useT()
  const [jobs, setJobs] = useState<BackgroundCommandInfo[]>([])
  const [open, setOpen] = useState(false)
  const [pos, setPos] = useState<{ top: number; left: number; maxH: number } | null>(null)
  const btnRef = useRef<HTMLButtonElement>(null)
  const killing = useRef<Set<string>>(new Set())

  useEffect(() => {
    let cancelled = false
    const tick = async () => {
      if (document.hidden) return
      try {
        const next = await api.chatListBackgroundCommands()
        if (!cancelled) setJobs(next)
      } catch {
        if (!cancelled) setJobs([])
      }
    }
    void tick()
    const timer = window.setInterval(tick, POLL_MS)
    return () => {
      cancelled = true
      window.clearInterval(timer)
    }
  }, [])

  const place = useCallback(() => {
    const rect = btnRef.current?.getBoundingClientRect()
    if (rect) {
      const top = rect.bottom + 6
      setPos({ top, left: rect.left, maxH: Math.max(160, Math.min(360, window.innerHeight - top - 8)) })
    }
  }, [])

  // Nothing running → don't show, and don't keep a stale popover open.
  useEffect(() => {
    if (jobs.length === 0 && open) setOpen(false)
  }, [jobs.length, open])

  // Keep the portaled dropdown anchored to the button while it's open.
  useEffect(() => {
    if (!open) return
    place()
    window.addEventListener('resize', place)
    return () => window.removeEventListener('resize', place)
  }, [open, place])

  if (jobs.length === 0) return null

  const kill = async (jobId: string) => {
    if (killing.current.has(jobId)) return
    killing.current.add(jobId)
    try {
      await api.chatKillBackgroundCommand(jobId)
      setJobs((prev) => prev.filter((j) => j.jobId !== jobId))
    } catch {
      // leave it; next poll reflects the real state
    } finally {
      killing.current.delete(jobId)
    }
  }

  const toggle = () => {
    if (!open) place()
    setOpen((v) => !v)
  }

  const runningLabel = t.chatBgJobsRunning.replace('{n}', String(jobs.length))

  return (
    <div className="relative" data-tauri-drag-region="false">
      <button
        ref={btnRef}
        type="button"
        onClick={toggle}
        className={`relative ${chatTitlebarIconButtonClass} ${
          open
            ? 'bg-black/[0.06] text-neutral-800 dark:bg-white/[0.09] dark:text-neutral-100'
            : 'text-emerald-600 hover:text-emerald-700 dark:text-emerald-500 dark:hover:text-emerald-400'
        }`}
        title={runningLabel}
        aria-label={runningLabel}
      >
        <TerminalSquare size={16} strokeWidth={1.8} />
        <span className="absolute -right-0.5 -top-0.5 flex size-2">
          <span className="absolute inline-flex size-full animate-ping rounded-full bg-emerald-400 opacity-75" />
          <span className="relative inline-flex size-2 rounded-full bg-emerald-500" />
        </span>
      </button>
      {open &&
        pos &&
        createPortal(
          <>
            <div className="fixed inset-0 z-[2000]" onClick={() => setOpen(false)} aria-hidden />
            <div
              className="chat-motion-popover chat-popover-scroll fixed z-[2001] w-[320px] overflow-y-auto kv-menu"
              style={{ top: pos.top, left: pos.left, maxHeight: pos.maxH }}
            >
              <div className="px-2 py-1.5 text-[12px] font-medium text-neutral-500 dark:text-neutral-400">
                {t.chatBgJobs.replace('{n}', String(jobs.length))}
              </div>
              {jobs.map((job) => (
                <div
                  key={job.jobId}
                  className="flex items-start gap-2 rounded-xl px-2 py-2 hover:bg-neutral-100/70 dark:hover:bg-neutral-800/60"
                >
                  <div className="min-w-0 flex-1">
                    <div
                      className="truncate font-mono text-[12.5px] text-neutral-800 dark:text-neutral-100"
                      title={job.command}
                    >
                      {job.command}
                    </div>
                    <div className="mt-0.5 text-[11px] text-neutral-400">
                      {job.pid != null ? `pid ${job.pid} · ` : ''}
                      {formatElapsed(job.elapsedSecs)}
                    </div>
                  </div>
                  <button
                    type="button"
                    onClick={() => void kill(job.jobId)}
                    className="grid size-7 shrink-0 place-items-center rounded-md text-neutral-400 hover:bg-red-50 hover:text-red-600 dark:hover:bg-red-950/30 dark:hover:text-red-300"
                    title={t.chatKill}
                    aria-label={t.chatKillNamed.replace('{name}', job.command)}
                  >
                    <Square size={13} strokeWidth={2} fill="currentColor" />
                  </button>
                </div>
              ))}
            </div>
          </>,
          document.body,
        )}
    </div>
  )
}
