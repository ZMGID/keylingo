import { beforeEach, describe, expect, it } from 'vitest'
import {
  canReuseLiveRowHeight,
  chatMessageLayoutRevision,
  contentRevision,
  clearRowMeasurementCache,
  estimateMessageRenderCost,
  estimateRenderCost,
  getCachedRowMeasurement,
  layoutScopedVirtualKey,
  measureChatVirtualRow,
  restoreMeasurementSnapshot,
  saveMeasurementSnapshot,
  sendReserveHeight,
  setCachedRowMeasurement,
  shouldAdjustChatItemSizeChange,
} from './messageListVirtualization'
import type { ChatMessage } from './types'

beforeEach(() => clearRowMeasurementCache())

const fence = (body = 'x') => '```ts\n' + body + '\n```\n'

function assistant(overrides: Partial<ChatMessage> = {}): ChatMessage {
  return {
    id: 'a1',
    role: 'assistant',
    content: '回答',
    timestamp: 1,
    ...overrides,
  }
}

describe('message layout revision', () => {
  it('终止状态和 usage 会切换测量 key', () => {
    const base = assistant()
    expect(chatMessageLayoutRevision(assistant({ stream_outcome: 'interrupted' })))
      .not.toBe(chatMessageLayoutRevision(base))
    expect(chatMessageLayoutRevision(assistant({ usage: { input_tokens: 10, output_tokens: 20 } })))
      .not.toBe(chatMessageLayoutRevision(base))
  })

  it('正文相同则继承 live 高度，页脚 usage/终止态交给测高补', () => {
    const live = assistant()
    expect(canReuseLiveRowHeight(live, assistant({ stream_outcome: 'interrupted' }))).toBe(true)
    expect(canReuseLiveRowHeight(live, assistant({ usage: { input_tokens: 10, output_tokens: 20 } }))).toBe(true)
    expect(canReuseLiveRowHeight(live, assistant({ content: '回答已补全' }))).toBe(false)
    expect(canReuseLiveRowHeight(live, assistant())).toBe(true)
  })

  it('有时间线分段时，顶层 content/reasoning 的拼接差异不影响 live 高度复用', () => {
    const segments: ChatMessage['segments'] = [
      { id: 's1', kind: 'reasoning', phase: 'plain', order: 1000, text: '想一想' },
      { id: 's2', kind: 'text', phase: 'plain', order: 1002, text: '回答' },
    ]
    const live = assistant({ segments, content: '回答', reasoning: '想一想' })
    // 后端落库时用 "\n\n" 拼多步文本 / 推理，与前端流式累加的字符串不同
    const settled = assistant({ segments, content: '回答\n\n', reasoning: '想一想\n\n' })
    expect(canReuseLiveRowHeight(live, settled)).toBe(true)
    // 分段本身变了才算正文变了
    const changed = assistant({
      segments: [segments![0]!, { ...segments![1]!, text: '回答已补全' }],
    })
    expect(canReuseLiveRowHeight(live, changed)).toBe(false)
  })

  it('工具状态 completed（流式）与 success（落库）视为同一几何', () => {
    const segments: ChatMessage['segments'] = [
      { id: 's1', kind: 'tool', phase: 'tool_loop', order: 1003, tool_call_id: 'c1' },
      { id: 's2', kind: 'text', phase: 'plain', order: 1007, text: '完成' },
    ]
    const live = assistant({
      segments,
      tool_calls: [{ id: 'c1', name: 'read_file', source: 'native', status: 'completed' }],
    })
    const settled = assistant({
      segments,
      tool_calls: [{ id: 'c1', name: 'read_file', source: 'native', status: 'success' }],
    })
    expect(canReuseLiveRowHeight(live, settled)).toBe(true)
    const failed = assistant({
      segments,
      tool_calls: [{ id: 'c1', name: 'read_file', source: 'native', status: 'error' }],
    })
    expect(canReuseLiveRowHeight(live, failed)).toBe(false)
  })

  it('同长度中间改写会换测量 key', () => {
    const prefix = 'x'.repeat(300)
    const suffix = 'y'.repeat(300)
    const before = assistant({ content: `${prefix}AAAA${suffix}` })
    const after = assistant({ content: `${prefix}BBBB${suffix}` })
    expect(chatMessageLayoutRevision(after)).not.toBe(chatMessageLayoutRevision(before))
  })

  it('超长正文按窗口采样：首尾和窗口内改写换 key，相同大文本稳定', () => {
    const large = 'x'.repeat(20_000)
    expect(contentRevision(large)).toBe(contentRevision(large))
    expect(contentRevision(`AAAA${large.slice(4)}`)).not.toBe(contentRevision(large))
    expect(contentRevision(`${large.slice(0, -4)}BBBB`)).not.toBe(contentRevision(large))
    const windowStart = Math.floor((16 * (large.length - 128)) / 31)
    const midEdited = `${large.slice(0, windowStart)}YYYY${large.slice(windowStart + 4)}`
    expect(contentRevision(midEdited)).not.toBe(contentRevision(large))
  })
})

describe('virtual row measurement', () => {
  it('同步挂载时读取真实 DOM 高度，不复用旧缓存高度', () => {
    const element = { offsetHeight: 252, offsetWidth: 640 } as HTMLElement
    expect(measureChatVirtualRow(element, undefined)).toBe(252)
  })

  it('ResizeObserver 投递时使用 border-box 高度', () => {
    const element = { offsetHeight: 214, offsetWidth: 640 } as HTMLElement
    const entry = {
      borderBoxSize: [{ blockSize: 252, inlineSize: 640 }],
    } as unknown as ResizeObserverEntry
    expect(measureChatVirtualRow(element, entry)).toBe(252)
  })
})

