import { describe, expect, it } from 'vitest'
import {
  findMountedWindowStart,
  splitHistoryForVirtualization,
  VIRTUALIZE_THRESHOLD,
} from './messageListVirtualization'

function items(kinds: string[]) {
  return kinds.map((kind, i) => ({ kind, key: `${kind}-${i}` }))
}

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
    const split = splitHistoryForVirtualization(list, { minMounted: 32 })
    expect(split.useVirtual).toBe(true)
    expect(split.virtualized.length + split.mounted.length).toBe(list.length)
    expect(split.mounted.length).toBeGreaterThanOrEqual(32)
    expect(split.mountedStartIndex).toBe(split.virtualized.length)
  })

  it('冻结时新消息只让实挂载区变长，不把已有行挤进虚拟区', () => {
    const before = items(Array.from({ length: 80 }, () => 'message'))
    const frozenStart = splitHistoryForVirtualization(before, { minMounted: 32 }).mountedStartIndex
    const after = items(Array.from({ length: 100 }, () => 'message'))

    const thawed = splitHistoryForVirtualization(after, { minMounted: 32 })
    expect(thawed.mountedStartIndex).toBeGreaterThan(frozenStart) // 不冻结就会往前挪

    const frozen = splitHistoryForVirtualization(after, { minMounted: 32, frozenStart })
    expect(frozen.mountedStartIndex).toBe(frozenStart)
    expect(frozen.mounted.length).toBe(100 - frozenStart)
  })

  it('冻结期实挂载区超过上限则放弃冻结', () => {
    const list = items(Array.from({ length: 300 }, () => 'message'))
    const frozen = splitHistoryForVirtualization(list, { minMounted: 32, frozenStart: 16 })
    expect(frozen.mountedStartIndex).toBeGreaterThan(16)
  })
})
