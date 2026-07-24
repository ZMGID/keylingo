import { useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { ChevronRight, Download, Folder, Layers, Pencil, Trash2 } from 'lucide-react'
import type { Lang } from '../settings/i18n'
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
  projects: ChatProject[]
  sets: ChatSet[]
  lang: Lang
  onRename: () => void
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
  projects,
  sets,
  lang,
  onRename,
  onExport,
  onMoveToProject,
  onMoveToSet,
  onDelete,
  onClose: onCloseProp,
}: ConversationContextMenuProps) {
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
      className={`${closing ? 'chat-motion-popover-out' : 'chat-motion-popover chat-motion-menu-cascade'} fixed z-[200] min-w-[200px] rounded-xl border border-neutral-200/90 bg-white py-1.5 shadow-lg dark:border-neutral-700 dark:bg-[#2a2a2c]`}
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
        重命名
      </button>

      <div className="group/sub relative">
        <button
          type="button"
          role="menuitem"
          className="flex w-full items-center gap-3 px-3.5 py-2 text-left text-[13px] text-neutral-800 transition-colors hover:bg-black/[0.04] dark:text-neutral-100 dark:hover:bg-white/[0.06]"
        >
          <Folder size={16} strokeWidth={1.75} className="shrink-0 text-neutral-500" />
          <span className="min-w-0 flex-1">添加到项目</span>
          <ChevronRight size={16} className="shrink-0 text-neutral-400" />
        </button>

        <div className="pointer-events-none absolute left-full top-0 z-[201] min-w-[168px] pl-1 opacity-0 transition-opacity group-hover/sub:pointer-events-auto group-hover/sub:opacity-100">
          <div className="rounded-xl border border-neutral-200/90 bg-white py-1.5 shadow-lg dark:border-neutral-700 dark:bg-[#2a2a2c]">
            {projects.length === 0 ? (
              <div className="px-3.5 py-2 text-[13px] text-neutral-400">暂无项目</div>
            ) : (
              projects.map((project) => {
                const active = conversationProjectId
                  ? conversationProjectId === project.id
                  : conversationFolder === project.name
                return (
                <button
                  key={project.id}
                  type="button"
                  className={`flex w-full px-3.5 py-2 text-left text-[13px] transition-colors hover:bg-black/[0.04] dark:hover:bg-white/[0.06] ${
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
                  className="flex w-full px-3.5 py-2 text-left text-[13px] text-neutral-600 transition-colors hover:bg-black/[0.04] dark:text-neutral-300 dark:hover:bg-white/[0.06]"
                  onClick={() => {
                    onMoveToProject(undefined)
                    onClose()
                  }}
                >
                  移出项目
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
          className="flex w-full items-center gap-3 px-3.5 py-2 text-left text-[13px] text-neutral-800 transition-colors hover:bg-black/[0.04] dark:text-neutral-100 dark:hover:bg-white/[0.06]"
        >
          <Layers size={16} strokeWidth={1.75} className="shrink-0 text-neutral-500" />
          <span className="min-w-0 flex-1">移动到集</span>
          <ChevronRight size={16} className="shrink-0 text-neutral-400" />
        </button>

        <div className="pointer-events-none absolute left-full top-0 z-[201] min-w-[168px] pl-1 opacity-0 transition-opacity group-hover/subset:pointer-events-auto group-hover/subset:opacity-100">
          <div className="rounded-xl border border-neutral-200/90 bg-white py-1.5 shadow-lg dark:border-neutral-700 dark:bg-[#2a2a2c]">
            {sets.length === 0 ? (
              <div className="px-3.5 py-2 text-[13px] text-neutral-400">暂无集</div>
            ) : (
              sets.map((set) => {
                const active = conversationSetId === set.id
                return (
                  <button
                    key={set.id}
                    type="button"
                    className={`flex w-full px-3.5 py-2 text-left text-[13px] transition-colors hover:bg-black/[0.04] dark:hover:bg-white/[0.06] ${
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
                  className="flex w-full px-3.5 py-2 text-left text-[13px] text-neutral-600 transition-colors hover:bg-black/[0.04] dark:text-neutral-300 dark:hover:bg-white/[0.06]"
                  onClick={() => {
                    onMoveToSet(undefined)
                    onClose()
                  }}
                >
                  移出集
                </button>
              </>
            )}
          </div>
        </div>
      </div>

      <button
        type="button"
        role="menuitem"
        className="flex w-full items-center gap-3 px-3.5 py-2 text-left text-[13px] text-neutral-800 transition-colors hover:bg-black/[0.04] dark:text-neutral-100 dark:hover:bg-white/[0.06]"
        onClick={() => {
          onExport()
          onClose()
        }}
      >
        <Download size={16} strokeWidth={1.75} className="shrink-0 text-neutral-500" />
        {lang === 'zh' ? '导出' : 'Export'}
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
        删除
      </button>
    </div>
  )

  return createPortal(menu, document.body)
}
