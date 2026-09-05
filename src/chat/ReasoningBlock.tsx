import { useEffect, useRef, useState } from 'react'
import { ChatDisclosureBody } from './ChatDisclosureBody'

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
  const [liveDurationMs, setLiveDurationMs] = useState(0)
  const userExpandedRef = useRef(false)
  const durationStartedAtRef = useRef<number | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  const showCollapsed = collapsible && !open
  /** 生成完毕的折叠态只留标题行，正文完全隐藏 */
  const hideBody = !streaming && showCollapsed

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

  useEffect(() => {
    if (!streaming || hideBody) return
    const scrollBox = scrollRef.current
    if (!scrollBox) return
    scrollBox.scrollTop = scrollBox.scrollHeight
  }, [reasoning, streaming, hideBody, open])

  const titleClass =
    'mb-1 flex w-full items-center gap-1 text-left text-[11.5px] font-medium text-neutral-700 transition-colors dark:text-neutral-200'
  const scrollClass = [
    'reasoning-scroll-box custom-scrollbar',
    streaming ? 'is-streaming' : 'is-expanded',
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
          : 'border-[var(--border-input)]'
      }`}
    >
      {collapsible ? (
        <button
          type="button"
          onClick={handleToggle}
          className={`${titleClass} hover:text-neutral-900 dark:hover:text-neutral-50`}
          aria-expanded={!hideBody}
          data-chat-disclosure
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

      <ChatDisclosureBody open={!hideBody} animate={!streaming && userExpandedRef.current} keepMounted>
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
      </ChatDisclosureBody>
    </section>
  )
}
