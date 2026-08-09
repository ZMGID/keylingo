import { beforeEach, describe, expect, it } from 'vitest'
import {
  clearRowMeasurementCache,
  estimateMessageRenderCost,
  estimateRenderCost,
  getCachedRowMeasurement,
  restoreMeasurementSnapshot,
  saveMeasurementSnapshot,
  sendReserveHeight,
  setCachedRowMeasurement,
  shouldAdjustChatItemSizeChange,
} from './messageListVirtualization'

beforeEach(() => clearRowMeasurementCache())

const fence = (body = 'x') => '```ts\n' + body + '\n```\n'

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

  it('adjusts a measured row only when it is entirely above the reading anchor', () => {
    expect(shouldAdjustChatItemSizeChange(
      { key: 'measured', start: 100, end: 200 },
      base,
    )).toBe(true)
    expect(shouldAdjustChatItemSizeChange(
      { key: 'measured', start: 450, end: 550 },
      base,
    )).toBe(false)
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

  it('compensates an unmeasured row above the anchor once', () => {
    expect(shouldAdjustChatItemSizeChange(
      { key: 'unmeasured', start: 300, end: 700 },
      base,
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
