/**
 * @vitest-environment jsdom
 *
 * 路由判定读 window.location.hash，需要 DOM 环境。
 * vite.config.ts 只给 *.test.tsx 配了 jsdom，这里按文件声明。
 */
import { describe, expect, it, beforeEach } from 'vitest'
import {
  conversationHash,
  getRouteConversationId,
  hashPath,
  isChatAssistantCenterPath,
  isChatKnowledgeCenterPath,
  isChatMcpCenterPath,
  isChatNotesPath,
  isChatOnboardingRoute,
  isChatPluginCenterPath,
  isChatSettingsPath,
  isChatSkillCenterPath,
  setHash,
} from './chatRoutes'

function withHash(hash: string) {
  window.location.hash = hash
}

describe('chatRoutes 判定', () => {
  it('各中心页判定互不误命中', () => {
    const cases: Array<[string, (p: string) => boolean]> = [
      ['chat/settings', isChatSettingsPath],
      ['chat/assistants', isChatAssistantCenterPath],
      ['chat/skill', isChatSkillCenterPath],
      ['chat/plugins', isChatPluginCenterPath],
      ['chat/mcp', isChatMcpCenterPath],
      ['chat/knowledge', isChatKnowledgeCenterPath],
      ['chat/notes', isChatNotesPath],
      ['chat/onboarding', isChatOnboardingRoute],
    ]
    for (const [path, predicate] of cases) {
      expect(predicate(path)).toBe(true)
      // 任一判定不应命中其他路由
      for (const [otherPath, otherPredicate] of cases) {
        if (otherPath === path) continue
        expect(otherPredicate(path)).toBe(false)
      }
    }
  })

  it('子路径也命中（chat/settings/xxx）', () => {
    expect(isChatSettingsPath('chat/settings/providers')).toBe(true)
    expect(isChatSkillCenterPath('chat/skill/store')).toBe(true)
  })

  it('前缀相近但不同的路径不命中', () => {
    // 'chat/settingsx' 不是 settings 的子路径
    expect(isChatSettingsPath('chat/settingsx')).toBe(false)
    expect(isChatNotesPath('chat/notesarchive')).toBe(false)
  })

  it('会话路径不被任何中心页判定命中', () => {
    const convPath = 'chat/abc-123'
    for (const predicate of [
      isChatSettingsPath, isChatAssistantCenterPath, isChatSkillCenterPath,
      isChatPluginCenterPath, isChatMcpCenterPath, isChatKnowledgeCenterPath,
      isChatNotesPath, isChatOnboardingRoute,
    ]) {
      expect(predicate(convPath)).toBe(false)
    }
  })
})

describe('hashPath', () => {
  it('去掉 # 并截断 query', () => {
    withHash('#chat/abc?mode=x')
    expect(hashPath()).toBe('chat/abc')
  })

  it('空 hash 返回空串', () => {
    withHash('')
    expect(hashPath()).toBe('')
  })
})

describe('getRouteConversationId', () => {
  it('会话路由返回解码后的 id', () => {
    withHash('#chat/abc-123')
    expect(getRouteConversationId()).toBe('abc-123')
  })

  it('URL 编码的 id 被解码', () => {
    withHash(`#chat/${encodeURIComponent('a/b c')}`)
    expect(getRouteConversationId()).toBe('a/b c')
  })

  it('空会话路由返回 null', () => {
    withHash('#chat')
    expect(getRouteConversationId()).toBeNull()
  })

  it('非 chat 路由返回 null', () => {
    withHash('#settings')
    expect(getRouteConversationId()).toBeNull()
  })

  it('排除清单里的中心页返回 null', () => {
    for (const seg of ['settings', 'assistants', 'skill', 'knowledge', 'onboarding']) {
      withHash(`#chat/${seg}`)
      expect(getRouteConversationId()).toBeNull()
    }
  })

  it('mcp / notes / plugins 未在排除清单内（保留搬迁前的既有行为）', () => {
    // 这三条由 loadFromRoute 更靠前的分支拦截，走不到这里；
    // 此断言锁住"解析函数本身不排除它们"这一事实，避免后续误以为是遗漏而改坏。
    for (const seg of ['mcp', 'notes', 'plugins']) {
      withHash(`#chat/${seg}`)
      expect(getRouteConversationId()).toBe(seg)
    }
  })
})

describe('setHash / conversationHash', () => {
  beforeEach(() => {
    withHash('#chat')
  })

  it('conversationHash 对空 id 返回 #chat', () => {
    expect(conversationHash(null)).toBe('#chat')
  })

  it('conversationHash 编码特殊字符', () => {
    expect(conversationHash('a/b')).toBe('#chat/a%2Fb')
  })

  it('setHash 写入目标值', () => {
    setHash('#chat/settings')
    expect(window.location.hash).toBe('#chat/settings')
  })

  it('已是目标值时不重复写（避免多余 hashchange）', () => {
    setHash('#chat')
    let fired = 0
    const onChange = () => { fired += 1 }
    window.addEventListener('hashchange', onChange)
    setHash('#chat')
    window.removeEventListener('hashchange', onChange)
    expect(fired).toBe(0)
  })
})
