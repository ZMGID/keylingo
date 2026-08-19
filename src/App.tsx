import { lazy, Suspense, useState, useEffect, useLayoutEffect, useRef, useCallback } from 'react'
import { Settings as SettingsIcon, Cpu } from 'lucide-react'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { api, isTauriRuntime } from './api/tauri'
import { getSettingsCached } from './api/settingsCache'
import { i18n, type Lang } from './settings/i18n'
import { useWindowInteractionFocus } from './utils/windowFocus'
import { ChatWindowHost } from './chat/ChatWindowHost'
import {
  getRememberedChatRoute,
  hashPath,
  isChatWindowPlacementVisible,
  isChatPath,
  isChatSettingsPath,
  rememberChatGeometry,
  rememberCurrentChatRoute,
  restoreChatWindowGeometry,
  snapshotChatWindowGeometry,
} from './chat/persistence'
import { ChatErrorBoundary } from './chat/ChatErrorBoundary'
import { normalizeThemeColorId } from './themeColors'
import './index.css'

const Lens = lazy(() => import('./Lens'))
const Chat = lazy(() => import('./chat/Chat'))

/**
 * 翻译器主组件
 * 磨砂玻璃风格悬浮窗：顶部 drag bar、输入与结果分层级、底部提示与模型芯片。
 */
