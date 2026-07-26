import { useEffect } from 'react'

/**
 * 订阅一个 Tauri 事件，处理「订阅是异步的、但组件可能在 await 期间卸载」这个竞态。
 *
 * Chat.tsx 里这套样板重复了 13 次（let cancelled / let unlisten / setupListener /
 * 卸载时 unlisten?.()），每处都得自己记着「await 返回后要再查一次 cancelled，
 * 否则订阅泄漏」。收敛到这里，调用点只写 handler。
 *
 * @param subscribe Tauri 侧的 `api.onXxx`，返回 unlisten 的 Promise
 * @param handler   事件处理；卸载后不再被调用
 * @param deps      handler 的依赖；变化时重新订阅（与手写 effect 语义一致）
 */
export function useTauriEvent<T>(
  subscribe: (handler: (payload: T) => void) => Promise<() => void>,
  handler: (payload: T) => void,
  deps: React.DependencyList,
): void {
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined

    void (async () => {
      try {
        const dispose = await subscribe((payload) => {
          if (cancelled) return
          handler(payload)
        })
        // await 期间可能已卸载：此时立即退订，否则订阅泄漏。
        if (cancelled) dispose()
        else unlisten = dispose
      } catch (err) {
        console.error('Failed to subscribe Tauri event:', err)
      }
    })()

    return () => {
      cancelled = true
      unlisten?.()
    }
    // subscribe 是稳定的模块级函数；handler 的依赖由调用方经 deps 传入。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps)
}
