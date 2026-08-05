import { useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { Copy, ClipboardCopy } from 'lucide-react'
import { useCloseAnimation } from './useCloseAnimation'

export interface MessageMenuAnchor {
  left: number
  top: number
}

interface MessageContextMenuProps {
  anchor: MessageMenuAnchor
  hasSelection: boolean
  canCopyMessage: boolean
  onCopySelection: () => void
  onCopyMessage: () => void
  onClose: () => void
}

// 消息区内置右键菜单：替代被屏蔽的原生菜单，提供「复制选中」/「复制整条消息」。
export function MessageContextMenu({
  anchor,
  hasSelection,
  canCopyMessage,
  onCopySelection,
  onCopyMessage,
  onClose: onCloseProp,
}: MessageContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const { closing, startClose, onAnimationEnd } = useCloseAnimation(onCloseProp)
  const onClose = startClose

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

  const itemClass =
    'kv-menu-item'

  const menu = (
    <div
      ref={menuRef}
      className={`kv-menu ${closing ? 'chat-motion-popover-out' : 'chat-motion-popover chat-motion-menu-cascade'} fixed z-[200] min-w-[176px]`}
      style={{ left: anchor.left, top: anchor.top }}
      role="menu"
      onAnimationEnd={onAnimationEnd}
    >
      {hasSelection && (
        <button
          type="button"
          role="menuitem"
          className={itemClass}
          onClick={() => {
            onCopySelection()
            onClose()
          }}
        >
          <Copy strokeWidth={1.75} />
          复制
        </button>
      )}
      {canCopyMessage && (
        <button
          type="button"
          role="menuitem"
          className={itemClass}
          onClick={() => {
            onCopyMessage()
            onClose()
          }}
        >
          <ClipboardCopy strokeWidth={1.75} />
          复制整条消息
        </button>
      )}
    </div>
  )

  return createPortal(menu, document.body)
}
