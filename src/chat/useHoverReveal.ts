import { useEffect, useRef, useState } from 'react'

/** 悬停显隐（事件驱动）：把 `ref` 挂在要显隐的元素上，指针悬停**该元素自身**的
 *  区域时 `hovered` 为 true。元素平时 opacity-0 但保留布局占位，仍可命中指针。
 *
 *  两个刻意选择：
 *  - 监听挂元素自身而非外层消息容器：交互上「鼠标停到那一条的位置才显示」；
 *    实现上没有 closest/跨元素绑定，虚拟列表卸载重挂也不会漂（此前挂根容器的
 *    版本在部分消息上会失联）。
 *  - 不用 CSS `:hover`：macOS WKWebView 存在 :hover 粘滞——鼠标移出后样式不失效
 *    （Chromium 下同一套 CSS 实测正常）。事件派发与 :hover 样式失效在 WebKit 里
 *    是两条路径，pointerleave 可靠。窗口失焦一并收起，兜住光标快速甩出窗口时
 *    漏发 pointerleave 的场景。 */
export function useHoverReveal<T extends HTMLElement>() {
  const ref = useRef<T>(null)
  const [hovered, setHovered] = useState(false)
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const show = () => setHovered(true)
    const hide = () => setHovered(false)
    el.addEventListener('pointerenter', show)
    el.addEventListener('pointerleave', hide)
    window.addEventListener('blur', hide)
    return () => {
      el.removeEventListener('pointerenter', show)
      el.removeEventListener('pointerleave', hide)
      window.removeEventListener('blur', hide)
    }
  }, [])
  return { ref, hovered }
}
