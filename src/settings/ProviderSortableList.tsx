import { useEffect, useRef, useState, type ReactNode } from 'react'
import { GripHorizontal } from 'lucide-react'
import type { ModelProvider } from '../api/tauri'
import { ProviderIcon } from '../chat/ModelIcon'
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
  /** 追加在真实供应商项之后、同一列表容器内的节点（如快速预设项），保证连续排列不被撑到底部。 */
  trailing?: ReactNode
}

const clamp = (v: number, min: number, max: number) => Math.max(min, Math.min(max, v))

export function ProviderSortableList({
  providers,
  selectedId,
  lang,
  providerNameLabel,
  icons,
  onSelect,
  onReorder,
  trailing,
}: ProviderSortableListProps) {
  const [draggingId, setDraggingId] = useState<string | null>(null)
  const [overIndex, setOverIndex] = useState<number | null>(null)
  const [dragOffsetY, setDragOffsetY] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)
  const rowHeight = useRef(31)

  const draggingIndex = draggingId ? providers.findIndex((p) => p.id === draggingId) : -1

  useEffect(() => {
    if (!draggingId) return
    const prev = document.body.style.userSelect
    document.body.style.userSelect = 'none'
    return () => {
      document.body.style.userSelect = prev
    }
  }, [draggingId])

  const handlePointerDown = (e: React.PointerEvent<HTMLButtonElement>, providerId: string, index: number) => {
    e.preventDefault()
    e.stopPropagation()
    const list = listRef.current
    const item = (e.currentTarget as HTMLElement).closest('.kv-provider-item') as HTMLElement | null
    if (!list || !item) return

    // 行高 + 列表 gap(1px)。索引全程用「起始索引 + 位移/行高」算出，
    // 绝不读拖动中被 transform/过渡位移的 rect（旧实现的抖动与重叠根源）。
    const rowH = (item.getBoundingClientRect().height || 30) + 1
    rowHeight.current = rowH
    const startY = e.clientY
    const startScrollTop = list.scrollTop
    const maxIndex = providers.length - 1
    let lastY = startY
    let raf = 0

    setDraggingId(providerId)
    setOverIndex(index)
    setDragOffsetY(0)

    const currentOffset = () => {
      const raw = lastY - startY + (list.scrollTop - startScrollTop)
      return clamp(raw, -index * rowH, (maxIndex - index) * rowH)
    }

    const update = () => {
      const offset = currentOffset()
      setDragOffsetY(offset)
      setOverIndex(clamp(index + Math.round(offset / rowH), 0, maxIndex))
    }

    // 指针悬在列表上下边缘时持续滚动（pointermove 不动时也要滚，所以走 rAF）
    const autoScroll = () => {
      const rect = list.getBoundingClientRect()
      const zone = 24
      let delta = 0
      if (lastY < rect.top + zone) delta = -Math.min(10, Math.ceil((rect.top + zone - lastY) / 4))
      else if (lastY > rect.bottom - zone) delta = Math.min(10, Math.ceil((lastY - (rect.bottom - zone)) / 4))
      if (delta) {
        const prev = list.scrollTop
        list.scrollTop += delta
        if (list.scrollTop !== prev) update()
      }
      raf = requestAnimationFrame(autoScroll)
    }
    raf = requestAnimationFrame(autoScroll)

    const onMove = (ev: PointerEvent) => {
      lastY = ev.clientY
      update()
    }

    const onUp = () => {
      cancelAnimationFrame(raf)
      document.removeEventListener('pointermove', onMove)
      document.removeEventListener('pointerup', onUp)
      document.removeEventListener('pointercancel', onUp)
      const toIndex = clamp(index + Math.round(currentOffset() / rowH), 0, maxIndex)
      const toId = providers[toIndex]?.id
      if (toId && toId !== providerId) onReorder(providerId, toId)
      setDraggingId(null)
      setOverIndex(null)
      setDragOffsetY(0)
    }

    try {
      e.currentTarget.setPointerCapture(e.pointerId)
    } catch {
      /* 捕获失败不致命，document 监听仍覆盖窗口内拖动 */
    }
    document.addEventListener('pointermove', onMove)
    document.addEventListener('pointerup', onUp)
    document.addEventListener('pointercancel', onUp)
  }

  const getItemTransform = (index: number) => {
    if (draggingIndex < 0 || overIndex === null) return undefined
    const h = rowHeight.current
    if (index === draggingIndex) return `translateY(${dragOffsetY}px)`
    if (draggingIndex < overIndex) {
      if (index > draggingIndex && index <= overIndex) return `translateY(${-h}px)`
    } else if (draggingIndex > overIndex) {
      if (index >= overIndex && index < draggingIndex) return `translateY(${h}px)`
    }
    return undefined
  }

  const dragLabel = lang === 'zh' ? '拖动调整顺序' : 'Drag to reorder'

  return (
    <div ref={listRef} className={`kv-provider-list-items custom-scrollbar${draggingId ? ' is-sorting' : ''}`}>
      {providers.map((provider, index) => {
        const configured = provider.apiKeys.some((key) => key.trim())
        const isDragging = draggingId === provider.id
        const transform = getItemTransform(index)

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
              onPointerDown={(e) => handlePointerDown(e, provider.id, index)}
              onClick={(e) => e.stopPropagation()}
              data-tauri-drag-region="false"
            >
              <GripHorizontal size={13} strokeWidth={2} />
            </button>
          </div>
        )
      })}
      {trailing}
    </div>
  )
}
