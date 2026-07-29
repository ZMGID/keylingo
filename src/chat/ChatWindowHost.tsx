import { useEffect, useState, type ReactNode } from 'react'
import { getCurrentWindow, type PhysicalSize } from '@tauri-apps/api/window'
import { isMac, isWindows, usesNativeTitlebar } from './platform'
import { isTauriRuntime } from './utils'
import { syncChatWindowEffect, type ChatEffectPlatform } from './chatWindowEffects'

type ChatWindowHostProps = {
  children: ReactNode
  translucentSidebar: boolean
}

const effectPlatform: ChatEffectPlatform = isMac ? 'macos' : isWindows ? 'windows' : 'linux'

/** Mica 变体要跟应用主题走（见 chatWindowEffects）。主题由 App.tsx 切 html.dark 类，
 *  没有事件可订阅，只能观察 class。 */
function useDocumentDark(): boolean {
  const [dark, setDark] = useState(() => document.documentElement.classList.contains('dark'))
  useEffect(() => {
    const root = document.documentElement
    const observer = new MutationObserver(() => setDark(root.classList.contains('dark')))
    observer.observe(root, { attributes: true, attributeFilter: ['class'] })
    return () => observer.disconnect()
  }, [])
  return dark
}

/**
 * Chat 专用窗口外壳：Windows 自绘圆角边缘，最大化时收起圆角；
 * macOS 全屏时系统隐藏交通灯，撤掉顶栏为灯预留的左缩进（否则空一大块）；
 * 并驱动系统窗口材质（macOS Menu / Windows Mica）的开关。
 */
export function ChatWindowHost({ children, translucentSidebar }: ChatWindowHostProps) {
  const [maximized, setMaximized] = useState(false)
  const [nativeEffectActive, setNativeEffectActive] = useState(false)
  // 材质输入（焦点 + 物理尺寸）存 state，让 React 自己收敛重复渲染，不手搓 pending/ready 标志。
  const [effectInput, setEffectInput] = useState<{ focused: boolean; size: PhysicalSize } | null>(null)
  // mac 全屏：以 isFullscreen() 为权威（菜单栏「始终显示」时 innerHeight 到不了 screen.height，
  // 纯几何判定会漏判，顶栏 pl-[92px] 留出空块）。
  //
  // 几何只允许「关」全屏态，绝不单靠贴屏「开」——否则取消最小化 / 窗口动画贴屏的末帧
  // 会误撤缩进，和已显示的交通灯重叠闪一下；随后 IPC 回 false 又恢复，就是用户看到的闪烁。
  // 退出动画中段 isFullscreen 仍可能 true，但尺寸已离开全屏、灯已画回，同样必须立刻恢复缩进。
  // IPC 用 generation 丢弃过期响应，避免先发出的 true 在后发出的 false 之后才回来把状态打脏。
  const [macFullscreen, setMacFullscreen] = useState(false)
  const dark = useDocumentDark()

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

  // 系统窗口材质的输入采集：焦点 + 物理尺寸。resize 同样按 150ms 收敛（理由见上方
  // syncMaximized —— setEffects 也是一次 IPC，不能每帧发）。
  useEffect(() => {
    if (!isTauriRuntime() || effectPlatform === 'linux') return

    const win = getCurrentWindow()
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const stops: Array<() => void> = []

    // prev 为 null = 初始快照还没落地，事件直接丢（紧随其后的 setEffectInput 会带上完整状态）。
    const push = (next: Partial<{ focused: boolean; size: PhysicalSize }>) => {
      if (!cancelled) setEffectInput(prev => (prev ? { ...prev, ...next } : prev))
    }

    void (async () => {
      try {
        const [focused, size] = await Promise.all([win.isFocused(), win.innerSize()])
        if (cancelled) return
        setEffectInput({ focused, size })

        const stopResize = await win.onResized(({ payload }) => {
          if (timer !== undefined) clearTimeout(timer)
          timer = setTimeout(() => push({ size: payload }), 150)
        })
        cancelled ? stopResize() : stops.push(stopResize)

        // macOS 的 Menu 材质要跟随焦点；Windows 的 Mica 失焦不变，不必订阅。
        if (effectPlatform === 'macos') {
          const stopFocus = await win.onFocusChanged(({ payload }) => push({ focused: payload }))
          cancelled ? stopFocus() : stops.push(stopFocus)
        }
      } catch {
        if (!cancelled) setNativeEffectActive(false)
      }
    })()

    return () => {
      cancelled = true
      if (timer !== undefined) clearTimeout(timer)
      stops.forEach(stop => stop())
    }
  }, [])

  // 材质应用：输入或设置一变就重跑，React 负责收敛；晚到的结果由 cancelled 丢弃。
  useEffect(() => {
    if (!isTauriRuntime() || effectPlatform === 'linux' || !effectInput) return
    let cancelled = false
    void syncChatWindowEffect(
      getCurrentWindow(),
      effectPlatform,
      translucentSidebar,
      effectInput.focused,
      effectInput.size,
      dark,
    ).then(active => {
      if (!cancelled) setNativeEffectActive(active)
    })
    return () => {
      cancelled = true
    }
  }, [translucentSidebar, effectInput, dark])

  const nativeEffectClass = nativeEffectActive ? ' chat-window-host--native-effect' : ''

  if (usesNativeTitlebar) {
    return (
      <div
        className={`chat-window-host h-full w-full${macFullscreen ? ' chat-window-host--mac-fullscreen' : ''}${nativeEffectClass}`}
      >
        {children}
      </div>
    )
  }

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
