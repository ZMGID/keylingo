import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { getSettingsCached, subscribeSettings } from '../../api/settingsCache'
import { configureChatProtocolFilter } from '../../api/chatProtocol'
import { ApprovalCard } from '../ApprovalCard'
import { AskUserBlock } from '../AskUserBlock'
import { InputBar } from '../InputBar'
import { ChatTitlebar } from '../ChatTitlebar'
import { usesNativeTitlebar } from '../platform'
import { LangContext, type Lang } from '../../settings/i18n'
import {
  isEnterPlanApproval,
  isPlanApproval,
  PLAN_APPROVAL_ACTIONS,
  toolApprovalTitle,
} from '../toolApproval'
import { getPopoutConversationId } from './popoutRoutes'
import { PopoutTitlebar } from './PopoutTitlebar'
import { usePopoutSession } from './usePopoutSession'

const popoutConversationId = getPopoutConversationId()
if (popoutConversationId) configureChatProtocolFilter(popoutConversationId)

const MessageList = lazy(() => import('../MessageList').then((module) => ({
  default: module.MessageList,
})))

function MessageListLoading() {
  return (
    <div className="chat-themed-surface flex flex-1 items-center justify-center">
      <div className="h-5 w-5 animate-spin rounded-full border-2 border-neutral-300 border-t-neutral-800 dark:border-neutral-700 dark:border-t-neutral-200" />
    </div>
  )
}

type ChatPopoutProps = {
  onContentReady?: () => void
}

function PopoutPendingSlot({
  session,
}: {
  session: ReturnType<typeof usePopoutSession>
}): ReactNode {
  const {
    pendingToolConfirm,
    pendingSessionConsent,
    pendingUserPrompt,
    pendingUserPromptRecord,
    toolConfirmError,
    sessionConsentError,
    toolConfirmSubmitting,
    sessionConsentSubmitting,
    resolveToolConfirm,
    resolveSessionConsent,
    dismissUserPrompt,
  } = session
  if (!pendingToolConfirm && !pendingSessionConsent && !pendingUserPrompt) return null
  return (
    <div className="shrink-0 px-4">
      <div className="mx-auto w-full max-w-4xl">
        {pendingUserPrompt && pendingUserPromptRecord && (
          <AskUserBlock
            variant="docked"
            toolCall={pendingUserPromptRecord}
            onResolved={dismissUserPrompt}
          />
        )}
        {pendingToolConfirm && (
          <ApprovalCard
            title={toolApprovalTitle(pendingToolConfirm)}
            subtitle={`${pendingToolConfirm.source}${pendingToolConfirm.serverId ? ` · ${pendingToolConfirm.serverId}` : ''}`}
            detail={pendingToolConfirm.argumentsPreview}
            error={toolConfirmError}
            actions={isPlanApproval(pendingToolConfirm)
              ? [
                { label: '拒绝 / 让它改', disabled: toolConfirmSubmitting, onSelect: () => { void resolveToolConfirm(false) } },
                ...PLAN_APPROVAL_ACTIONS.map((action, index) => ({
                  label: action.label,
                  primary: index === PLAN_APPROVAL_ACTIONS.length - 1,
                  disabled: toolConfirmSubmitting,
                  onSelect: () => { void resolveToolConfirm(true, false, action.mode) },
                })),
              ]
              : isEnterPlanApproval(pendingToolConfirm)
                ? [
                  { label: '不用，直接做', disabled: toolConfirmSubmitting, onSelect: () => { void resolveToolConfirm(false) } },
                  { label: '进入计划模式', primary: true, disabled: toolConfirmSubmitting, onSelect: () => { void resolveToolConfirm(true) } },
                ]
                : [
                  { label: '拒绝', disabled: toolConfirmSubmitting, onSelect: () => { void resolveToolConfirm(false) } },
                  { label: '总是允许', disabled: toolConfirmSubmitting, onSelect: () => { void resolveToolConfirm(true, true) } },
                  { label: '允许一次', primary: true, disabled: toolConfirmSubmitting, onSelect: () => { void resolveToolConfirm(true) } },
                ]}
          />
        )}
        {pendingSessionConsent && (
          <ApprovalCard
            title="允许本次会话使用文件和命令工具？"
            error={sessionConsentError}
            actions={[
              { label: '拒绝', disabled: sessionConsentSubmitting, onSelect: () => { void resolveSessionConsent(false) } },
              { label: '允许本次会话', primary: true, disabled: sessionConsentSubmitting, onSelect: () => { void resolveSessionConsent(true) } },
            ]}
          />
        )}
      </div>
    </div>
  )
}

