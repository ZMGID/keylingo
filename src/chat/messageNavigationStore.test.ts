import { afterEach, describe, expect, it } from 'vitest'
import {
  beginMessageNavigationHydrate,
  endMessageNavigationHydrate,
  isMessageNavigationEagerHydrate,
  resetMessageNavigationStore,
} from './messageNavigationStore'

afterEach(() => {
  resetMessageNavigationStore()
})

describe('messageNavigationStore', () => {
  it('begin enables eager hydrate and end only clears matching generation', () => {
    expect(isMessageNavigationEagerHydrate()).toBe(false)

    const first = beginMessageNavigationHydrate()
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    const second = beginMessageNavigationHydrate()
    expect(second).not.toBe(first)
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    // 旧 generation 不能关掉新一轮 settle。
    endMessageNavigationHydrate(first)
    expect(isMessageNavigationEagerHydrate()).toBe(true)

    endMessageNavigationHydrate(second)
    expect(isMessageNavigationEagerHydrate()).toBe(false)
  })

  it('reset clears eager regardless of generation', () => {
    beginMessageNavigationHydrate()
    expect(isMessageNavigationEagerHydrate()).toBe(true)
    resetMessageNavigationStore()
    expect(isMessageNavigationEagerHydrate()).toBe(false)
  })
})
