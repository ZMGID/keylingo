import { useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { ChevronRight, Download, Folder, Layers, Pencil, Pin, PinOff, RotateCcw, Trash2 } from 'lucide-react'
import { i18n, type Lang } from '../settings/i18n'
import type { ChatProject, ChatSet } from './types'
import { useCloseAnimation } from './useCloseAnimation'
import { useClampedMenuPosition } from './useClampedMenuPosition'

export interface ConversationMenuAnchor {
  left: number
  top: number
}

interface ConversationContextMenuProps {
  anchor: ConversationMenuAnchor
  conversationFolder?: string
  conversationProjectId?: string | null
  conversationSetId?: string | null
  pinned?: boolean
  projects: ChatProject[]
  sets: ChatSet[]
  lang: Lang
  canRegenerateTitle?: boolean
  regeneratingTitle?: boolean
  onRename: () => void
  onRegenerateTitle: () => void
  onTogglePin?: () => void
  onExport: () => void
  onMoveToProject: (projectId: string | undefined) => void
  onMoveToSet: (setId: string | undefined) => void
  onDelete: () => void
  onClose: () => void
}

export function ConversationContextMenu({
  anchor,
  conversationFolder,
  conversationProjectId,
  conversationSetId,
  pinned = false,
  projects,
  sets,
  lang,
  canRegenerateTitle = true,
  regeneratingTitle = false,
  onRename,
  onRegenerateTitle,
  onTogglePin,
  onExport,
  onMoveToProject,
  onMoveToSet,
  onDelete,
  onClose: onCloseProp,
}: ConversationContextMenuProps) {
  const t = i18n[lang]
  const menuRef = useRef<HTMLDivElement>(null)
  const pos = useClampedMenuPosition(menuRef, anchor)
  const { closing, startClose, onAnimationEnd } = useCloseAnimation(onCloseProp)
  // 所有内部关闭触发（菜单项动作后 / 外部点击 / Esc）走 startClose，先播退场再卸载
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

  const menu = (
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
        {t.chatRename}
      </button>

      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        disabled={!canRegenerateTitle || regeneratingTitle}
        onClick={() => {
          onRegenerateTitle()
          onClose()
        }}
      >
        <RotateCcw strokeWidth={1.75} />
        {t.chatRegenerateTitle}
      </button>

      {onTogglePin && (
        <button
          type="button"
          role="menuitem"
          className="kv-menu-item"
          onClick={() => {
            onTogglePin()
            onClose()
          }}
        >
          {pinned ? <PinOff strokeWidth={1.75} /> : <Pin strokeWidth={1.75} />}
          {pinned ? t.chatUnpin : t.chatPin}
        </button>
      )}

      <div className="group/sub relative">
        <button
          type="button"
          role="menuitem"
          className="kv-menu-item"
        >
          <Folder strokeWidth={1.75} />
          <span className="min-w-0 flex-1">{t.chatAddToProject}</span>
          <ChevronRight size={16} className="shrink-0 text-neutral-400" />
        </button>

        <div className="pointer-events-none absolute left-full top-0 z-[201] min-w-[168px] pl-1 opacity-0 transition-opacity group-hover/sub:pointer-events-auto group-hover/sub:opacity-100">
          <div className="kv-menu">
            {projects.length === 0 ? (
              <div className="kv-menu-item">{t.chatNoProjects}</div>
            ) : (
              projects.map((project) => {
                const active = conversationProjectId
                  ? conversationProjectId === project.id
                  : conversationFolder === project.name
                return (
                <button
                  key={project.id}
                  type="button"
                  className={`kv-menu-item ${
                    active
                      ? 'font-medium text-neutral-900 dark:text-neutral-50'
                      : 'text-neutral-800 dark:text-neutral-100'
                  }`}
                  onClick={() => {
                    onMoveToProject(project.id)
                    onClose()
                  }}
                >
                  {project.name}
                </button>
                )
              })
            )}
            {(conversationProjectId || conversationFolder) && (
              <>
                <div className="my-1 border-t border-neutral-200/80 dark:border-neutral-700" />
                <button
                  type="button"
                  className="kv-menu-item"
                  onClick={() => {
                    onMoveToProject(undefined)
                    onClose()
                  }}
                >
                  {t.chatRemoveFromProject}
                </button>
              </>
            )}
          </div>
        </div>
      </div>

      <div className="group/subset relative">
        <button
          type="button"
          role="menuitem"
          className="kv-menu-item"
        >
          <Layers strokeWidth={1.75} />
          <span className="min-w-0 flex-1">{t.chatMoveToSet}</span>
          <ChevronRight size={16} className="shrink-0 text-neutral-400" />
        </button>

        <div className="pointer-events-none absolute left-full top-0 z-[201] min-w-[168px] pl-1 opacity-0 transition-opacity group-hover/subset:pointer-events-auto group-hover/subset:opacity-100">
          <div className="kv-menu">
            {sets.length === 0 ? (
              <div className="kv-menu-item">{t.chatNoSets}</div>
            ) : (
              sets.map((set) => {
                const active = conversationSetId === set.id
                return (
                  <button
                    key={set.id}
                    type="button"
                    className={`kv-menu-item ${
                      active
                        ? 'font-medium text-neutral-900 dark:text-neutral-50'
                        : 'text-neutral-800 dark:text-neutral-100'
                    }`}
                    onClick={() => {
                      onMoveToSet(set.id)
                      onClose()
                    }}
                  >
                    {set.name}
                  </button>
                )
              })
            )}
            {conversationSetId && (
              <>
                <div className="my-1 border-t border-neutral-200/80 dark:border-neutral-700" />
                <button
                  type="button"
                  className="kv-menu-item"
                  onClick={() => {
                    onMoveToSet(undefined)
                    onClose()
                  }}
                >
                  {t.chatRemoveFromSet}
                </button>
              </>
            )}
          </div>
        </div>
      </div>

      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        onClick={() => {
          onExport()
          onClose()
        }}
      >
        <Download strokeWidth={1.75} />
        {t.chatExport}
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
        {t.chatDelete}
      </button>
    </div>
  )

  return createPortal(menu, document.body)
}
