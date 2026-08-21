import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AtSign, Check, Star } from 'lucide-react'
import { type ModelProvider } from '../api/tauri'
import { getSettingsCached, setFavoriteModelsCached, subscribeSettings } from '../api/settingsCache'
import { IconButton } from '../components/Button'
import { useT } from '../settings/i18n'
import { isProviderEnabled } from '../settings/utils'
import { ModelIcon } from './ModelIcon'
import { MAX_REPLY_MODELS } from './messageGroups'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import type { ModelRef } from './types'

const favKey = (providerId: string, model: string) => `${providerId}:${model}`
const parseFavKey = (key: string): { providerId: string; model: string } | null => {
  const idx = key.indexOf(':')
  if (idx <= 0 || idx >= key.length - 1) return null
  return { providerId: key.slice(0, idx), model: key.slice(idx + 1) }
}

interface ReplyWithModelButtonProps {
  occupied: ModelRef[]
  onSelect: (providerId: string, model: string) => void
}

export function ReplyWithModelButton({ occupied, onSelect }: ReplyWithModelButtonProps) {
  const t = useT()
  const [open, setOpen] = useState(false)
  const [providers, setProviders] = useState<ModelProvider[]>([])
  const [favorites, setFavorites] = useState<string[]>([])
  const triggerRef = useRef<HTMLDivElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const maxH = usePopoverMaxHeight(open, popoverRef, 'up', 360)
  const atLimit = occupied.length >= MAX_REPLY_MODELS

  const loadSettings = useCallback(async () => {
    try {
      const settings = await getSettingsCached()
      setProviders(settings.providers || [])
      setFavorites(settings.favoriteModels || [])
    } catch (err) {
      console.error('Failed to load providers:', err)
      setProviders([])
    }
  }, [])

  useEffect(() => {
    if (open) void loadSettings()
  }, [open, loadSettings])

  useEffect(() => {
    return subscribeSettings((settings) => {
      setProviders(settings.providers || [])
      setFavorites(settings.favoriteModels || [])
    })
  }, [])

  useEffect(() => {
    if (!open) return
    const onDown = (event: MouseEvent) => {
      const node = event.target as Node
      if (triggerRef.current?.contains(node)) return
      setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open])

  const occupiedKeys = useMemo(
    () => new Set(occupied.map((item) => `${item.provider_id}\0${item.model}`)),
    [occupied],
  )

  const visibleProviders = useMemo(
    () =>
      providers
        .filter(isProviderEnabled)
        .map((provider) => ({
          provider,
          models: provider.enabledModels.length > 0 ? provider.enabledModels : provider.availableModels,
        }))
        .filter((entry) => entry.models.length > 0),
    [providers],
  )

  const favoriteEntries = useMemo(
    () =>
      favorites
        .map((key) => {
          const parsed = parseFavKey(key)
          if (!parsed) return null
          const entry = visibleProviders.find((item) => item.provider.id === parsed.providerId)
          if (!entry || !entry.models.includes(parsed.model)) return null
          return {
            key,
            providerId: parsed.providerId,
            model: parsed.model,
          }
        })
        .filter((item): item is { key: string; providerId: string; model: string } => item !== null),
    [favorites, visibleProviders],
  )

  const toggleFavorite = useCallback(
    (providerId: string, model: string) => {
      const key = favKey(providerId, model)
      const next = favorites.includes(key)
        ? favorites.filter((item) => item !== key)
        : [...favorites, key]
      const previous = favorites
      setFavorites(next)
      setFavoriteModelsCached(next).catch((err) => {
        console.error('Failed to save favorite models:', err)
        setFavorites(previous)
      })
    },
    [favorites],
  )

  const pick = (providerId: string, model: string) => {
    if (occupiedKeys.has(`${providerId}\0${model}`) || atLimit) return
    setOpen(false)
    onSelect(providerId, model)
  }

  const renderRow = (providerId: string, model: string, keySuffix: string) => {
    const occupiedAlready = occupiedKeys.has(`${providerId}\0${model}`)
    const disabled = occupiedAlready || atLimit
    const isFav = favorites.includes(favKey(providerId, model))
    const label = occupiedAlready
      ? t.chatReplyWithModelOccupied
      : atLimit
        ? t.chatReplyWithModelLimit.replace('{max}', String(MAX_REPLY_MODELS))
        : model
    return (
      <div
        key={`${providerId}:${model}:${keySuffix}`}
        className={`group flex w-full items-center gap-1 rounded-lg pr-1 ${
          disabled ? '' : 'hover:bg-neutral-50 dark:hover:bg-neutral-800/80'
        }`}
      >
        <button
          type="button"
          disabled={disabled}
          title={label}
          onClick={() => pick(providerId, model)}
          className={`kv-menu-row min-w-0 flex-1 ${
            disabled
              ? 'cursor-default text-neutral-300 dark:text-neutral-600'
              : 'text-neutral-700 dark:text-neutral-300'
          }`}
        >
          <ModelIcon model={model} size={16} />
          <span className="min-w-0 truncate">{model}</span>
          {occupiedAlready && <Check size={12} strokeWidth={2.5} className="ml-auto shrink-0" />}
        </button>
        <button
          type="button"
          aria-label={isFav ? t.chatUnfavorite : t.chatFavorite}
          title={isFav ? t.chatUnfavorite : t.chatFavorite}
          onClick={(event) => {
            event.stopPropagation()
            toggleFavorite(providerId, model)
          }}
          className={`shrink-0 rounded-md p-1.5 transition-colors ${
            isFav
              ? 'text-amber-500'
              : 'text-neutral-300 opacity-0 group-hover:opacity-100 hover:text-amber-500 dark:text-neutral-600'
          }`}
        >
          <Star size={12} strokeWidth={2} fill={isFav ? 'currentColor' : 'none'} />
        </button>
      </div>
    )
  }

  return (
    <div ref={triggerRef} className="relative">
      <IconButton
        size="xs"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-haspopup="menu"
        label={t.chatReplyWithModelAria}
        title={t.chatReplyWithModel}
      >
        <AtSign size={13} strokeWidth={2} />
      </IconButton>
      {open && (
        <div
          ref={popoverRef}
          className="chat-motion-popover chat-popover-scroll absolute bottom-full left-0 z-50 mb-1.5 w-[min(280px,calc(100vw-24px))] overflow-y-auto kv-menu"
          style={{ ['--chat-popover-origin' as string]: 'bottom left', maxHeight: maxH }}
          data-tauri-drag-region="false"
          role="menu"
        >
          <div className="px-2.5 py-1 text-[11px] font-medium text-neutral-400">
            {t.chatReplyWithModelHint.replace('{max}', String(MAX_REPLY_MODELS))}
          </div>
          {favoriteEntries.length > 0 && (
            <div className="px-1 py-0.5">
              <div className="px-2.5 pt-1 pb-0.5 text-[10px] font-semibold uppercase tracking-wide text-neutral-400">
                {t.chatFavorites}
              </div>
              {favoriteEntries.map((entry) => renderRow(entry.providerId, entry.model, 'fav'))}
            </div>
          )}
          {visibleProviders.map(({ provider, models }) => (
            <div key={provider.id} className="px-1 py-0.5">
              <div className="px-2.5 pt-1 pb-0.5 text-[10px] font-semibold uppercase tracking-wide text-neutral-400">
                {provider.name}
              </div>
              {models.map((model) => renderRow(provider.id, model, provider.id))}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
