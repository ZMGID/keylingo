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
const scroll = (gap: number, now = 0): FollowEvent => ({ type: 'scroll', gap, now })
const growth = (gap: number): FollowEvent => ({ type: 'contentGrowth', gap })

describe('scrollFollowCore', () => {
  it('内容增长在跟随中钉底，且不改变跟随状态', () => {
    const { state, pin } = run([growth(500)])
    expect(pin).toBe(true)
    expect(state.following).toBe(true)
  })

  it('裸滚动帧显著 gap 再钉一次纠正，永不解除跟随', () => {
    // 跟随中出现一个撑开的 gap（非用户手势）→ 应 pin 纠正，仍保持 following。
    const { state, pin } = run([scroll(120, 100)])
    expect(state.following).toBe(true)
    expect(pin).toBe(true)
  })

  it('跟随中的小 gap（virtua/DPR 测量噪声）不 pin，避免底部抽搐', () => {
    const { state, pin } = run([scroll(12, 100)])
    expect(state.following).toBe(true)
    expect(pin).toBe(false)
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

  it('拖滚动条到底部附近松手 → 恢复跟随（release 区仍是 192，不受滚动路径收紧影响）', () => {
    const { state, pin } = run([{ type: 'pointerRelease', gap: 50 }], {
      ...createFollowState(),
      following: false,
      pointerHeld: true,
      pointerDragging: true,
      dragTowardBottom: true,
      lastGap: 300,
    })
    expect(state.following).toBe(true)
    expect(pin).toBe(true)
  })

  it('滚轮上滚（有溢出）是明确的用户离开意图，解除跟随', () => {
    const { state } = run([wheelUp({ gap: 300 })])
    expect(state.following).toBe(false)
  })

  it('解除后滚轮下滚到底部重新跟随', () => {
    const detached = run([wheelUp({ gap: 300 })]).state
    // 滚轮下滚且已在底部 → 重新跟随并钉底。
    const { state, pin } = run([wheelUp({ deltaY: 40, gap: 0, now: 200 })], detached)
    expect(state.following).toBe(true)
    expect(pin).toBe(true)
  })

  it('裸滚动到底部不自动重跟随（防内容收缩钉底误触发）', () => {
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
