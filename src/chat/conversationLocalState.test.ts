import { describe, expect, it } from 'vitest'
import {
  clearConversationLocalState,
  type ConversationLocalState,
} from './conversationLocalState'

/**
 * 回归重点：哪些字段无条件清、哪些必须按 scope 清。
 *
 * 这正是搬迁前靠人记的部分 —— 6 处手写删除块的字段组合各不相同，
 * 少清一项会漏状态（ghost 会话），多清一项会吞掉本该展示的错误或延后的 done。
 */
function makeState(): ConversationLocalState {
  return {
    inFlight: new Set(['c1', 'c2']),
    pendingStreamDone: { c1: async () => {}, c2: async () => {} },
    streamSnapshots: { c1: { content: 'a' } as never, c2: { content: 'b' } as never },
    streamErrors: { c1: 'err1', c2: 'err2' },
    pendingToolConfirms: { c1: { conversationId: 'c1' } as never, c2: {} as never },
    pendingSessionConsents: { c1: { conversationId: 'c1' } as never, c2: {} as never },
    pendingUserPrompts: { c1: [{ conversationId: 'c1' }] as never, c2: [] as never },
  }
}

describe('clearConversationLocalState 无条件清理', () => {
  it('默认清快照 / 待确认工具 / 待确认授权三项', () => {
    const s = makeState()
    clearConversationLocalState(s, 'c1')
    expect(s.streamSnapshots.c1).toBeUndefined()
    expect(s.pendingToolConfirms.c1).toBeUndefined()
    expect(s.pendingSessionConsents.c1).toBeUndefined()
  })

  it('默认不动 inFlight / pendingStreamDone / streamErrors', () => {
    const s = makeState()
    clearConversationLocalState(s, 'c1')
    // 正常一轮结束：错误要留着展示，延后的 done 要留着 flush，in-flight 由别处收
    expect(s.inFlight.has('c1')).toBe(true)
    expect(s.pendingStreamDone.c1).toBeDefined()
    expect(s.streamErrors.c1).toBe('err1')
  })

  it('只影响目标会话，不碰其他会话', () => {
    const s = makeState()
    clearConversationLocalState(s, 'c1', { inFlight: true, pendingStreamDone: true, streamErrors: true })
    expect(s.streamSnapshots.c2).toBeDefined()
    expect(s.pendingToolConfirms.c2).toBeDefined()
    expect(s.pendingSessionConsents.c2).toBeDefined()
    expect(s.inFlight.has('c2')).toBe(true)
    expect(s.pendingStreamDone.c2).toBeDefined()
    expect(s.streamErrors.c2).toBe('err2')
  })
})

describe('clearConversationLocalState scope', () => {
  it('inFlight=true 时移出 in-flight 集合', () => {
    const s = makeState()
    clearConversationLocalState(s, 'c1', { inFlight: true })
    expect(s.inFlight.has('c1')).toBe(false)
  })

  it('pendingStreamDone=true 时丢弃延后的 done', () => {
    const s = makeState()
    clearConversationLocalState(s, 'c1', { pendingStreamDone: true })
    expect(s.pendingStreamDone.c1).toBeUndefined()
  })

  it('streamErrors=true 时清错误', () => {
    const s = makeState()
    clearConversationLocalState(s, 'c1', { streamErrors: true })
    expect(s.streamErrors.c1).toBeUndefined()
  })

  it('全开等价于彻底剔除该会话（dropConversationLocally 的语义）', () => {
    const s = makeState()
    clearConversationLocalState(s, 'c1', {
      inFlight: true, pendingStreamDone: true, streamErrors: true,
    })
    expect(s.inFlight.has('c1')).toBe(false)
    expect(s.pendingStreamDone.c1).toBeUndefined()
    expect(s.streamSnapshots.c1).toBeUndefined()
    expect(s.streamErrors.c1).toBeUndefined()
    expect(s.pendingToolConfirms.c1).toBeUndefined()
    expect(s.pendingSessionConsents.c1).toBeUndefined()
  })
})

describe('clearConversationLocalState 边界', () => {
  it('清不存在的会话不报错、不影响其他', () => {
    const s = makeState()
    clearConversationLocalState(s, 'nope', { inFlight: true, streamErrors: true })
    expect(s.streamSnapshots.c1).toBeDefined()
    expect(s.inFlight.size).toBe(2)
  })

  it('重复清理幂等', () => {
    const s = makeState()
    clearConversationLocalState(s, 'c1', { inFlight: true })
    clearConversationLocalState(s, 'c1', { inFlight: true })
    expect(s.streamSnapshots.c1).toBeUndefined()
    expect(s.inFlight.has('c1')).toBe(false)
  })
})
