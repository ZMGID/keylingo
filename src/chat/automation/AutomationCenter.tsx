import { useCallback, useEffect, useRef, useState } from 'react'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { api, isTauriRuntime } from '../../api/tauri'
import { useT } from '../../settings/i18n'
import { getRouteAutomationId, setHash } from '../chatRoutes'
import { automationApi } from './api'
import { AutomationEditor } from './AutomationEditor'
import { AutomationList } from './AutomationList'
import { createBlankAutomation } from './graph'
import type { Automation, AutomationMeta } from './types'

function clearTimeoutRef(ref: { current: ReturnType<typeof setTimeout> | null }) {
  if (ref.current == null) return
  window.clearTimeout(ref.current)
  ref.current = null
}

export function AutomationCenter() {
  const t = useT()
  const [items, setItems] = useState<AutomationMeta[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [editing, setEditing] = useState<Automation | null>(null)
  const [canvasEpoch, setCanvasEpoch] = useState(0)
  const [remoteHint, setRemoteHint] = useState('')
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const editingRef = useRef<Automation | null>(null)
  const lastSelfUpdatedAtRef = useRef('')
  const selfSaveInFlightRef = useRef(0)
  editingRef.current = editing

  const loadList = useCallback(async () => {
    if (!isTauriRuntime()) {
      setLoading(false)
      setError(t.chatAutomationAppOnly)
      return
    }
    setError('')
    try {
      setItems(await automationApi.list())
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [t])

  useEffect(() => {
    void loadList()
  }, [loadList])

  useEffect(() => {
    if (!isTauriRuntime()) return
    let cancelled = false
    let unlisten: (() => void) | undefined
    void api.onAutomationChanged((event) => {
      if (cancelled) return
      void loadList()
      const current = editingRef.current
      if (!current || event.id !== current.id) return
      if (event.kind === 'deleted') {
        clearTimeoutRef(saveTimerRef)
        setRemoteHint('')
        setEditing(null)
        setHash('#chat/automations')
        return
      }
      if (selfSaveInFlightRef.current > 0) return
      if (event.updatedAt && event.updatedAt === lastSelfUpdatedAtRef.current) return
      if (saveTimerRef.current) {
        setRemoteHint(t.chatAutomationRemoteUpdate)
        return
      }
      void automationApi.get(current.id).then((fresh) => {
        if (cancelled) return
        if (editingRef.current?.id !== fresh.id) return
        if (saveTimerRef.current || selfSaveInFlightRef.current > 0) {
          setRemoteHint(t.chatAutomationRemoteUpdate)
          return
        }
        if (
          fresh.updatedAt === lastSelfUpdatedAtRef.current
          || fresh.updatedAt === editingRef.current.updatedAt
        ) {
          return
        }
        lastSelfUpdatedAtRef.current = fresh.updatedAt
        setRemoteHint('')
        setEditing(fresh)
        setCanvasEpoch((n) => n + 1)
      }).catch(() => {})
    }).then((fn) => {
      if (cancelled) fn()
      else unlisten = fn
    }).catch(() => {})
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [loadList, t])

  const openId = useCallback(async (id: string) => {
    if (editingRef.current?.id === id) return
    setError('')
    try {
      const automation = await automationApi.get(id)
      lastSelfUpdatedAtRef.current = automation.updatedAt
      setRemoteHint('')
      setCanvasEpoch(0)
      setEditing(automation)
      setHash(`#chat/automations/${encodeURIComponent(id)}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  useEffect(() => {
    const syncFromHash = () => {
      const id = getRouteAutomationId()
      if (!id) {
        clearTimeoutRef(saveTimerRef)
        const current = editingRef.current
        if (current && isTauriRuntime()) {
          void automationApi.save(current).catch((err) => {
            setError(err instanceof Error ? err.message : String(err))
          })
        }
        setRemoteHint('')
        setEditing(null)
        return
      }
      void openId(id)
    }
    syncFromHash()
    window.addEventListener('hashchange', syncFromHash)
    return () => window.removeEventListener('hashchange', syncFromHash)
  }, [openId])

  const persist = useCallback((next: Automation) => {
    setEditing(next)
    if (!isTauriRuntime()) return
    clearTimeoutRef(saveTimerRef)
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = null
      selfSaveInFlightRef.current += 1
      void automationApi
        .save(next)
        .then((saved) => {
          lastSelfUpdatedAtRef.current = saved.updatedAt
          setEditing((current) => (
            current && current.id === saved.id
              ? { ...current, updatedAt: saved.updatedAt }
              : current
          ))
          void loadList()
        })
        .catch((err) => {
          setError(err instanceof Error ? err.message : String(err))
        })
        .finally(() => {
          selfSaveInFlightRef.current = Math.max(0, selfSaveInFlightRef.current - 1)
        })
    }, 400)
  }, [loadList])

  const create = useCallback(async () => {
    const blank = createBlankAutomation()
    blank.name = t.chatAutomationUntitled
    try {
      const saved = isTauriRuntime() ? await automationApi.save(blank) : blank
      lastSelfUpdatedAtRef.current = saved.updatedAt
      setRemoteHint('')
      setCanvasEpoch(0)
      setEditing(saved)
      setHash(`#chat/automations/${encodeURIComponent(saved.id)}`)
      void loadList()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [loadList, t])

  const importFromFile = useCallback(async () => {
    if (!isTauriRuntime()) return
    try {
      const picked = await openDialog({
        multiple: false,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      })
      if (typeof picked !== 'string') return
      const imported = await automationApi.importFromFile(picked)
      lastSelfUpdatedAtRef.current = imported.updatedAt
      setRemoteHint('')
      setCanvasEpoch(0)
      setEditing(imported)
      setHash(`#chat/automations/${encodeURIComponent(imported.id)}`)
      void loadList()
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setError(`${t.chatAutomationImportFailed}${message}`)
    }
  }, [loadList, t])

  const backToList = useCallback(() => {
    clearTimeoutRef(saveTimerRef)
    const current = editingRef.current
    const finish = () => {
      setRemoteHint('')
      setEditing(null)
      setHash('#chat/automations')
      void loadList()
    }
    if (current && isTauriRuntime()) {
      void automationApi.save(current).finally(finish)
      return
    }
    finish()
  }, [loadList])

  if (editing) {
    return (
      <div className="flex h-full min-h-0 flex-1 flex-col">
        {error ? (
          <p className="shrink-0 px-6 py-2 text-[13px] text-red-600 dark:text-red-400">{error}</p>
        ) : null}
        {remoteHint ? (
          <p className="shrink-0 px-6 py-2 text-[13px] text-amber-700 dark:text-amber-400">{remoteHint}</p>
        ) : null}
        <AutomationEditor
          key={`${editing.id}:${canvasEpoch}`}
          automation={editing}
          onChange={persist}
          onBack={backToList}
          onFlushSave={async () => {
            clearTimeoutRef(saveTimerRef)
            const current = editingRef.current
            if (current && isTauriRuntime()) {
              selfSaveInFlightRef.current += 1
              try {
                const saved = await automationApi.save(current)
                lastSelfUpdatedAtRef.current = saved.updatedAt
                setEditing((existing) => (
                  existing && existing.id === saved.id
                    ? { ...existing, updatedAt: saved.updatedAt }
                    : existing
                ))
              } finally {
                selfSaveInFlightRef.current = Math.max(0, selfSaveInFlightRef.current - 1)
              }
            }
          }}
        />
      </div>
    )
  }

  return (
    <AutomationList
      items={items}
      loading={loading}
      error={error}
      onCreate={() => void create()}
      onImport={() => void importFromFile()}
      onOpen={(id) => void openId(id)}
      onToggle={(id, enabled) => {
        void automationApi.setEnabled(id, enabled).then(loadList).catch((err) => {
          setError(err instanceof Error ? err.message : String(err))
        })
      }}
      onDelete={(id) => {
        if (!window.confirm(t.chatAutomationDeleteConfirm)) return
        void automationApi.remove(id).then(loadList).catch((err) => {
          setError(err instanceof Error ? err.message : String(err))
        })
      }}
    />
  )
}
