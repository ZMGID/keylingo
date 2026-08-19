import { useCallback, useRef, useState } from 'react'
import { chatApi } from '../api'
import { userFollowUpId, userSteerId } from '../segments'
import type { Conversation, PendingAttachment } from '../types'

/** 排队中的一条消息。`id` 一路带到后端并回到插话卡上，用来对账出队。 */
export interface QueuedMessage {
  id: string
  content: string
  attachments: PendingAttachment[]
  /** 已提交「立刻引导」、等下一个轮次边界生效。仍在队列里（见下方自愈规则）。 */
  steering: boolean
  /** 正在提交原生 follow-up；确认前不能撤回或被普通 drain 抢走。 */
  followingUp?: boolean
  /** 对端拒绝 follow-up 后显示一次降级提示，消息仍由本地队列轮末发送。 */
  followUpRejected?: boolean
  /**
   * 上一次「立刻引导」没被受理（此刻没有在跑的轮次 / 该轮次不可注入）。
   * 仅用于在条目上说一句话 —— 消息本身照旧留在队列里等轮末发出。
   */
  steerRejected?: boolean
}

export function isQueuedSubmitted(message: QueuedMessage): boolean {
  return message.steering || Boolean(message.followingUp)
}

interface UseMessageQueueParams {
  /**
   * 发出一条排队消息；返回 false = 此刻发不出去（仍在生成 / 模型未配置），条目留在队首。
   * 与 `useExternalSendQueue` 同一约定。
   */
  onSendMessage: (
    content: string,
    attachments: PendingAttachment[],
    options: { conversationOverride: Conversation },
  ) => Promise<boolean>
  onRestoreToComposer: (message: QueuedMessage) => void
}

let steerSeq = 0
function nextQueuedId(): string {
  steerSeq += 1
  return `steer-${Date.now().toString(36)}-${steerSeq}`
}

/**
 * 运行中的消息队列（Codex 式）：生成期间发的消息先排队，本轮结束后逐条自动发出；
 * 也可以点「立刻引导」注入正在跑的这一轮。
 *
 * 只在内存里。`steer` / `followUp` 成功只标记已提交，**不出队**；出队等插话卡
 * （实时事件或落库对账）。收尾一律走 `settleAfterRun`。
 */
