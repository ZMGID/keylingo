import { useEffect, useRef, useState } from 'react'

/** 悬停显隐（事件驱动）：把 `ref` 挂在要显隐的元素上，它会向上找最近的
 *  `[data-hover-reveal-root]` 容器挂 pointerenter/pointerleave，悬停容器时 `hovered`
 *  为 true。
 *
 *  不用 CSS `:hover`（group-hover）的原因：macOS WKWebView 存在 :hover 粘滞——鼠标
 *  移出后样式不失效（Chromium 下同一套 CSS 实测正常）。事件派发与 :hover 样式失效在
 *  WebKit 里是两条路径，pointerleave 可靠。窗口失焦一并收起，兜住光标快速甩出窗口时
 *  漏发 pointerleave 的场景。 */
export function useHoverReveal<T extends HTMLElement>() {
  const ref = useRef<T>(null)
  const [hovered, setHovered] = useState(false)
  useEffect(() => {
    const host = ref.current?.closest('[data-hover-reveal-root]')
    if (!(host instanceof HTMLElement)) return
    const show = () => setHovered(true)
    const hide = () => setHovered(false)
    host.addEventListener('pointerenter', show)
    host.addEventListener('pointerleave', hide)
    window.addEventListener('blur', hide)
    return () => {
      host.removeEventListener('pointerenter', show)
      host.removeEventListener('pointerleave', hide)
      window.removeEventListener('blur', hide)
    }
  }, [])
  return { ref, hovered }
}
