import { ChatTitlebar } from '../ChatTitlebar'
import { ModelSelector } from '../ModelSelector'
import { PermissionPicker } from '../PermissionPicker'
import { ExternalModelSelector, RuntimePicker } from '../RuntimePicker'
import { ThinkingLevelSelector } from '../ThinkingLevelSelector'
import { chatTitlebarMacInsetClass, chatTitlebarRowClass, usesNativeTitlebar } from '../platform'
import type { AgentRuntimeConfig, Conversation, ThinkingLevel } from '../types'

type PopoutTitlebarProps = {
  conversation: Conversation | null
  runtime: AgentRuntimeConfig
  usesExternalRuntime: boolean
  approvalPolicy: string
  onRuntimeChange: (runtime: AgentRuntimeConfig) => void | Promise<void>
  onModelChange: (providerId: string, model: string) => void | Promise<void>
  onExternalModelChange: (model: string, reasoning?: string | null) => void | Promise<void>
  onThinkingLevelChange: (level: ThinkingLevel | null) => void | Promise<void>
  onApprovalPolicyChange: (policy: string) => void | Promise<void>
}

function TitlebarPills({
  conversation,
  runtime,
  usesExternalRuntime,
  approvalPolicy,
  onRuntimeChange,
  onModelChange,
  onExternalModelChange,
  onThinkingLevelChange,
  onApprovalPolicyChange,
}: PopoutTitlebarProps) {
  const title = conversation?.title?.trim() || 'Kivio'
  const providerId = conversation?.provider_id ?? ''
  const model = conversation?.model ?? ''
  const locked = Boolean(conversation && (conversation.messages?.length ?? 0) > 0)

  return (
    <>
      <div className="flex min-w-0 items-center gap-1" data-popout-pills>
        <div className="shrink-0" data-tauri-drag-region="false">
          <RuntimePicker
            agentRuntime={runtime}
            onRuntimeChange={onRuntimeChange}
            conversationId={conversation?.id}
            locked={locked}
          />
        </div>
        <div className="min-w-0 max-w-full shrink" data-tauri-drag-region="false">
          {usesExternalRuntime ? (
            <ExternalModelSelector
              agentRuntime={runtime}
              onModelChange={onExternalModelChange}
              conversationId={conversation?.id}
            />
          ) : (
            <ModelSelector
              currentProviderId={providerId}
              currentModel={model}
              onModelChange={onModelChange}
            />
          )}
        </div>
        {!usesExternalRuntime && (
          <div className="chat-thinking-pill-wrap shrink-0" data-tauri-drag-region="false">
            <ThinkingLevelSelector
              currentProviderId={providerId}
              currentModel={model}
              value={conversation?.thinking_level ?? conversation?.thinkingLevel ?? null}
              onChange={onThinkingLevelChange}
            />
          </div>
        )}
        <div className="shrink-0" data-tauri-drag-region="false">
          <PermissionPicker
            agentRuntime={runtime}
            approvalPolicy={approvalPolicy}
            onApprovalPolicyChange={onApprovalPolicyChange}
          />
        </div>
      </div>
      <div className="min-w-4 flex-1 self-stretch" data-tauri-drag-region />
      <div
        data-popout-title
        className="min-w-0 max-w-[42%] shrink truncate px-2 text-right text-[13px] font-medium text-neutral-700 dark:text-neutral-200"
        title={title}
      >
        {title}
      </div>
    </>
  )
}

export function PopoutTitlebar(props: PopoutTitlebarProps) {
  if (!usesNativeTitlebar) {
    return (
      <ChatTitlebar
        sidebarExpanded={false}
        hideNav
        onToggleSidebar={() => {}}
        onNewConversation={() => {}}
      >
        <TitlebarPills {...props} />
      </ChatTitlebar>
    )
  }

  return (
    <header
      className={`chat-titlebar-row ${chatTitlebarRowClass} min-w-0 gap-2 ${chatTitlebarMacInsetClass} chat-titlebar-row--collapsed-mac pr-3`}
      data-tauri-drag-region
    >
      <TitlebarPills {...props} />
    </header>
  )
}

