import { useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { Download, FolderOpen, Pencil, Trash2 } from 'lucide-react'
import type { ConversationMenuAnchor } from './ConversationContextMenu'
import { useCloseAnimation } from './useCloseAnimation'
import { useClampedMenuPosition } from './useClampedMenuPosition'

interface ProjectContextMenuProps {
  anchor: ConversationMenuAnchor
  hasRootFolder: boolean
  onRename: () => void
  onOpenFolder: () => void
  onImportFromCli: () => void
  onDelete: () => void
  onClose: () => void
}

export function ProjectContextMenu({
  anchor,
  hasRootFolder,
  onRename,
  onOpenFolder,
  onImportFromCli,
  onDelete,
  onClose: onCloseProp,
}: ProjectContextMenuProps) {
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
      className={`kv-menu ${closing ? 'chat-motion-popover-out' : 'chat-motion-popover chat-motion-menu-cascade'} fixed z-[200] min-w-[176px]`}
      style={{ left: pos.left, top: pos.top }}
      role="menu"
      onAnimationEnd={onAnimationEnd}
    >
      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        onClick={() => {
          onRename()
          onClose()
        }}
      >
        <Pencil strokeWidth={1.75} />
        重命名
      </button>
      <button
        type="button"
        role="menuitem"
        disabled={!hasRootFolder}
        title={hasRootFolder ? undefined : '请先在项目设置中选择文件夹'}
        className="kv-menu-item"
        onClick={() => {
          if (!hasRootFolder) return
          onOpenFolder()
          onClose()
        }}
      >
        <FolderOpen strokeWidth={1.75} />
        打开项目文件夹
      </button>
      <button
        type="button"
        role="menuitem"
        disabled={!hasRootFolder}
        // 导入按工作目录匹配会话，没有项目根就无从比对（ADR-0001）。
        title={hasRootFolder ? undefined : '请先在项目设置中选择文件夹'}
        className="kv-menu-item"
        onClick={() => {
          if (!hasRootFolder) return
          onImportFromCli()
          onClose()
        }}
      >
        <Download strokeWidth={1.75} />
        从 CLI 导入对话
      </button>
      <div className="my-1 border-t border-neutral-200/80 dark:border-neutral-700" />
      <button
        type="button"
        role="menuitem"
        className="kv-menu-item kv-menu-item--danger"
        onClick={() => {
          onDelete()
          onClose()
        }}
      >
        <Trash2 strokeWidth={1.75} />
        删除项目
      </button>
    </div>,
    document.body,
  )
}
