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
  // mac 全屏：以 isFullscreen() 为权威（菜单栏「始终显示」时 innerHeight 到不了 screen.height，
  // 纯几何判定会漏判，顶栏 pl-[92px] 留出空块）。
  //
  // 几何只允许「关」全屏态，绝不单靠贴屏「开」——否则取消最小化 / 窗口动画贴屏的末帧
  // 会误撤缩进，和已显示的交通灯重叠闪一下；随后 IPC 回 false 又恢复，就是用户看到的闪烁。
  // 退出动画中段 isFullscreen 仍可能 true，但尺寸已离开全屏、灯已画回，同样必须立刻恢复缩进。
  // IPC 用 generation 丢弃过期响应，避免先发出的 true 在后发出的 false 之后才回来把状态打脏。
  const [macFullscreen, setMacFullscreen] = useState(false)

  useEffect(() => {
    if (!usesNativeTitlebar) return

    let cancelled = false
    let unlisten: (() => void) | undefined
    let generation = 0

    /** 尺寸已明显不是全屏（含退出动画中段）。用 availHeight：菜单栏常显的稳定全屏仍 ≥ availHeight。 */
    const clearlyNotFullscreen = () =>
      window.innerWidth < window.screen.width - 2 ||
      window.innerHeight < window.screen.availHeight - 2

    const syncIpc = async () => {
      if (!isTauriRuntime()) {
        // 浏览器预览无 IPC：只能用严格贴 screen 的几何。
        const full =
          window.innerWidth >= window.screen.width - 2 &&
          window.innerHeight >= window.screen.height - 2
        if (!cancelled) setMacFullscreen(full)
        return
      }
      const gen = ++generation
      try {
        const fs = await getCurrentWindow().isFullscreen()
        if (cancelled || gen !== generation) return
        // fs 但已离开全屏尺寸 = 退出动画：灯已显，保持缩进。
        setMacFullscreen(fs && !clearlyNotFullscreen())
      } catch {
        if (!cancelled && gen === generation) setMacFullscreen(false)
      }
    }

    const onResize = () => {
      if (clearlyNotFullscreen()) setMacFullscreen(false)
      void syncIpc()
    }

    void syncIpc()
    window.addEventListener('resize', onResize)

    void (async () => {
      if (!isTauriRuntime()) return
      try {
        unlisten = await getCurrentWindow().onResized(() => {
          void syncIpc()
        })
      } catch {
        // ignore
      }
    })()

    return () => {
      cancelled = true
      window.removeEventListener('resize', onResize)
      unlisten?.()
    }
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
