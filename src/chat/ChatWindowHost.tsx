import { useEffect, useRef, useState, type ReactNode } from 'react'
import { getCurrentWindow, type PhysicalSize } from '@tauri-apps/api/window'
import { isMac, isWindows, usesNativeTitlebar } from './platform'
import { isTauriRuntime } from './utils'
import { syncChatWindowEffect, type ChatEffectPlatform } from './chatWindowEffects'

type ChatWindowHostProps = {
  children: ReactNode
  translucentSidebar: boolean
}

/** Chat 专用窗口外壳：Windows 自绘圆角边缘，最大化时收起圆角。 */
export function ChatWindowHost({ children, translucentSidebar }: ChatWindowHostProps) {
  const [maximized, setMaximized] = useState(false)
  const [nativeEffectActive, setNativeEffectActive] = useState(false)
  const translucentSidebarRef = useRef(translucentSidebar)
  const requestEffectSyncRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    if (!isTauriRuntime() || usesNativeTitlebar) return

    let cancelled = false
    let unlisten: (() => void) | undefined
    let timer: ReturnType<typeof setTimeout> | undefined

    const syncMaximized = async () => {
      try {
        const next = await getCurrentWindow().isMaximized()
        if (!cancelled) setMaximized(next)
      } catch {
        // ignore
      }
    }

    const setup = async () => {
      await syncMaximized()
      // resize 事件在拖动伸缩时高频触发；isMaximized() 是一次 IPC 往返。只在伸缩停止后查一次，
      // 避免每帧 IPC 洪流拖慢窗口伸缩。最大化/还原是离散动作，延迟 ~150ms 更新圆角无感知。
      const handler = await getCurrentWindow().onResized(() => {
        if (timer !== undefined) clearTimeout(timer)
        timer = setTimeout(() => {
          void syncMaximized()
        }, 150)
      })
      if (cancelled) {
        handler()
      } else {
        unlisten = handler
      }
    }

    void setup()
    return () => {
      cancelled = true
      if (timer !== undefined) clearTimeout(timer)
      unlisten?.()
    }
  }, [])

  useEffect(() => {
    translucentSidebarRef.current = translucentSidebar
    requestEffectSyncRef.current?.()
  }, [translucentSidebar])

  useEffect(() => {
    if (!isTauriRuntime()) return

    const platform: ChatEffectPlatform = isMac ? 'macos' : isWindows ? 'windows' : 'linux'
    if (platform === 'linux') return
    const window = getCurrentWindow()
    let focused = false
    let size: PhysicalSize
    let cancelled = false
    let ready = false
    let syncPending = false
    let syncing = false
    let unlistenFocus: (() => void) | undefined
    let unlistenResize: (() => void) | undefined

    const sync = () => {
      syncPending = true
      if (!ready || syncing) return
      syncing = true
      void (async () => {
        while (syncPending && !cancelled) {
          syncPending = false
          const active = await syncChatWindowEffect(
            window,
            platform,
            translucentSidebarRef.current,
            focused,
            size,
          )
          if (!syncPending && !cancelled) setNativeEffectActive(active)
        }
        syncing = false
      })()
    }
    requestEffectSyncRef.current = sync

    const setup = async () => {
      [focused, size] = await Promise.all([window.isFocused(), window.innerSize()])
      if (cancelled) return

      const stopResize = await window.onResized(({ payload }) => {
        size = payload
        sync()
      })
      if (cancelled) {
        stopResize()
        return
      }
      unlistenResize = stopResize
      if (platform === 'macos') {
        const stopFocus = await window.onFocusChanged(({ payload }) => {
          focused = payload
          sync()
        })
        if (cancelled) {
          stopFocus()
        } else {
          unlistenFocus = stopFocus
        }
      }
      if (cancelled) return
      [focused, size] = await Promise.all([window.isFocused(), window.innerSize()])
      if (cancelled) return
      ready = true
      sync()
    }

    void setup().catch(() => {
      if (!cancelled) setNativeEffectActive(false)
      void window.clearEffects().catch(() => {})
    })

    return () => {
      cancelled = true
      requestEffectSyncRef.current = null
      unlistenFocus?.()
      unlistenResize?.()
    }
  }, [])

  const hostClassName = [
    'chat-window-host h-full w-full',
    isWindows ? 'chat-window-host--win' : '',
    maximized ? 'chat-window-host--maximized' : '',
    nativeEffectActive ? 'chat-window-host--native-effect' : '',
  ].filter(Boolean).join(' ')

  return (
    <div className={hostClassName}>
      {children}
    </div>
  )
}
