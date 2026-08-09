import { useSyncExternalStore } from 'react'

export interface ConversationTransitionSnapshot {
  requestId: number
  targetConversationId: string | null
  loading: boolean
  showLoading: boolean
}

export interface ConversationLoadHint {
  /** 侧栏索引里的消息数量，仅用于决定是否立即显示加载态，不限制实际加载内容。 */
  messageCount?: number
}

let requestSequence = 0
let snapshot: ConversationTransitionSnapshot = {
  requestId: 0,
  targetConversationId: null,
  loading: false,
  showLoading: false,
}

const listeners = new Set<() => void>()

function emit() {
  for (const listener of listeners) listener()
}

export function beginConversationTransition(
  conversationId: string,
  hint?: ConversationLoadHint,
): number {
  const requestId = ++requestSequence
  // 小会话直接切换，不铺 Logo；这个阈值只影响加载反馈，不影响消息是否完整加载。
  const showLoading = hint?.messageCount == null || hint.messageCount > 12
  snapshot = { requestId, targetConversationId: conversationId, loading: true, showLoading }
  emit()
  return requestId
}

export function completeConversationTransition(conversationId: string, requestId: number) {
  if (
    snapshot.requestId !== requestId
    || snapshot.targetConversationId !== conversationId
    || !snapshot.loading
  ) return
  snapshot = { requestId, targetConversationId: conversationId, loading: false, showLoading: false }
  emit()
}

export function cancelConversationTransition(requestId: number) {
  if (snapshot.requestId !== requestId) return
  snapshot = { requestId, targetConversationId: null, loading: false, showLoading: false }
  emit()
}

export function invalidateConversationTransition() {
  const requestId = ++requestSequence
  snapshot = { requestId, targetConversationId: null, loading: false, showLoading: false }
  emit()
}

export function isCurrentConversationTransition(requestId: number, conversationId: string): boolean {
  return snapshot.requestId === requestId && snapshot.targetConversationId === conversationId
}

export function getConversationTransitionSnapshot(): ConversationTransitionSnapshot {
  return snapshot
}

export function useConversationTransition(): ConversationTransitionSnapshot {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    getConversationTransitionSnapshot,
    getConversationTransitionSnapshot,
  )
}
