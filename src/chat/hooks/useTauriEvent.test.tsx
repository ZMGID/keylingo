import { renderHook, act } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { useTauriEvent } from './useTauriEvent'

/**
 * 回归重点全在竞态上（这是抽出它的理由）：
 *   1. await 期间卸载 → 必须立即退订，否则订阅泄漏
 *   2. 卸载后到达的事件 → 不得调用 handler
 *   3. 正常卸载 → 调用 unlisten
 */
describe('useTauriEvent', () => {
  it('挂载后订阅，事件透传给 handler', async () => {
    let emit: ((p: string) => void) | undefined
    const unlisten = vi.fn()
    const subscribe = vi.fn(async (h: (p: string) => void) => { emit = h; return unlisten })
    const handler = vi.fn()

    await act(async () => {
      renderHook(() => useTauriEvent(subscribe, handler, []))
    })
    act(() => { emit?.('hello') })
    expect(handler).toHaveBeenCalledWith('hello')
  })

  it('卸载时调用 unlisten', async () => {
    const unlisten = vi.fn()
    const subscribe = vi.fn(async () => unlisten)
    let rendered: ReturnType<typeof renderHook> | undefined
    await act(async () => {
      rendered = renderHook(() => useTauriEvent(subscribe, vi.fn(), []))
    })
    expect(unlisten).not.toHaveBeenCalled()
    rendered!.unmount()
    expect(unlisten).toHaveBeenCalledTimes(1)
  })

  it('await 期间卸载：订阅完成后立即退订（防泄漏）', async () => {
    let resolveSubscribe: ((u: () => void) => void) | undefined
    const unlisten = vi.fn()
    const subscribe = vi.fn(() => new Promise<() => void>((res) => { resolveSubscribe = res }))

    const rendered = renderHook(() => useTauriEvent(subscribe, vi.fn(), []))
    // 订阅还挂着就卸载
    rendered.unmount()
    await act(async () => { resolveSubscribe?.(unlisten) })
    // 这是关键：晚到的 dispose 必须被立刻调用，否则监听器永远留在 Tauri 侧
    expect(unlisten).toHaveBeenCalledTimes(1)
  })

  it('卸载后到达的事件不调用 handler', async () => {
    let emit: ((p: string) => void) | undefined
    const subscribe = vi.fn(async (h: (p: string) => void) => { emit = h; return vi.fn() })
    const handler = vi.fn()

    let rendered: ReturnType<typeof renderHook> | undefined
    await act(async () => {
      rendered = renderHook(() => useTauriEvent(subscribe, handler, []))
    })
    rendered!.unmount()
    act(() => { emit?.('late') })
    expect(handler).not.toHaveBeenCalled()
  })

  it('订阅失败不抛出（只记日志）', async () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    const subscribe = vi.fn(async () => { throw new Error('boom') })
    await act(async () => {
      renderHook(() => useTauriEvent(subscribe as never, vi.fn(), []))
    })
    expect(spy).toHaveBeenCalled()
    spy.mockRestore()
  })

  it('deps 变化时重新订阅并退掉旧的', async () => {
    const unlisten1 = vi.fn()
    const unlisten2 = vi.fn()
    const subscribe = vi.fn()
      .mockImplementationOnce(async () => unlisten1)
      .mockImplementationOnce(async () => unlisten2)

    let dep = 1
    let rendered: ReturnType<typeof renderHook> | undefined
    await act(async () => {
      rendered = renderHook(() => useTauriEvent(subscribe, vi.fn(), [dep]))
    })
    expect(subscribe).toHaveBeenCalledTimes(1)

    dep = 2
    await act(async () => { rendered!.rerender() })
    expect(unlisten1).toHaveBeenCalledTimes(1)
    expect(subscribe).toHaveBeenCalledTimes(2)
  })

  it('deps 不变时不重复订阅', async () => {
    const subscribe = vi.fn(async () => vi.fn())
    let rendered: ReturnType<typeof renderHook> | undefined
    await act(async () => {
      rendered = renderHook(() => useTauriEvent(subscribe, vi.fn(), ['stable']))
    })
    await act(async () => { rendered!.rerender() })
    expect(subscribe).toHaveBeenCalledTimes(1)
  })
})
