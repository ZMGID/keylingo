import { useCallback, useRef, useState } from 'react'
import { chatApi } from '../api'
import type { Conversation, PendingAttachment } from '../types'

/** 排队中的一条消息。`steerId` 一路带到后端并回到 `user_steer` 卡上，用来对账出队。 */
export interface QueuedMessage {
  /** 也是投给后端的 steer id（同一条消息不管走哪条路，标识都是它）。 */
  id: string
  content: string
  attachments: PendingAttachment[]
  /** 已提交「立刻引导」、等下一个轮次边界生效。仍在队列里（见下方自愈规则）。 */
  steering: boolean
  /**
   * 上一次「立刻引导」没被受理（此刻没有在跑的轮次 / 该轮次不可注入，如 codex 的
   * review、compact 轮）。仅用于在条目上说一句话 —— 消息本身照旧留在队列里等轮末发出。
   * 再点一次会清掉这个标记重试。
   */
  steerRejected?: boolean
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
  /** 撤回一条到输入框（把文字和附件还给 composer）。 */
  onRestoreToComposer: (message: QueuedMessage) => void
}

let steerSeq = 0
function nextQueuedId(): string {
  steerSeq += 1
  return `steer-${Date.now().toString(36)}-${steerSeq}`
}

/**
 * 运行中的消息队列（Codex 式）：生成期间发的消息先排队，本轮结束后**逐条**自动发出；
 * 也可以点「立刻引导」把某一条注入正在跑的这一轮（后端在下一个轮次边界注入，不打断）。
 *
 * 只在内存里，与 `composerDraft.ts` 同一取舍：队列描述的是「这个窗口正在等的这一轮」，
 * 关窗即清（那时候 run 也已经没了，附件的 temp 路径也可能被 GC 掉）。
 *
 * **自愈规则（这块的关键）**：`steer()` 成功只把条目标成 `steering`，**不出队**。真正出队要等
 * `confirmSteered(steerId)` —— 由那张 `user_steer` 卡的实时事件驱动。若模型此刻已经在写终答、
 * 后面再没有轮次边界，这条永远收不到卡片，于是它仍在队列里，被运行结束后的 `drain` 当普通消息
 * 发出去。所以「引导没赶上」最坏退化成「下一轮再问」，绝不静默丢消息。
 */
export function useMessageQueue({ onSendMessage, onRestoreToComposer }: UseMessageQueueParams) {
  const [queued, setQueued] = useState<Record<string, QueuedMessage[]>>({})
  /**
   * 已认领待发的条目 id。**同步**认领、发完才释放，用来挡住「同一条被两个 drain 各发一遍」
   * （同会话并发 run、或 React 还没重渲时两次触发读到同一个队首）。
   *
   * 不能用「每会话一个 draining 标志」：drain 要 await 完整的一轮，而那一轮结束时它自己的
   * finally 又会调 drain —— 标志此刻仍然挂着，第二条就永远发不出去了。
   */
  const claimedRef = useRef<Set<string>>(new Set())

  const callbacksRef = useRef({ onSendMessage, onRestoreToComposer })
  callbacksRef.current = { onSendMessage, onRestoreToComposer }

  /** 读最新队列而不把 queued 列进 useCallback 依赖（drain 要保持稳定身份）。 */
  const queuedRef = useRef(queued)
  queuedRef.current = queued

  const patch = useCallback((
    conversationId: string,
    update: (items: QueuedMessage[]) => QueuedMessage[],
  ) => {
    setQueued((previous) => {
      const next = update(previous[conversationId] ?? [])
      if (next.length === 0) {
        if (!(conversationId in previous)) return previous
        const rest = { ...previous }
        delete rest[conversationId]
        return rest
      }
      return { ...previous, [conversationId]: next }
    })
  }, [])

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
    patch(conversationId, (items) => items.filter((item) => item.id !== messageId))
  }, [patch])

  /** 撤回到输入框：出队 + 把内容交还 composer。已提交引导的那条不给撤（后端可能正在注入）。 */
  const restoreToComposer = useCallback((conversationId: string, messageId: string) => {
    const message = (queuedRef.current[conversationId] ?? []).find((item) => item.id === messageId)
    if (!message || message.steering) return
    patch(conversationId, (items) => items.filter((item) => item.id !== messageId))
    callbacksRef.current.onRestoreToComposer(message)
  }, [patch])

  /** 会话被删除时连带清掉它的队列。 */
  const clearConversation = useCallback((conversationId: string) => {
    patch(conversationId, () => [])
  }, [patch])

  /**
   * 一轮结束后发出队首那一条。**只发一条** —— 它自己会再跑一轮，跑完再由那一轮的结束触发下一次
   * drain。这样每条排队消息各得一轮，不会被合并成一条（Claude Code 那个被吐槽的行为）。
   *
   * 先出队再发（发送被拒 / 抛错时原位放回队首）：发送要 await 一整轮，期间队首必须已经不是它，
   * 否则被这一轮内部触发的下一次 drain 会把同一条再发一遍。
   */
  const drain = useCallback(async (conversation: Conversation) => {
    const conversationId = conversation.id
    const next = (queuedRef.current[conversationId] ?? [])
      .find((item) => !claimedRef.current.has(item.id))
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
    // 发不出去（仍在生成 / 模型未配置 / 抛错）就放回队首，别把用户打的字吞掉。
    if (!accepted) {
      patch(conversationId, (items) => [next, ...items])
    }
  }, [patch])

  /** 点「立刻引导」。返回 false = 没投进信箱（此刻没有活跃 run），条目留在队列里等自动发送。 */
  const steer = useCallback(async (
    conversationId: string,
    messageId: string,
  ): Promise<boolean> => {
    const message = (queuedRef.current[conversationId] ?? []).find((item) => item.id === messageId)
    if (!message || message.steering) return false
    // 重试时先清掉上次的拒绝标记，避免「点了但界面还写着没成功」。
    patch(conversationId, (items) => items.map((item) => (
      item.id === messageId ? { ...item, steerRejected: false } : item
    )))
    let accepted = false
    try {
      // Steering 的通用协议只有文本信道。虚拟文本附件可由后端复用正常发送时的
      // 内联合成逻辑；磁盘/图片附件不能静默丢掉，留在队列等本轮结束后正常发送。
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

  /** 收到 `user_steer` 卡 = 这条真的进了模型历史，现在才出队。找不到则 no-op（幂等）。 */
  const confirmSteered = useCallback((conversationId: string, steerId: string) => {
    patch(conversationId, (items) => items.filter((item) => item.id !== steerId))
  }, [patch])

  return {
    queued,
    enqueue,
    remove,
    restoreToComposer,
    clearConversation,
    drain,
    steer,
    confirmSteered,
  }
}
