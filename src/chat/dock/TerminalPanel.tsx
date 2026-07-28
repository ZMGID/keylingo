// 终端面板：xterm.js 前端 + 后端 portable-pty PTY 会话（dock::terminal）。
// 面板常驻挂载（切 tab 不断 PTY），卸载即关会话；workdir 变化 / 「重启会话」时
// 走 effect cleanup 关旧建新。非 Tauri 运行时（npm run dev:ui）渲染占位说明。
// 渲染引擎与 VS Code 内置终端相同（xterm.js），主题随 App 深浅色切换。
import { useCallback, useEffect, useRef, useState } from 'react'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebglAddon } from '@xterm/addon-webgl'
import '@xterm/xterm/css/xterm.css'
import { RotateCw, XCircle } from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'
import { IconButton } from '../../components/Button'
import { isTauriRuntime } from '../utils'
import { dockApi } from './api'

const TERMINAL_OUTPUT_EVENT = 'dock:terminal-output'
const TERMINAL_EXIT_EVENT = 'dock:terminal-exit'

/** mono 栈 + Nerd Font 兜底（starship 等提示符的图标字形，普通 mono 字体没有会变豆腐块）。 */
const TERMINAL_FONT_FAMILY =
  'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Symbols Nerd Font Mono", "NerdFontsSymbols Nerd Font Mono", monospace'

/** 终端是常驻深色面（用户指定 #1e1e2d，对齐其参考终端），不随 App 深浅色翻转；
    文字/ANSI 用深色底调色板。 */
const TERMINAL_BG = '#1e1e2d'

const TERMINAL_THEME = {
  background: TERMINAL_BG,
  foreground: '#d4d4d8',
  cursor: '#e4e4e7',
  cursorAccent: '#18181b',
  selectionBackground: 'rgba(255, 255, 255, 0.22)',
  black: '#3f3f46',
  red: '#f87171',
  green: '#4ade80',
  yellow: '#facc15',
  blue: '#60a5fa',
  magenta: '#c084fc',
  cyan: '#22d3ee',
  white: '#e4e4e7',
  brightBlack: '#71717a',
  brightRed: '#fca5a5',
  brightGreen: '#86efac',
  brightYellow: '#fde047',
  brightBlue: '#93c5fd',
  brightMagenta: '#d8b4fe',
  brightCyan: '#67e8f9',
  brightWhite: '#fafafa',
} satisfies Record<string, string>

type TerminalPanelProps = {
  workdir: string
  active: boolean
  lang: Lang
}

