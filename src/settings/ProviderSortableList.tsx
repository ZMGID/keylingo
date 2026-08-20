import { useMemo, useRef } from 'react'
import { GripHorizontal } from 'lucide-react'
import type { ModelProvider } from '../api/tauri'
import { ProviderIcon } from '../chat/ModelIcon'
import { usePointerReorder } from '../utils/pointerReorder'
import { isProviderEnabled } from './utils'

type ProviderSortableListProps = {
  providers: ModelProvider[]
  selectedId: string | undefined
  lang: 'zh' | 'en'
  providerNameLabel: string
  /** provider id → 手选的图标 key；缺省时按名字/域名自动匹配。 */
  icons?: Record<string, string>
  onSelect: (id: string) => void
  onReorder: (fromId: string, toId: string) => void
}

export function ProviderSortableList({
  providers,
  selectedId,
  lang,
  providerNameLabel,
  icons,
  onSelect,
  onReorder,
}: ProviderSortableListProps) {
  const listRef = useRef<HTMLDivElement>(null)
  const ids = useMemo(() => providers.map((p) => p.id), [providers])
  const { draggingId, startDrag, itemTransform } = usePointerReorder({
    ids,
    listRef,
    itemSelector: '.kv-provider-item',
    onReorder,
  })

  const dragLabel = lang === 'zh' ? '拖动调整顺序' : 'Drag to reorder'

  return (
    <div ref={listRef} className={`kv-provider-list-items custom-scrollbar${draggingId ? ' is-sorting' : ''}`}>
      {providers.map((provider, index) => {
        const configured = provider.apiKeys.some((key) => key.trim())
        const isDragging = draggingId === provider.id
        const transform = itemTransform(index)

        return (
          <div
            key={provider.id}
            className={`kv-provider-item ${selectedId === provider.id ? 'active' : ''}${isDragging ? ' is-dragging' : ''}`}
            style={transform ? { transform } : undefined}
            data-tauri-drag-region="false"
            role="button"
            tabIndex={0}
            onClick={() => onSelect(provider.id)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault()
                onSelect(provider.id)
              }
            }}
          >
            <span className="kv-provider-item-select">
              <span className={`kv-provider-dot ${!isProviderEnabled(provider) ? 'off' : configured ? 'on' : 'warn'}`} />
              <ProviderIcon
                name={provider.name}
                baseUrl={provider.baseUrl}
                iconKey={icons?.[provider.id]}
                size={15}
              />
              <span className="kv-provider-name">{provider.name || providerNameLabel}</span>
            </span>
            <button
              type="button"
              className="kv-provider-drag-handle"
              aria-label={dragLabel}
              title={dragLabel}
              onPointerDown={(e) => startDrag(e, provider.id, index)}
              onClick={(e) => e.stopPropagation()}
              data-tauri-drag-region="false"
            >
              <GripHorizontal size={13} strokeWidth={2} />
            </button>
          </div>
        )
      })}
    </div>
  )
}
