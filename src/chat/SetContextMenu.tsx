import { useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { Pencil, Trash2 } from 'lucide-react'
import type { ConversationMenuAnchor } from './ConversationContextMenu'
import { useCloseAnimation } from './useCloseAnimation'
import { useClampedMenuPosition } from './useClampedMenuPosition'

interface SetContextMenuProps {
  anchor: ConversationMenuAnchor
  onRename: () => void
  onDelete: () => void
  onClose: () => void
}

export function SetContextMenu({ anchor, onRename, onDelete, onClose: onCloseProp }: SetContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const pos = useClampedMenuPosition(menuRef, anchor)
  const { closing, startClose, onAnimationEnd } = useCloseAnimation(onCloseProp)
  const onClose = startClose

  useEffect(() => {
    const onPointerDown = (e: MouseEvent) => {
      const target = e.target as Node
      if (menuRef.current?.contains(target)) return
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
      className={`${closing ? 'chat-motion-popover-out' : 'chat-motion-popover chat-motion-menu-cascade'} fixed z-[200] min-w-[180px] rounded-xl border border-neutral-200/90 bg-white py-1.5 shadow-lg dark:border-neutral-700 dark:bg-[#2a2a2c]`}
      style={{ left: pos.left, top: pos.top }}
      role="menu"
      onAnimationEnd={onAnimationEnd}
    >
      <button
        type="button"
        role="menuitem"
        className="flex w-full items-center gap-3 px-3.5 py-2 text-left text-[13px] text-neutral-800 transition-colors hover:bg-black/[0.04] dark:text-neutral-100 dark:hover:bg-white/[0.06]"
        onClick={() => {
          onRename()
          onClose()
        }}
      >
        <Pencil size={16} strokeWidth={1.75} className="shrink-0 text-neutral-500" />
        重命名 / 设置
      </button>
      <div className="my-1 border-t border-neutral-200/80 dark:border-neutral-700" />
      <button
        type="button"
        role="menuitem"
        className="flex w-full items-center gap-3 px-3.5 py-2 text-left text-[13px] text-red-600 transition-colors hover:bg-red-50 dark:text-red-400 dark:hover:bg-red-500/10"
        onClick={() => {
          onDelete()
          onClose()
        }}
      >
        <Trash2 size={16} strokeWidth={1.75} className="shrink-0" />
        删除集
      </button>
    </div>,
    document.body,
  )
}
