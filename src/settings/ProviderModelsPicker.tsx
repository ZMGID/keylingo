import { useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { ChevronDown, Minus, Plus, RefreshCw, Search, X } from 'lucide-react'
import type { ModelProvider } from '../api/tauri'
import { Button, IconButton } from '../components/Button'
import { ModelIcon } from '../chat/ModelIcon'
import { Input } from './components'

type Lang = 'zh' | 'en'

export type ProviderModelsPickerLabels = {
  title: string
  searchPlaceholder: string
  fetchModels: string
  fetching: string
  addModel: string
  manualAddModel: string
  noModels: string
  noSearchResults: string
  enabled: string
  addAllModels: string
  close: string
}

type ProviderModelsPickerProps = {
  provider: ModelProvider
  lang: Lang
  labels: ProviderModelsPickerLabels
  fetching: boolean
  onClose: () => void
  onFetch: () => void
  onAdd: (model: string) => void
  onAddAll: (models: string[]) => void
  onRemove: (model: string) => void
}

function modelKey(model: string) {
  return model.toLowerCase()
}

export function ProviderModelsPicker({
  provider,
  lang,
  labels,
  fetching,
  onClose,
  onFetch,
  onAdd,
  onAddAll,
  onRemove,
}: ProviderModelsPickerProps) {
  const [query, setQuery] = useState('')
  const [groupOpen, setGroupOpen] = useState(true)
  const [manualOpen, setManualOpen] = useState(false)
  const [manualValue, setManualValue] = useState('')

  // 每次打开都拉一次：缓存列表可能已经过期，不能只在 availableModels 为空时才请求。
  useEffect(() => {
    onFetch()
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 只在挂载时刷新；手动刷新走按钮。
  }, [])

  const enabledSet = useMemo(
    () => new Set(provider.enabledModels.map(modelKey)),
    [provider.enabledModels],
  )

  const allModels = useMemo(() => {
    const seen = new Set<string>()
    const merged: string[] = []
    for (const model of [...provider.availableModels, ...provider.enabledModels]) {
      const key = modelKey(model)
      if (!model.trim() || seen.has(key)) continue
      seen.add(key)
      merged.push(model)
    }
    return merged.sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }))
  }, [provider.availableModels, provider.enabledModels])

  const filteredModels = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return allModels
    return allModels.filter((model) => model.toLowerCase().includes(q))
  }, [allModels, query])

  const addableModels = useMemo(
    () => filteredModels.filter((model) => !enabledSet.has(modelKey(model))),
    [enabledSet, filteredModels],
  )

  const submitManual = () => {
    const value = manualValue.trim()
    if (!value) return
    onAdd(value)
    setManualValue('')
    setManualOpen(false)
  }

  return createPortal(
    <div
      className="kv-modal-backdrop kv-modal-backdrop--portal"
      data-tauri-drag-region="false"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div
        className="kv kv-modal kv-model-picker"
        role="dialog"
        aria-modal="true"
        aria-labelledby="kv-model-picker-title"
        data-tauri-drag-region="false"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="kv-model-picker-header">
          <div className="kv-model-picker-title-row">
            <h3 id="kv-model-picker-title" className="kv-model-picker-title">
              {labels.title}
            </h3>
            <span className="kv-tag">{provider.enabledModels.length}</span>
          </div>
          <IconButton
            size="xs"
            onClick={onClose}
            data-tauri-drag-region="false"
            label={labels.close}
          >
            <X size={14} />
          </IconButton>
        </div>

        <div className="kv-model-picker-toolbar">
          <div className="kv-model-picker-search">
            <Search size={14} className="kv-model-picker-search-icon" />
            <Input
              value={query}
              onChange={setQuery}
              placeholder={labels.searchPlaceholder}
              mono={false}
            />
          </div>
          <IconButton
            size="sm"
            onClick={onFetch}
            disabled={fetching}
            data-tauri-drag-region="false"
            label={fetching ? labels.fetching : labels.fetchModels}
          >
            <RefreshCw size={14} className={fetching ? 'animate-spin' : ''} />
          </IconButton>
          <IconButton
            size="sm"
            className={manualOpen ? 'is-active' : ''}
            onClick={() => setManualOpen((open) => !open)}
            data-tauri-drag-region="false"
            aria-expanded={manualOpen}
            label={labels.manualAddModel}
          >
            <Plus size={14} strokeWidth={2.25} />
          </IconButton>
        </div>

        {manualOpen && (
          <div className="kv-model-picker-manual">
            <Input
              className="!text-[12px]"
              value={manualValue}
              onChange={setManualValue}
              placeholder={labels.manualAddModel}
              mono
              onKeyDown={(e: React.KeyboardEvent<HTMLInputElement>) => {
                if (e.key !== 'Enter') return
                if (e.nativeEvent.isComposing || e.keyCode === 229) return
                submitManual()
              }}
            />
            <Button
              size="sm"
              onClick={submitManual}
              data-tauri-drag-region="false"
            >
              {labels.addModel}
            </Button>
          </div>
        )}

        <div className="kv-model-picker-body custom-scrollbar">
          <div className="kv-model-picker-group-head">
            <button
              type="button"
              className="kv-model-picker-group-toggle"
              onClick={() => setGroupOpen((open) => !open)}
              data-tauri-drag-region="false"
            >
              <ChevronDown
                size={14}
                className={`kv-model-picker-chevron ${groupOpen ? 'open' : ''}`}
              />
              <span className="kv-model-picker-group-name truncate">
                {provider.name || provider.id}
              </span>
              <span className="kv-tag">{filteredModels.length}</span>
            </button>
            {addableModels.length > 0 && (
              <IconButton
                size="sm"
                className="shrink-0"
                onClick={() => onAddAll(addableModels)}
                data-tauri-drag-region="false"
                label={labels.addAllModels}
              >
                <Plus size={14} strokeWidth={2.25} />
              </IconButton>
            )}
          </div>

          {groupOpen && (
            <ul className="kv-model-picker-list">
              {fetching && filteredModels.length === 0 && (
                <li className="kv-model-picker-empty">{labels.fetching}</li>
              )}
              {!fetching && filteredModels.length === 0 && (
                <li className="kv-model-picker-empty">
                  {query.trim() ? labels.noSearchResults : labels.noModels}
                </li>
              )}
              {filteredModels.map((model) => {
                const isEnabled = enabledSet.has(modelKey(model))
                return (
                  <li key={model} className="kv-model-picker-row">
                    <ModelIcon model={model} size={16} />
                    <span className="kv-model-picker-row-name" title={model}>
                      {model}
                    </span>
                    {isEnabled ? (
                      <span className="kv-tag ok shrink-0">{labels.enabled}</span>
                    ) : null}
                    <IconButton
                      size="sm"
                      variant={isEnabled ? 'danger' : 'default'}
                      onClick={() => (isEnabled ? onRemove(model) : onAdd(model))}
                      data-tauri-drag-region="false"
                      label={
                        isEnabled
                          ? (lang === 'zh' ? `移除 ${model}` : `Remove ${model}`)
                          : (lang === 'zh' ? `添加 ${model}` : `Add ${model}`)
                      }
                    >
                      {isEnabled ? <Minus size={14} /> : <Plus size={14} strokeWidth={2.25} />}
                    </IconButton>
                  </li>
                )
              })}
            </ul>
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}
