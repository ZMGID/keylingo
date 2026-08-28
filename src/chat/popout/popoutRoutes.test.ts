/**
 * @vitest-environment jsdom
 */
import { describe, expect, it } from 'vitest'
import {
  getPopoutConversationId,
  getPopoutConversationIdFromPath,
  isChatPopoutPath,
  popoutConversationHash,
} from './popoutRoutes'

describe('popoutRoutes', () => {
  it('recognizes popout paths', () => {
    expect(isChatPopoutPath('chat/popout')).toBe(true)
    expect(isChatPopoutPath('chat/popout/conv_abc')).toBe(true)
    expect(isChatPopoutPath('chat/conv_abc')).toBe(false)
    expect(isChatPopoutPath('chat/settings')).toBe(false)
  })

  it('encodes the conversation id into the hash', () => {
    expect(popoutConversationHash('conv_a/b')).toBe('#chat/popout/conv_a%2Fb')
  })

  it('parses the conversation id from the path', () => {
    expect(getPopoutConversationIdFromPath('chat/popout/conv_abc-1')).toBe('conv_abc-1')
    expect(getPopoutConversationIdFromPath('chat/popout/')).toBeNull()
    expect(getPopoutConversationIdFromPath('chat/abc')).toBeNull()
  })

  it('reads the id from location.hash', () => {
    window.location.hash = '#chat/popout/conv_live'
    expect(getPopoutConversationId()).toBe('conv_live')
  })
})
