import { describe, expect, it } from 'vitest'
import {
  createFollowState,
  reduceFollowEvent,
  type FollowEvent,
  type FollowState,
} from './scrollFollowCore'

function run(events: FollowEvent[], start: FollowState = createFollowState()) {
  let state = start
  let pin = false
  for (const event of events) {
    const step = reduceFollowEvent(state, event)
    state = step.state
    pin = step.pin
  }
  return { state, pin }
}

const wheelUp = (over: Partial<Extract<FollowEvent, { type: 'wheel' }>> = {}): FollowEvent => ({
  type: 'wheel',
  deltaX: 0,
  deltaY: -40,
  gap: 0,
  hasOverflow: true,
  nestedCanConsume: false,
  now: 0,
  ...over,
})
// 默认 self：绝大多数测试关心的是「我们自己钉底/内容增长」这条路径。
const scroll = (gap: number, now = 0, source: 'self' | 'user' = 'self'): FollowEvent =>
  ({ type: 'scroll', gap, now, source })
const growth = (gap: number): FollowEvent => ({ type: 'contentGrowth', gap })

describe('scrollFollowCore', () => {
  it('内容增长在跟随中钉底，且不改变跟随状态', () => {
    const { state, pin } = run([growth(500)])
    expect(pin).toBe(true)
    expect(state.following).toBe(true)
  })

  it('自己钉出来的显著 gap 再钉一次纠正，永不解除跟随', () => {
    // 跟随中 self 来源的 gap（钉底写入后高度又变了）→ 应 pin 纠正，仍保持 following。
    const { state, pin } = run([scroll(120, 100)])
    expect(state.following).toBe(true)
    expect(pin).toBe(true)
  })

  it('跟随中的小 gap（virtua/DPR 测量噪声）不 pin，避免底部抽搐', () => {
    const { state, pin } = run([scroll(12, 100)])
    expect(state.following).toBe(true)
    expect(pin).toBe(false)
  })

  // 这条是本模块最容易回归的行为：拖原生滚动条 / 页内查找 / iframe 滚动链都拿不到 wheel，
  // 只会送来一个 user 来源的 scroll。若继续钉底就会和外部反复互写 scrollTop（抽搐、拖不动）。
  it('外部把视口拉离底部（user 来源）解除跟随而不是钉回去', () => {
    const { state, pin } = run([scroll(120, 100, 'user')])
    expect(state.following).toBe(false)
    expect(pin).toBe(false)
  })

  it('user 来源但 gap 在纠正阈值内 → 也解除（避免慢速拖滚动条的死带）', () => {
    // isAtBottom(12) 已经把底部容差筛掉了，走到这里就是真离开了底部。
    // 若在这里再叠 32px 门槛，慢速拖原生滚动条每帧只挪几 px，会一直被 contentGrowth 钉回去。
    const { state, pin } = run([scroll(20, 100, 'user')])
    expect(state.following).toBe(false)
    expect(pin).toBe(false)
  })

  it('解除后滚回底部（user 来源）自动重新跟随', () => {
    const detached = run([scroll(300, 0, 'user')]).state
    expect(detached.following).toBe(false)
    const { state } = run([scroll(2, 50, 'user')], detached)
    expect(state.following).toBe(true)
  })

  // 以下三条是子代理审查发现的卡死路径，各自兜一条「跟随再也回不来」的死路。
  it('松手时若已贴底则补接跟随（拖选文本回到底部后不再有 scroll 事件）', () => {
    const { state, pin } = run([{ type: 'pointerRelease', gap: 4 }], {
      ...createFollowState(),
      following: false,
      pointerHeld: true,
      lastGap: 4,
    })
    expect(state.following).toBe(true)
    expect(pin).toBe(true)
  })

  it('松手时没贴底则不接', () => {
    const { state } = run([{ type: 'pointerRelease', gap: 300 }], {
      ...createFollowState(),
      following: false,
      pointerHeld: true,
      lastGap: 300,
    })
    expect(state.following).toBe(false)
  })

  it('contentGrowth 自愈：视口已贴底却没在跟随就接回来', () => {
    // 用户滚回底部的那个 scroll 落在 resize 窗口里被记成 self，或者贴底后不再产生
    // scroll 事件时，这是唯一的重跟随入口。
    const { state, pin } = run([growth(3)], { ...createFollowState(), following: false })
    expect(state.following).toBe(true)
    expect(pin).toBe(true)
  })

  it('contentGrowth 自愈不误伤在上方看历史的读者', () => {
    const { state, pin } = run([growth(800)], { ...createFollowState(), following: false })
    expect(state.following).toBe(false)
    expect(pin).toBe(false)
  })

  it('contentGrowth 自愈在按住指针时不触发', () => {
    const { state } = run([growth(3)], {
      ...createFollowState(),
      following: false,
      pointerHeld: true,
    })
    expect(state.following).toBe(false)
  })

  it('向下滚但未贴底时不重跟随（避免 192px 硬拽）', () => {
    const detached = run([wheelUp({ gap: 400, now: 0 })]).state
    const { state, pin } = run([scroll(100, 80)], {
      ...detached,
      latchUntil: 600,
      following: false,
      lastGap: 150,
    })
    expect(state.following).toBe(false)
    expect(pin).toBe(false)
  })

  it('向下滚且贴底时重跟随但不 pin（由 contentGrowth 钉）', () => {
    const detached = run([wheelUp({ gap: 400, now: 0 })]).state
    const { state, pin } = run([scroll(4, 80)], {
      ...detached,
      latchUntil: 600,
      following: false,
      lastGap: 40,
    })
    // isAtBottom(4) → following；scroll 路径不 pin
    expect(state.following).toBe(true)
    expect(pin).toBe(false)
  })

  it('按住指针（拖选文本）期间不自动重跟随', () => {
    const { state } = run([scroll(4, 80)], {
      ...createFollowState(),
      following: false,
      pointerHeld: true,
      lastGap: 40,
    })
    expect(state.following).toBe(false)
  })

  it('滚轮上滚（有溢出）是明确的用户离开意图，解除跟随', () => {
    const { state } = run([wheelUp({ gap: 300 })])
    expect(state.following).toBe(false)
  })

  it('慢速上滚经过底部容差区时不会被 scroll 事件重新接回', () => {
    const detached = run([wheelUp({ gap: 4 })]).state
    const { state } = run([scroll(4, 1, 'user')], detached)
    expect(state.following).toBe(false)
  })

  it('解除后滚轮下滚到底部重新跟随', () => {
    const detached = run([wheelUp({ gap: 300 })]).state
    // 滚轮下滚且已在底部 → 重新跟随并钉底。
    const { state, pin } = run([wheelUp({ deltaY: 40, gap: 0, now: 200 })], detached)
    expect(state.following).toBe(true)
    expect(pin).toBe(true)
  })

  it('self 来源滚到底部不自动重跟随（防内容收缩钉底误触发）', () => {
    const detached = run([wheelUp({ gap: 300 })]).state
    const { state } = run([scroll(0, 200)], detached)
    expect(state.following).toBe(false)
  })

  it('release 事件主动脱离跟随且不钉底（导航跳转用）', () => {
    const { state, pin } = run([{ type: 'release' }])
    expect(state.following).toBe(false)
    expect(pin).toBe(false)
  })

  it('release 后内容增长不会把读者拽回底部', () => {
    const released = run([{ type: 'release' }]).state
    const { pin } = run([growth(800)], released)
    expect(pin).toBe(false)
  })
})
