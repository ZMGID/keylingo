import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { ChevronDown } from 'lucide-react'
import { useT, type I18n } from '../../settings/i18n'
import type { AutomationRunSummary } from './types'

const RECENT_LIMIT = 8

export function RunStatusCapsule({
  running,
  runs,
  error,
  liveStartedAt,
  resetKey,
}: {
  running: boolean
  runs: AutomationRunSummary[]
  error: string
  liveStartedAt: string | null
  resetKey: string
}) {
  const t = useT()
  const wrapRef = useRef<HTMLDivElement>(null)
  const [open, setOpen] = useState(false)
  const listId = useId()

  useEffect(() => {
    setOpen(false)
  }, [resetKey])

  useEffect(() => {
    if (!open) return
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    const onPointer = (event: MouseEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) setOpen(false)
    }
    window.addEventListener('keydown', onKey)
    window.addEventListener('mousedown', onPointer)
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('mousedown', onPointer)
    }
  }, [open])

  const recent = useMemo(
    () => visibleRuns(running, liveStartedAt, runs, error),
    [error, liveStartedAt, running, runs],
  )
  const head = recent[0]
  const status = running ? 'running' : head?.status ?? 'idle'
  const origin = running ? undefined : head?.origin
  const time = running ? liveStartedAt : head?.startedAt
  const detail = status === 'cancelled' ? '' : (head?.error || error)

  return (
    <div ref={wrapRef} className="kv-automation-run-capsule-wrap">
      {open ? (
        <div
          id={listId}
          className="kv-automation-run-popover"
          role="dialog"
          aria-label={t.chatAutomationExecutions}
        >
          <div className="kv-automation-run-popover-head">{t.chatAutomationExecutions}</div>
          {recent.length === 0 ? (
            <p className="kv-automation-run-popover-empty">{t.chatAutomationExecutionsEmpty}</p>
          ) : (
            <ul>
              {recent.map((run) => (
                <li key={run.id} className={`is-${run.status}`}>
                  <div className="kv-automation-run-row">
                    <span className="kv-automation-run-status">{statusLabel(t, run.status)}</span>
                    <span className="kv-automation-run-origin">
                      {run.origin ? originLabel(t, run.origin) : ''}
                    </span>
                    {run.startedAt ? (
                      <time dateTime={run.startedAt}>{formatRunTime(run.startedAt)}</time>
                    ) : (
                      <span />
                    )}
                  </div>
                  {run.error ? <p className="kv-automation-run-row-error">{run.error}</p> : null}
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
      <button
        type="button"
        className={`kv-automation-run-capsule is-${status}`}
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        title={detail || undefined}
        onClick={() => setOpen((current) => !current)}
      >
        <span className="kv-automation-run-dot" aria-hidden />
        <span className="kv-automation-run-capsule-copy">
          <span className="kv-automation-run-status">{statusLabel(t, status)}</span>
          {origin ? (
            <span className="kv-automation-run-origin">{originLabel(t, origin)}</span>
          ) : null}
          {time ? <time dateTime={time}>{formatRunTime(time)}</time> : null}
        </span>
        <ChevronDown
          size={14}
          strokeWidth={2}
          className={`kv-automation-run-chevron${open ? ' is-open' : ''}`}
          aria-hidden
        />
      </button>
    </div>
  )
}

function visibleRuns(
  running: boolean,
  liveStartedAt: string | null,
  runs: AutomationRunSummary[],
  error: string,
): AutomationRunSummary[] {
  const rest = runs.slice(0, RECENT_LIMIT)
  if (!running) return rest
  if (rest[0]?.status === 'running') return rest
  return [
    {
      id: '__live__',
      origin: '',
      status: 'running',
      startedAt: liveStartedAt ?? rest[0]?.startedAt ?? '',
      error: error || null,
    },
    ...rest.slice(0, RECENT_LIMIT - 1),
  ]
}

function originLabel(t: I18n, origin: string) {
  if (origin === 'manual') return t.chatAutomationTriggerManual
  if (origin === 'schedule') return t.chatAutomationTriggerSchedule
  if (origin === 'hotkey') return t.chatAutomationTriggerHotkey
  return origin
}

function statusLabel(t: I18n, status: string) {
  if (status === 'success') return t.chatAutomationStatusSuccess
  if (status === 'error') return t.chatAutomationStatusError
  if (status === 'running') return t.chatAutomationStatusRunning
  if (status === 'cancelled') return t.chatAutomationCancelled
  if (status === 'idle') return t.chatAutomationNeverRun
  return status
}

function formatRunTime(iso: string) {
  if (!iso) return ''
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso.replace('T', ' ').replace('Z', '')
  return date.toLocaleString(undefined, {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}
