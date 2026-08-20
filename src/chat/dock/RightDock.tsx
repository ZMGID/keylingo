// Right Dock 容器：tab 条（文件树 / Git / 终端 / 任务）+ 常驻面板 + 左缘拖拽调宽 + 折叠滑出。
// 宽度通过 CSS 变量 --chat-dock-width 直写（拖拽过程不触发 React 重渲），松手才持久化。
import { memo, useCallback, useRef, type PointerEvent as ReactPointerEvent } from 'react'
import { Activity, FolderTree, GitBranch, Terminal, X } from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'
import { IconButton } from '../../components/Button'
import { FileTreePanel } from './FileTreePanel'
import { GitPanel } from './GitPanel'
import { TerminalPanel } from './TerminalPanel'
import { BackgroundTasksPanel } from './BackgroundTasksPanel'

export type DockTab = 'files' | 'git' | 'terminal' | 'tasks'

export const DOCK_MIN_WIDTH = 320
export const DOCK_MAX_WIDTH = 560
export const DOCK_DEFAULT_WIDTH = 360

export type DockRevealRequest = {
  path: string
  nonce: number
} | null

/** 工具卡片 → 右侧栏预览：点文件名预览文件（workdir 可以不同于文件树，如写到桌面的
 *  文件用其所在目录）；点 +N -N 徽标预览整份带色 diff；claude 提交计划时预览整份计划。 */
export type DockPreviewRequest =
  | { kind: 'file'; workdir: string; path: string; nonce: number }
  | { kind: 'diff'; title: string; patch: string; nonce: number }
  | { kind: 'markdown'; title: string; text: string; nonce: number }
  | null

type RightDockProps = {
  open: boolean
  width: number
  activeTab: DockTab
  workdir: string
  lang: Lang
  /** 当前对话 id（任务页按对话隔离）。null = 还没有对话。 */
  conversationId: string | null
  treeExpanded: string[]
  revealRequest: DockRevealRequest
  previewRequest: DockPreviewRequest
  onToggleTab: (tab: DockTab) => void
  onWidthChange: (width: number) => void
  onClose: () => void
  onTreeExpandedChange: (paths: string[]) => void
  onInsertMention?: (path: string) => void
  onRevealInTree: (path: string) => void
}

