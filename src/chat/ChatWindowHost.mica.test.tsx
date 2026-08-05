import { act, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// 注意：jsdom 里跑不了真 DWM，这里测的是 React 状态流转 —— 透明外壳那个 class 有没有被
// 无谓地撤掉 / 被过期的焦点快照盖回去。真实的 Mica 观感只能靠手动冒烟。

type ResizeHandler = (event: { payload: { width: number; height: number } }) => void
type FocusHandler = (event: { payload: boolean }) => void

const hoisted = vi.hoisted(() => ({
  resizeHandlers: [] as ResizeHandler[],
  focusHandlers: [] as FocusHandler[],
  isFocusedQueue: [] as Array<Promise<boolean>>,
  applyMica: vi.fn(async () => true),
}))

vi.mock('./platform', () => ({ isMac: false, isWindows: true, usesNativeTitlebar: false }))
vi.mock('./utils', () => ({ isTauriRuntime: () => true }))
vi.mock('../api/tauri', () => ({
  api: {
    chatWindowApplyMica: () => hoisted.applyMica(),
    chatWindowSetOpaque: vi.fn(),
    chatTrafficLightCenterY: vi.fn(),
  },
}))
vi.mock('@tauri-apps/api/window', () => ({
  Effect: { Menu: 'menu' },
  EffectState: { FollowsWindowActiveState: 'followsWindowActiveState' },
  getCurrentWindow: () => ({
    isFocused: () => hoisted.isFocusedQueue.shift() ?? Promise.resolve(true),
    innerSize: () => Promise.resolve({ width: 1200, height: 800 }),
    isMaximized: () => Promise.resolve(false),
    setEffects: () => Promise.resolve(),
    clearEffects: () => Promise.resolve(),
    onResized: (cb: ResizeHandler) => {
      hoisted.resizeHandlers.push(cb)
      return Promise.resolve(() => {})
    },
    onFocusChanged: (cb: FocusHandler) => {
      hoisted.focusHandlers.push(cb)
      return Promise.resolve(() => {})
    },
  }),
}))

const { ChatWindowHost } = await import('./ChatWindowHost')

/** 假计时器下没有宏任务可等，靠多轮微任务把 IPC promise 链推到底。 */
async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i += 1) await Promise.resolve()
  })
}

function renderHost() {
  const { container } = render(
    <ChatWindowHost translucentSidebar>
      <div>chat</div>
    </ChatWindowHost>,
  )
  return container.querySelector('.chat-window-host') as HTMLElement
}

const isTranslucent = (host: HTMLElement) => host.classList.contains('chat-window-host--native-effect')

beforeEach(() => {
  vi.useFakeTimers()
  hoisted.resizeHandlers.length = 0
  hoisted.focusHandlers.length = 0
  hoisted.isFocusedQueue = []
  hoisted.applyMica.mockClear()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('ChatWindowHost 的 Mica 透明外壳', () => {
  it('缩放窗口触发的材质重跑，中途不会撤掉透明外壳', async () => {
    const host = renderHost()
    await flush()
    expect(isTranslucent(host)).toBe(true)

    act(() => {
      hoisted.resizeHandlers.forEach(cb => cb({ payload: { width: 1400, height: 900 } }))
    })
    // 防抖到点、effect 同步重跑的这一帧，正是旧代码闪烁的那一帧。
    act(() => {
      vi.advanceTimersByTime(150)
    })
    expect(isTranslucent(host)).toBe(true)

    await flush()
    expect(isTranslucent(host)).toBe(true)
  })

  it('监听器已报 focused=true 后，补读回来的过期 false 不会盖回去', async () => {
    let resolveGapRead: (focused: boolean) => void = () => {}
    hoisted.isFocusedQueue = [
      Promise.resolve(false),
      new Promise<boolean>(resolve => { resolveGapRead = resolve }),
    ]

    const host = renderHost()
    await flush()
    expect(isTranslucent(host)).toBe(false)

    act(() => {
      hoisted.focusHandlers.forEach(cb => cb({ payload: true }))
    })
    await flush()
    expect(isTranslucent(host)).toBe(true)

    resolveGapRead(false)
    await flush()
    expect(isTranslucent(host)).toBe(true)
  })

  it('失焦当帧立刻撤掉透明外壳', async () => {
    const host = renderHost()
    await flush()
    expect(isTranslucent(host)).toBe(true)

    act(() => {
      hoisted.focusHandlers.forEach(cb => cb({ payload: false }))
    })
    expect(isTranslucent(host)).toBe(false)
  })
})
