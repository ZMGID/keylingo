import type { ChatMessage, ModelRef } from './types'

// 多模型一问多答（任务 06-30）：把消息线性数组折叠成「单条消息 / 多答组」两类项。
// 同一 group_id 的连续 assistant 消息聚成一组（横向并排多列渲染）；其余保持线性。
// 纯函数，便于单测（grouping 边界 / 单模型零回归）。

export const MAX_REPLY_MODELS = 4

export type MessageListItem =
  | { type: 'message'; message: ChatMessage }
  | { type: 'group'; groupId: string; messages: ChatMessage[] }

function messageGroupId(message: ChatMessage): string | null {
  return message.group_id ?? message.groupId ?? null
}

export function foldMessageGroups(messages: ChatMessage[]): MessageListItem[] {
  const items: MessageListItem[] = []
  for (let i = 0; i < messages.length; i++) {
    const message = messages[i]
    const groupId = messageGroupId(message)
    if (message.role === 'assistant' && groupId) {
      const groupMessages: ChatMessage[] = [message]
      let j = i + 1
      while (j < messages.length) {
        const next = messages[j]
        if (next.role === 'assistant' && messageGroupId(next) === groupId) {
          groupMessages.push(next)
          j++
        } else {
          break
        }
      }
      items.push({ type: 'group', groupId, messages: groupMessages })
      i = j - 1
      continue
    }
    items.push({ type: 'message', message })
  }
  return items
}

function sameReplyGroup(target: ChatMessage, other: ChatMessage): boolean {
  const targetGroup = messageGroupId(target)
  const otherGroup = messageGroupId(other)
  if (targetGroup && otherGroup) return targetGroup === otherGroup
  return !targetGroup && !otherGroup
}

export type AssistantTurnSpan = {
  userIndex: number
  start: number
  end: number
  groupId: string | null
  siblings: ChatMessage[]
}

/** Inclusive sibling span of the assistant turn that contains `messageId`. */
export function assistantTurnSpan(
  messages: ChatMessage[],
  messageId: string,
): AssistantTurnSpan | null {
  const targetIdx = messages.findIndex((message) => message.id === messageId)
  if (targetIdx < 0 || messages[targetIdx].role !== 'assistant') return null
  let userIndex = -1
  for (let i = targetIdx - 1; i >= 0; i--) {
    if (messages[i].role === 'user') {
      userIndex = i
      break
    }
  }
  if (userIndex < 0) return null
  const target = messages[targetIdx]
  let start = targetIdx
  while (start > userIndex + 1) {
    const previous = messages[start - 1]
    if (previous.role !== 'assistant' || !sameReplyGroup(target, previous)) break
    start -= 1
  }
  let end = targetIdx
  while (end + 1 < messages.length) {
    const next = messages[end + 1]
    if (next.role !== 'assistant' || !sameReplyGroup(target, next)) break
    end += 1
  }
  return {
    userIndex,
    start,
    end,
    groupId: messageGroupId(target),
    siblings: messages.slice(start, end + 1),
  }
}

export function isLastAssistantTurn(messages: ChatMessage[], messageId: string): boolean {
  const span = assistantTurnSpan(messages, messageId)
  return Boolean(span && span.end === messages.length - 1)
}

function armKey(
  message: ChatMessage,
  fallbackProvider: string,
  fallbackModel: string,
): ModelRef {
  const provider = (message.provider_id ?? message.providerId ?? '').trim() || fallbackProvider
  const model = (message.model ?? '').trim() || fallbackModel
  return { provider_id: provider, model }
}

export function occupiedReplyModels(
  messages: ChatMessage[],
  messageId: string,
  fallbackProvider: string,
  fallbackModel: string,
): ModelRef[] {
  const span = assistantTurnSpan(messages, messageId)
  if (!span) return []
  const seen = new Set<string>()
  const occupied: ModelRef[] = []
  for (const sibling of span.siblings) {
    const ref = armKey(sibling, fallbackProvider, fallbackModel)
    if (!ref.provider_id || !ref.model) continue
    const key = `${ref.provider_id}\0${ref.model}`
    if (seen.has(key)) continue
    seen.add(key)
    occupied.push(ref)
  }
  return occupied
}
