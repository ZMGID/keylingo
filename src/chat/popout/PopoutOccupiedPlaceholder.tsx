import { ChatTitlebarActions } from '../ChatTitlebarActions'
import { Button } from '../../components/Button'
import { i18n, type Lang } from '../../settings/i18n'
import { chatTitlebarMacInsetClass, chatTitlebarRowClass, usesNativeTitlebar } from '../platform'
import type { ReactNode } from 'react'

type PopoutOccupiedPlaceholderProps = {
  lang: Lang
  onFocus: () => void
  onDock: () => void
  sidebarCollapsed: boolean
  titlebarControls: ReactNode
  onToggleSidebar: () => void
  onNewConversation: () => void
}

export function PopoutOccupiedPlaceholder({
  lang,
  onFocus,
  onDock,
  sidebarCollapsed,
  titlebarControls,
  onToggleSidebar,
  onNewConversation,
}: PopoutOccupiedPlaceholderProps) {
  const t = i18n[lang]
  return (
    <div className="chat-motion-pane-in chat-main-pane relative flex min-w-0 flex-1 flex-col">
      {usesNativeTitlebar && (
        <header
          className={`chat-titlebar-row ${chatTitlebarRowClass} min-w-0 gap-2 ${
            sidebarCollapsed
              ? `${chatTitlebarMacInsetClass} chat-titlebar-row--collapsed-mac pr-3`
              : 'px-6'
          }`}
          data-tauri-drag-region
        >
          {sidebarCollapsed && (
            <ChatTitlebarActions
              sidebarExpanded={false}
              onToggleSidebar={onToggleSidebar}
              onNewConversation={onNewConversation}
            />
          )}
          {titlebarControls}
        </header>
      )}
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-6">
        <p className="max-w-sm text-center text-[13px] text-neutral-500 dark:text-neutral-400">
          {t.chatPopoutOccupied}
        </p>
        <div className="flex items-center justify-center gap-2">
          <Button onClick={onFocus}>{t.chatPopoutFocus}</Button>
          <Button variant="primary" onClick={onDock}>{t.chatPopoutDock}</Button>
        </div>
      </div>
    </div>
  )
}
