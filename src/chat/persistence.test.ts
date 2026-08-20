/**
 * @vitest-environment jsdom
 */
import { beforeEach, describe, expect, it } from 'vitest'
import {
  forgetRememberedChatRoute,
  getRememberedChatRoute,
  getRememberedDockTab,
  hashPath,
  isChatPath,
  normalizeStoredChatRoute,
  rememberCurrentChatRoute,
  rememberDockTab,
} from './persistence'


describe('hashPath', () => {
  it('strips hash prefix and query string', () => {
    window.location.hash = '#chat/settings?tab=general'
    expect(hashPath()).toBe('chat/settings')
  })
})

describe('isChatPath', () => {
  it('matches chat routes', () => {
    expect(isChatPath('chat')).toBe(true)
    expect(isChatPath('chat/conv-1')).toBe(true)
    expect(isChatPath('settings')).toBe(false)
  })
})


describe('normalizeStoredChatRoute', () => {
  it('accepts conversation routes and normalizes missing hash', () => {
    expect(normalizeStoredChatRoute('#chat/conv-1')).toBe('#chat/conv-1')
    expect(normalizeStoredChatRoute('chat/conv-1')).toBe('#chat/conv-1')
  })

  it('rejects settings / onboarding / non-chat values', () => {
    expect(normalizeStoredChatRoute('#chat/settings')).toBeNull()
    expect(normalizeStoredChatRoute('#chat/settings?tab=general')).toBeNull()
    expect(normalizeStoredChatRoute('#chat/onboarding')).toBeNull()
    expect(normalizeStoredChatRoute('#lens')).toBeNull()
    expect(normalizeStoredChatRoute(null)).toBeNull()
  })
})

describe('right dock tab persistence', () => {
  it('falls back to files for the removed trajectory / Pi sessions keys', () => {
    window.localStorage.clear()
    window.localStorage.setItem('kivio-chat-dock-tab', 'trajectory')
    expect(getRememberedDockTab()).toBe('files')
    window.localStorage.setItem('kivio-chat-dock-tab', 'piSessions')
    expect(getRememberedDockTab()).toBe('files')
    rememberDockTab('git')
    expect(getRememberedDockTab()).toBe('git')
  })
})

describe('last route memory (Rust-persisted, auto-migrates from localStorage)', () => {
  beforeEach(() => {
    window.localStorage.clear()
    forgetRememberedChatRoute()
  })

  it('remembers the current conversation route in the in-memory cache', () => {
    window.location.hash = '#chat/conv-a'
    rememberCurrentChatRoute()
    expect(getRememberedChatRoute()).toBe('#chat/conv-a')
  })

  it('does not remember the list / settings / onboarding routes', () => {
    window.location.hash = '#chat'
    rememberCurrentChatRoute()
    expect(getRememberedChatRoute()).toBeNull()

    window.location.hash = '#chat/settings'
    rememberCurrentChatRoute()
    expect(getRememberedChatRoute()).toBeNull()
  })

  it('auto-migrates legacy localStorage on first getRememberedChatRoute call', () => {
    window.localStorage.setItem('kivio-chat-last-route', '#chat/conv-legacy')
    const route = getRememberedChatRoute()
    expect(route).toBe('#chat/conv-legacy')
    expect(window.localStorage.getItem('kivio-chat-last-route')).toBeNull()
    
    // 第二次调用应返回缓存值，不再读 localStorage
    expect(getRememberedChatRoute()).toBe('#chat/conv-legacy')
  })


  it('falls back to the legacy localStorage value only when the cache is empty', () => {
    window.localStorage.setItem('kivio-chat-last-route', '#chat/conv-legacy')
    expect(getRememberedChatRoute()).toBe('#chat/conv-legacy')
  })
})
