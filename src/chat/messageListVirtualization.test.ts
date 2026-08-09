import { beforeEach, describe, expect, it } from 'vitest'
import {
  earlierBatchStart,
  estimateMessageRenderCost,
  estimateRenderCost,
  findMountedWindowStart,
  clearRowMeasurementCache,
  getCachedRowMeasurement,
  HEAVY_MIGRATION_STEP,
  MOUNTED_MIN_ITEMS,
  mountedCountForBudget,
  sendReserveHeight,
  splitHistoryForVirtualization,
  VIRTUALIZE_COST_THRESHOLD,
  VIRTUALIZE_THRESHOLD,
  setCachedRowMeasurement,
} from './messageListVirtualization'

beforeEach(() => clearRowMeasurementCache())

function items(kinds: string[]) {
  return kinds.map((kind, i) => ({ kind, key: `${kind}-${i}` }))
}

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

  it('正文单独低于重会话门槛时，大量工具记录能推动它触发渐进加载', () => {
    const textOnly = estimateMessageRenderCost({ texts: ['a'.repeat(150_000)] })
    const withTools = estimateMessageRenderCost({
      texts: ['a'.repeat(150_000)],
      toolCallCount: 80,
      timelineSegmentCount: 80,
    })
    expect(textOnly).toBeLessThan(VIRTUALIZE_COST_THRESHOLD)
    expect(withTools).toBeGreaterThan(VIRTUALIZE_COST_THRESHOLD)
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
})

describe('mountedCountForBudget', () => {
  it('累到预算就停', () => {
    expect(mountedCountForBudget([100, 100, 100, 900], 800)).toBe(MOUNTED_MIN_ITEMS)
    expect(mountedCountForBudget([500, 300, 300, 300], 800)).toBe(3)
  })

  it('内容太轻时返回全部（等于不虚拟化）', () => {
    expect(mountedCountForBudget([1, 1, 1, 1, 1], 800)).toBe(5)
  })

  it('至少留 MOUNTED_MIN_ITEMS 条，视口要填满', () => {
    expect(mountedCountForBudget([5000, 5000, 5000, 5000], 800)).toBe(MOUNTED_MIN_ITEMS)
  })
})

describe('earlierBatchStart', () => {
  it('到顶了就返回 0', () => {
    expect(earlierBatchStart([1, 2, 3], 0)).toBe(0)
  })

  it('至少揭示一条，哪怕它自己就超预算（否则滚到顶会卡住）', () => {
    expect(earlierBatchStart([100, 100, 5000, 100], 3, 800)).toBe(2)
  })

  it('不超预算就继续往前吃', () => {
    expect(earlierBatchStart([100, 100, 100, 100, 100], 5, 800)).toBe(0)
    expect(earlierBatchStart([100, 100, 700, 100, 100], 5, 800)).toBe(3)
  })

  it('每次至少前进一格，不会原地不动', () => {
    let from = 6
    const costs = [900, 900, 900, 900, 900, 900]
    for (let i = 0; i < 6; i += 1) {
      const next = earlierBatchStart(costs, from, 800)
      expect(next).toBeLessThan(from)
      from = next
    }
    expect(from).toBe(0)
  })
})

describe('findMountedWindowStart', () => {
  it('短列表从 0 开始', () => {
    expect(findMountedWindowStart(items(['message', 'message']), 32)).toBe(0)
  })

  it('长列表从末尾保留 minMounted，并落在 message 边界', () => {
    const list = items([
      ...Array.from({ length: 40 }, () => 'message'),
      'spacer',
      'message',
      'message',
    ])
    const start = findMountedWindowStart(list, 5)
    expect(start).toBeLessThanOrEqual(list.length - 5)
    expect(list[start]?.kind).toBe('message')
  })

  // 回归：重会话条数很少（14 条消息 ≈ 16 个 render item），按默认步长 16 量化会把边界
  // 压回 0 —— 边界回到 0 等于没虚拟化，成本预算白算。这就是 migrationStep 存在的理由。
  it('短列表下默认步长会把边界压回 0，小步长才留得住实挂载窗口', () => {
    const list = items(Array.from({ length: 16 }, () => 'message'))
    expect(findMountedWindowStart(list, 3)).toBe(0)
    expect(findMountedWindowStart(list, 3, HEAVY_MIGRATION_STEP)).toBeGreaterThan(0)
  })
})

describe('splitHistoryForVirtualization', () => {
  it('低于阈值时全部实挂载', () => {
    const list = items(Array.from({ length: 10 }, () => 'message'))
    const split = splitHistoryForVirtualization(list)
    expect(split.useVirtual).toBe(false)
    expect(split.virtualized).toHaveLength(0)
    expect(split.mounted).toHaveLength(10)
  })

  it('超过阈值时拆成上方虚拟 + 底部实挂载', () => {
    const list = items(Array.from({ length: VIRTUALIZE_THRESHOLD + 20 }, () => 'message'))
    const split = splitHistoryForVirtualization(list, { minMounted: 24 })
    expect(split.useVirtual).toBe(true)
    expect(split.virtualized.length + split.mounted.length).toBe(list.length)
    expect(split.mounted.length).toBeGreaterThanOrEqual(24)
    expect(split.mountedStartIndex).toBe(split.virtualized.length)
  })

  // 重会话旁路：条数远低于 36，但成本判定要虚拟化 → threshold 让位、尾部按预算、步长换小。
  it('重会话：条数不到 36 也能虚拟化，且只实挂载尾部少数几条', () => {
    const list = items(Array.from({ length: 16 }, () => 'message'))
    const split = splitHistoryForVirtualization(list, {
      threshold: 0,
      minMounted: MOUNTED_MIN_ITEMS,
      migrationStep: HEAVY_MIGRATION_STEP,
    })
    expect(split.useVirtual).toBe(true)
    expect(split.mounted.length).toBeLessThan(8)
    expect(split.virtualized.length + split.mounted.length).toBe(16)
  })

  it('冻结时新消息只让实挂载区变长，不把已有行挤进虚拟区', () => {
    const before = items(Array.from({ length: 80 }, () => 'message'))
    const frozenStart = splitHistoryForVirtualization(before, { minMounted: 24 }).mountedStartIndex
    const after = items(Array.from({ length: 100 }, () => 'message'))

    const thawed = splitHistoryForVirtualization(after, { minMounted: 24 })
    expect(thawed.mountedStartIndex).toBeGreaterThan(frozenStart) // 不冻结就会往前挪

    const frozen = splitHistoryForVirtualization(after, { minMounted: 24, frozenStart })
    expect(frozen.mountedStartIndex).toBe(frozenStart)
    expect(frozen.mounted.length).toBe(100 - frozenStart)
  })

  it('冻结期实挂载区超过上限则放弃冻结', () => {
    const list = items(Array.from({ length: 300 }, () => 'message'))
    const frozen = splitHistoryForVirtualization(list, { minMounted: 24, frozenStart: 16 })
    expect(frozen.mountedStartIndex).toBeGreaterThan(16)
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
