/**
 * Live-row key model (LiveAgent-style transcript keys).
 *
 * Assign a stable virtualizer row key at run start (`live-turn-N` /
 * `live-group-<id>`). On settle, alias the committed twin onto that key so the
 * first history mount reuses the live estimate/cache identity.
 *
 * Live rides outside the virtualizer; key continuity stabilizes the twin's
 * first history mount, not DOM reuse. Pure and DOM-free — MessageList feeds
 * run/history facts each render.
 */

export type LiveRowSyncInput = {
  conversationId: string | null | undefined
  liveActive: boolean
  /** Multi-model group id while that group is the live tail; null for single-stream. */
  liveGroupId: string | null
  /**
   * Preferred twin message id from the stream snapshot. When the twin is
   * already in history (or lands this frame), it is aliased onto the live key.
   */
  preferredTwinId: string | null
  /**
   * Assistant message ids currently present in the committed history, oldest
   * → newest. Used to find a twin when preferredTwinId is missing (persist lag).
   */
  historyAssistantIds: readonly string[]
  /**
   * Group ids currently present as folded group rows. Used to alias a settled
   * multi-model group onto its live key.
   */
  historyGroupIds: readonly string[]
}

export type LiveRowSyncResult = {
  /** Stable key for the live tail while a run is active; null when idle. */
  liveKey: string | null
}

export type LiveRowModel = {
  /** Advance turn state from the latest run/history facts. Call once per render. */
  sync: (input: LiveRowSyncInput) => LiveRowSyncResult
  /** Stable resolvers — same function identity for the life of the model. */
  resolveMessageKey: (messageId: string) => string
  resolveGroupKey: (groupId: string) => string
  reset: () => void
}

type Turn = {
  liveKey: string
  conversationId: string | null
  /** historyAssistantIds.length when the turn started — twins are only after this. */
  assistantCountAtStart: number
  groupId: string | null
}

function groupLiveKey(groupId: string): string {
  return `live-group-${groupId}`
}

export function createLiveRowModel(): LiveRowModel {
  let turnSeq = 0
  let activeTurn: Turn | null = null
  let pendingSettle: Turn | null = null
  /** Last conversation the model has seen; switch clears aliases even while idle. */
  let boundConversationId: string | null | undefined = undefined
  /** committed message id → live row key */
  let messageOrigins = new Map<string, string>()
  /** committed group id → live row key */
  let groupOrigins = new Map<string, string>()

  const reset = () => {
    turnSeq = 0
    activeTurn = null
    pendingSettle = null
    messageOrigins = new Map()
    groupOrigins = new Map()
    // boundConversationId is updated by sync after reset — leave it alone here
    // so a reset()+sync(same conv) path still works.
  }

  const adoptMessageTwin = (
    turn: Turn,
    preferredTwinId: string | null,
    historyAssistantIds: readonly string[],
  ): boolean => {
    if (turn.groupId) return false
    const candidates: string[] = []
    if (preferredTwinId) candidates.push(preferredTwinId)
    for (let i = historyAssistantIds.length - 1; i >= turn.assistantCountAtStart; i -= 1) {
      const id = historyAssistantIds[i]
      if (id && !candidates.includes(id)) candidates.push(id)
    }
    const twinId = candidates[0]
    if (!twinId) return false
    messageOrigins.set(twinId, turn.liveKey)
    return true
  }

  const adoptGroupTwin = (
    turn: Turn,
    historyGroupIds: readonly string[],
  ): boolean => {
    if (!turn.groupId) return false
    if (!historyGroupIds.includes(turn.groupId)) return false
    groupOrigins.set(turn.groupId, turn.liveKey)
    return true
  }

  const adoptTwin = (
    turn: Turn,
    input: Pick<LiveRowSyncInput, 'preferredTwinId' | 'historyAssistantIds' | 'historyGroupIds'>,
  ): boolean => {
    if (turn.groupId) {
      return adoptGroupTwin(turn, input.historyGroupIds)
    }
    return adoptMessageTwin(turn, input.preferredTwinId, input.historyAssistantIds)
  }

  const sync = (input: LiveRowSyncInput): LiveRowSyncResult => {
    const conversationId = input.conversationId ?? null

    // Conversation switch: drop all aliases so keys never cross chats —
    // including the idle case where activeTurn/pendingSettle are already null.
    if (boundConversationId !== conversationId) {
      reset()
      boundConversationId = conversationId
    }

    if (input.liveActive) {
      const nextGroupId = input.liveGroupId
      const needsNewTurn = !activeTurn
        || activeTurn.conversationId !== conversationId
        || activeTurn.groupId !== nextGroupId

      if (needsNewTurn) {
        // A pending settle without a twin is superseded by the new run.
        pendingSettle = null
        if (nextGroupId) {
          activeTurn = {
            liveKey: groupLiveKey(nextGroupId),
            conversationId,
            assistantCountAtStart: input.historyAssistantIds.length,
            groupId: nextGroupId,
          }
        } else {
          activeTurn = {
            liveKey: `live-turn-${++turnSeq}`,
            conversationId,
            assistantCountAtStart: input.historyAssistantIds.length,
            groupId: null,
          }
        }
      }

      // Twin aliasing only on settle (or pendingSettle lag). During stream Kivio
      // filters the active messageId out of history, so aliasing mid-run would
      // either no-op or risk a double row if the filter ever missed.
    } else if (activeTurn) {
      if (!adoptTwin(activeTurn, input)) {
        pendingSettle = activeTurn
      }
      activeTurn = null
    } else if (pendingSettle) {
      if (adoptTwin(pendingSettle, input)) {
        pendingSettle = null
      }
    }

    const liveKey = input.liveActive && activeTurn ? activeTurn.liveKey : null
    return { liveKey }
  }

  const resolveMessageKey = (messageId: string) => messageOrigins.get(messageId) ?? messageId
  const resolveGroupKey = (groupId: string) => groupOrigins.get(groupId) ?? `group-${groupId}`

  return { sync, resolveMessageKey, resolveGroupKey, reset }
}
