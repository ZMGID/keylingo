import { useEffect, useRef, useState, type ReactNode } from 'react'
import { getCurrentWindow, type PhysicalSize } from '@tauri-apps/api/window'
import { api } from '../api/tauri'
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
  const [effectInput, setEffectInput] = useState<{
    focused: boolean
    size: PhysicalSize
  } | null>(null)
  // DWM mutations cannot be cancelled once invoke() has crossed the IPC boundary.
  // Serialize them so an older theme/focus request can never land after the latest one.
  const effectSyncQueueRef = useRef<Promise<void>>(Promise.resolve())
  const effectSyncGenerationRef = useRef(0)
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

  // 顶栏那条线对齐交通灯。灯的 y 由 AppKit 布局决定、随 macOS 版本变（见 windows.rs
  // CHAT_TRAFFIC_LIGHT_INSET_Y 注释），写死常数必然「这台对了那台错」—— 所以量一次真值，
  // 让 CSS 跟着灯走（index.css --chat-traffic-center-y）。
  // 窗口刚建出来时 contentView 可能还没尺寸，量不到就隔一会儿再试，别永远卡在默认值。
  useEffect(() => {
    if (!usesNativeTitlebar || !isTauriRuntime()) return

    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined

    const measure = async (attempt: number) => {
      const y = await api.chatTrafficLightCenterY().catch(() => null)
      if (cancelled) return
      if (y != null) {
        document.documentElement.style.setProperty('--chat-traffic-center-y', `${y}px`)
      } else if (attempt < 3) {
        timer = setTimeout(() => void measure(attempt + 1), 250)
      }
    }

    void measure(0)
    return () => {
      cancelled = true
      if (timer !== undefined) clearTimeout(timer)
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

  // 系统窗口材质的输入采集：物理尺寸 + 焦点。Microsoft 明确规定桌面 Mica 在失焦时
  // 退回实色；Windows 下必须在失焦帧撤掉透明 CSS 外壳，重新聚焦后再应用一次主题变体。
  useEffect(() => {
    if (!isTauriRuntime() || effectPlatform === 'linux') return

    const win = getCurrentWindow()
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const stops: Array<() => void> = []

    const push = (next: Partial<{ focused: boolean; size: PhysicalSize }>) => {
      if (!cancelled) setEffectInput(previous => previous ? { ...previous, ...next } : previous)
    }

    void (async () => {
      try {
        const [focused, size] = await Promise.all([win.isFocused(), win.innerSize()])
        if (cancelled) return
        setEffectInput({ focused, size })

        const stop = await win.onResized(({ payload }) => {
          if (timer !== undefined) clearTimeout(timer)
          timer = setTimeout(() => {
            push({ size: payload })
          }, 150)
        })
        cancelled ? stop() : stops.push(stop)

        if (effectPlatform === 'windows') {
          // 移动窗口不需要重设 DWM：Mica 是 per-HWND 属性，跨屏由 DWM 自己重绘；
          // 跨屏 DPI 变化会带来 WM_SIZE，走上面的 onResized 就够了。

          let focusEventSeen = false
          const stopFocus = await win.onFocusChanged(({ payload }) => {
            focusEventSeen = true
            // Fail closed in the same event turn; waiting for IPC would expose the
            // system's light inactive fallback through the transparent title bar.
            if (!payload) setNativeEffectActive(false)
            push({ focused: payload })
          })
          cancelled ? stopFocus() : stops.push(stopFocus)

          // Close the gap between the first isFocused() snapshot and listener
          // installation. A deactivation in that interval would otherwise leave
          // the transparent class stuck until the next focus transition.
          // 监听器一旦报过值，它就是更新的事实 —— 这次补读只可能是过期快照，丢掉。
          const latestFocused = await win.isFocused()
          if (cancelled || focusEventSeen) return
          if (!latestFocused) setNativeEffectActive(false)
          push({ focused: latestFocused })
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

  // 材质应用：调用严格串行，generation 只允许最后一份结果控制透明外壳。
  // React effect 的 cancelled 只能阻止 setState，阻止不了已发出的原生 DWM 调用；若不排队，
  // 旧的 light/focus 请求可能在新的 dark/focus 请求之后才落地，制造偶发混合主题。
  useEffect(() => {
    if (!isTauriRuntime() || effectPlatform === 'linux' || !effectInput) return
    let cancelled = false
    const generation = ++effectSyncGenerationRef.current

    // 这里刻意不预先 setNativeEffectActive(false)：尺寸变化也会重跑本 effect，预清等于
    // 「撤透明外壳 → 等一次 IPC → 再加回来」，拖动/缩放窗口时肉眼一闪。失焦那一帧的
    // fail-closed 由 onFocusChanged 同帧完成，不需要在这里再兜一遍。

    const task = effectSyncQueueRef.current
      .catch(() => {})
      .then(async () => {
        if (cancelled) return
        const active = await syncChatWindowEffect(
          getCurrentWindow(),
          effectPlatform,
          translucentSidebar,
          effectInput.focused,
          effectInput.size,
          dark,
        )
        if (cancelled || generation !== effectSyncGenerationRef.current) return
        setNativeEffectActive(active)
        // 材质没上就把窗口设回 opaque：内容本来铺满整窗，非 opaque 只是白付合成开销
        // （台前调度缩放整窗时掉帧）。macOS 专属，其他平台后端 no-op。
        if (effectPlatform === 'macos') void api.chatWindowSetOpaque(!active)
      })
    effectSyncQueueRef.current = task

    return () => {
      cancelled = true
    }
  }, [translucentSidebar, effectInput, dark])

  const nativeEffectClass = nativeEffectActive ? ' chat-window-host--native-effect' : ''

  if (usesNativeTitlebar) {
    return (
      <div
        className={`chat-window-host chat-window-host--mac-titlebar h-full w-full${macFullscreen ? ' chat-window-host--mac-fullscreen' : ''}${nativeEffectClass}`}
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
