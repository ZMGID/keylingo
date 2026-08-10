import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  beginGroup,
  endGroup,
  ensureGroupColumn,
  flushGroups,
  getActiveGroup,
  getGroupVersion,
  getGroupsVersion,
  hasActiveGroup,
  resetGroups,
  restoreGroupArm,
  subscribeGroup,
  subscribeGroups,
  touchGroup,
} from './groupStreamingStore'

afterEach(() => {
  resetGroups()
  vi.restoreAllMocks()
})

describe('groupStreamingStore', () => {
  it('beginGroup 建出 N 个占位列并登记会话', () => {
    beginGroup('c1', 'g1', [
      { providerId: 'p1', model: 'm1' },
      { providerId: 'p2', model: 'm2' },
    ])
    expect(hasActiveGroup('c1')).toBe(true)
    const group = getActiveGroup('c1')
    expect(group?.groupId).toBe('g1')
    expect(group?.expectedColumns).toBe(2)
    expect(group?.columns).toHaveLength(2)
    expect(group?.columns[0].providerId).toBe('p1')
    expect(group?.columns[1].model).toBe('m2')
  })

  it('ensureGroupColumn 第一次见到 messageId 时认领占位列、绑定真实 id', () => {
    beginGroup('c1', 'g1', [
      { providerId: 'p1', model: 'm1' },
      { providerId: 'p2', model: 'm2' },
    ])
    const colA = ensureGroupColumn('c1', 'msg_a')
    const colB = ensureGroupColumn('c1', 'msg_b')
    expect(colA?.messageId).toBe('msg_a')
    expect(colB?.messageId).toBe('msg_b')
    // 两次认领的是不同的占位列。
    expect(colA?.providerId).toBe('p1')
    expect(colB?.providerId).toBe('p2')
    // 再次以同 id 取回同一列（按 messageId 聚合，同一会话多条流并存）。
    const colAagain = ensureGroupColumn('c1', 'msg_a')
    expect(colAagain).toBe(colA)
  })

  it('多条流靠 messageId 区分、各自累积，互不串', () => {
    beginGroup('c1', 'g1', [
      { providerId: 'p1', model: 'm1' },
      { providerId: 'p2', model: 'm2' },
    ])
    const a = ensureGroupColumn('c1', 'msg_a')!
    const b = ensureGroupColumn('c1', 'msg_b')!
    a.content += 'hello from A'
    b.content += 'hi from B'
    expect(getActiveGroup('c1')?.columns.find((c) => c.messageId === 'msg_a')?.content).toBe('hello from A')
    expect(getActiveGroup('c1')?.columns.find((c) => c.messageId === 'msg_b')?.content).toBe('hi from B')
  })

  it('未知会话的 ensureGroupColumn 返回 null（单模型路径不受影响）', () => {
    expect(ensureGroupColumn('no-group', 'msg_x')).toBeNull()
  })

  it('endGroup 清掉活跃组', () => {
    beginGroup('c1', 'g1', [{ providerId: 'p1', model: 'm1' }])
    endGroup('c1')
    expect(hasActiveGroup('c1')).toBe(false)
    expect(getActiveGroup('c1')).toBeUndefined()
  })

  it('touchGroup 合帧：N 个 delta 只通知一次（性能）；flushGroups 立即 flush', () => {
    // 用假的 rAF 控制何时执行合帧回调。
    const rafCallbacks: FrameRequestCallback[] = []
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      rafCallbacks.push(cb)
      return rafCallbacks.length
    })
    vi.stubGlobal('cancelAnimationFrame', () => {})

    beginGroup('c1', 'g1', [{ providerId: 'p1', model: 'm1' }])
    const col = ensureGroupColumn('c1', 'msg_a')!
    const sub = vi.fn()
    const unsub = subscribeGroups(sub)
    const versionBefore = getGroupsVersion()

    // 多个 delta：内容即时累积，但只调度一帧（不立即通知）。
    col.content += 'a'
    touchGroup()
    col.content += 'b'
    touchGroup()
    col.content += 'c'
    touchGroup()
    expect(sub).not.toHaveBeenCalled()
    expect(getGroupsVersion()).toBe(versionBefore)

    // 执行合帧帧：只通知一次。
    rafCallbacks.forEach((cb) => cb(0))
    expect(sub).toHaveBeenCalledTimes(1)
    expect(getActiveGroup('c1')?.columns[0].content).toBe('abc')

    // flushGroups 立即 flush 待合帧的更新。
    rafCallbacks.length = 0
    col.content += 'd'
    touchGroup()
    flushGroups()
    expect(sub).toHaveBeenCalledTimes(2)

    unsub()
  })

  it('only notifies subscribers for the conversation whose stream changed', () => {
    const rafCallbacks: FrameRequestCallback[] = []
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      rafCallbacks.push(cb)
      return rafCallbacks.length
    })
    vi.stubGlobal('cancelAnimationFrame', () => {})

    beginGroup('c1', 'g1', [{ providerId: 'p1', model: 'm1' }])
    beginGroup('c2', 'g2', [{ providerId: 'p2', model: 'm2' }])
    const c1Subscriber = vi.fn()
    const c2Subscriber = vi.fn()
    const unsubscribeC1 = subscribeGroup('c1', c1Subscriber)
    const unsubscribeC2 = subscribeGroup('c2', c2Subscriber)
    const c1Version = getGroupVersion('c1')
    const c2Version = getGroupVersion('c2')

    ensureGroupColumn('c1', 'msg-a')!.content += 'delta'
    touchGroup('c1')
    rafCallbacks.splice(0).forEach((callback) => callback(0))

    expect(c1Subscriber).toHaveBeenCalledTimes(1)
    expect(c2Subscriber).not.toHaveBeenCalled()
    expect(getGroupVersion('c1')).toBe(c1Version + 1)
    expect(getGroupVersion('c2')).toBe(c2Version)

    unsubscribeC1()
    unsubscribeC2()
  })

  it('restores fan-out arms by recovery index when snapshots arrive out of order', () => {
    const second = restoreGroupArm('c1', 'recovered-group', 3, 1, 'msg-b', 'provider-b', 'model-b')
    const first = restoreGroupArm('c1', 'recovered-group', 3, 0, 'msg-a', 'provider-a', 'model-a')

    const group = getActiveGroup('c1')
    expect(group).toMatchObject({
      conversationId: 'c1',
      groupId: 'recovered-group',
      expectedColumns: 3,
    })
    expect(group?.columns).toHaveLength(3)
    expect(group?.columns[0]).toBe(first)
    expect(group?.columns[1]).toBe(second)
    expect(group?.columns[0]).toMatchObject({
      messageId: 'msg-a',
      providerId: 'provider-a',
      model: 'model-a',
    })
    expect(group?.columns[1]).toMatchObject({
      messageId: 'msg-b',
      providerId: 'provider-b',
      model: 'model-b',
    })
    expect(group?.columns[2]).toMatchObject({
      messageId: 'pending-recovered-group-2',
      providerId: null,
      model: null,
    })
  })

  it('restores the same arm idempotently without replacing sibling state', () => {
    const first = restoreGroupArm('c1', 'recovered-group', 2, 0, 'msg-a', 'provider-a', 'model-a')
    const second = restoreGroupArm('c1', 'recovered-group', 2, 1, 'msg-b', 'provider-b', 'model-b')
    first.content = 'partial answer'
    second.content = 'sibling answer'

    const restoredAgain = restoreGroupArm(
      'c1',
      'recovered-group',
      2,
      0,
      'msg-a-new',
      'provider-a-new',
      'model-a-new',
    )

    expect(restoredAgain).toBe(first)
    expect(restoredAgain).toMatchObject({
      messageId: 'msg-a-new',
      providerId: 'provider-a-new',
      model: 'model-a-new',
      content: 'partial answer',
    })
    expect(getActiveGroup('c1')?.columns[1]).toBe(second)
    expect(getActiveGroup('c1')?.columns[1]).toMatchObject({
      messageId: 'msg-b',
      content: 'sibling answer',
    })
  })

  it('replaces a stale group when recovery metadata names a new group', () => {
    beginGroup('c1', 'stale-group', [{ providerId: 'old-provider', model: 'old-model' }])

    restoreGroupArm('c1', 'recovered-group', 2, 1, 'msg-b', 'provider-b', 'model-b')

    const group = getActiveGroup('c1')
    expect(group?.groupId).toBe('recovered-group')
    expect(group?.expectedColumns).toBe(2)
    expect(group?.columns).toHaveLength(2)
    expect(group?.columns[0].messageId).toBe('pending-recovered-group-0')
    expect(group?.columns[1].messageId).toBe('msg-b')
    expect(group?.columns.some((column) => column.providerId === 'old-provider')).toBe(false)
  })
})
