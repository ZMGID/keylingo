import { useEffect, useState, type ReactNode } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { isWindows, usesNativeTitlebar } from './platform'
import { isTauriRuntime } from './utils'

type ChatWindowHostProps = {
  children: ReactNode
}

/**
 * Chat 专用窗口外壳：Windows 自绘圆角边缘，最大化时收起圆角；
 * macOS 全屏时系统隐藏交通灯，撤掉顶栏为灯预留的左缩进（否则空一大块）。
 */
export function ChatWindowHost({ children }: ChatWindowHostProps) {
  const [maximized, setMaximized] = useState(false)
  // mac 全屏：不走 Tauri 的 isFullscreen()（IPC 往返，只能节流查 → 灯已画回来缩进还没恢复，
  // 退出全屏时图标和灯要重叠一拍）。全屏时 webview 高度 == 屏幕高度，非全屏一定被菜单栏切掉；
  // DOM resize 与布局同帧、读 innerHeight 同步，所以缩进和窗口逐帧同步，零 IPC。
  // ponytail: 靠高度判定，与「窗口手动拉到正好等于屏高」不可区分 —— 但那也需要盖住菜单栏，做不到。
  const [macFullscreen, setMacFullscreen] = useState(false)

  useEffect(() => {
    if (!usesNativeTitlebar) return
    const sync = () => setMacFullscreen(window.innerHeight >= window.screen.height)
    sync()
    window.addEventListener('resize', sync)
    return () => window.removeEventListener('resize', sync)
  }, [])

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

  if (usesNativeTitlebar) {
    return (
      <div className={`h-full w-full${macFullscreen ? ' chat-window-host--mac-fullscreen' : ''}`}>
        {children}
      </div>
    )
  }

  const hostClassName = [
    'chat-window-host h-full w-full',
    isWindows ? 'chat-window-host--win' : '',
    maximized ? 'chat-window-host--maximized' : '',
  ].filter(Boolean).join(' ')

  return (
    <div className={hostClassName}>
      {children}
    </div>
  )
}
