import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach } from 'vitest'

afterEach(() => {
  cleanup()
})

if (typeof window !== 'undefined') {
  // Node 22's jsdom environment may expose window.localStorage as an unavailable
  // getter unless a --localstorage-file is configured. Keep storage-dependent
  // UI tests deterministic without requiring a process-wide file.
  let storageAvailable = false
  try {
    storageAvailable = Boolean(window.localStorage)
  } catch {
    storageAvailable = false
  }

  if (!storageAvailable) {
    const values = new Map<string, string>()
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        get length() {
          return values.size
        },
        key(index: number) {
          return [...values.keys()][index] ?? null
        },
        getItem(key: string) {
          return values.get(key) ?? null
        },
        setItem(key: string, value: string) {
          values.set(key, String(value))
        },
        removeItem(key: string) {
          values.delete(key)
        },
        clear() {
          values.clear()
        },
      } satisfies Storage,
    })
  }

  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  })

  // TanStack Virtual 在 jsdom 下需要一个会回调的 ResizeObserver + 非零尺寸，否则测不出可见项
  // （jsdom 无真实布局，默认尺寸全 0，且 offsetParent 恒为 null，虚拟列表会跳过所有测量
  // 项、渲不出任何 item）。这是测试 shim：给虚拟列表喂一个固定视口/项尺寸并伪造
  // offsetParent，让它把项挂载出来供断言。
  const RO_VIEWPORT = 800
  const RO_ITEM = 80

  // Virtualizer 读取 contentRect.height 设置视口/项尺寸，并以 offsetParent 真值过滤未布局元素。
  Object.defineProperty(window.HTMLElement.prototype, 'offsetParent', {
    configurable: true,
    get() {
      return this.parentElement
    },
  })

  // Virtualizer 用第一个被 observe 的元素作为视口；之后的是各项。
  class ResizeObserverMock {
    private cb: ResizeObserverCallback
    private isFirst = true
    constructor(cb: ResizeObserverCallback) {
      this.cb = cb
    }
    observe(target: Element) {
      const size = this.isFirst ? RO_VIEWPORT : RO_ITEM
      this.isFirst = false
      const entry = {
        target,
        contentRect: { height: size, width: 600 } as DOMRectReadOnly,
      } as unknown as ResizeObserverEntry
      // 浏览器的 ResizeObserver 通知发生在独立的 observer delivery 阶段，
      // 不能在 React/virtualizer 正在挂载的 lifecycle 内同步回调。异步投递既更贴近
      // 真实行为，也避免 virtualizer 为修正尺寸调用 flushSync 时触发 React 警告。
      queueMicrotask(() => this.cb([entry], this as unknown as ResizeObserver))
    }
    unobserve() {}
    disconnect() {}
  }

  window.ResizeObserver = ResizeObserverMock as unknown as typeof ResizeObserver

  // KivioBlob2（流式蓝点）用 IntersectionObserver 做可见性节流；jsdom 没有实现。
  // 测试里不需要真实交叉信息，静默 stub 即可（不回调 = 视为不可见，动画不启动）。
  if (typeof window.IntersectionObserver === 'undefined') {
    class IntersectionObserverMock {
      observe() {}
      unobserve() {}
      disconnect() {}
      takeRecords() {
        return []
      }
    }
    window.IntersectionObserver =
      IntersectionObserverMock as unknown as typeof IntersectionObserver
  }

  // Virtualizer 读取视口高度走 clientHeight；jsdom 默认 0，给个非零值。
  Object.defineProperty(window.HTMLElement.prototype, 'clientHeight', {
    configurable: true,
    get() {
      return RO_VIEWPORT
    },
  })
}
