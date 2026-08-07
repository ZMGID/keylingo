import { memo, useRef } from 'react'
import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { MessageList } from './MessageList'
import {
  getCoarse,
  patchSnapshot,
  reset,
  setCoarse,
  setSnapshot,
} from './streamingStore'
import { createEmptyStreamSnapshot } from './conversationRuns'
import type { ConversationStreamSnapshot } from './conversationRuns'
import type { ChatMessage } from './types'
import * as messageNavigator from './messageNavigator'

// 真实集成：挂载真 MessageList（订阅真 streamingStore），按 Chat 各 helper 的调用方式驱动 store，
// 验证「流式更新只重渲订阅者、不波及兄弟节点」这一核心收益，以及各 helper→store 映射的渲染结果。

function snapWith(partial: Partial<ConversationStreamSnapshot>): ConversationStreamSnapshot {
  return { ...createEmptyStreamSnapshot(), ...partial }
}

// MessageList 现在用 virtua 虚拟化：视口测量 + 可见区间计算发生在 mount 后的一个微任务，
// 故断言渲染结果前需让 React 把这次异步更新刷出来。
async function flush() {
  await act(async () => {
    await Promise.resolve()
  })
}

afterEach(() => {
  vi.restoreAllMocks()
  act(() => {
    reset()
    setCoarse({ streaming: false, streamFrozen: false, cancelling: false, streamError: '' })
  })
})

// 不订阅 store 的兄弟节点，记录自身渲染次数。
let siblingRenders = 0
const Sibling = memo(function Sibling() {
  const count = useRef(0)
  count.current += 1
  siblingRenders = count.current
  return <div data-testid="sibling">sibling</div>
})

function mountList() {
  return render(
    <>
      <MessageList messages={[]} conversationId="c1" />
      <Sibling />
    </>,
  )
}

function message(id: number): ChatMessage {
  return {
    id: `m-${id}`,
    role: id % 2 === 0 ? 'user' : 'assistant',
    content: `message ${id}`,
    timestamp: id,
  }
}

