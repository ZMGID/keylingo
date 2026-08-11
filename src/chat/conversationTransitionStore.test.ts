import { describe, expect, it } from 'vitest'
import {
  beginConversationTransition,
  cancelConversationTransition,
  completeConversationTransition,
  getConversationTransitionSnapshot,
  invalidateConversationTransition,
  isCurrentConversationTransition,
} from './conversationTransitionStore'

describe('conversationTransitionStore', () => {
  it('keeps sidebar selection on the newest request while older loads finish harmlessly', () => {
    const first = beginConversationTransition('conversation-a')
    const second = beginConversationTransition('conversation-b')

    expect(isCurrentConversationTransition(first, 'conversation-a')).toBe(false)
    expect(isCurrentConversationTransition(second, 'conversation-b')).toBe(true)

    completeConversationTransition('conversation-a', first)
    expect(getConversationTransitionSnapshot()).toMatchObject({
      targetConversationId: 'conversation-b',
      loading: true,
    })

    completeConversationTransition('conversation-b', second)
    expect(getConversationTransitionSnapshot()).toMatchObject({
      targetConversationId: 'conversation-b',
      loading: false,
    })
  })

  it('can invalidate a pending load when starting a new conversation', () => {
    const requestId = beginConversationTransition('conversation-a')
    invalidateConversationTransition()
    cancelConversationTransition(requestId)

    expect(getConversationTransitionSnapshot()).toMatchObject({
      targetConversationId: null,
      loading: false,
    })
  })

  it('only shows the loading shell for larger conversations', () => {
    // threshold is exclusive: ≤12 messages skip the logo shell so small opens feel instant
    beginConversationTransition('small', { messageCount: 12 })
    expect(getConversationTransitionSnapshot().showLoading).toBe(false)

    beginConversationTransition('large', { messageCount: 13 })
    expect(getConversationTransitionSnapshot().showLoading).toBe(true)

    // unknown size stays conservative
    beginConversationTransition('unknown')
    expect(getConversationTransitionSnapshot().showLoading).toBe(true)
  })
})

