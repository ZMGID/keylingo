import { lazy, memo, Profiler, Suspense, type ProfilerOnRenderCallback, type ReactNode } from 'react'
import { GitBranch, TriangleAlert, X } from 'lucide-react'
import { ChatImageViewer } from './ChatImageViewer'
import { ConversationLoadingState } from './ConversationLoadingState'
import { ChatTitlebarActions } from './ChatTitlebarActions'
import { useConversationTransition } from './conversationTransitionStore'
import { InputBar, type InputBarProps } from './InputBar'
import { KivioBlob } from './KivioBlob'
import { emptyHeroLine } from './emptyHero'
import { TypewriterText } from './TypewriterText'
import { QueuedMessages } from './QueuedMessages'
import { IconButton } from '../components/Button'
import type { QueuedMessage } from './hooks/useMessageQueue'
import type { MessageListProps } from './MessageList'
import type { ChatImageViewerItem } from './imageViewer'
import type { ChatHookPayload } from '../api/tauri'
import type { Lang } from '../settings/i18n'
import { i18n } from '../settings/i18n'

const MessageList = lazy(() => import('./MessageList').then((module) => ({
  default: module.MessageList,
})))

function MessageListLoading() {
  return (
    <div className="chat-themed-surface flex flex-1 items-center justify-center">
      <div className="h-5 w-5 animate-spin rounded-full border-2 border-neutral-300 border-t-neutral-800 dark:border-neutral-700 dark:border-t-neutral-200" />
    </div>
  )
}

export interface ChatConversationPaneProps {
  titlebarControls: ReactNode
  usesNativeTitlebar: boolean
  sidebarCollapsed: boolean
  titlebarRowClass: string
  titlebarMacInsetClass: string
  onToggleSidebar: () => void
  onNewConversation: () => void
  protocolVersionMismatch: boolean
  showEmptyHero: boolean
  currentAssistantName: string | null
  selectedProjectName: string | null
  selectedSetName: string | null
  inputBarProps: InputBarProps
  messageListProps: MessageListProps
  hookWarning: ChatHookPayload | null
  currentConversationId: string | null
  onDismissHookWarning: () => void
  forkOrigin: { sourceId: string; title: string } | null
  onSelectConversation: (id: string) => void
  importedHistoryStale: boolean
  pendingSlot: ReactNode
  queuedMessages: QueuedMessage[]
  canSteerQueuedMessages: boolean
  onSteerQueuedMessage: (messageId: string) => void
  onRemoveQueuedMessage: (id: string) => void
  onRestoreQueuedMessage: (messageId: string) => void
  lang: Lang
  imageViewerItem: ChatImageViewerItem | null
  onCloseImageViewer: () => void
  onRender: ProfilerOnRenderCallback
}

/**
 * 会话主区的完整渲染边界。
 *
 * Chat 只负责状态和路由协调；正文、空态、消息列表、审批槽和输入栏都在这里。
 * 侧栏/设置切换不再让 Chat.tsx 直接重建这段巨型 JSX。
 */