export const RightDock = memo(function RightDock({
  open,
  width,
  activeTab,
  workdir,
  lang,
  conversationId,
  treeExpanded,
  revealRequest,
  previewRequest,
  onToggleTab,
  onWidthChange,
  onClose,
  onTreeExpandedChange,
  onInsertMention,
  onRevealInTree,
}: RightDockProps) {
  const t = i18n[lang]
  const shellRef = useRef<HTMLElement>(null)
  const dragStateRef = useRef<{ startX: number; startWidth: number; width: number; raf: number } | null>(null)

  const handleResizeStart = useCallback(
    (e: ReactPointerEvent<HTMLDivElement>) => {
      e.preventDefault()
      const startWidth = shellRef.current?.getBoundingClientRect().width ?? width
      dragStateRef.current = { startX: e.clientX, startWidth, width: startWidth, raf: 0 }

      const applyWidth = (nextWidth: number) => {
        shellRef.current?.style.setProperty('--chat-dock-width', `${nextWidth}px`)
      }

      const onMove = (moveEvent: PointerEvent) => {
        const state = dragStateRef.current
        if (!state) return
        // 左缘手柄：向左拖变宽。
        const delta = state.startX - moveEvent.clientX
        const nextWidth = Math.min(DOCK_MAX_WIDTH, Math.max(DOCK_MIN_WIDTH, Math.round(state.startWidth + delta)))
        state.width = nextWidth
        if (!state.raf) {
          state.raf = window.requestAnimationFrame(() => {
            state.raf = 0
            applyWidth(state.width)
          })
        }
      }
      const onUp = () => {
        window.removeEventListener('pointermove', onMove)
        window.removeEventListener('pointerup', onUp)
        const state = dragStateRef.current
        dragStateRef.current = null
        if (!state) return
        if (state.raf) window.cancelAnimationFrame(state.raf)
        applyWidth(state.width)
        if (state.width !== Math.round(startWidth)) onWidthChange(state.width)
      }
      window.addEventListener('pointermove', onMove)
      window.addEventListener('pointerup', onUp)
    },
    [width, onWidthChange],
  )

  const filesActive = open && activeTab === 'files'
  const gitActive = open && activeTab === 'git'
  const terminalActive = open && activeTab === 'terminal'
  return (
    <aside
      ref={shellRef}
      className={`chat-dock-shell relative flex min-h-0 shrink-0 flex-col overflow-hidden${open ? '' : ' is-collapsed'}`}
      style={{ ['--chat-dock-width' as string]: `${width}px` }}
      aria-hidden={!open}
    >
      {/* 左缘拖拽手柄 */}
      <div
        className="absolute bottom-0 left-0 top-0 z-20 w-[6px] cursor-col-resize"
        onPointerDown={handleResizeStart}
      />

      {/* tab 条 */}
      <div className="flex shrink-0 items-center gap-0.5 border-b border-neutral-200/70 py-1.5 pl-3 pr-1.5 dark:border-neutral-700/50">
        {(
          [
            { tab: 'files' as DockTab, label: t.dockTabFiles, icon: FolderTree },
            { tab: 'git' as DockTab, label: t.dockTabGit, icon: GitBranch },
            { tab: 'terminal' as DockTab, label: t.dockTabTerminal, icon: Terminal },
            { tab: 'tasks' as DockTab, label: t.dockTabTasks, icon: Activity },
          ]
        ).map(({ tab, label, icon: Icon }) => (
          <button
            key={tab}
            type="button"
            className={`flex items-center gap-1.5 rounded-md px-2 py-1 text-[12px] transition-colors ${
              activeTab === tab
                ? 'bg-neutral-500/10 font-medium text-neutral-800 dark:bg-neutral-400/10 dark:text-neutral-100'
                : 'text-neutral-500 hover:bg-neutral-500/5 hover:text-neutral-700 dark:text-neutral-400 dark:hover:text-neutral-200'
            }`}
            onClick={() => onToggleTab(tab)}
          >
            <Icon size={13} strokeWidth={1.75} />
            {label}
          </button>
        ))}
        <div className="flex-1" />
        <IconButton label={t.dockClose} size="sm" variant="ghost" onClick={onClose}>
          <X size={13} />
        </IconButton>
      </div>

      {/* 文件 / Git / 终端 / 任务常驻挂载，inactive 只 hidden。 */}
      <div className="flex min-h-0 flex-1 flex-col" hidden={activeTab !== 'files'}>
        <FileTreePanel
          workdir={workdir}
          active={filesActive}
          lang={lang}
          expandedPaths={treeExpanded}
          onExpandedChange={onTreeExpandedChange}
          revealPath={revealRequest?.path ?? null}
          revealNonce={revealRequest?.nonce ?? 0}
          previewRequest={previewRequest}
          onInsertMention={onInsertMention}
        />
      </div>
      <div className="flex min-h-0 flex-1 flex-col" hidden={activeTab !== 'git'}>
        <GitPanel workdir={workdir} active={gitActive} lang={lang} onRevealInTree={onRevealInTree} />
      </div>
      <div className="flex min-h-0 flex-1 flex-col" hidden={activeTab !== 'terminal'}>
        <TerminalPanel workdir={workdir} active={terminalActive} lang={lang} />
      </div>
      <div className="flex min-h-0 flex-1 flex-col" hidden={activeTab !== 'tasks'}>
        <BackgroundTasksPanel
          active={open && activeTab === 'tasks'}
          lang={lang}
          conversationId={conversationId}
        />
      </div>
    </aside>
  )
})
