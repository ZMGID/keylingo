// @vitest-environment jsdom
import { act, render, renderHook, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MessageGroup } from './MessageGroup'
import { beginGroup, ensureGroupColumn, flushGroups, resetGroups } from './groupStreamingStore'
import { useMultiAnswerViewMode, type MultiAnswerViewMode } from './multiAnswerViewMode'
import type { ChatMessage } from './types'

// 用公开 API 驱动展示模式（内存态 + storage 同步），替代已删除的测试专用导出。
function setMultiAnswerViewMode(mode: MultiAnswerViewMode) {
  const { result, unmount } = renderHook(() => useMultiAnswerViewMode())
  act(() => {
    result.current[1](mode)
  })
  unmount()
}

afterEach(() => {
  resetGroups()
  setMultiAnswerViewMode('tabs')
  window.localStorage.clear()
})

function assistant(id: string, content: string, providerId: string, model: string): ChatMessage {
  return {
    id,
    role: 'assistant',
    content,
    provider_id: providerId,
    model,
    group_id: 'g1',
    timestamp: 1,
  }
}

describe('MessageGroup — columns 模式', () => {
  beforeEach(() => {
    setMultiAnswerViewMode('columns')
  })

  it('落库态：渲染每列的「model | provider」标签', () => {
    render(
      <MessageGroup
        conversationId="c1"
        groupId="g1"
        messages={[
          assistant('a1', 'answer one', 'openai', 'gpt-4o'),
          assistant('a2', 'answer two', 'anthropic', 'claude-3'),
        ]}
      />,
    )
    // 列头 + footer chip 都含标签 → getAllByText。
    expect(screen.getAllByText('gpt-4o | openai').length).toBeGreaterThan(0)
    expect(screen.getAllByText('claude-3 | anthropic').length).toBeGreaterThan(0)
  })

  it('选中条：默认第一列高亮；点选其它列触发回调', async () => {
    const onSelect = vi.fn()
    render(
      <MessageGroup
        conversationId="c1"
        groupId="g1"
        messages={[
          assistant('a1', 'answer one', 'openai', 'gpt-4o'),
          assistant('a2', 'answer two', 'anthropic', 'claude-3'),
        ]}
        onSelectColumn={onSelect}
      />,
    )
    // 默认第一列已选（列头显示「已选」）。
    expect(screen.getByText('已选')).toBeInTheDocument()
    const continueButtons = screen.getAllByText('用这条继续')
    expect(continueButtons).toHaveLength(1)
    await act(async () => {
      continueButtons[0].click()
    })
    expect(onSelect).toHaveBeenCalledWith('g1', 'a2')
  })

  it('显式选中条：高亮所记列', () => {
    render(
      <MessageGroup
        conversationId="c1"
        groupId="g1"
        messages={[
          assistant('a1', 'answer one', 'openai', 'gpt-4o'),
          assistant('a2', 'answer two', 'anthropic', 'claude-3'),
        ]}
        selectedMessageId="a2"
        onSelectColumn={() => {}}
      />,
    )
    // a2 被选 → a1 显示「用这条继续」，a2 显示「已选」。
    expect(screen.getByText('已选')).toBeInTheDocument()
    expect(screen.getAllByText('用这条继续')).toHaveLength(1)
  })

  it('流式态：从 group store 读实时列，无选中标记', async () => {
    act(() => {
      beginGroup('c1', 'g1', [
        { providerId: 'openai', model: 'gpt-4o' },
        { providerId: 'anthropic', model: 'claude-3' },
      ])
      const a = ensureGroupColumn('c1', 'msg_a', 'openai', 'gpt-4o')!
      a.content = 'streaming A'
      // touchGroup 现在 rAF 合帧；测试用 flushGroups 立即同步通知订阅者。
      flushGroups()
    })
    render(<MessageGroup conversationId="c1" groupId="g1" messages={[]} />)
    expect(screen.getByText(/streaming A/)).toBeInTheDocument()
    // 流式态不显示选中标记（还没落库）。
    expect(screen.queryByText('已选')).not.toBeInTheDocument()
    expect(screen.queryByText('用这条继续')).not.toBeInTheDocument()
  })

  it('性能降级（R10）：非聚焦列折叠 reasoning（正文 hideBody），聚焦列展开流式思考', async () => {
    act(() => {
      beginGroup('c1', 'g1', [
        { providerId: 'openai', model: 'gpt-4o' },
        { providerId: 'anthropic', model: 'claude-3' },
      ])
      const a = ensureGroupColumn('c1', 'msg_a', 'openai', 'gpt-4o')!
      a.streaming = true
      a.reasoning = 'focused thinking'
      const b = ensureGroupColumn('c1', 'msg_b', 'anthropic', 'claude-3')!
      b.streaming = true
      b.reasoning = 'unfocused thinking'
      flushGroups()
    })
    const { container } = render(<MessageGroup conversationId="c1" groupId="g1" messages={[]} />)
    // 默认聚焦第一列（msg_a）：其 ReasoningBlock 流式展开（aria-hidden=false）。
    // 非聚焦第二列（msg_b）：reasoningStreaming=false → 折叠 hideBody（aria-hidden=true）。
    const reasoningSections = container.querySelectorAll('section[aria-label="Thinking"] > [aria-hidden]')
    expect(reasoningSections.length).toBe(2)
    expect(reasoningSections[0].getAttribute('aria-hidden')).toBe('false')
    expect(reasoningSections[1].getAttribute('aria-hidden')).toBe('true')
  })

  it('整组铺满主区，单列封顶与单对话阅读宽一致', () => {
    const { container } = render(
      <MessageGroup
        conversationId="c1"
        groupId="g1"
        messages={[
          assistant('a1', 'answer one', 'openai', 'gpt-4o'),
          assistant('a2', 'answer two', 'anthropic', 'claude-3'),
        ]}
        onSelectColumn={() => {}}
      />,
    )
    expect(container.querySelector('.chat-message-group-wide')).toBeTruthy()
    expect(container.querySelector('.chat-message-group-stack')).toBeTruthy()
    expect(container.querySelector('.chat-message-rail')).toBeNull()
    const cols = container.querySelectorAll('.chat-message-group-col')
    expect(cols).toHaveLength(2)
    for (const col of cols) {
      expect(col.className).toMatch(/max-w-4xl/)
      expect(col.className).not.toMatch(/max-w-\[420px\]/)
    }
    expect(
      (container.querySelector('.chat-message-group-stack') as HTMLElement).style.getPropertyValue('--chat-message-group-cols'),
    ).toBe('2')
  })
})

