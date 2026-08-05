import { renderHook, act } from '@testing-library/react'
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { useRef } from 'react'
import { useChatRouting } from './useChatRouting'

/**
 * 回归重点（搬迁时最容易破的三件事）：
 *   1. 分支顺序 —— 中心页判定必须早于会话解析，否则 '#chat/mcp' 会被当成会话 id
 *   2. 挂载即执行一次 + 订阅 hashchange 的时序
 *   3. 「已是当前会话则跳过重载」这条防双读逻辑
 */
function setup(initialHash = '#chat') {
  window.location.hash = initialHash
  const onViewChange = vi.fn()
  const onLoadConversation = vi.fn()
  const onResetConversation = vi.fn()

  const rendered = renderHook(() => {
    const currentConversationIdRef = useRef<string | null>(null)
    const routing = useChatRouting({
      onViewChange,
      onLoadConversation,
      onResetConversation,
      currentConversationIdRef,
    })
    return { routing, currentConversationIdRef }
  })

  return { ...rendered, onViewChange, onLoadConversation, onResetConversation }
}

describe('useChatRouting 挂载即解析', () => {
  beforeEach(() => {
    window.location.hash = '#chat'
  })

  it('挂载时立刻按当前 hash 解析一次', () => {
    const { onViewChange } = setup('#chat/settings')
    expect(onViewChange).toHaveBeenCalledWith('settings')
  })

  it('空会话路由触发 reset 而非 load', () => {
    const { onViewChange, onResetConversation, onLoadConversation } = setup('#chat')
    expect(onViewChange).toHaveBeenCalledWith('conversation')
    expect(onResetConversation).toHaveBeenCalled()
    expect(onLoadConversation).not.toHaveBeenCalled()
  })

  it('会话路由触发 load 且带 id', () => {
    const { onLoadConversation, onResetConversation } = setup('#chat/conv-1')
    expect(onLoadConversation).toHaveBeenCalledWith('conv-1')
    expect(onResetConversation).not.toHaveBeenCalled()
  })
})

describe('useChatRouting 分支顺序', () => {
  // 关键：中心页判定必须早于会话解析。若顺序反了，这些路由会被当作会话 id 去加载。
  const centerRoutes: Array<[string, string]> = [
    ['#chat/settings', 'settings'],
    ['#chat/assistants', 'assistants'],
    ['#chat/skill', 'skill'],
    ['#chat/mcp', 'mcp'],
    ['#chat/knowledge', 'knowledge'],
    ['#chat/notes', 'notes'],
    ['#chat/plugins', 'plugins'],
    ['#chat/onboarding', 'onboarding'],
  ]

  for (const [hash, view] of centerRoutes) {
    it(`${hash} → view=${view} 且不当作会话加载`, () => {
      const { onViewChange, onLoadConversation, onResetConversation } = setup(hash)
      expect(onViewChange).toHaveBeenCalledWith(view)
      expect(onLoadConversation).not.toHaveBeenCalled()
      expect(onResetConversation).not.toHaveBeenCalled()
    })
  }
})

describe('useChatRouting hashchange', () => {
  beforeEach(() => {
    window.location.hash = '#chat'
  })

  it('hash 变化后重新解析', () => {
    const { onViewChange } = setup('#chat')
    onViewChange.mockClear()
    act(() => {
      window.location.hash = '#chat/skill'
      window.dispatchEvent(new HashChangeEvent('hashchange'))
    })
    expect(onViewChange).toHaveBeenCalledWith('skill')
  })

  it('卸载后不再响应 hash 变化', () => {
    const { onViewChange, unmount } = setup('#chat')
    unmount()
    onViewChange.mockClear()
    act(() => {
      window.location.hash = '#chat/mcp'
      window.dispatchEvent(new HashChangeEvent('hashchange'))
    })
    expect(onViewChange).not.toHaveBeenCalled()
  })

  it('已是当前会话时跳过重载（防双读）', () => {
    const { result, onLoadConversation } = setup('#chat')
    act(() => {
      result.current.currentConversationIdRef.current = 'conv-9'
    })
    onLoadConversation.mockClear()
    act(() => {
      window.location.hash = '#chat/conv-9'
      window.dispatchEvent(new HashChangeEvent('hashchange'))
    })
    expect(onLoadConversation).not.toHaveBeenCalled()
  })

  it('切到不同会话时正常重载', () => {
    const { result, onLoadConversation } = setup('#chat')
    act(() => {
      result.current.currentConversationIdRef.current = 'conv-9'
    })
    onLoadConversation.mockClear()
    act(() => {
      window.location.hash = '#chat/conv-10'
      window.dispatchEvent(new HashChangeEvent('hashchange'))
    })
    expect(onLoadConversation).toHaveBeenCalledWith('conv-10')
  })
})

describe('useChatRouting sync*Route', () => {
  beforeEach(() => {
    window.location.hash = '#chat'
  })

  it('九个 sync 各写对应 hash（不串）', () => {
    const { result } = setup('#chat')
    const r = result.current.routing
    const cases: Array<[() => void, string]> = [
      [r.syncSettingsRoute, '#chat/settings'],
      [r.syncOnboardingRoute, '#chat/onboarding'],
      [r.syncAssistantCenterRoute, '#chat/assistants'],
      [r.syncSkillCenterRoute, '#chat/skill'],
      [r.syncPluginCenterRoute, '#chat/plugins'],
      [r.syncMcpCenterRoute, '#chat/mcp'],
      [r.syncKnowledgeCenterRoute, '#chat/knowledge'],
      [r.syncNotesRoute, '#chat/notes'],
    ]
    for (const [sync, expected] of cases) {
      act(() => { sync() })
      expect(window.location.hash).toBe(expected)
    }
    act(() => { r.syncConversationRoute('conv-2') })
    expect(window.location.hash).toBe('#chat/conv-2')
    act(() => { r.syncConversationRoute(null) })
    expect(window.location.hash).toBe('#chat')
  })
})
