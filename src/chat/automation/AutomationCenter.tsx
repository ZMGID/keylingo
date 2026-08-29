import { useCallback, useEffect, useRef, useState } from 'react'
import { isTauriRuntime } from '../../api/tauri'
import { useT } from '../../settings/i18n'
import { getRouteAutomationId, setHash } from '../chatRoutes'
import { automationApi } from './api'
import { AutomationEditor } from './AutomationEditor'
import { AutomationList } from './AutomationList'
import { createBlankAutomation } from './graph'
import type { Automation, AutomationMeta } from './types'

export function AutomationCenter() {
  const t = useT()
  const [items, setItems] = useState<AutomationMeta[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [editing, setEditing] = useState<Automation | null>(null)
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const editingRef = useRef<Automation | null>(null)
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

  const openId = useCallback(async (id: string) => {
    if (editingRef.current?.id === id) return
    setError('')
    try {
      const automation = await automationApi.get(id)
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
        if (saveTimerRef.current) window.clearTimeout(saveTimerRef.current)
        const current = editingRef.current
        if (current && isTauriRuntime()) {
          void automationApi.save(current).catch((err) => {
            setError(err instanceof Error ? err.message : String(err))
          })
        }
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
    if (saveTimerRef.current) window.clearTimeout(saveTimerRef.current)
    saveTimerRef.current = window.setTimeout(() => {
      void automationApi
        .save(next)
        .then((saved) => {
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
    }, 400)
  }, [loadList])

  const create = useCallback(async () => {
    const blank = createBlankAutomation()
    blank.name = t.chatAutomationUntitled
    try {
      const saved = isTauriRuntime() ? await automationApi.save(blank) : blank
      setEditing(saved)
      setHash(`#chat/automations/${encodeURIComponent(saved.id)}`)
      void loadList()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [loadList, t])

  const backToList = useCallback(() => {
    if (saveTimerRef.current) window.clearTimeout(saveTimerRef.current)
    const current = editingRef.current
    const finish = () => {
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
        <AutomationEditor
          key={editing.id}
          automation={editing}
          onChange={persist}
          onBack={backToList}
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
