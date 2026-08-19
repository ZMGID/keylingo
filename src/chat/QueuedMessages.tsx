import { Clock3, CornerDownRight, Loader2, X } from 'lucide-react'
import { IconButton } from '../components/Button'
import { isQueuedSubmitted, type QueuedMessage } from './hooks/useMessageQueue'
import type { Lang } from '../settings/i18n'
import { i18n } from '../settings/i18n'

interface QueuedMessagesProps {
  messages: QueuedMessage[]
  /** 是否给「立刻引导」入口。协议不支持注入时为 false（claude / ACP，见 Chat.tsx 的判据）。 */
  canSteer: boolean
  onSteer: (messageId: string) => void
  onRemove: (messageId: string) => void
  /** 点文本本体 = 撤回到输入框继续改。 */
  onRestore: (messageId: string) => void
  lang: Lang
}

/**
 * 排队中的消息，挂在输入框正上方。对齐 Codex CLI：**每条**都看得见、能单独删、能单独
 * 「立刻引导」——而不是 Claude Code 那种看不见也撤不了的隐式队列。
 *
 * 视觉上刻意**压到最轻**（soft 底、无描边、faint 字，与 `.chat-composer-status` 同一家族）：
 * 此刻屏幕上正在生成回答，队列是「待办」而非「待决定」，用审批卡那种描边白卡会抢走注意力。
 * 动作键平时隐形、悬停/聚焦才现（见 `.chat-composer-queue-actions`）。
 */
export function QueuedMessages({
  messages,
  canSteer,
  onSteer,
  onRemove,
  onRestore,
  lang,
}: QueuedMessagesProps) {
  if (messages.length === 0) return null
  const t = i18n[lang]

  return (
    <div className="not-prose chat-composer-queue w-full">
      {/* 排太多条不许把输入框顶上去：约 4 行后内部滚动（custom-scrollbar = 全站同一根细条）。 */}
      <ul className="custom-scrollbar max-h-[7.5rem] overflow-y-auto p-1">
        {messages.map((message) => {
          // 撤回只把**文字**还给输入框（走 composer 的文本信道），所以带附件的那条不给撤 ——
          // 否则附件会静默消失。它仍能删、仍能立刻引导。
          const submitted = isQueuedSubmitted(message)
          const restorable = !submitted && message.attachments.length === 0
          return (
            <li
              key={message.id}
              className="chat-composer-queue-row flex h-7 items-center gap-1.5 rounded-md pl-1.5 pr-0.5"
            >
              {/* 行首这一格是**状态**：等着 → 已提交。宽度固定，切换时不推着文字横跳。
                  刻意不用 CornerDownRight —— 那个图标已经是右边「立刻引导」的意思，
                  同一个图标担两个意思会让人以为整行都是引导键。 */}
              {submitted ? (
                <Loader2
                  size={12}
                  className="shrink-0 animate-spin text-neutral-400 dark:text-neutral-500"
                />
              ) : (
                <Clock3
                  size={12}
                  strokeWidth={1.9}
                  className="shrink-0 text-neutral-400 dark:text-neutral-500"
                />
              )}
              <button
                type="button"
                onClick={() => restorable && onRestore(message.id)}
                title={
                  submitted
                    ? message.followingUp ? t.chatFollowUpPending : t.chatSteerPending
                    : restorable ? t.chatQueueRestore : t.chatQueueRestoreBlocked
                }
                className={`min-w-0 flex-1 truncate text-left text-[12px] ${
                  submitted
                    ? 'cursor-default text-neutral-400 dark:text-neutral-500'
                    : `text-neutral-700 dark:text-neutral-200 ${restorable ? '' : 'cursor-default'}`
                }`}
              >
                {message.content || t.chatQueuedAttachmentOnly}
                {message.attachments.length > 0 && (
                  <span className="ml-1.5 text-[11px] text-neutral-400 dark:text-neutral-500">
                    +{message.attachments.length}
                  </span>
                )}
              </button>
              {/* 引导被拒：这条得留在原地说话（不能只做 tooltip），否则用户点了按钮什么都看不见。 */}
              {message.steerRejected && !submitted && (
                <span className="shrink-0 pr-1 text-[11px] text-amber-600 dark:text-amber-300">
                  {t.chatSteerRejected}
                </span>
              )}
              {message.followUpRejected && !submitted && (
                <span className="shrink-0 pr-1 text-[11px] text-amber-600 dark:text-amber-300">
                  {t.chatFollowUpRejected}
                </span>
              )}
              {!submitted && (
                <span className="chat-composer-queue-actions flex shrink-0 items-center gap-0.5">
                  {canSteer && (
                    <IconButton
                      size="xs"
                      variant="ghost"
                      label={t.chatSteerNow}
                      onClick={() => onSteer(message.id)}
                    >
                      <CornerDownRight size={12} />
                    </IconButton>
                  )}
                  <IconButton
                    size="xs"
                    variant="ghost"
                    label={t.chatQueueRemove}
                    onClick={() => onRemove(message.id)}
                  >
                    <X size={12} />
                  </IconButton>
                </span>
              )}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
