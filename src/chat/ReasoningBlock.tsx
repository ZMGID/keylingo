import { useEffect, useRef, useState } from 'react'
import { ChevronDown } from 'lucide-react'

type ReasoningBlockProps = {
  reasoning: string
  /** 思维链正在流式写入 */
  streaming?: boolean
  /** 已知思考耗时，用于流式完成后继续展示 */
  durationMs?: number | null
}

function formatThinkingDuration(durationMs: number | null | undefined): string {
  if (durationMs == null || !Number.isFinite(durationMs) || durationMs <= 0) return ''
  const totalSeconds = Math.max(1, Math.round(durationMs / 1000))
  if (totalSeconds < 60) return `${totalSeconds}s`
  const minutes = Math.floor(totalSeconds / 60)
  const seconds = totalSeconds % 60
  return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`
}

export function ReasoningBlock({ reasoning, streaming = false, durationMs = null }: ReasoningBlockProps) {
  const collapsible = reasoning.trim().length > 0
  const [open, setOpen] = useState(false)
  const [contentPulse, setContentPulse] = useState(false)
  const [bodyMaxHeight, setBodyMaxHeight] = useState<number | null>(null)
  const [liveDurationMs, setLiveDurationMs] = useState(0)
  const userExpandedRef = useRef(false)
  const durationStartedAtRef = useRef<number | null>(null)
  const bodyRef = useRef<HTMLDivElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  const showCollapsed = collapsible && !open
  /** 生成完毕的折叠态只留标题行，正文完全隐藏 */
  const hideBody = !streaming && showCollapsed

  useEffect(() => {
    if (!streaming) return
    setContentPulse(true)
    const timer = window.setTimeout(() => setContentPulse(false), 220)
    return () => window.clearTimeout(timer)
  }, [reasoning, streaming])

  useEffect(() => {
    if (!streaming || !collapsible) {
      durationStartedAtRef.current = null
      setLiveDurationMs(0)
      return
    }

    if (durationStartedAtRef.current == null) {
      durationStartedAtRef.current = Date.now() - (durationMs ?? 0)
    }

    const updateDuration = () => {
      const startedAt = durationStartedAtRef.current
      if (startedAt == null) return
      setLiveDurationMs(Date.now() - startedAt)
    }
    updateDuration()
    const interval = window.setInterval(updateDuration, 1000)
    return () => window.clearInterval(interval)
  }, [collapsible, durationMs, streaming])

  useEffect(() => {
    if (!streaming && collapsible && !userExpandedRef.current) {
      setOpen(false)
    }
  }, [streaming, collapsible])

  // 流式期间不要测量 max-height：思考文本每个 delta 都在增删，重设 max-height 会重启一段
  // 缓动过渡，内容高度于是逐帧变化（实测一次收起跑 18 帧、1061→919px）。消息区的钉底跟随
  // 由 ResizeObserver 驱动，每一帧都会被当成内容增长而重钉一次 scrollTop —— 表现为流式时
  // 整个消息区抖动、像被强制滚动。折叠/展开是离散的用户操作，仍走动画。
  useEffect(() => {
    const body = bodyRef.current
    if (!body || !collapsible || streaming) {
      setBodyMaxHeight(null)
      return
    }
    setBodyMaxHeight(hideBody ? 0 : body.scrollHeight)
  }, [collapsible, open, reasoning, hideBody, streaming])

  useEffect(() => {
    if (!streaming || hideBody) return
    const scrollBox = scrollRef.current
    if (!scrollBox) return
    scrollBox.scrollTop = scrollBox.scrollHeight
  }, [reasoning, streaming, hideBody, open])

  const titleClass =
    'mb-1 flex w-full items-center gap-1 text-left text-[12.5px] font-medium text-neutral-700 transition-colors dark:text-neutral-200'
  const bodyClass = [
    'chat-motion-reasoning-body',
    streaming ? 'opacity-95' : 'opacity-90',
    showCollapsed ? 'is-collapsed' : 'is-open',
  ].join(' ')
  const scrollClass = [
    'reasoning-scroll-box custom-scrollbar',
    streaming ? 'is-streaming' : 'is-expanded',
    contentPulse ? 'reasoning-stream-tail' : '',
  ].join(' ')

  const handleToggle = () => {
    userExpandedRef.current = true
    setOpen((value) => !value)
  }
  const visibleReasoning = reasoning.trimEnd()
  const thinkingDuration = formatThinkingDuration(streaming ? (durationMs ?? liveDurationMs) : durationMs)
  const titleText = streaming ? 'Thinking…' : 'Thinking'

  return (
    <section
      aria-label="Thinking"
      className={`mb-3 border-l pl-3 transition-colors duration-[var(--kv-dur-normal)] ease-[var(--kv-ease-out)] ${
        streaming
          ? 'border-neutral-300 dark:border-neutral-600'
          : 'border-neutral-200 dark:border-neutral-700'
      }`}
    >
      {collapsible ? (
        <button
          type="button"
          onClick={handleToggle}
          className={`${titleClass} hover:text-neutral-900 dark:hover:text-neutral-50`}
          aria-expanded={!hideBody}
          data-tauri-drag-region="false"
        >
          <span className="inline-flex min-w-0 items-baseline gap-1.5">
            {streaming ? (
              <span className="reasoning-shimmer-text">{titleText}</span>
            ) : (
              <span>{titleText}</span>
            )}
            {thinkingDuration && (
              <span className="shrink-0 text-[11px] font-normal text-neutral-400 dark:text-neutral-500">
                {thinkingDuration}
              </span>
            )}
          </span>
          <ChevronDown
            size={12}
            strokeWidth={2}
            className={`shrink-0 transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] ${open ? 'rotate-180' : ''}`}
          />
        </button>
      ) : (
        <div className={titleClass}>
          <span className="inline-flex min-w-0 items-baseline gap-1.5">
            {streaming ? (
              <span className="reasoning-shimmer-text">{titleText}</span>
            ) : (
              <span>{titleText}</span>
            )}
            {thinkingDuration && (
              <span className="shrink-0 text-[11px] font-normal text-neutral-400 dark:text-neutral-500">
                {thinkingDuration}
              </span>
            )}
          </span>
        </div>
      )}

      <div
        ref={bodyRef}
        className={bodyClass}
        aria-hidden={hideBody}
        style={
          bodyMaxHeight == null
            ? (hideBody ? { maxHeight: '0px' } : undefined)
            : { maxHeight: `${bodyMaxHeight}px` }
        }
      >
        {collapsible && (
          <div data-testid="reasoning-frame" className="reasoning-scroll-frame">
            <div
              ref={scrollRef}
              data-testid="reasoning-scroll"
              className={scrollClass}
            >
              <div data-testid="reasoning-text" className="reasoning-plain-text">
                {visibleReasoning}
              </div>
            </div>
          </div>
        )}
      </div>
    </section>
  )
}
