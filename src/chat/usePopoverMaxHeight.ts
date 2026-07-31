import { useLayoutEffect, useState, type RefObject } from 'react'

// 弹层高度按其在视口中的位置收缩到窗口边缘，避免窗口变小时底部（或顶部）被裁、末项点不到。
// 传入弹层自身的 ref 与展开方向（down = 向下 top-full / up = 向上 bottom-full）。
// 测的是弹层的 top/bottom（由触发点锚定，与自身高度无关），故无反馈循环。
// 必须是 layout effect：useEffect 在绘制之后跑，maxHeight 还是 undefined 的那一帧
// 弹层按完整列表高度铺开（模型多时直接顶到窗口外），下一帧才收回去 —— 就是那下闪。
export function usePopoverMaxHeight(
  open: boolean,
  ref: RefObject<HTMLElement | null>,
  dir: 'up' | 'down',
  max = 480,
): number | undefined {
  const [maxH, setMaxH] = useState<number | undefined>(undefined)
  useLayoutEffect(() => {
    if (!open) {
      setMaxH(undefined)
      return
    }
    const measure = () => {
      const rect = ref.current?.getBoundingClientRect()
      if (!rect) return
      const avail = dir === 'down' ? window.innerHeight - rect.top - 8 : rect.bottom - 8
      setMaxH(Math.max(160, Math.min(max, avail)))
    }
    measure()
    window.addEventListener('resize', measure)
    return () => window.removeEventListener('resize', measure)
  }, [open, dir, ref, max])
  return maxH
}
