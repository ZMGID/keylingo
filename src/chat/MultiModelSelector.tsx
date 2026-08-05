import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Layers } from 'lucide-react'
import { type ModelProvider } from '../api/tauri'
import { getSettingsCached } from '../api/settingsCache'
import { useT } from '../settings/i18n'
import { isProviderEnabled } from '../settings/utils'
import { ModelIcon } from './ModelIcon'
import { IconButton } from '../components/Button'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import type { ModelRef } from './types'

const MAX_REPLY_MODELS = 4

interface MultiModelSelectorProps {
  // 当前会话级多答模型集（含单模型时的会话主模型 0/1 个）。
  value: ModelRef[]
  onChange: (models: ModelRef[]) => void
  // 弹层方向：footer 朝上、inline 朝下。
  placement?: 'up' | 'down'
}

function sameRef(a: ModelRef, b: ModelRef): boolean {
  return a.provider_id === b.provider_id && a.model === b.model
}

function MultiModelSelectorBase({ value, onChange, placement = 'up' }: MultiModelSelectorProps) {
  const t = useT()
  const [open, setOpen] = useState(false)
  const [providers, setProviders] = useState<ModelProvider[]>([])
  const triggerRef = useRef<HTMLDivElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const maxH = usePopoverMaxHeight(open, popoverRef, placement === 'down' ? 'down' : 'up', 360)

  const loadProviders = useCallback(async () => {
    try {
      const settings = await getSettingsCached()
      setProviders(settings.providers || [])
    } catch (err) {
      console.error('Failed to load providers:', err)
      setProviders([])
    }
  }, [])

  useEffect(() => {
    if (open) void loadProviders()
  }, [open, loadProviders])

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node
      if (triggerRef.current?.contains(t)) return
      setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open])

  const activeProviders = useMemo(() => providers.filter(isProviderEnabled), [providers])
  // 只显示有可选模型的服务商，避免没配置模型的服务商变成空的分组标题。
  const visibleProviders = useMemo(
    () =>
      activeProviders
        .map((provider) => ({
          provider,
          models: provider.enabledModels.length > 0 ? provider.enabledModels : provider.availableModels,
        }))
        .filter((entry) => entry.models.length > 0),
    [activeProviders],
  )
  const atLimit = value.length >= MAX_REPLY_MODELS

  const providerName = useCallback(
    (providerId: string) =>
      activeProviders.find((p) => p.id === providerId)?.name
      ?? providers.find((p) => p.id === providerId)?.name
      ?? providerId,
    [activeProviders, providers],
  )

  const toggle = useCallback(
    (providerId: string, model: string) => {
      const ref: ModelRef = { provider_id: providerId, model }
      const exists = value.some((item) => sameRef(item, ref))
      if (exists) {
        onChange(value.filter((item) => !sameRef(item, ref)))
        return
      }
      if (value.length >= MAX_REPLY_MODELS) return
      onChange([...value, ref])
    },
    [onChange, value],
  )

  const removeChip = useCallback(
    (ref: ModelRef) => onChange(value.filter((item) => !sameRef(item, ref))),
    [onChange, value],
  )

  const enabled = value.length >= 2

  const placementClass = placement === 'down' ? 'top-full mt-1.5' : 'bottom-full mb-1.5'
  const popoverOrigin = placement === 'down' ? 'top left' : 'bottom left'

  return (
    <div ref={triggerRef} className="relative flex min-w-0 items-center gap-1" data-tauri-drag-region="false">
      <IconButton
        size="sm"
        shape="circle"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-haspopup="menu"
        label={t.chatMultiModelLabel.replace('{max}', String(MAX_REPLY_MODELS))}
        className={`shrink-0 ${enabled ? 'text-emerald-600 dark:text-emerald-400' : ''}`}
      >
        <Layers size={18} strokeWidth={1.75} className="shrink-0" />
      </IconButton>

      {value.length > 0 && (
        <div className="group/stack flex items-center pl-0.5" data-tauri-drag-region="false">
          {value.map((ref, i) => (
            <button
              key={`${ref.provider_id}:${ref.model}`}
              type="button"
              onClick={() => removeChip(ref)}
              aria-label={t.chatRemoveNamed.replace('{name}', ref.model)}
              className="chat-model-stack-item relative -ml-2 transition-[margin,transform] duration-[var(--kv-dur-normal)] ease-[var(--kv-ease-standard)] first:ml-0 hover:scale-110 active:scale-95 group-hover/stack:ml-1 group-hover/stack:first:ml-0"
              style={{ zIndex: value.length - i }}
              title={t.chatModelChipHint
                .replace('{model}', ref.model)
                .replace('{provider}', providerName(ref.provider_id))}
            >
              <span className="grid size-6 place-items-center rounded-full border border-neutral-200 bg-white dark:border-neutral-600 dark:bg-neutral-800">
                <ModelIcon model={ref.model} size={14} />
              </span>
            </button>
          ))}
        </div>
      )}

      {open && (
        <div
          ref={popoverRef}
          className={`chat-motion-popover chat-popover-scroll absolute left-0 z-50 w-[min(280px,calc(100vw-24px))] overflow-y-auto kv-menu ${placementClass}`}
          style={{ ['--chat-popover-origin' as string]: popoverOrigin, maxHeight: maxH }}
          data-tauri-drag-region="false"
          role="menu"
        >
          <div className="px-2.5 py-1 text-[11px] font-medium text-neutral-400">
            {t.chatMultiModelHint
              .replace('{n}', String(value.length))
              .replace('{max}', String(MAX_REPLY_MODELS))}
          </div>
          {visibleProviders.map(({ provider, models }) => (
            <div key={provider.id} className="px-1 py-0.5">
              <div className="px-2.5 pt-1 pb-0.5 text-[10px] font-semibold uppercase tracking-wide text-neutral-400">
                {provider.name}
              </div>
              {models.map((model) => {
                const checked = value.some((item) => sameRef(item, { provider_id: provider.id, model }))
                const disabled = !checked && atLimit
                return (
                  <button
                    key={model}
                    type="button"
                    disabled={disabled}
                    onClick={() => toggle(provider.id, model)}
                    className={`kv-menu-row transition-colors ${
                      checked
                        ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100'
                        : disabled
                          ? 'cursor-default text-neutral-300 dark:text-neutral-600'
                          : 'text-neutral-700 hover:bg-neutral-50 dark:text-neutral-300 dark:hover:bg-neutral-800/80'
                    }`}
                  >
                    <span
                      className={`flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded border ${
                        checked
                          ? 'border-emerald-500 bg-emerald-500 text-white'
                          : 'border-neutral-300 dark:border-neutral-600'
                      }`}
                    >
                      {checked && <span className="text-[10px] leading-none">✓</span>}
                    </span>
                    <ModelIcon model={model} size={16} />
                    <span className="min-w-0 truncate">{model}</span>
                  </button>
                )
              })}
            </div>
          ))}
          {visibleProviders.length === 0 && (
            <div className="px-4 py-6 text-center text-sm text-neutral-500">{t.chatNoModels}</div>
          )}
        </div>
      )}
    </div>
  )
}

export const MultiModelSelector = memo(MultiModelSelectorBase)
