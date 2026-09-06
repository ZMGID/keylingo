import { useCallback, useEffect, useRef, useState } from 'react'
import { api, isTauriRuntime } from '../../api/tauri'
import { automationApi } from './api'
import type { AutomationRunSummary, NodeRunStatus } from './types'

function nodeStatus(status: string): NodeRunStatus {
  if (status === 'running' || status === 'error') return status
  if (status === 'skipped' || status === 'cancelled') return 'skipped'
  return 'success'
}

export function useAutomationRunState(automationId: string) {
  const [running, setRunning] = useState(false)
  const [runError, setRunError] = useState('')
  const [statuses, setStatuses] = useState<Record<string, NodeRunStatus>>({})
  const [nodeOutput, setNodeOutput] = useState<Record<string, string>>({})
  const [runs, setRuns] = useState<AutomationRunSummary[]>([])
  const [liveStartedAt, setLiveStartedAt] = useState<string | null>(null)
  const startedAtRef = useRef<string | null>(null)
  const historyRequestRef = useRef(0)

  const loadRuns = useCallback(async () => {
    if (!isTauriRuntime()) return
    const request = ++historyRequestRef.current
    try {
      const history = await automationApi.listRuns(automationId)
      if (request === historyRequestRef.current) setRuns(history)
    } catch {
      // History is best effort; it must not disable execution controls.
    }
  }, [automationId])

  useEffect(() => { void loadRuns() }, [loadRuns])

  useEffect(() => {
    if (!isTauriRuntime()) return
    let disposed = false
    let revision = 0
    let unlisten: (() => void) | undefined
    void api.onAutomationRun((event) => {
      if (disposed || event.automationId !== automationId) return
      revision += 1
      if (event.kind === 'run_started') {
        const startedAt = new Date().toISOString()
        startedAtRef.current = startedAt
        setLiveStartedAt(startedAt)
        setRunning(true)
        setRunError('')
        setStatuses({})
        setNodeOutput({})
      }
      if (event.kind === 'node_started' && event.nodeId) {
        setRunning(true)
        setStatuses((current) => ({ ...current, [event.nodeId!]: 'running' }))
      }
      if (event.kind === 'node_finished' && event.nodeId) {
        setStatuses((current) => ({ ...current, [event.nodeId!]: nodeStatus(event.status ?? 'success') }))
        if (event.output != null) {
          setNodeOutput((current) => ({ ...current, [event.nodeId!]: event.output! }))
        }
        if (event.error && event.status === 'error') setRunError(event.error)
      }
      if (event.kind === 'run_finished') {
        setRuns((current) => [{
          id: event.runId,
          origin: current.find((run) => run.id === event.runId)?.origin ?? '',
          status: event.status ?? 'success',
          startedAt: startedAtRef.current ?? new Date().toISOString(),
          error: event.error ?? null,
        }, ...current.filter((run) => run.id !== event.runId)])
        startedAtRef.current = null
        setRunning(false)
        setLiveStartedAt(null)
        setRunError(event.status === 'error' ? event.error ?? '' : '')
        void loadRuns()
      }
    }).then(async (fn) => {
      if (disposed) { fn(); return }
      unlisten = fn
      // Subscribe first. If an event arrives during the query, fetch again so
      // a stale running snapshot cannot undo a newer run_finished event.
      while (!disposed) {
        const before = revision
        const run = await automationApi.activeRun(automationId)
        if (disposed) return
        if (before !== revision) continue
        if (!run) return
        const active = run.status === 'running'
        setRunning(active)
        startedAtRef.current = active ? run.startedAt : null
        setLiveStartedAt(startedAtRef.current)
        setStatuses(Object.fromEntries(run.nodes.map((node) => [node.nodeId, nodeStatus(node.status)])))
        setNodeOutput(Object.fromEntries(run.nodes.filter((node) => node.output != null).map((node) => [node.nodeId, node.output!])))
        setRunError(run.status === 'error' ? run.error ?? '' : '')
        setRuns((current) => [run, ...current.filter((item) => item.id !== run.id)])
        return
      }
    }).catch((error: unknown) => {
      if (!disposed) setRunError(error instanceof Error ? error.message : String(error))
    })
    return () => { disposed = true; unlisten?.() }
  }, [automationId, loadRuns])

  return { running, setRunning, runError, setRunError, nodeStatus: statuses, nodeOutput, runs, liveStartedAt }
}
