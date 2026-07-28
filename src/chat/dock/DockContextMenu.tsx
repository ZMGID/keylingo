// Dock 内共享的右键菜单：fixed 定位 + 视口夹取（面板贴窗口右缘，夹取后不会溢出面板）。
// 样式沿用 kv-menu / kv-menu-item（与 ConversationContextMenu 一致）。
import { useEffect, useRef, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useClampedMenuPosition } from '../useClampedMenuPosition'

export type DockMenuItem = {
  key: string
  label: string
  icon?: ReactNode
  danger?: boolean
  disabled?: boolean
  onSelect: () => void
}

export type DockMenuAnchor = {
  left: number
  top: number
}

type DockContextMenuProps = {
  anchor: DockMenuAnchor
  items: DockMenuItem[]
  onClose: () => void
}

export function DockContextMenu({ anchor, items, onClose }: DockContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const pos = useClampedMenuPosition(menuRef, anchor)

  useEffect(() => {
    const onPointerDown = (e: MouseEvent) => {
      if (menuRef.current?.contains(e.target as Node)) return
      onClose()
    }
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('mousedown', onPointerDown)
    window.addEventListener('keydown', onKeyDown)
    return () => {
      window.removeEventListener('mousedown', onPointerDown)
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [onClose])

  return createPortal(
    <div
      ref={menuRef}
      className="kv-menu chat-motion-popover fixed z-[250] min-w-[168px]"
      style={{ left: pos.left, top: pos.top }}
      role="menu"
    >
      {items.map((item) => (
        <button
          key={item.key}
          type="button"
          role="menuitem"
          className={`kv-menu-item${item.danger ? ' kv-menu-item--danger' : ''}`}
          disabled={item.disabled}
          onClick={() => {
            onClose()
            item.onSelect()
          }}
        >
          {item.icon}
          <span className="min-w-0 flex-1">{item.label}</span>
        </button>
      ))}
    </div>,
    document.body,
  )
}