export function TerminalPanel({ workdir, active, lang }: TerminalPanelProps) {
  const t = i18n[lang]
  const tauri = isTauriRuntime()

  const containerRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const sessionIdRef = useRef<string | null>(null)
  /** 自增触发 effect 重建会话（「重启会话」按钮）。 */
  const [restartNonce, setRestartNonce] = useState(0)
  // 退出提示文案要随语言切换，但输出发生在事件回调里 → 走 ref 避免重挂 effect。
  const exitedTextRef = useRef(t.dockTerminalExited)
  exitedTextRef.current = t.dockTerminalExited

  /** fit 量出的尺寸可能是 NaN（面板 hidden / 未渲染时），`??` 挡不住 NaN，
      而 JSON 会把 NaN 序列化成 null → 后端 u16 报 invalid type。 */
  const saneDim = (value: number | undefined, fallback: number, min: number): number =>
    Number.isFinite(value) && (value as number) >= min ? Math.floor(value as number) : fallback

  /** fit 并把量出的行列同步给 PTY；hidden（display:none）时量不出尺寸，直接跳过。 */
  const syncResize = useCallback(() => {
    const term = termRef.current
    const fit = fitRef.current
    if (!term || !fit) return
    const dims = fit.proposeDimensions()
    if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows) || dims.cols < 2 || dims.rows < 1) return
    fit.fit()
    const sid = sessionIdRef.current
    if (sid) void dockApi.terminalResize(sid, term.cols, term.rows).catch(() => {})
  }, [])

  // 会话生命周期 = 面板挂载 × workdir × 重启计数。
  useEffect(() => {
    if (!tauri || !workdir) return
    const container = containerRef.current
    if (!container) return

    let disposed = false
    const unlisteners: UnlistenFn[] = []
    /** create resolve 前到达的输出：先攒着，拿到 sessionId 后补写（否则丢首个 prompt）。 */
    const pendingOutput: string[] = []

    const term = new Terminal({
      fontSize: 12,
      fontFamily: TERMINAL_FONT_FAMILY,
      lineHeight: 1.25,
      cursorBlink: true,
      scrollback: 2000,
      theme: TERMINAL_THEME,
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(container)
    // WebGL 渲染（更平滑、高分屏更清晰），失败自动退回默认 canvas。
    try {
      term.loadAddon(new WebglAddon())
    } catch (error) {
      console.warn('[dock] webgl addon unavailable, fallback to canvas:', error)
    }
    termRef.current = term
    fitRef.current = fit

    const writeExitedLine = (code?: number) => {
      const suffix = code === undefined ? '' : ` (code ${code})`
      term.write(`\r\n\x1b[2m[${exitedTextRef.current}${suffix}]\x1b[0m\r\n`)
    }

    void listen<{ sessionId: string; data: string }>(TERMINAL_OUTPUT_EVENT, (event) => {
      if (event.payload.sessionId === sessionIdRef.current) {
        term.write(event.payload.data)
      } else if (!sessionIdRef.current) {
        pendingOutput.push(event.payload.data)
      }
    }).then((unlisten) => {
      if (disposed) unlisten()
      else unlisteners.push(unlisten)
    })
    void listen<{ sessionId: string; code: number }>(TERMINAL_EXIT_EVENT, (event) => {
      if (event.payload.sessionId !== sessionIdRef.current) return
      sessionIdRef.current = null
      writeExitedLine(event.payload.code)
    }).then((unlisten) => {
      if (disposed) unlisten()
      else unlisteners.push(unlisten)
    })

    const dataDisposable = term.onData((data) => {
      const sid = sessionIdRef.current
      if (sid) void dockApi.terminalWrite(sid, data).catch(() => {})
    })

    const dims = fit.proposeDimensions()
    dockApi
      .terminalCreate(
        workdir,
        saneDim(dims?.cols, 80, 2),
        saneDim(dims?.rows, 24, 1),
      )
      .then(({ sessionId }) => {
        // resolve 时面板已卸载：会话立刻关掉，不留孤儿。
        if (disposed) {
          void dockApi.terminalClose(sessionId).catch(() => {})
          return
        }
        sessionIdRef.current = sessionId
        for (const chunk of pendingOutput) term.write(chunk)
        pendingOutput.length = 0
      })
      .catch((err) => {
        if (!disposed) term.writeln(`\x1b[31m${String(err)}\x1b[0m`)
      })

    const resizeObserver = new ResizeObserver(() => syncResize())
    resizeObserver.observe(container)

    return () => {
      disposed = true
      resizeObserver.disconnect()
      for (const unlisten of unlisteners) unlisten()
      dataDisposable.dispose()
      const sid = sessionIdRef.current
      sessionIdRef.current = null
      if (sid) void dockApi.terminalClose(sid).catch(() => {})
      term.dispose()
      termRef.current = null
      fitRef.current = null
    }
    // syncResize 只读 ref，无需进依赖。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tauri, workdir, restartNonce])

  // 从 hidden 恢复（切回终端 tab / dock 重新展开）：尺寸可能变了，重新 fit + resize。
  useEffect(() => {
    if (!active) return
    const frame = window.requestAnimationFrame(() => syncResize())
    return () => window.cancelAnimationFrame(frame)
  }, [active, syncResize])

  const handleRestart = useCallback(() => {
    setRestartNonce((nonce) => nonce + 1)
  }, [])

  const handleKill = useCallback(() => {
    const sid = sessionIdRef.current
    sessionIdRef.current = null
    if (sid) void dockApi.terminalClose(sid).catch(() => {})
    // 后端 exit watcher 发现会话已摘除会静默退出，这里手动补一行提示。
    const term = termRef.current
    if (term) term.write(`\r\n\x1b[2m[${exitedTextRef.current}]\x1b[0m\r\n`)
  }, [])

  if (!tauri) {
    return (
      <div className="flex flex-1 items-center justify-center p-4 text-center text-[12px] text-neutral-400 dark:text-neutral-500">
        {t.dockTerminalUnavailable}
      </div>
    )
  }

  return (
    <>
      {/* 工具行：重启会话 / 关闭会话 */}
      <div className="flex shrink-0 items-center gap-0.5 border-b border-neutral-200/70 px-2 py-1 dark:border-neutral-700/50">
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-neutral-400 dark:text-neutral-500">
          {workdir}
        </span>
        <IconButton label={t.dockTerminalRestart} size="sm" variant="ghost" onClick={handleRestart}>
          <RotateCw size={12} />
        </IconButton>
        <IconButton label={t.dockTerminalKill} size="sm" variant="ghost" onClick={handleKill}>
          <XCircle size={12} />
        </IconButton>
      </div>
      <div ref={containerRef} className="min-h-0 flex-1 px-2 py-1 [&_.xterm]:h-full" />
    </>
  )
}