describe('MessageList ← streamingStore 集成', () => {
  it('does not render a detached global agent plan row', async () => {
    const onExecute = vi.fn()
    render(
      <MessageList
        conversationId="c-plan"
        messages={[{
          id: 'msg-plan',
          role: 'assistant',
          content: '1. Read code\n2. Implement',
          timestamp: 1,
        }]}
        agentPlanState={{ mode: 'plan', status: 'draft', plan: '1. Read code\n2. Implement', updated_at: 1 }}
        onExecuteAgentPlan={onExecute}
      />,
    )
    await flush()

    expect(document.querySelector('[data-chat-message-list-item="plan"]')).not.toBeInTheDocument()
    const button = screen.getByRole('button', { name: '执行这条计划' })
    expect(button).toBeInTheDocument()
    await act(async () => {
      button.click()
    })
    expect(onExecute).toHaveBeenCalledWith('msg-plan')
  })

  it('does not attach a legacy agent plan row to non-plan text', async () => {
    render(
      <MessageList
        conversationId="c-plan-fragment"
        messages={[{
          id: 'msg-plan-fragment',
          role: 'assistant',
          content: '没问题！积萌,',
          timestamp: 1,
        }]}
        agentPlanState={{ mode: 'plan', status: 'draft', plan: '没问题！积萌,', updated_at: 1 }}
        onExecuteAgentPlan={() => {}}
      />,
    )
    await flush()

    expect(screen.queryByText('计划草案')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: '执行这条计划' })).not.toBeInTheDocument()
  })

  it('applyStreamSnapshotToState 等价：内容快照 + coarse streaming → 渲染流式预览文本', async () => {
    siblingRenders = 0
    mountList()
    expect(siblingRenders).toBe(1)

    // 模拟 applyStreamSnapshotToState：setSnapshot(snapshot) + setCoarse({streaming:true})
    act(() => {
      setSnapshot(snapWith({ content: 'hello streaming world', streaming: true }))
      setCoarse({ streaming: true, cancelling: false })
    })
    await flush()
    expect(screen.getByText(/hello streaming world/)).toBeInTheDocument()
  })

  it('流式逐帧更新只重渲 MessageList，不波及未订阅的兄弟节点', async () => {
    siblingRenders = 0
    mountList()
    const baseline = siblingRenders // 1

    act(() => setCoarse({ streaming: true }))
    // 连续多帧内容更新（模拟 RAF 每帧 setSnapshot）
    for (let i = 0; i < 5; i++) {
      act(() => setSnapshot(snapWith({ content: `frame ${i}`, streaming: true })))
    }
    await flush()
    expect(screen.getByText(/frame 4/)).toBeInTheDocument()
    // 兄弟节点渲染次数不变 —— 证明 store 把更新隔离到订阅者。
    expect(siblingRenders).toBe(baseline)
  })

  it('流式内容帧不重建历史导航索引', async () => {
    const buildNavigator = vi.spyOn(messageNavigator, 'buildMessageNavigatorNodes')
    const messages = Array.from({ length: 40 }, (_, index) => message(index))
    render(<MessageList messages={messages} conversationId="navigator-perf-c1" />)
    await flush()
    const baseline = buildNavigator.mock.calls.length

    act(() => setCoarse({ streaming: true }))
    for (let i = 0; i < 8; i++) {
      act(() => setSnapshot(snapWith({ content: `token frame ${i}`, streaming: true })))
    }
    await flush()

    expect(buildNavigator).toHaveBeenCalledTimes(baseline)
  })

  it('cancelCurrentRunLocally 等价：coarse streaming:false+frozen:true + patchSnapshot 冻结保留文本', async () => {
    mountList()
    act(() => {
      setSnapshot(snapWith({ content: 'partial answer', streaming: true }))
      setCoarse({ streaming: true })
    })
    await flush()
    expect(screen.getByText(/partial answer/)).toBeInTheDocument()

    act(() => {
      setCoarse({ streaming: false, streamFrozen: true })
      patchSnapshot({ reasoningStreaming: false })
    })
    await flush()
    // 冻结态下已生成文本仍在（streamFrozen 让预览继续渲染）。
    expect(screen.getByText(/partial answer/)).toBeInTheDocument()
    expect(getCoarse().streamFrozen).toBe(true)
  })

  it('reset（clearStreamingPreview 等价）清掉预览但保留 streamError', async () => {
    mountList()
    act(() => {
      setSnapshot(snapWith({ content: 'to be cleared', streaming: true }))
      setCoarse({ streaming: true, streamError: 'boom' })
    })
    await flush()
    expect(screen.getByText(/to be cleared/)).toBeInTheDocument()

    act(() => reset())
    await flush()
    expect(screen.queryByText(/to be cleared/)).not.toBeInTheDocument()
    // streamError 不被 reset 清除（与原 clearStreamingPreview 语义一致），错误文案仍展示。
    expect(screen.getByText('boom')).toBeInTheDocument()
  })

  it('流读取错误使用独立错误卡片并保留原始详情', async () => {
    mountList()
    act(() => setCoarse({ streamError: 'Chat stream: stream_read_error' }))
    await flush()

    expect(screen.getByText('超时 / 连接中断')).toBeInTheDocument()
    expect(screen.getByText('Chat stream: stream_read_error')).toBeInTheDocument()
  })

  it('长列表只挂载可见窗口，而不是把所有历史消息留在 DOM', async () => {
    const messages = Array.from({ length: 100 }, (_, index) => message(index))
    render(<MessageList messages={messages} conversationId="long-c1" />)
    await flush()

    const mountedMessages = document.querySelectorAll('[data-chat-message-list-item="message"]')
    expect(mountedMessages.length).toBeGreaterThan(0)
    expect(mountedMessages.length).toBeLessThan(messages.length)
  })

  it('用户翻历史时，完成消息跨重会话阈值也不切换渲染模式', async () => {
    const initialMessages = Array.from({ length: 4 }, (_, index) => message(index))
    const { container, rerender } = render(
      <MessageList messages={initialMessages} conversationId="heavy-completion-c1" />,
    )
    await flush()

    const scroller = container.querySelector('.chat-motion-view-in.custom-scrollbar') as HTMLDivElement
    Object.defineProperties(scroller, {
      scrollHeight: { configurable: true, get: () => 2400 },
      scrollTop: { configurable: true, writable: true, value: 400 },
    })
    fireEvent.wheel(scroller, { deltaY: -40 })
    await flush()
    expect(screen.getByRole('button', { name: '回到底部' })).toBeInTheDocument()

    // 80 个围栏代码块的估算成本超过 1500；旧逻辑会在这一帧切到 heavyHistory，
    // 卸载上方行并让 scrollTop 被 clamp。本次仍应保留用户正在看的首条历史。
    const heavyAnswer = Array.from({ length: 80 }, (_, index) => (
      `\`\`\`text\nblock ${index}\n\`\`\``
    )).join('\n')
    rerender(
      <MessageList
        conversationId="heavy-completion-c1"
        messages={[
          ...initialMessages,
          { id: 'heavy-answer', role: 'assistant', content: heavyAnswer, timestamp: 5 },
        ]}
      />,
    )
    await flush()

    expect(container.querySelector('[data-message-id="m-0"]')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '回到底部' })).toBeInTheDocument()
  })

  it('仍在跟随时，assistant 落库后重新钉到底部', async () => {
    const initialMessages = [{
      id: 'completion-user',
      role: 'user' as const,
      content: 'question',
      timestamp: 1,
    }]
    const { container, rerender } = render(
      <MessageList messages={initialMessages} conversationId="completion-pin-c1" />,
    )
    await flush()

    const scroller = container.querySelector('.chat-motion-view-in.custom-scrollbar') as HTMLDivElement
    let scrollTop = 120
    let writes = 0
    Object.defineProperties(scroller, {
      scrollHeight: { configurable: true, get: () => 2400 },
      scrollTop: {
        configurable: true,
        get: () => scrollTop,
        set: (value: number) => {
          scrollTop = value
          writes += 1
        },
      },
    })

    rerender(
      <MessageList
        conversationId="completion-pin-c1"
        messages={[
          ...initialMessages,
          { id: 'completion-answer', role: 'assistant', content: 'answer', timestamp: 2 },
        ]}
      />,
    )
    await flush()

    expect(writes).toBeGreaterThan(0)
    expect(scrollTop).toBe(2400)
  })

  it('流式快照更新在 ResizeObserver 未报告时仍补钉到底部', async () => {
    const originalResizeObserver = window.ResizeObserver
    window.ResizeObserver = class {
      observe() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver

    try {
      const { container } = render(
        <MessageList
          conversationId="streaming-fallback-pin-c1"
          messages={[{
            id: 'streaming-fallback-user',
            role: 'user',
            content: 'question',
            timestamp: 1,
          }]}
        />,
      )
      await flush()

      const scroller = container.querySelector('.chat-motion-view-in.custom-scrollbar') as HTMLDivElement
      let scrollTop = 0
      let writes = 0
      Object.defineProperties(scroller, {
        scrollHeight: { configurable: true, get: () => 2400 },
        clientHeight: { configurable: true, get: () => 500 },
        scrollTop: {
          configurable: true,
          get: () => scrollTop,
          set: (value: number) => {
            scrollTop = value
            writes += 1
          },
        },
      })

      act(() => {
        setSnapshot(snapWith({ content: 'streaming answer', streaming: true }))
        setCoarse({ streaming: true })
      })
      await flush()

      expect(writes).toBeGreaterThan(0)
      expect(scrollTop).toBe(2400)
    } finally {
      window.ResizeObserver = originalResizeObserver
    }
  })

  it('底部阈值内的小幅向上滚动不会闪现回到底部按钮', async () => {
    const { container } = render(
      <MessageList messages={[message(1)]} conversationId="wheel-threshold-c1" />,
    )
    await flush()

    const scroller = container.querySelector('.chat-motion-view-in.custom-scrollbar')
    expect(scroller).not.toBeNull()
    expect(screen.queryByRole('button', { name: '回到底部' })).not.toBeInTheDocument()

    fireEvent.wheel(scroller!, { deltaY: -2 })

    expect(screen.queryByRole('button', { name: '回到底部' })).not.toBeInTheDocument()
  })
})