export function useMessageQueue({ onSendMessage, onRestoreToComposer }: UseMessageQueueParams) {
  const [queued, setQueued] = useState<Record<string, QueuedMessage[]>>({})
  const queuedRef = useRef(queued)
  /**
   * 已认领待发的条目 id。同步认领、发完才释放，挡住同一条被两个 drain 各发一遍。
   * 不能用「每会话一个 draining 标志」：drain 要 await 一整轮，那一轮结束时它自己
   * 又会调 drain —— 标志此刻仍然挂着，第二条就永远发不出去了。
   */
  const claimedRef = useRef<Set<string>>(new Set())
  const callbacksRef = useRef({ onSendMessage, onRestoreToComposer })
  callbacksRef.current = { onSendMessage, onRestoreToComposer }

  const patch = useCallback((
    conversationId: string,
    update: (items: QueuedMessage[]) => QueuedMessage[],
  ) => {
    const previous = queuedRef.current
    const nextItems = update(previous[conversationId] ?? [])
    let next: Record<string, QueuedMessage[]>
    if (nextItems.length === 0) {
      if (!(conversationId in previous)) return
      next = { ...previous }
      delete next[conversationId]
    } else {
      next = { ...previous, [conversationId]: nextItems }
    }
    queuedRef.current = next
    setQueued(next)
  }, [])

  const find = (conversationId: string, messageId: string) => (
    (queuedRef.current[conversationId] ?? []).find((item) => item.id === messageId)
  )

  const enqueue = useCallback((
    conversationId: string,
    content: string,
    attachments: PendingAttachment[],
  ): QueuedMessage | null => {
    const trimmed = content.trim()
    if (!trimmed && attachments.length === 0) return null
    const message: QueuedMessage = {
      id: nextQueuedId(),
      content: trimmed,
      attachments,
      steering: false,
    }
    patch(conversationId, (items) => [...items, message])
    return message
  }, [patch])

  const remove = useCallback((conversationId: string, messageId: string) => {
    const message = find(conversationId, messageId)
    if (!message || isQueuedSubmitted(message)) return
    patch(conversationId, (items) => items.filter((item) => item.id !== messageId))
  }, [patch])

  const restoreToComposer = useCallback((conversationId: string, messageId: string) => {
    const message = find(conversationId, messageId)
    if (!message || isQueuedSubmitted(message)) return
    patch(conversationId, (items) => items.filter((item) => item.id !== messageId))
    callbacksRef.current.onRestoreToComposer(message)
  }, [patch])

  const clearConversation = useCallback((conversationId: string) => {
    patch(conversationId, () => [])
  }, [patch])

  /**
   * 一轮结束后发出队首一条。已提交的条目留给 `settleAfterRun` 先对账/解开，
   * 运行中不能把 CLI 已接住的那条再发一遍。
   */
  const drain = useCallback(async (conversation: Conversation) => {
    const conversationId = conversation.id
    const next = (queuedRef.current[conversationId] ?? [])
      .find((item) => !isQueuedSubmitted(item) && !claimedRef.current.has(item.id))
    if (!next) return
    claimedRef.current.add(next.id)
    patch(conversationId, (items) => items.filter((item) => item.id !== next.id))
    let accepted = false
    try {
      accepted = await callbacksRef.current.onSendMessage(
        next.content,
        next.attachments,
        { conversationOverride: conversation },
      )
    } catch (err) {
      console.error('Failed to send a queued message:', err)
    } finally {
      claimedRef.current.delete(next.id)
    }
    if (!accepted) {
      patch(conversationId, (items) => [next, ...items])
    }
  }, [patch])

  const steer = useCallback(async (
    conversationId: string,
    messageId: string,
  ): Promise<boolean> => {
    const message = find(conversationId, messageId)
    if (!message || isQueuedSubmitted(message)) return false
    patch(conversationId, (items) => items.map((item) => (
      item.id === messageId ? { ...item, steerRejected: false } : item
    )))
    let accepted = false
    try {
      const hasUnsupportedAttachments = message.attachments.some(
        (attachment) => attachment.content === undefined,
      )
      if (!hasUnsupportedAttachments) {
        accepted = await chatApi.steerMessage(
          conversationId,
          message.id,
          message.content,
          message.attachments,
        )
      }
    } catch (err) {
      console.error('Failed to steer the running turn:', err)
    }
    patch(conversationId, (items) => items.map((item) => (
      item.id === messageId
        ? { ...item, steering: accepted, steerRejected: !accepted }
        : item
    )))
    return accepted
  }, [patch])

  const followUp = useCallback(async (
    conversation: Conversation,
    messageId: string,
  ): Promise<boolean> => {
    const conversationId = conversation.id
    const message = find(conversationId, messageId)
    if (!message || isQueuedSubmitted(message)) return false
    patch(conversationId, (items) => items.map((item) => (
      item.id === messageId
        ? { ...item, followingUp: true, followUpRejected: false }
        : item
    )))
    let accepted = false
    try {
      accepted = await chatApi.followUpMessage(
        conversationId,
        message.id,
        message.content,
        message.attachments,
      )
    } catch (err) {
      console.error('Failed to queue a follow-up:', err)
    }
    if (accepted) return true
    patch(conversationId, (items) => items.map((item) => (
      item.id === messageId
        ? { ...item, followingUp: false, followUpRejected: true }
        : item
    )))
    await drain(conversation)
    return false
  }, [drain, patch])

  /** 插话卡到了才出队。找不到则 no-op（卡可能比 ack 先到）。 */
  const confirm = useCallback((conversationId: string, injectionId: string) => {
    patch(conversationId, (items) => items.filter((item) => item.id !== injectionId))
  }, [patch])

  const releaseSubmitted = useCallback((conversationId: string) => {
    patch(conversationId, (items) => {
      if (!items.some(isQueuedSubmitted)) return items
      return items.map((item) => (
        isQueuedSubmitted(item)
          ? { ...item, followingUp: false, steering: false }
          : item
      ))
    })
  }, [patch])

  /**
   * run 收尾的唯一入口。有落库对话就先按插话卡对账，剩下的解开转圈；
   * 再 drain 一条。硬失败只传 conversationId，只解开、不自动发。
   */
  const settleAfterRun = useCallback((
    conversationId: string,
    conversation?: Conversation | null,
  ) => {
    if (conversation) {
      for (const message of conversation.messages ?? []) {
        for (const record of message.tool_calls ?? message.toolCalls ?? []) {
          const injectionId = userSteerId(record) ?? userFollowUpId(record)
          if (injectionId) confirm(conversation.id, injectionId)
        }
      }
      releaseSubmitted(conversation.id)
      return drain(conversation)
    }
    releaseSubmitted(conversationId)
  }, [confirm, drain, releaseSubmitted])

  return {
    queued,
    enqueue,
    remove,
    restoreToComposer,
    clearConversation,
    drain,
    steer,
    followUp,
    confirm,
    settleAfterRun,
  }
}
