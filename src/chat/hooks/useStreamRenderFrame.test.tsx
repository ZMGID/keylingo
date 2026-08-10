import { renderHook, act } from '@testing-library/react'
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { useRef } from 'react'
import { useStreamRenderFrame } from './useStreamRenderFrame'
import type { ConversationStreamSnapshot } from '../conversationRuns'

/**
 * 回归重点（合帧是流式渲染的性能关键路径，改错会卡顿或串会话）：
 *   1. 同一帧内多次 show 只渲染一次（节流本身）
 *   2. immediate=true 立即刷，不等下一帧（done 帧不能丢）
 *   3. 快照不属于当前会话时一律不渲染（防串会话）
 *   4. cancel 后挂起帧不再刷出
 */
function snap(content: string): ConversationStreamSnapshot {
  return { content, reasoning: '', streaming: true } as ConversationStreamSnapshot
}

function setup(initialId: string | null = 'c1') {
  const applySnapshot = vi.fn()
  const rendered = renderHook(() => {
    const currentConversationIdRef = useRef<string | null>(initialId)
    const frame = useStreamRenderFrame({ applySnapshot, currentConversationIdRef })
    return { frame, currentConversationIdRef }
  })
  return { ...rendered, applySnapshot }
}

let rafQueue: FrameRequestCallback[] = []

beforeEach(() => {
  rafQueue = []
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
    rafQueue.push(cb)
    return rafQueue.length
  })
  vi.stubGlobal('cancelAnimationFrame', (handle: number) => {
    rafQueue[handle - 1] = () => {}
  })
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

function runFrame() {
  const queued = rafQueue
  rafQueue = []
  act(() => { queued.forEach((cb) => cb(0)) })
}

describe('useStreamRenderFrame 节流', () => {
  it('同一帧内多次 show 只渲染最后一份', () => {
    const { result, applySnapshot } = setup()
    act(() => {
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('a'))
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('ab'))
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('abc'))
    })
    expect(applySnapshot).not.toHaveBeenCalled()
    runFrame()
    expect(applySnapshot).toHaveBeenCalledTimes(1)
    expect(applySnapshot.mock.calls[0][0].content).toBe('abc')
  })

  it('同一帧内多次 show 只排一次 rAF（节流本身）', () => {
    const { result } = setup()
    act(() => {
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('a'))
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('ab'))
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('abc'))
    })
    // 若丢掉 rafRef 判断，这里会排 3 帧 —— 长回复下就是每 token 一帧的开销
    expect(rafQueue).toHaveLength(1)
  })

  it('跨帧的两次 show 各渲染一次', () => {
    const { result, applySnapshot } = setup()
    act(() => { result.current.frame.showStreamSnapshotIfCurrent('c1', snap('a')) })
    runFrame()
    act(() => { result.current.frame.showStreamSnapshotIfCurrent('c1', snap('ab')) })
    runFrame()
    expect(applySnapshot).toHaveBeenCalledTimes(2)
  })
})

describe('useStreamRenderFrame immediate', () => {
  it('immediate=true 立即渲染，不等下一帧', () => {
    const { result, applySnapshot } = setup()
    act(() => { result.current.frame.showStreamSnapshotIfCurrent('c1', snap('done'), true) })
    expect(applySnapshot).toHaveBeenCalledTimes(1)
    expect(applySnapshot.mock.calls[0][0].content).toBe('done')
  })

  it('immediate 会顶掉同帧内已挂起的普通帧（只渲染一次）', () => {
    const { result, applySnapshot } = setup()
    act(() => {
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('partial'))
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('final'), true)
    })
    expect(applySnapshot).toHaveBeenCalledTimes(1)
    expect(applySnapshot.mock.calls[0][0].content).toBe('final')
    runFrame()
    expect(applySnapshot).toHaveBeenCalledTimes(1)
  })
})

describe('useStreamRenderFrame 会话隔离', () => {
  it('快照不属于当前会话时不入队', () => {
    const { result, applySnapshot } = setup('c1')
    act(() => { result.current.frame.showStreamSnapshotIfCurrent('c2', snap('other')) })
    runFrame()
    expect(applySnapshot).not.toHaveBeenCalled()
  })

  it('非当前会话的快照根本不入队（不占用 rAF 槽位）', () => {
    const { result } = setup('c1')
    act(() => { result.current.frame.showStreamSnapshotIfCurrent('c2', snap('other')) })
    // 若丢掉入队校验，这里会排一帧，且会顶掉当前会话真正待渲染的内容
    expect(rafQueue).toHaveLength(0)
  })

  it('后台会话的帧不覆盖当前会话已挂起的帧', () => {
    const { result, applySnapshot } = setup('c1')
    act(() => {
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('mine'))
      result.current.frame.showStreamSnapshotIfCurrent('c2', snap('theirs'))
    })
    runFrame()
    expect(applySnapshot).toHaveBeenCalledTimes(1)
    expect(applySnapshot.mock.calls[0][0].content).toBe('mine')
  })

  it('入队后会话被切走，帧到时也不渲染', () => {
    const { result, applySnapshot } = setup('c1')
    act(() => { result.current.frame.showStreamSnapshotIfCurrent('c1', snap('x')) })
    act(() => { result.current.currentConversationIdRef.current = 'c2' })
    runFrame()
    expect(applySnapshot).not.toHaveBeenCalled()
  })
})

describe('useStreamRenderFrame 取消', () => {
  it('cancelPendingFrame 后挂起帧不再刷出', () => {
    const { result, applySnapshot } = setup()
    act(() => {
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('x'))
      result.current.frame.cancelPendingFrame()
    })
    runFrame()
    expect(applySnapshot).not.toHaveBeenCalled()
  })

  it('cancelPendingFrameFor 只取消匹配会话的挂起帧', () => {
    const { result, applySnapshot } = setup('c1')
    act(() => {
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('x'))
      // 取消的是另一个会话，挂起帧应保留
      result.current.frame.cancelPendingFrameFor('c2')
    })
    runFrame()
    expect(applySnapshot).toHaveBeenCalledTimes(1)
  })

  it('cancelPendingFrameFor 命中时取消', () => {
    const { result, applySnapshot } = setup('c1')
    act(() => {
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('x'))
      result.current.frame.cancelPendingFrameFor('c1')
    })
    runFrame()
    expect(applySnapshot).not.toHaveBeenCalled()
  })

  it('取消后仍可继续接收新帧', () => {
    const { result, applySnapshot } = setup()
    act(() => {
      result.current.frame.showStreamSnapshotIfCurrent('c1', snap('x'))
      result.current.frame.cancelPendingFrame()
    })
    runFrame()
    act(() => { result.current.frame.showStreamSnapshotIfCurrent('c1', snap('y')) })
    runFrame()
    expect(applySnapshot).toHaveBeenCalledTimes(1)
    expect(applySnapshot.mock.calls[0][0].content).toBe('y')
  })
})

describe('useStreamRenderFrame 后台 cadence', () => {
  it('后台窗口改用低频 timer，不占用前台 rAF', () => {
    vi.useFakeTimers()
    Object.defineProperty(document, 'hidden', { configurable: true, value: true })
    const { result, applySnapshot } = setup()

    act(() => { result.current.frame.showStreamSnapshotIfCurrent('c1', snap('background')) })
    expect(rafQueue).toHaveLength(0)
    expect(applySnapshot).not.toHaveBeenCalled()

    act(() => { vi.advanceTimersByTime(800) })
    runFrame()
    expect(applySnapshot).toHaveBeenCalledTimes(1)

    Object.defineProperty(document, 'hidden', { configurable: true, value: false })
  })
})
