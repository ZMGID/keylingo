import { useSyncExternalStore } from 'react'

export interface ConversationTransitionSnapshot {
  requestId: number
  targetConversationId: string | null
  loading: boolean
}

let requestSequence = 0
let snapshot: ConversationTransitionSnapshot = {
  requestId: 0,
  targetConversationId: null,
  loading: false,
}

const listeners = new Set<() => void>()

function emit() {
  for (const listener of listeners) listener()
}

export function beginConversationTransition(conversationId: string): number {
  const requestId = ++requestSequence
  snapshot = { requestId, targetConversationId: conversationId, loading: true }
  emit()
  return requestId
}

export function completeConversationTransition(conversationId: string, requestId: number) {
  if (
    snapshot.requestId !== requestId
    || snapshot.targetConversationId !== conversationId
    || !snapshot.loading
  ) return
  snapshot = { requestId, targetConversationId: conversationId, loading: false }
  emit()
}

export function cancelConversationTransition(requestId: number) {
  if (snapshot.requestId !== requestId) return
  snapshot = { requestId, targetConversationId: null, loading: false }
  emit()
}

export function invalidateConversationTransition() {
  const requestId = ++requestSequence
  snapshot = { requestId, targetConversationId: null, loading: false }
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
