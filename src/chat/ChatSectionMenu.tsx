import { useEffect, useRef } from 'react'
import type { RefObject } from 'react'
import { createPortal } from 'react-dom'
import { Search, SquarePen, Trash2 } from 'lucide-react'
import type { ConversationMenuAnchor } from './ConversationContextMenu'
import { useCloseAnimation } from './useCloseAnimation'

interface ChatSectionMenuProps {
  anchor: ConversationMenuAnchor
  hasConversations: boolean
  onNewConversation: () => void
  onOpenSearch: () => void
  onClearAll: () => void
  onClose: () => void
  triggerRef?: RefObject<HTMLElement | null>
}

export function ChatSectionMenu({
  anchor,
  hasConversations,
  onNewConversation,
  onOpenSearch,
  onClearAll,
  onClose: onCloseProp,
  triggerRef,
}: ChatSectionMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const { closing, startClose, onAnimationEnd } = useCloseAnimation(onCloseProp)
  const onClose = startClose

  useEffect(() => {
    const onPointerDown = (e: MouseEvent) => {
      const target = e.target as Node
      if (menuRef.current?.contains(target)) return
      // 触发按钮自己处理 toggle，别让外部关闭抢先把菜单关掉再被点开。
      if (triggerRef?.current?.contains(target)) return
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
  }, [onClose, triggerRef])

  const menu = (
    <div
      ref={menuRef}
      className={`kv-menu ${closing ? 'chat-motion-popover-out' : 'chat-motion-popover chat-motion-menu-cascade'} fixed z-[200] min-w-[176px]`}
      style={{ left: anchor.left, top: anchor.top }}
      role="menu"
      onAnimationEnd={onAnimationEnd}
    >
      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        onClick={() => {
          onNewConversation()
          onClose()
        }}
      >
        <SquarePen strokeWidth={1.75} />
        新建聊天
      </button>
      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        onClick={() => {
          onOpenSearch()
          onClose()
        }}
      >
        <Search strokeWidth={1.75} />
        搜索对话
      </button>

      <div className="my-1 border-t border-neutral-200/80 dark:border-neutral-700" />

      <button
        type="button"
        role="menuitem"
        disabled={!hasConversations}
        className="kv-menu-item kv-menu-item--danger"
        onClick={() => {
          onClearAll()
          onClose()
        }}
      >
        <Trash2 strokeWidth={1.75} />
        清空全部对话
      </button>
    </div>
  )

  return createPortal(menu, document.body)
}