describe('MessageGroup — tabs 模式（默认）', () => {
  it('默认只整宽渲染选中条（第一条），不显示其它条正文', () => {
    const { container } = render(
      <MessageGroup
        conversationId="c1"
        groupId="g1"
        messages={[
          assistant('a1', 'answer one', 'openai', 'gpt-4o'),
          assistant('a2', 'answer two', 'anthropic', 'claude-3'),
        ]}
        onSelectColumn={() => {}}
      />,
    )
    // tabs 模式：只渲染第一条正文，走单列阅读轨。
    expect(container.querySelector('.chat-message-rail')).toBeTruthy()
    expect(container.querySelector('.chat-message-group-wide')).toBeNull()
    expect(screen.getByText('answer one')).toBeInTheDocument()
    expect(screen.queryByText('answer two')).not.toBeInTheDocument()
    // 列头「用这条继续」按钮在 tabs 模式不渲染（交给 footer chip）。
    expect(screen.queryByText('用这条继续')).not.toBeInTheDocument()
    expect(screen.queryByText('已选')).not.toBeInTheDocument()
  })

  it('显式选中条：默认整宽显示所记列', () => {
    render(
      <MessageGroup
        conversationId="c1"
        groupId="g1"
        messages={[
          assistant('a1', 'answer one', 'openai', 'gpt-4o'),
          assistant('a2', 'answer two', 'anthropic', 'claude-3'),
        ]}
        selectedMessageId="a2"
        onSelectColumn={() => {}}
      />,
    )
    expect(screen.getByText('answer two')).toBeInTheDocument()
    expect(screen.queryByText('answer one')).not.toBeInTheDocument()
  })

  it('点 footer 模型 chip：切换显示条并触发 onSelectColumn（一举两用）', async () => {
    const onSelect = vi.fn()
    render(
      <MessageGroup
        conversationId="c1"
        groupId="g1"
        messages={[
          assistant('a1', 'answer one', 'openai', 'gpt-4o'),
          assistant('a2', 'answer two', 'anthropic', 'claude-3'),
        ]}
        onSelectColumn={onSelect}
      />,
    )
    // 初始显示第一条。
    expect(screen.getByText('answer one')).toBeInTheDocument()
    // footer 第二个模型 chip（claude-3）。
    const chip = screen.getByTitle('claude-3 | anthropic')
    await act(async () => {
      chip.click()
    })
    // 切换到第二条 + 触发续聊选中回调。
    expect(screen.getByText('answer two')).toBeInTheDocument()
    expect(screen.queryByText('answer one')).not.toBeInTheDocument()
    expect(onSelect).toHaveBeenCalledWith('g1', 'a2')
  })

  it('切到 columns 模式：N 列横向并排出现', async () => {
    const { container } = render(
      <MessageGroup
        conversationId="c1"
        groupId="g1"
        messages={[
          assistant('a1', 'answer one', 'openai', 'gpt-4o'),
          assistant('a2', 'answer two', 'anthropic', 'claude-3'),
        ]}
        onSelectColumn={() => {}}
      />,
    )
    expect(screen.queryByText('answer two')).not.toBeInTheDocument()
    expect(container.querySelector('.chat-message-rail')).toBeTruthy()
    // 点 footer「并排」按钮。
    const columnsBtn = screen.getByTitle('并排显示（多列）')
    await act(async () => {
      columnsBtn.click()
    })
    // 两条都整列渲染出来，并改走铺满主区的并排栈。
    expect(screen.getByText('answer one')).toBeInTheDocument()
    expect(screen.getByText('answer two')).toBeInTheDocument()
    expect(container.querySelector('.chat-message-group-wide')).toBeTruthy()
    expect(container.querySelector('.chat-message-rail')).toBeNull()
  })
})
