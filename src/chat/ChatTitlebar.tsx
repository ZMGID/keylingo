import { memo, type ReactNode } from 'react'
import { ChatTitlebarActions } from './ChatTitlebarActions'
import { WindowControls } from './WindowControls'

type ChatTitlebarProps = {
  sidebarExpanded: boolean
  onToggleSidebar: () => void
  onNewConversation: () => void
  /** 侧栏卡片当前是否占位。true 时业务控件让到主区那一列，与主内容左缘对齐。 */
  sidebarVisible?: boolean
  /**
   * 设置页语境：带改为浮在内容之上（不占行高），左段不放按钮。
   *
   * 设置页的 200px 灰导航栏是齐窗口左缘的实心列，带若仍是独立一行，灰栏上方会横着
   * 一条主背景白带 —— 那就是顶栏「突兀」的来源。带浮起后导航栏自己从 y=0 画起，
   * 宽度与底色全归它一家管；这很关键，因为它的宽度由 `@container settings-shell`
   * 收窄（200→52px），而容器在内容行内，带在容器外收不到那个查询，无法跟随。
   *
   * 左段留空：「侧栏开合 / 新建聊天」在设置页无意义（聊天侧栏已被导航栏顶掉），
   * 「返回对话」导航栏底部已有一枚，不重复放。带保持全宽透明，故窗口拖拽区不丢。
   */
  settingsMode?: boolean
  /** 弹出窗：不渲染侧栏开合 / 新建，带仍占行高（不要用 settingsMode，那会浮到正文上）。 */
  hideNav?: boolean
  /** 会话页的业务控件（模型 / 思考档位 / 权限 / Dock 等）。中心页传 null，带内只剩导航与三键。 */
  children?: ReactNode
}

/**
 * Windows / Linux 的全宽标题栏带（macOS 走系统 Overlay 标题栏，不渲染此组件）。
 *
 * 贯穿窗口全宽：左端常驻「侧栏开合 + 新建聊天」，中段是会话页业务控件，右端贴角 caption 键。
 * 业务控件并进这一条 —— 非 mac 不再有主区 52px 顶栏，chrome 只此一行。
 * 侧栏展开时导航区撑到侧栏宽，业务控件随之让到主区一列（否则它们压在侧栏卡片上方，读作错位）。
 * 侧栏卡片 / Dock / 主内容都在带下方开始，所以三键不压内容，
 * 各视图也不必各自留出躲避空间（旧的 `.chat-win-titlebar-safe` 与中心页兜底拖拽带因此删除）。
 */
export const ChatTitlebar = memo(function ChatTitlebar({
  sidebarExpanded,
  onToggleSidebar,
  onNewConversation,
  sidebarVisible = false,
  settingsMode = false,
  hideNav = false,
  children,
}: ChatTitlebarProps) {
  return (
    <div
      className={`chat-titlebar-strip${settingsMode ? ' chat-titlebar-strip--settings' : ''}${hideNav && !settingsMode ? ' chat-titlebar-strip--solid' : ''}`}
      data-tauri-drag-region
    >
      {!settingsMode && !hideNav && (
        <div
          className={`chat-titlebar-strip-nav${sidebarVisible ? ' chat-titlebar-strip-nav--reserve' : ''}`}
          data-tauri-drag-region
        >
          <ChatTitlebarActions
            sidebarExpanded={sidebarExpanded}
            onToggleSidebar={onToggleSidebar}
            onNewConversation={onNewConversation}
          />
        </div>
      )}
      <div
        className="flex min-w-0 flex-1 items-center gap-1 self-stretch"
        data-tauri-drag-region
      >
        {children}
      </div>
      <WindowControls />
    </div>
  )
})