export const ChatConversationPane = memo(function ChatConversationPane({
  titlebarControls,
  usesNativeTitlebar,
  sidebarCollapsed,
  titlebarRowClass,
  titlebarMacInsetClass,
  onToggleSidebar,
  onNewConversation,
  protocolVersionMismatch,
  showEmptyHero,
  currentAssistantName,
  selectedProjectName,
  selectedSetName,
  inputBarProps,
  messageListProps,
  hookWarning,
  currentConversationId,
  onDismissHookWarning,
  forkOrigin,
  onSelectConversation,
  importedHistoryStale,
  pendingSlot,
  queuedMessages,
  canSteerQueuedMessages,
  onSteerQueuedMessage,
  onRemoveQueuedMessage,
  onRestoreQueuedMessage,
  lang,
  imageViewerItem,
  onCloseImageViewer,
  onRender,
}: ChatConversationPaneProps) {
  const transition = useConversationTransition()
  const conversationLoading = transition.loading
  return (
    <div className="chat-motion-pane-in chat-main-pane relative flex min-w-0 flex-1 flex-col">
      {usesNativeTitlebar && (
        <header
          className={`chat-titlebar-row ${titlebarRowClass} min-w-0 gap-2 ${
            sidebarCollapsed
              ? `${titlebarMacInsetClass} chat-titlebar-row--collapsed-mac pr-3`
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

      {protocolVersionMismatch && (
        <div
          className="flex shrink-0 items-center gap-2 border-y border-amber-200 bg-amber-50 px-4 py-2 text-xs font-medium text-amber-900 dark:border-amber-400/20 dark:bg-amber-400/10 dark:text-amber-100"
          role="alert"
        >
          <TriangleAlert className="shrink-0" size={15} aria-hidden="true" />
          <span>组件版本不一致，请重启 Kivio</span>
        </div>
      )}

      <div className="relative flex min-h-0 flex-1 flex-col">
        {showEmptyHero ? (
          <div className="chat-empty-hero flex flex-1 flex-col items-center justify-center px-6 pb-10">
            <div className="chat-empty-hero-stack relative z-10 w-full max-w-4xl">
              <div className="chat-motion-fade-up chat-empty-hero-heading">
                <KivioBlob size={56} mood="idle" />
                <h2 className="chat-empty-hero-title">
                  <TypewriterText
                    key={`${lang}:${currentConversationId}:${currentAssistantName}:${selectedProjectName}:${selectedSetName}`}
                    text={emptyHeroLine({
                      lang,
                      assistantName: currentAssistantName,
                      projectName: selectedProjectName,
                      setName: selectedSetName,
                      seed: currentConversationId,
                    })}
                    active={showEmptyHero}
                  />
                </h2>
              </div>
              <div className="chat-motion-fade-up" style={{ ['--chat-motion-delay' as string]: '120ms' }}>
                <InputBar {...inputBarProps} layout="inline" />
              </div>
            </div>
          </div>
        ) : (
          <>
            {hookWarning && hookWarning.conversationId === currentConversationId && (
              <div className="flex items-start gap-2 px-4 pt-2">
                <div className="flex min-w-0 flex-1 items-start gap-2 rounded-md bg-amber-50 px-2.5 py-1.5 text-[11px] leading-4 text-amber-700 dark:bg-amber-400/10 dark:text-amber-200">
                  <span className="min-w-0 flex-1">
                    {i18n[lang].chatHookFailed
                      .replace('{name}', hookWarning.hookName || hookWarning.event)
                      .replace('{event}', hookWarning.event)}
                    {` — ${hookWarning.message}`}
                  </span>
                  <IconButton
                    size="xs"
                    variant="ghost"
                    label={i18n[lang].chatHookDismiss}
                    onClick={onDismissHookWarning}
                  >
                    <X size={12} />
                  </IconButton>
                </div>
              </div>
            )}

            {forkOrigin && (
              <div className="flex justify-center px-4 pt-2">
                <button
                  type="button"
                  onClick={() => onSelectConversation(forkOrigin.sourceId)}
                  className="inline-flex max-w-full items-center gap-1 rounded-full bg-neutral-100 px-2.5 py-1 text-[11px] text-neutral-500 transition-colors hover:bg-neutral-200 hover:text-neutral-700 dark:bg-neutral-800 dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-neutral-200"
                  title={`分叉自「${forkOrigin.title}」，点击回到源对话`}
                >
                  <GitBranch size={12} strokeWidth={2} className="shrink-0" />
                  <span className="truncate">分叉自 {forkOrigin.title}</span>
                </button>
              </div>
            )}

            {importedHistoryStale && (
              <div className="flex justify-center px-4 pt-2">
                <span
                  className="inline-flex max-w-full items-center gap-1 rounded-full bg-amber-100 px-2.5 py-1 text-[11px] text-amber-700 dark:bg-amber-500/15 dark:text-amber-400"
                  title="这条会话在 CLI 那边继续聊过。Kivio 里的历史是导入时的快照，不会自动同步；续聊时 CLI 用的仍是它自己那份完整上下文。"
                >
                  <span className="truncate">这条会话在 CLI 那边有新内容，此处显示的历史不完整</span>
                </span>
              </div>
            )}

            <Suspense fallback={<MessageListLoading />}>
              <Profiler id="MessageList" onRender={onRender}>
                <MessageList key={messageListProps.conversationId ?? 'empty'} {...messageListProps} />
              </Profiler>
            </Suspense>

            {pendingSlot}

            {queuedMessages.length > 0 && (
              <div className="shrink-0 px-6">
                <div className="mx-auto w-full max-w-4xl">
                  <QueuedMessages
                    messages={queuedMessages}
                    canSteer={canSteerQueuedMessages}
                    onSteer={onSteerQueuedMessage}
                    onRemove={onRemoveQueuedMessage}
                    onRestore={onRestoreQueuedMessage}
                    lang={lang}
                  />
                </div>
              </div>
            )}

            <InputBar {...inputBarProps} />
          </>
        )}
        {conversationLoading && (
          <ConversationLoadingState showAnimation={transition.showLoading} />
        )}
      </div>

      {imageViewerItem && (
        <div className="absolute inset-0 z-40 flex flex-col">
          <ChatImageViewer item={imageViewerItem} onClose={onCloseImageViewer} />
        </div>
      )}
    </div>
  )
})