function ChatPopoutBody({
  conversationId,
  lang,
}: {
  conversationId: string
  lang: Lang
}) {
  const session = usePopoutSession(conversationId, lang)
  const titlebar = (
    <PopoutTitlebar
      conversation={session.conversation}
      runtime={session.runtime}
      usesExternalRuntime={session.usesExternalRuntime}
      approvalPolicy={session.approvalPolicy}
      onRuntimeChange={session.handleRuntimeChange}
      onModelChange={session.handleModelChange}
      onExternalModelChange={session.handleExternalModelChange}
      onThinkingLevelChange={session.handleThinkingLevelChange}
      onApprovalPolicyChange={session.handleApprovalPolicyChange}
    />
  )
  const pane = session.loadError ? (
    <div className="flex min-h-0 flex-1 items-center justify-center px-6 text-center text-[13px] text-neutral-500 dark:text-neutral-400">
      {session.loadError}
    </div>
  ) : (
    <>
      {session.streamError && (
        <div className="shrink-0 px-4 pt-2 text-center text-[12px] text-red-600 dark:text-red-400">
          {session.streamError}
        </div>
      )}
      <Suspense fallback={<MessageListLoading />}>
        <MessageList key={conversationId} {...session.messageListProps} />
      </Suspense>
      <PopoutPendingSlot session={session} />
      <InputBar {...session.inputBarProps} />
    </>
  )

  return (
    <div className={`chat-window-shell${usesNativeTitlebar ? ' chat-window-shell--native-titlebar' : ''}`}>
      {!usesNativeTitlebar && titlebar}
      <div className="flex min-h-0 w-full flex-1">
        <div className="chat-motion-pane-in chat-main-pane chat-main-pane--lone relative flex min-w-0 flex-1 flex-col">
          {usesNativeTitlebar && titlebar}
          {pane}
        </div>
      </div>
    </div>
  )
}

export default function ChatPopout({ onContentReady }: ChatPopoutProps) {
  const [lang, setLang] = useState<Lang>('zh')
  const readyRef = useRef(false)

  useLayoutEffect(() => {
    if (popoutConversationId) configureChatProtocolFilter(popoutConversationId)
  }, [])

  useLayoutEffect(() => {
    if (readyRef.current) return
    readyRef.current = true
    onContentReady?.()
  }, [onContentReady])

  useEffect(() => {
    void getSettingsCached().then((settings) => {
      setLang((settings.settingsLanguage as Lang) || 'zh')
    }).catch(() => {})
    return subscribeSettings((next) => {
      setLang((next.settingsLanguage as Lang) || 'zh')
    })
  }, [])

  if (!popoutConversationId) {
    return (
      <div className={`chat-window-shell${usesNativeTitlebar ? ' chat-window-shell--native-titlebar' : ''}`}>
        {!usesNativeTitlebar && (
          <ChatTitlebar
            hideNav
            sidebarExpanded={false}
            onToggleSidebar={() => {}}
            onNewConversation={() => {}}
          />
        )}
        <div className="flex min-h-0 w-full flex-1">
          <div className="chat-main-pane chat-main-pane--lone relative flex min-w-0 flex-1 flex-col">
            <div className="flex flex-1 items-center justify-center px-6 text-center text-[13px] text-neutral-500">
              无法打开独立对话窗口：缺少对话 id
            </div>
          </div>
        </div>
      </div>
    )
  }

  return (
    <LangContext.Provider value={lang}>
      <ChatPopoutBody conversationId={popoutConversationId} lang={lang} />
    </LangContext.Provider>
  )
}
