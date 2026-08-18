import { describe, expect, it } from 'vitest'
import type { ChatMessage } from './types'
import {
  buildConversationTrajectory,
  compactTrajectoryText,
  filterTrajectorySteps,
  formatTrajectoryDuration,
  summarizeTrajectory,
} from './trajectory'

function message(partial: Partial<ChatMessage> & Pick<ChatMessage, 'id' | 'role'>): ChatMessage {
  return {
    content: '',
    timestamp: 1,
    ...partial,
  }
}

describe('conversation trajectory', () => {
  it('flattens user, tool, and assistant segments into a human ledger', () => {
    const steps = buildConversationTrajectory([
      message({ id: 'u1', role: 'user', content: '列出可用能力' }),
      message({
        id: 'a1',
        role: 'assistant',
        content: 'ignored when segments exist',
        segments: [
          { id: 's0', kind: 'reasoning', phase: 'plain', order: 0, text: 'hidden thinking' },
          { id: 's1', kind: 'tool', phase: 'tool_loop', order: 1, toolCallId: 'c1' },
          { id: 's2', kind: 'text', phase: 'synthesis', order: 2, text: '我可以读写文件并搜索网页。' },
        ],
        toolCalls: [{
          id: 'c1',
          toolName: 'read_file',
          argumentPreview: 'src/chat/Chat.tsx',
          resultPreview: '80 lines',
          status: 'success',
        }],
      }),
    ])

    expect(steps.map((step) => [step.kind, step.title, step.preview, step.result])).toEqual([
      ['user', 'user', '列出可用能力', undefined],
      ['tool', 'read_file', 'src/chat/Chat.tsx', '80 lines'],
      ['assistant', 'assistant', '我可以读写文件并搜索网页。', undefined],
    ])
  })

  it('falls back to tool_calls plus content when segments are missing', () => {
    const steps = buildConversationTrajectory([
      message({
        id: 'a1',
        role: 'assistant',
        content: '已完成搜索',
        toolCalls: [{
          id: 'c1',
          name: 'web_search',
          arguments: { query: 'kivio trajectory' },
          resultPreview: '8 hits',
          status: 'completed',
        }],
      }),
    ])
    expect(steps).toEqual([
      expect.objectContaining({ kind: 'tool', title: 'web_search', preview: 'kivio trajectory' }),
      expect.objectContaining({ kind: 'assistant', preview: '已完成搜索' }),
    ])
  })

  it('filters the ledger and summarizes turns and tool calls', () => {
    const steps = buildConversationTrajectory([
      message({ id: 'u1', role: 'user', content: '先搜索再总结' }),
      message({
        id: 'a1',
        role: 'assistant',
        content: '总结完成',
        toolCalls: [{ id: 'c1', toolName: 'grep', argumentPreview: 'trajectory', status: 'success' }],
      }),
    ])
    expect(filterTrajectorySteps(steps, 'grep').map((step) => step.title)).toEqual(['grep'])
    expect(summarizeTrajectory(steps)).toMatchObject({ turns: 1, steps: 3, calls: 1 })
  })

  it('compacts whitespace and formats short durations', () => {
    expect(compactTrajectoryText('  hello\nworld  ')).toBe('hello world')
    expect(formatTrajectoryDuration(10, 85)).toBe('1m15s')
    expect(formatTrajectoryDuration(10, 10)).toBeNull()
  })
})
