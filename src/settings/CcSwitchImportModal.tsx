import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { X } from 'lucide-react'
import { chatApi, type CcSwitchProvider } from '../chat/api'
import { Button, IconButton } from '../components/Button'
import { i18n, type Lang } from './i18n'

/**
 * 从 cc-switch 导入供应商。后端只读打开它的库，这里只负责勾选和落库。
 *
 * ponytail: 只列当前这个 CLI 的条目 —— 设置页本来就是按 CLI 逐个看的，跨 CLI 一次导入
 * 需要多一层分组 UI，收益不抵复杂度。切到另一个 CLI 再点一次导入即可。
 */
export function CcSwitchImportModal({
  lang,
  agentId,
  existingIds,
  onImport,
  onClose,
}: {
  lang: Lang
  agentId: string
  existingIds: string[]
  onImport: (providers: CcSwitchProvider[]) => void
  onClose: () => void
}) {
  const t = i18n[lang]
  const [items, setItems] = useState<CcSwitchProvider[] | null>(null)
  const [skipped, setSkipped] = useState(0)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [error, setError] = useState('')

  useEffect(() => {
    let cancelled = false
    void chatApi
      .externalCliScanCcSwitch()
      .then((scan) => {
        if (cancelled) return
        const mine = scan.providers.filter((p) => p.agentId === agentId)
        setItems(mine)
        setSkipped(scan.skipped)
        setSelected(new Set(mine.map((p) => p.id)))
      })
      .catch((err) => {
        if (!cancelled) setError(String(err))
      })
    return () => {
      cancelled = true
    }
  }, [agentId])

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const allSelected = !!items && items.length > 0 && selected.size === items.length

  return createPortal(
    <div
      className="kv-modal-backdrop kv-modal-backdrop--portal"
      data-tauri-drag-region="false"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div
        className="kv kv-modal max-w-lg space-y-3"
        role="dialog"
        aria-modal="true"
        aria-label={t.externalAgentsProviderImport}
        data-tauri-drag-region="false"
      >
        <div className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <h3 className="text-[14px] font-semibold">{t.externalAgentsProviderImport}</h3>
            <p className="kv-row-desc">{t.externalAgentsProviderImportHint}</p>
          </div>
          <IconButton size="xs" label={t.cancel} onClick={onClose} data-tauri-drag-region="false">
            <X size={14} />
          </IconButton>
        </div>

        {error && <p className="text-[12px] text-red-500 dark:text-red-400">{error}</p>}
        {!error && items === null && <p className="kv-row-desc">{t.externalAgentsRescanning}</p>}
        {!error && items?.length === 0 && <p className="kv-row-desc">{t.externalAgentsProviderImportEmpty}</p>}

        {!!items?.length && (
          <>
            <label className="flex items-center gap-2 text-[12px]">
              <input
                type="checkbox"
                checked={allSelected}
                onChange={() => setSelected(allSelected ? new Set() : new Set(items.map((p) => p.id)))}
              />
              {t.externalAgentsProviderImportSelectAll}
            </label>
            <div className="custom-scrollbar max-h-[50vh] space-y-1 overflow-y-auto pr-0.5">
              {items.map((item) => (
                <label key={item.id} className="kv-row !py-1.5">
                  <input
                    type="checkbox"
                    checked={selected.has(item.id)}
                    onChange={() => toggle(item.id)}
                    className="mr-2"
                  />
                  <div className="kv-row-text">
                    <div className="kv-row-label">{item.name}</div>
                    <p className="kv-row-desc">
                      {existingIds.includes(item.id)
                        ? t.externalAgentsProviderImportUpdate
                        : t.externalAgentsProviderImportNew}
                      {item.hasApiKey ? '' : ` · ${t.externalAgentsProviderImportNoKey}`}
                    </p>
                  </div>
                </label>
              ))}
            </div>
          </>
        )}

        {skipped > 0 && (
          <p className="kv-row-desc">
            {t.externalAgentsProviderImportSkipped.replace('{count}', String(skipped))}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-1">
          <Button onClick={onClose} data-tauri-drag-region="false">
            {t.cancel}
          </Button>
          <Button
            variant="primary"
            disabled={selected.size === 0}
            onClick={() => onImport((items ?? []).filter((item) => selected.has(item.id)))}
            data-tauri-drag-region="false"
          >
            {t.externalAgentsProviderImportConfirm.replace('{count}', String(selected.size))}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  )
}