function Translator({
  translateSource,
  lang,
  onOpenSettings,
}: {
  translateSource: string
  lang: Lang
  onOpenSettings: () => void
}) {
  const [input, setInput] = useState('')
  const [result, setResult] = useState('')
  const [resultInput, setResultInput] = useState('')
  const [loading, setLoading] = useState(false)
  const resultRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const translateSeq = useRef(0)
  const requestWindowFocus = useWindowInteractionFocus()
  const t = i18n[lang]

  // 输入防抖翻译：600ms 延迟后发送翻译请求
  useEffect(() => {
    const seq = ++translateSeq.current
    setResult('')
    setResultInput('')
    const trimmed = input.trim()
    if (!trimmed) {
      setLoading(false)
      return
    }

    const timer = setTimeout(async () => {
      if (seq !== translateSeq.current) return
      setLoading(true)
      try {
        const translated = await api.translateText(input)
        if (seq !== translateSeq.current) return
        setResult(translated)
        setResultInput(input)
      } catch (e) {
        if (seq !== translateSeq.current) return
        console.error(e)
        setResult(typeof e === 'string' ? e : (e as Error).message || 'Error')
        setResultInput(input)
      } finally {
        if (seq === translateSeq.current) setLoading(false)
      }
    }, 600)
    return () => clearTimeout(timer)
  }, [input])

  // Esc 键关闭输入翻译窗口，释放不常用的 main WebView。
  useEffect(() => {
    const handler = async (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        try {
          await api.closeTranslatorWindow()
        } catch (err) {
          console.error('[Translator] Failed to close window:', err)
        }
      }
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [])

  // 结果区域自动滚动到底部
  useEffect(() => {
    if (resultRef.current) {
      resultRef.current.scrollTop = resultRef.current.scrollHeight
    }
  }, [result])

  // 输入框自动滚动到右侧（显示最新输入）
  useEffect(() => {
    if (inputRef.current) {
      inputRef.current.scrollLeft = inputRef.current.scrollWidth
    }
  }, [input])

  // Enter 键提交翻译结果
  // IME 合成中（中/日/韩输入法选词按回车）不要触发：isComposing 是组合事件官方标志，
  // keyCode === 229 是浏览器在 IME 拦截 keydown 时的兜底信号，两个条件并查更稳。
  const handleKeyDown = async (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== 'Enter') return
    if (e.nativeEvent.isComposing || e.keyCode === 229) return
    if (loading || !result || resultInput !== input) return
    const textToCommit = result
    await api.commitTranslation(textToCommit)
    setInput('')
    setResult('')
    setResultInput('')
  }

  return (
    <div
      className="window-container"
      onPointerEnter={requestWindowFocus}
      onPointerMove={requestWindowFocus}
      onPointerDownCapture={requestWindowFocus}
    >
      {/* 卡片：填满外壳 padding 内区域；圆角 + 阴影都在这层 */}
      <div className="window-frosted h-full w-full flex flex-col select-none overflow-hidden relative group">
        {/* 顶部隐形 drag bar */}
        <div
          className="absolute top-0 left-0 right-0 h-6 z-10"
          data-tauri-drag-region
        />

        {/* 设置按钮（悬浮右上角） */}
        <button
          onClick={onOpenSettings}
          className="absolute top-1.5 right-2 z-20 p-1 text-neutral-400 hover:text-neutral-700 dark:text-neutral-500 dark:hover:text-neutral-200 rounded-md hover:bg-black/5 dark:hover:bg-white/10 opacity-60 hover:opacity-100 transition-all duration-150"
          title={t.translatorSettings}
        >
          <SettingsIcon size={13} strokeWidth={1.75} />
        </button>

        {/* 主内容区 */}
        <div className="relative z-0 flex-1 flex flex-col justify-center px-3.5 pt-3 pb-2.5">
        {/* 翻译结果展示（微渐变背景 + 柔光内描边） */}
        {(result || loading) && (
          <div
            ref={resultRef}
            className="mb-2 px-3 py-2 rounded-xl max-h-14 overflow-y-auto custom-scrollbar bg-gradient-to-br from-neutral-100/90 to-neutral-50/80 dark:from-neutral-800/70 dark:to-neutral-800/40 ring-1 ring-black/[0.04] dark:ring-white/[0.06] shadow-sm"
          >
            {loading ? (
              <div className="flex items-center gap-2 text-neutral-500 dark:text-neutral-400">
                <span className="flex gap-0.5">
                  <span className="w-1 h-1 rounded-full bg-neutral-400 dark:bg-neutral-500 animate-pulse" />
                  <span className="w-1 h-1 rounded-full bg-neutral-400 dark:bg-neutral-500 animate-pulse [animation-delay:0.2s]" />
                  <span className="w-1 h-1 rounded-full bg-neutral-400 dark:bg-neutral-500 animate-pulse [animation-delay:0.4s]" />
                </span>
                <span className="text-[11px]">{t.translatorTranslating}</span>
              </div>
            ) : (
              <p className="text-neutral-800 dark:text-neutral-100 text-[14.5px] font-normal select-text leading-[1.5]">
                {result}
              </p>
            )}
          </div>
        )}

        {/* 输入框（更精致的圆角 + focus 渐变） */}
        <input
          ref={inputRef}
          autoFocus
          autoCapitalize="off"
          autoCorrect="off"
          autoComplete="off"
          spellCheck={false}
          className="w-full px-3.5 py-2 bg-white/70 dark:bg-neutral-800/40 ring-1 ring-black/[0.05] dark:ring-white/[0.06] rounded-xl text-[14.5px] text-neutral-900 dark:text-white placeholder-neutral-400 dark:placeholder-neutral-500 focus:outline-none focus:ring-black/[0.12] dark:focus:ring-white/[0.18] focus:bg-white dark:focus:bg-neutral-800/70 transition-all"
          placeholder={t.translatorPlaceholder}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
        />

        {/* 底部提示 */}
        <div className="mt-1.5 flex justify-between items-center text-[10px] text-neutral-400 dark:text-neutral-500">
          <div className="flex items-center gap-2">
            <span>{t.translatorHintEnter}</span>
            <span>{t.translatorHintEsc}</span>
          </div>
          {translateSource && (
            <span className="flex items-center gap-1 opacity-70 max-w-[140px] truncate">
              <Cpu size={9} strokeWidth={1.5} className="shrink-0" />
              <span className="truncate">{translateSource}</span>
            </span>
          )}
        </div>
        </div>
      </div>
    </div>
  )
}

/**
 * 应用根组件
 * 根据 URL hash 切换不同视图模式（翻译器、设置、lens）
 */
