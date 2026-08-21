import { useLayoutEffect, useState } from 'react'
import { i18n, type Lang } from '../settings/i18n'
import type { ContextClearBoundaryView } from './contextClearBoundary'

interface ContextClearDividerProps {
  boundary: ContextClearBoundaryView
  lang?: Lang
  animate?: boolean
}

export function ContextClearDivider({ boundary, lang = 'zh', animate = false }: ContextClearDividerProps) {
  const t = i18n[lang]
  const [entered, setEntered] = useState(!animate)

  useLayoutEffect(() => {
    if (!animate) {
      setEntered(true)
      return
    }
    setEntered(false)
    let cancelled = false
    const frame = requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (!cancelled) setEntered(true)
      })
    })
    return () => {
      cancelled = true
      cancelAnimationFrame(frame)
    }
  }, [animate, boundary.record.id])

  return (
    <div
      className={`chat-compaction-divider ${entered ? 'chat-compaction-divider--animate' : 'chat-compaction-divider--pre-enter'}`}
      data-context-clear-divider-id={boundary.record.id}
      role="separator"
      aria-label={t.contextClearDividerAria}
    >
      <span className="chat-compaction-divider-line" aria-hidden="true" />
      <span className="chat-context-clear-badge">{t.contextClearDivider}</span>
      <span className="chat-compaction-divider-line" aria-hidden="true" />
    </div>
  )
}
