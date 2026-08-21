import { describe, expect, it } from 'vitest'
import { foldMessageGroups, assistantTurnSpan, isLastAssistantTurn, occupiedReplyModels } from './messageGroups'
import type { ChatMessage } from './types'

function msg(id: string, role: 'user' | 'assistant', groupId?: string): ChatMessage {
  return {
    id,
    role,
    content: id,
    timestamp: 1,
    ...(groupId ? { group_id: groupId } : {}),
  }
}

describe('foldMessageGroups', () => {
  it('单模型（无 group_id）保持线性，不折叠（零回归）', () => {
    const items = foldMessageGroups([
      msg('u1', 'user'),
      msg('a1', 'assistant'),
      msg('u2', 'user'),
      msg('a2', 'assistant'),
    ])
    expect(items.map((i) => i.type)).toEqual(['message', 'message', 'message', 'message'])
  })

  it('同 group_id 的连续 assistant 折成一个组', () => {
    const items = foldMessageGroups([
      msg('u1', 'user', 'g1'),
      msg('a1', 'assistant', 'g1'),
      msg('a2', 'assistant', 'g1'),
      msg('a3', 'assistant', 'g1'),
    ])
    // user 即使带 group_id 也不并入组（只折 assistant）。
    expect(items[0].type).toBe('message')
    expect(items[1].type).toBe('group')
    const group = items[1]
    if (group.type !== 'group') throw new Error('expected group')
    expect(group.groupId).toBe('g1')
    expect(group.messages.map((m) => m.id)).toEqual(['a1', 'a2', 'a3'])
  })

  it('不同 group_id 的 assistant 分成两组', () => {
    const items = foldMessageGroups([
      msg('a1', 'assistant', 'g1'),
      msg('a2', 'assistant', 'g1'),
      msg('a3', 'assistant', 'g2'),
    ])
    expect(items).toHaveLength(2)
    expect(items[0].type).toBe('group')
    expect(items[1].type).toBe('group')
    if (items[0].type === 'group') expect(items[0].messages).toHaveLength(2)
    if (items[1].type === 'group') expect(items[1].messages).toHaveLength(1)
  })

  it('组之间被普通消息打断', () => {
    const items = foldMessageGroups([
      msg('a1', 'assistant', 'g1'),
      msg('u1', 'user'),
      msg('a2', 'assistant', 'g1'),
    ])
    // 同 group_id 但被 user 打断 → 两个独立组（连续性被破坏）。
    expect(items.map((i) => i.type)).toEqual(['group', 'message', 'group'])
  })

  it('多答轮 + 单模型续聊轮混合', () => {
    const items = foldMessageGroups([
      msg('u1', 'user', 'g1'),
      msg('a1', 'assistant', 'g1'),
      msg('a2', 'assistant', 'g1'),
      msg('u2', 'user'),
      msg('a3', 'assistant'),
    ])
    expect(items.map((i) => i.type)).toEqual(['message', 'group', 'message', 'message'])
  })
})

describe('assistantTurnSpan', () => {
  it('ungrouped last reply is a one-message turn', () => {
    const messages = [
      msg('u1', 'user'),
      msg('a1', 'assistant'),
      msg('u2', 'user'),
      msg('a2', 'assistant'),
    ]
    const span = assistantTurnSpan(messages, 'a2')
    expect(span?.siblings.map((m) => m.id)).toEqual(['a2'])
    expect(isLastAssistantTurn(messages, 'a2')).toBe(true)
    expect(isLastAssistantTurn(messages, 'a1')).toBe(false)
  })

  it('grouped siblings share one turn', () => {
    const messages = [
      msg('u1', 'user'),
      msg('a1', 'assistant', 'g1'),
      msg('a2', 'assistant', 'g1'),
    ]
    const span = assistantTurnSpan(messages, 'a1')
    expect(span?.siblings.map((m) => m.id)).toEqual(['a1', 'a2'])
    expect(isLastAssistantTurn(messages, 'a1')).toBe(true)
  })

  it('occupied models fall back to the session model', () => {
    const messages = [
      msg('u1', 'user'),
      { ...msg('a1', 'assistant'), provider_id: 'p1', model: 'm1' },
    ]
    expect(occupiedReplyModels(messages, 'a1', 'p0', 'm0')).toEqual([
      { provider_id: 'p1', model: 'm1' },
    ])
    expect(occupiedReplyModels([msg('u1', 'user'), msg('a1', 'assistant')], 'a1', 'p0', 'm0')).toEqual([
      { provider_id: 'p0', model: 'm0' },
    ])
  })
})
