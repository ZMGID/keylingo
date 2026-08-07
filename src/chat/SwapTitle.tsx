import { useEffect, useRef, useState } from 'react'

/** 打字机逐字间隔（ms）。标题 ≤14 汉字时全程 ~0.6s，干脆不拖沓。 */
const TYPE_INTERVAL_MS = 40

/**
 * 会话标题替换过渡（打字机）：`text` 变化时清空旧文字，新标题逐字打出，末尾带闪烁光标。
 * 首次挂载不播（行级入场由 chat-motion-row 负责）；连续快速变化时取消上一轮、从空重新打字。
 */
export function SwapTitle({
  text,
  title,
  className,
}: {
  text: string
  /** 传给原生 title 属性的悬浮提示；不传则无（由外层按钮统一提供）。 */
  title?: string
  className?: string
}) {
  const [display, setDisplay] = useState(text)
  const [typing, setTyping] = useState(false)
  const prevRef = useRef(text)
  const timerRef = useRef<number | null>(null)

  useEffect(() => {
    if (prevRef.current === text) return
    prevRef.current = text
    if (timerRef.current !== null) window.clearInterval(timerRef.current)
    // 从空开始逐字打出新标题（按码点切分，避免拆散代理对字符）
    const chars = Array.from(text)
    setDisplay('')
    setTyping(true)
    let index = 0
    timerRef.current = window.setInterval(() => {
      index += 1
      setDisplay(chars.slice(0, index).join(''))
      if (index >= chars.length) {
        if (timerRef.current !== null) window.clearInterval(timerRef.current)
        timerRef.current = null
        setTyping(false)
      }
    }, TYPE_INTERVAL_MS)
  }, [text])

  useEffect(() => {
    return () => {
      if (timerRef.current !== null) window.clearInterval(timerRef.current)
    }
  }, [])

  return (
    <span className={className} title={title}>
      {display}
      {typing && <span className="kv-title-caret" aria-hidden="true" />}
    </span>
  )
}
