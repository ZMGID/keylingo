import { useLayoutEffect, useRef, useState, type RefObject } from 'react'

/** fixed 定位的右键/上下文菜单：挂载后按实际尺寸把坐标夹回视口内（8px 边距），
 *  避免锚点靠近窗口底/右缘时菜单被裁出屏幕外。返回夹取后的坐标。 */
export function useClampedMenuPosition(
  ref: RefObject<HTMLElement | null>,
  anchor: { left: number; top: number },
): { left: number; top: number } {
  const [pos, setPos] = useState(anchor)
  const anchorRef = useRef(anchor)
  // anchor 变化（同一菜单换目标重开）时重新夹取。
  if (
    anchorRef.current.left !== anchor.left ||
    anchorRef.current.top !== anchor.top
  ) {
    anchorRef.current = anchor
  }
  const current = anchorRef.current
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const margin = 8
    setPos({
      left: Math.max(margin, Math.min(current.left, window.innerWidth - rect.width - margin)),
      top: Math.max(margin, Math.min(current.top, window.innerHeight - rect.height - margin)),
    })
  }, [ref, current])
  return pos
}