// 与 index.css 的 --font-sans 默认栈保持一致（跨 mac/Win 含 CJK 覆盖）。
const UI_FONT_FALLBACK_STACK =
  'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", "Hiragino Sans GB", sans-serif'
const UI_MONO_FALLBACK_STACK =
  'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Monaco, Consolas, monospace'

function App() {
  // 从 URL hash 和查询参数解析当前模式
  const getMode = () => {
    const urlParams = new URLSearchParams(window.location.search)
    const hash = window.location.hash.replace('#', '')
    const path = urlParams.get('mode') || hash.split('?')[0] || ''

    // 支持 #chat 或 #chat/conversation-id
    if (isChatPath(path)) {
      return 'chat'
    }

    return path
  }

  const [mode, setMode] = useState(getMode)
  const [themeMode, setThemeMode] = useState<'system' | 'light' | 'dark'>('system')
  const [translucentSidebar, setTranslucentSidebar] = useState(false)
  const [translateSource, setTranslateSource] = useState<string>('')
  const [lang, setLang] = useState<Lang>('zh')

  useEffect(() => {
    const path = hashPath()
    if (path === 'chat') {
      // 窗口以 `#chat` 创建说明 Rust 侧没有已存路由（有的话会直接烤进 URL，见
      // windows.rs::ensure_chat_window）。getRememberedChatRoute() 会自动迁移
      // localStorage 遗留值（如果有的话），无需显式调用 adoptLegacyRememberedChatRoute。
      const rememberedRoute = getRememberedChatRoute()
      if (rememberedRoute && rememberedRoute !== window.location.hash) {
        window.location.hash = rememberedRoute
        setMode('chat')
      }
      return
    }

    if (isChatPath(path)) {
      rememberCurrentChatRoute()
    }
  }, [])

  // 应用主题设置
  const applyTheme = async () => {
    const settings = await getSettingsCached()
    const nextMode = (settings.theme || 'system') as 'system' | 'light' | 'dark'
    setThemeMode(nextMode)
    setTranslucentSidebar(settings.translucentSidebar)
    const isDark = nextMode === 'dark' || (nextMode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)
    if (isDark) {
      document.documentElement.classList.add('dark')
    } else {
      document.documentElement.classList.remove('dark')
    }
    document.documentElement.dataset.themeColor = normalizeThemeColorId(settings.themeColor)
    // UI 字号（整体缩放）+ 自定义字体：仅作用于聊天窗口，翻译窗/Lens 保持原始几何与布局。
    // 直接读 hash（稳定的 import）而非 mode state，避免让 applyTheme 变成不稳定依赖。
    const root = document.documentElement
    if (isChatPath(hashPath())) {
      const scale = Math.min(1.4, Math.max(0.8, settings.uiFontScale ?? 1))
      // 用原生 webview 缩放（等同浏览器 Cmd+加号），而非 CSS zoom —— CSS zoom 会打乱
      // 聊天消息列表 virtualizer 虚拟滚动的 scrollTop/scrollHeight 几何量，导致流式生成时跟随钉底失效。
      if (isTauriRuntime()) void getCurrentWebview().setZoom(scale).catch(() => {})
      const family = (settings.uiFontFamily ?? '').trim()
      // 默认字体栈与 index.css 的 --font-sans 保持一致；自定义字体拼到最前，缺失时回退系统字体。
      root.style.setProperty(
        '--font-sans',
        family
          ? `"${family}", ${UI_FONT_FALLBACK_STACK}`
          : UI_FONT_FALLBACK_STACK,
      )
      const mono = (settings.uiFontMono ?? '').trim()
      root.style.setProperty(
        '--font-mono',
        mono
          ? `"${mono}", ${UI_MONO_FALLBACK_STACK}`
          : UI_MONO_FALLBACK_STACK,
      )
    }
    setTranslateSource(settings.translatorModel || 'AI')
    setLang((settings.settingsLanguage as Lang) || 'zh')
    // 首次应用主题后（下一帧）再开启主题色过渡，避免初始 light↔dark 闪烁；
    // 之后用户切换主题/系统主题变化时才平滑过渡。classList.add 幂等。
    requestAnimationFrame(() => {
      document.documentElement.classList.add('theme-transitions-ready')
    })
  }

  // 初始化主题并监听系统主题变化
  useEffect(() => {
    applyTheme()
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const changeHandler = () => {
      if (themeMode === 'system') applyTheme()
    }
    mq.addEventListener('change', changeHandler)
    return () => mq.removeEventListener('change', changeHandler)
  }, [themeMode])

  // 监听 hash 变化切换模式
  useEffect(() => {
    const handler = () => {
      const path = hashPath()
      const nextMode = getMode()
      if (isChatPath(path)) {
        rememberCurrentChatRoute()
      }
      setMode(nextMode)
    }
    window.addEventListener('hashchange', handler)
    return () => window.removeEventListener('hashchange', handler)
  }, [])

  useEffect(() => {
    if (!isTauriRuntime()) return
    let cancelled = false
    let cleanup: (() => void) | undefined

    listen('chat-open-request', () => {
      const path = hashPath()
      // 全局 listen 会收到 emit_to("chat") 的事件（Tauri v2 的 Any 目标语义），所以 lens/translate/
      // settings/translator 窗也会收到这条广播。只有 chat 窗该响应，否则其它窗口会把自己导航成 chat。
      if (!isChatPath(path)) return
      if (path !== 'chat' && !isChatSettingsPath(path)) return
      const rememberedRoute = getRememberedChatRoute()
      if (rememberedRoute && rememberedRoute !== window.location.hash) {
        window.location.hash = rememberedRoute
        setMode('chat')
      }
    }).then((unlisten) => {
      if (cancelled) {
        unlisten()
      } else {
        cleanup = unlisten
      }
    }).catch((err) => {
      console.error('[App] Failed to listen for chat open requests:', err)
    })

    return () => {
      cancelled = true
      cleanup?.()
    }
  }, [])

  const persistChatWindowGeometry = useCallback(async () => {
    if (!isTauriRuntime()) return
    try {
      const win = (await import('@tauri-apps/api/window')).getCurrentWindow()
      const geometry = await snapshotChatWindowGeometry(win)
      if (geometry) rememberChatGeometry(geometry)
    } catch (err) {
      console.error('[App] Failed to remember chat window geometry:', err)
    }
  }, [])

  const revealChatWindow = useCallback(async () => {
    if (!isTauriRuntime()) return
    try {
      const win = (await import('@tauri-apps/api/window')).getCurrentWindow()
      const [visible, minimized] = await Promise.all([win.isVisible(), win.isMinimized()])
      const placementVisible = visible && !minimized
        ? await isChatWindowPlacementVisible(win)
        : false
      if (!visible || minimized || !placementVisible) {
        if (minimized) {
          await win.unminimize()
        }
        await restoreChatWindowGeometry(win)
        await api.showWindow()
        await api.focusWindow()
      }
      await persistChatWindowGeometry()
    } catch (err) {
      console.error('[App] Failed to reveal chat window:', err)
    }
  }, [persistChatWindowGeometry])

  // 首次创建 chat 窗口时后端保持 hidden，把 show 交给前端；此处再把 show 从“App 挂载即弹出”
  // 推迟到“Chat 首屏内容就绪”（onContentReady → revealChatWindowNow），避免窗口弹出后还在转圈。
  const revealedRef = useRef(false)
  const revealTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const revealChatWindowNow = useCallback(() => {
    if (revealedRef.current) return
    revealedRef.current = true
    if (revealTimerRef.current !== undefined) {
      clearTimeout(revealTimerRef.current)
      revealTimerRef.current = undefined
    }
    void revealChatWindow()
  }, [revealChatWindow])

  useLayoutEffect(() => {
    if (mode !== 'chat') return
    if (!isTauriRuntime()) return
    // 不变量：chat 是专用窗口，其 hash 恒为 #chat（含子路由），mode 一旦为 'chat' 便不再变。
    // 本兜底据此成立——若未来 chat 窗允许 mode 离开 'chat'，cleanup 会清掉未触发的兜底 timer
    // 而新分支早退，可能导致窗口永久 hidden；届时需改为窗口存活期内独立保证 reveal。
    // 已 reveal 过（防御性：正常不会二次进入）→ 直接校正一次几何/可见性。
    if (revealedRef.current) {
      void revealChatWindow()
      return
    }
    // 兜底：内容就绪信号 3s 内未到达（chunk 加载失败 / 组件抛错被 ErrorBoundary 接住 / 信号丢失）
    // 也强制 show，绝不让窗口永久 hidden。
    revealTimerRef.current = setTimeout(() => {
      revealChatWindowNow()
    }, 3000)
    return () => {
      if (revealTimerRef.current !== undefined) {
        clearTimeout(revealTimerRef.current)
        revealTimerRef.current = undefined
      }
    }
  }, [mode, revealChatWindow, revealChatWindowNow])

  useEffect(() => {
    if (mode !== 'chat') return
    if (!isTauriRuntime()) return
    let cancelled = false
    let unlistenResize: (() => void) | undefined
    let unlistenMove: (() => void) | undefined
    let readyToRemember = false
    let geomTimer: ReturnType<typeof setTimeout> | undefined

    const setup = async () => {
      try {
        const win = (await import('@tauri-apps/api/window')).getCurrentWindow()
        await new Promise(resolve => window.setTimeout(resolve, 0))
        if (!cancelled) readyToRemember = true

        // resize/move 在拖动中高频触发；几何持久化（多次 IPC 读尺寸 + 写 store）debounce 到停止后做一次，
        // 否则每帧都发 IPC 会和窗口伸缩/拖动的渲染抢资源，造成明显卡顿（Windows/WebView2 尤甚）。
        const persistIfReady = () => {
          if (!readyToRemember || cancelled) return
          if (geomTimer !== undefined) clearTimeout(geomTimer)
          geomTimer = setTimeout(() => {
            if (!cancelled) void persistChatWindowGeometry()
          }, 250)
        }

        const resizeHandler = await win.onResized(() => {
          persistIfReady()
        })
        const moveHandler = await win.onMoved(() => {
          persistIfReady()
        })
        if (cancelled) {
          resizeHandler()
          moveHandler()
        } else {
          unlistenResize = resizeHandler
          unlistenMove = moveHandler
        }
      } catch (err) {
        console.error('[App] Failed to track chat window geometry:', err)
      }
    }

    void setup()
    return () => {
      cancelled = true
      if (geomTimer !== undefined) clearTimeout(geomTimer)
      unlistenResize?.()
      unlistenMove?.()
    }
  }, [mode, persistChatWindowGeometry])

  // 根据当前模式调整窗口大小
  useEffect(() => {
    const resize = async () => {
      if (mode === '' || mode === 'translator') {
        await api.resizeWindow(392, 152)
      }
    }
    resize()
  }, [mode])

  // 打开设置页
  const openSettings = async () => {
    try {
      await api.openSettingsWindow()
      await api.closeTranslatorWindow()
    } catch (err) {
      console.error('[App] Error opening settings window:', err)
    }
  }

  // 根据当前模式渲染对应视图
  if (mode === 'lens') {
    return (
      <Suspense fallback={null}>
        <Lens />
      </Suspense>
    )
  }
  if (mode === 'chat') {
    return (
      <ChatWindowHost translucentSidebar={translucentSidebar}>
        <Suspense
          fallback={
            <div className="flex h-full w-full items-center justify-center bg-transparent">
              <div className="h-6 w-6 animate-spin rounded-full border-2 border-neutral-300 border-t-neutral-800 dark:border-neutral-700 dark:border-t-neutral-200" />
            </div>
          }
        >
          <ChatErrorBoundary>
            <Chat onSettingsChange={applyTheme} onContentReady={revealChatWindowNow} />
          </ChatErrorBoundary>
        </Suspense>
      </ChatWindowHost>
    )
  }
  return <Translator translateSource={translateSource} lang={lang} onOpenSettings={openSettings} />
}

export default App
