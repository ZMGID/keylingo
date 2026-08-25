import { PanelLeftClose, PanelLeftOpen, SquarePen } from 'lucide-react'
import { useT } from '../settings/i18n'
import { chatTitlebarPillIconClass } from './platform'

type ChatTitlebarActionsProps = {
  sidebarExpanded: boolean
  onToggleSidebar: () => void
  onNewConversation: () => void
}

export function ChatTitlebarActions({
  sidebarExpanded,
  onToggleSidebar,
  onNewConversation,
}: ChatTitlebarActionsProps) {
  const t = useT()
  const ToggleIcon = sidebarExpanded ? PanelLeftClose : PanelLeftOpen
  const toggleLabel = sidebarExpanded ? t.chatCollapseSidebar : t.chatExpandSidebar

  return (
    <div
      className="inline-flex h-8 shrink-0 items-center gap-0.5"
      data-tauri-drag-region="false"
    >
      <button
        type="button"
        onClick={onToggleSidebar}
        className={`${chatTitlebarPillIconClass} group`}
        title={toggleLabel}
        aria-label={toggleLabel}
      >
        <ToggleIcon
          size={15}
          strokeWidth={1.75}
          className={`transition-transform duration-300 ease-out group-hover:scale-110 group-active:scale-90 ${
            sidebarExpanded ? 'group-hover:-translate-x-0.5' : 'group-hover:translate-x-0.5'
          }`}
        />
      </button>
      <button
        type="button"
        onClick={onNewConversation}
        className={`${chatTitlebarPillIconClass} group`}
        title={t.chatNewChat}
        aria-label={t.chatNewChat}
      >
        <SquarePen
          size={15}
          strokeWidth={1.75}
          className="transition-transform duration-300 ease-out group-hover:-rotate-6 group-hover:scale-110 group-active:scale-90"
        />
      </button>
    </div>
  )
}
