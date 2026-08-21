import { describe, expect, it } from 'vitest'
import type { CompactionBoundaryView } from './compactionBoundary'
import {
  activeMessageNavigatorNodeId,
  buildMessageNavigatorNodes,
  messageNavigatorProximityWidth,
  visibleMessageNavigatorNodeIds,
} from './messageNavigator'
import { foldMessageGroups } from './messageGroups'
import type { ChatMessage, CompactionBoundaryRecord } from './types'

function msg(id: string, role: 'user' | 'assistant', content: string, groupId?: string, model?: string): ChatMessage {
  return {
    id,
    role,
    content,
    timestamp: 1,
    ...(groupId ? { group_id: groupId } : {}),
    ...(model ? { model } : {}),
  }
}

function indexes(entries: Array<[string, number]>): Map<string, number> {
  return new Map(entries)
}

describe('message navigator model', () => {
  it('一轮 user + assistant 生成一个节点，并指向 user render item', () => {
    const folded = foldMessageGroups([
      msg('u1', 'user', '问题一'),
      msg('a1', 'assistant', '回答一', undefined, 'gpt-5'),
      msg('u2', 'user', '问题二'),
      msg('a2', 'assistant', '回答二', undefined, 'claude'),
    ])
    const nodes = buildMessageNavigatorNodes({
      folded,
      boundaries: [],
      renderIndexByKey: indexes([['u1', 1], ['a1', 2], ['u2', 3], ['a2', 4]]),
    })
    expect(nodes).toMatchObject([
      { id: 'turn-u1', targetRenderIndex: 1, title: '问题一', answerPreview: '回答一', modelLabel: 'gpt-5' },
      { id: 'turn-u2', targetRenderIndex: 3, title: '问题二', answerPreview: '回答二', modelLabel: 'claude' },
    ])
  })

  it('多模型回答组仍只生成一个轮次节点', () => {
    const folded = foldMessageGroups([
      msg('u1', 'user', '比较回答', 'g1'),
      msg('a1', 'assistant', '回答 A', 'g1', 'gpt-5'),
      msg('a2', 'assistant', '回答 B', 'g1', 'claude'),
    ])
    const nodes = buildMessageNavigatorNodes({
      folded,
      boundaries: [],
      renderIndexByKey: indexes([['u1', 1], ['group-g1', 2]]),
    })
    expect(nodes).toHaveLength(1)
    expect(nodes[0]).toMatchObject({ id: 'turn-u1', answerPreview: '回答 A', modelLabel: '2 个模型' })
  })

  it('连续 user 消息各自保留轮次，未回答轮次允许空摘要', () => {
    const nodes = buildMessageNavigatorNodes({
      folded: foldMessageGroups([
        msg('u1', 'user', '尚未回答'),
        msg('u2', 'user', '第二问'),
        msg('a2', 'assistant', '第二答'),
      ]),
      boundaries: [],
      renderIndexByKey: indexes([['u1', 1], ['u2', 2], ['a2', 3]]),
    })
    expect(nodes).toMatchObject([
      { id: 'turn-u1', answerPreview: '' },
      { id: 'turn-u2', answerPreview: '第二答' },
    ])
  })

  it('上下文压缩生成独立特殊节点并按 render index 排序', () => {
    const record: CompactionBoundaryRecord = {
      id: 'c1',
      source_until_message_id: 'a1',
      summary_content: '此前摘要',
      trigger: 'auto',
      created_at: 1,
    }
    const boundaries: CompactionBoundaryView[] = [{ afterIndex: 1, record }]
    const nodes = buildMessageNavigatorNodes({
      folded: foldMessageGroups([
        msg('u1', 'user', '问题一'),
        msg('a1', 'assistant', '回答一'),
        msg('u2', 'user', '问题二'),
      ]),
      boundaries,
      renderIndexByKey: indexes([
        ['u1', 1],
        ['a1', 2],
        ['compaction-summary-c1', 4],
        ['u2', 5],
      ]),
    })
    expect(nodes.map((node) => node.id)).toEqual(['turn-u1', 'compaction-c1', 'turn-u2'])
    expect(nodes[1]).toMatchObject({ kind: 'compaction', answerPreview: '此前摘要' })
  })

  it('上下文清空生成独立特殊节点并按 render index 排序', () => {
    const clearBoundaries = [{
      afterIndex: 1,
      record: {
        id: 'clr1',
        source_until_message_id: 'a1',
        created_at: 10,
      },
    }]
    const nodes = buildMessageNavigatorNodes({
      folded: foldMessageGroups([
        msg('u1', 'user', '问题一'),
        msg('a1', 'assistant', '回答一'),
        msg('u2', 'user', '问题二'),
      ]),
      boundaries: [],
      clearBoundaries,
      renderIndexByKey: indexes([
        ['u1', 1],
        ['a1', 2],
        ['context-clear-divider-clr1', 3],
        ['u2', 4],
      ]),
    })
    expect(nodes.map((node) => node.id)).toEqual(['turn-u1', 'clear-clr1', 'turn-u2'])
    expect(nodes[1]).toMatchObject({ kind: 'clear', title: '清空上下文' })
  })

  it('阅读索引取不晚于基准线的最近节点', () => {
    const nodes = buildMessageNavigatorNodes({
      folded: foldMessageGroups([
        msg('u1', 'user', '一'),
        msg('a1', 'assistant', '答一'),
        msg('u2', 'user', '二'),
      ]),
      boundaries: [],
      renderIndexByKey: indexes([['u1', 1], ['a1', 2], ['u2', 5]]),
    })
    expect(activeMessageNavigatorNodeId(nodes, 0)).toBe('turn-u1')
    expect(activeMessageNavigatorNodeId(nodes, 4)).toBe('turn-u1')
    expect(activeMessageNavigatorNodeId(nodes, 5)).toBe('turn-u2')
  })

  it('返回与当前虚拟列表可见区间相交的全部语义节点', () => {
    const nodes = buildMessageNavigatorNodes({
      folded: foldMessageGroups([
        msg('u1', 'user', '一'),
        msg('a1', 'assistant', '答一'),
        msg('u2', 'user', '二'),
        msg('a2', 'assistant', '答二'),
        msg('u3', 'user', '三'),
      ]),
      boundaries: [],
      renderIndexByKey: indexes([['u1', 1], ['a1', 2], ['u2', 4], ['a2', 5], ['u3', 7]]),
    })
    expect(visibleMessageNavigatorNodeIds(nodes, 2, 5)).toEqual(['turn-u1', 'turn-u2'])
    expect(visibleMessageNavigatorNodeIds(nodes, 6, 8)).toEqual(['turn-u2', 'turn-u3'])
  })

  it('鼠标距离映射为连续且有界的鱼眼刻度宽度', () => {
    expect(messageNavigatorProximityWidth(0)).toBe(22)
    expect(messageNavigatorProximityWidth(26)).toBeCloseTo(11.5)
    expect(messageNavigatorProximityWidth(52)).toBe(8)
    expect(messageNavigatorProximityWidth(100)).toBe(8)
  })
})