describe('estimateRenderCost', () => {
  it('代码围栏比同样长度的散文贵得多', () => {
    const prose = 'a'.repeat(300)
    expect(estimateRenderCost(fence())).toBeGreaterThan(estimateRenderCost(prose))
  })

  it('成本随围栏数增长（成本由块数驱动，不是字数）', () => {
    const one = estimateRenderCost(fence())
    const ten = estimateRenderCost(fence().repeat(10))
    expect(ten).toBeGreaterThan(one * 8)
  })

  it('正文里内联的 ``` 不算围栏（只认行首）', () => {
    expect(estimateRenderCost('看这个 ``` 符号')).toBeLessThan(5)
  })

  it('空串是 0', () => {
    expect(estimateRenderCost('')).toBe(0)
  })
})

describe('estimateMessageRenderCost', () => {
  it('折叠工具详情虽不挂 DOM，工具记录和时间线处理仍计入消息成本', () => {
    const textOnly = estimateMessageRenderCost({ texts: ['简短回答'] })
    const toolHeavy = estimateMessageRenderCost({
      texts: ['简短回答'],
      toolCallCount: 20,
      timelineSegmentCount: 30,
    })
    expect(toolHeavy).toBeGreaterThan(textOnly + 150)
  })

  it('大量工具记录会增加估算成本', () => {
    const textOnly = estimateMessageRenderCost({ texts: ['a'.repeat(150_000)] })
    const withTools = estimateMessageRenderCost({
      texts: ['a'.repeat(150_000)],
      toolCallCount: 80,
      timelineSegmentCount: 80,
    })
    expect(withTools).toBeGreaterThan(textOnly + 150)
  })
})

describe('row measurement cache', () => {
  it('uses different TanStack key spaces for different content widths', () => {
    expect(layoutScopedVirtualKey('c1:640', 'm1')).not.toBe(layoutScopedVirtualKey('c1:960', 'm1'))
  })

  it('按布局 key 隔离同一行在不同宽度下的高度', () => {
    setCachedRowMeasurement('c1:640', 'm1', 240)
    setCachedRowMeasurement('c1:960', 'm1', 160)
    expect(getCachedRowMeasurement('c1:640', 'm1')).toBe(240)
    expect(getCachedRowMeasurement('c1:960', 'm1')).toBe(160)
  })

  it('忽略无效高度，避免污染下一次切换的估算', () => {
    setCachedRowMeasurement('c1:640', 'm1', 0)
    setCachedRowMeasurement('c1:640', 'm2', Number.NaN)
    expect(getCachedRowMeasurement('c1:640', 'm1')).toBeUndefined()
    expect(getCachedRowMeasurement('c1:640', 'm2')).toBeUndefined()
  })

  it('按会话、布局和内容 revision 恢复 measured snapshot', () => {
    saveMeasurementSnapshot('c1', 'viewport:640', 'rev-a', [{
      index: 2,
      key: 'm2',
      start: 180,
      size: 420,
      end: 600,
      lane: 0,
    }])
    expect(restoreMeasurementSnapshot('c1', 'viewport:640', 'rev-a')).toHaveLength(1)
    expect(restoreMeasurementSnapshot('c1', 'viewport:960', 'rev-a')).toHaveLength(0)
    expect(restoreMeasurementSnapshot('c1', 'viewport:640', 'rev-b')).toHaveLength(0)
  })
})

describe('row resize anchoring', () => {
  const base = {
    scrollOffset: 500,
    scrollAdjustments: 0,
    itemSizeCache: new Map<string, number>([['measured', 100]]),
    scrollDirection: null as 'forward' | 'backward' | null,
  }

  it('adjusts rows that start above the reading anchor (LiveAgent / TanStack default)', () => {
    expect(shouldAdjustChatItemSizeChange(
      { key: 'measured', start: 100, end: 200 },
      base,
    )).toBe(true)
    // 跨过锚点但仍从上方开始：默认补偿，live 行特例由 MessageList 再裁。
    expect(shouldAdjustChatItemSizeChange(
      { key: 'measured', start: 450, end: 550 },
      base,
    )).toBe(true)
    expect(shouldAdjustChatItemSizeChange(
      { key: 'measured', start: 550, end: 650 },
      base,
    )).toBe(false)
  })

  it('does not compensate a measured row while scrolling backward', () => {
    expect(shouldAdjustChatItemSizeChange(
      { key: 'measured', start: 100, end: 200 },
      { ...base, scrollDirection: 'backward' },
    )).toBe(false)
  })

  it('compensates an unmeasured row above the anchor once (estimate→actual)', () => {
    expect(shouldAdjustChatItemSizeChange(
      { key: 'unmeasured', start: 300, end: 700 },
      base,
    )).toBe(true)
    // 首次测量即使 backward 也要补偿（上游「上滚时条目跳动」修复）。
    expect(shouldAdjustChatItemSizeChange(
      { key: 'unmeasured', start: 100, end: 200 },
      { ...base, scrollDirection: 'backward' },
    )).toBe(true)
  })
})

describe('sendReserveHeight', () => {
  it('视口够高时就是比例值', () => {
    expect(sendReserveHeight(800, 40, 16)).toBe(360)
  })

  it('视口被 ask_user 面板挤矮 / 锚点行很高时，夹到「锚点仍在屏幕里」', () => {
    // 视口只剩 300、锚点行 200：比例值 135 会把锚点顶出去，必须夹到 300-200-32=68
    expect(sendReserveHeight(300, 200, 16)).toBe(68)
  })

  it('锚点自己就超过视口时不给负数', () => {
    expect(sendReserveHeight(300, 400, 16)).toBe(0)
  })
})
