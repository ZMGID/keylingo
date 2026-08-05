import { useEffect, useRef, useState } from 'react'

/**
 * 指针拖拽重排。原本长在 `settings/ProviderSortableList` 里，侧栏的集/项目也要用，
 * 所以抽出来共用 —— 里面有踩过坑的知识（落点全程用「起始索引 + 位移/行高」算出，
 * **绝不**读拖动中被 transform 位移的 rect，那是旧实现抖动与重叠的根源），复制一份必然分叉。
 *
 * 前提：**所有行等高**。行高不等的调用方（侧栏分组可展开）必须在 `onDragStart` 里
 * 先把列表变成等高（全折叠），并在 `onDragEnd` 里恢复。
 */
export type PointerReorderOptions = {
  /** 当前显示顺序的 id 列表。 */
  ids: string[]
  /** 滚动容器；拖到上下边缘时自动滚动。 */
  listRef: React.RefObject<HTMLElement | null>
  /** 从 `.closest()` 找行元素用的选择器（拖拽单元）。 */
  itemSelector: string
  /**
   * 量行高用的选择器，默认同 `itemSelector`。
   * 拖拽单元高度会变的调用方**必须**指定一个高度恒定的内层元素（如分组头行）：
   * `onDragStart` 里的 collapse 是 React 异步 setState，量高发生在它生效之前，
   * 量外壳会量到展开态的高度，一格被当成几百 px，其余行直接飞出可视区。
   */
  measureSelector?: string
  /** 行与行之间的间距（px）。落点 = 起始索引 + 位移/(行高+间距)，写错会累积偏格。 */
  rowGap?: number
  /** 落点确定且发生了移动时调用。 */
  onReorder: (fromId: string, toId: string) => void
  onDragStart?: () => void
  onDragEnd?: () => void
}

const clamp = (v: number, min: number, max: number) => Math.max(min, Math.min(max, v))

/** 把 `id` 移到最终下标 `toIndex`（已扣掉自身），返回新的 id 顺序。 */
export function moveIdToIndex(ids: string[], id: string, toIndex: number): string[] {
  const from = ids.indexOf(id)
  if (from < 0) return ids
  const to = Math.min(Math.max(toIndex, 0), ids.length - 1)
  if (from === to) return ids
  const next = [...ids]
  next.splice(from, 1)
  next.splice(to, 0, id)
  return next
}

/**
 * 按 id 顺序重排对象数组（乐观更新本地列表用）。
 * `ids` 没提到的项排在最前、保持原相对顺序 —— 与后端 `reorder_by_ids` 同一规则，
 * 两边不一致会让乐观更新和落盘结果对不上。
 */
export function applyIdOrder<T extends { id: string }>(items: T[], ids: string[]): T[] {
  const rank = new Map(ids.map((id, i) => [id, i]))
  return [...items].sort((a, b) => (rank.get(a.id) ?? -1) - (rank.get(b.id) ?? -1))
}

export function usePointerReorder({
  ids,
  listRef,
  itemSelector,
  measureSelector,
  rowGap = 1,
  onReorder,
  onDragStart,
  onDragEnd,
}: PointerReorderOptions) {
  const [draggingId, setDraggingId] = useState<string | null>(null)
  const [overIndex, setOverIndex] = useState<number | null>(null)
  const [dragOffsetY, setDragOffsetY] = useState(0)
  const rowHeight = useRef(31)

  const draggingIndex = draggingId ? ids.indexOf(draggingId) : -1

  useEffect(() => {
    if (!draggingId) return
    const prev = document.body.style.userSelect
    document.body.style.userSelect = 'none'
    return () => {
      document.body.style.userSelect = prev
    }
  }, [draggingId])

  const startDrag = (e: React.PointerEvent<HTMLElement>, id: string, index: number) => {
    e.preventDefault()
    e.stopPropagation()
    const list = listRef.current
    const handle = e.currentTarget as HTMLElement
    const item = handle.closest(itemSelector) as HTMLElement | null
    if (!list || !item) return
    // 量高的元素必须高度恒定 —— 见 measureSelector 的注释。
    const measured = measureSelector
      ? ((handle.closest(measureSelector) as HTMLElement | null) ?? item)
      : item

    onDragStart?.()

    // 行高 + 行间距。索引全程用「起始索引 + 位移/行距」算出，
    // 绝不读拖动中被 transform/过渡位移的 rect（旧实现的抖动与重叠根源）。
    const rowH = (measured.getBoundingClientRect().height || 30) + rowGap
    rowHeight.current = rowH
    const startY = e.clientY
    const startScrollTop = list.scrollTop
    const maxIndex = ids.length - 1
    let lastY = startY
    let raf = 0

    setDraggingId(id)
    setOverIndex(index)
    setDragOffsetY(0)

    const currentOffset = () => {
      const raw = lastY - startY + (list.scrollTop - startScrollTop)
      return clamp(raw, -index * rowH, (maxIndex - index) * rowH)
    }

    const update = () => {
      const offset = currentOffset()
      setDragOffsetY(offset)
      setOverIndex(clamp(index + Math.round(offset / rowH), 0, maxIndex))
    }

    // 指针悬在列表上下边缘时持续滚动（pointermove 不动时也要滚，所以走 rAF）
    const autoScroll = () => {
      const rect = list.getBoundingClientRect()
      const zone = 24
      let delta = 0
      if (lastY < rect.top + zone) delta = -Math.min(10, Math.ceil((rect.top + zone - lastY) / 4))
      else if (lastY > rect.bottom - zone)
        delta = Math.min(10, Math.ceil((lastY - (rect.bottom - zone)) / 4))
      if (delta) {
        const prev = list.scrollTop
        list.scrollTop += delta
        if (list.scrollTop !== prev) update()
      }
      raf = requestAnimationFrame(autoScroll)
    }
    raf = requestAnimationFrame(autoScroll)

    const onMove = (ev: PointerEvent) => {
      lastY = ev.clientY
      update()
    }

    const onUp = () => {
      cancelAnimationFrame(raf)
      document.removeEventListener('pointermove', onMove)
      document.removeEventListener('pointerup', onUp)
      document.removeEventListener('pointercancel', onUp)
      const toIndex = clamp(index + Math.round(currentOffset() / rowH), 0, maxIndex)
      const toId = ids[toIndex]
      if (toId && toId !== id) onReorder(id, toId)
      setDraggingId(null)
      setOverIndex(null)
      setDragOffsetY(0)
      onDragEnd?.()
    }

    try {
      e.currentTarget.setPointerCapture(e.pointerId)
    } catch {
      /* 捕获失败不致命，document 监听仍覆盖窗口内拖动 */
    }
    document.addEventListener('pointermove', onMove)
    document.addEventListener('pointerup', onUp)
    document.addEventListener('pointercancel', onUp)
  }

  const itemTransform = (index: number) => {
    if (draggingIndex < 0 || overIndex === null) return undefined
    const h = rowHeight.current
    if (index === draggingIndex) return `translateY(${dragOffsetY}px)`
    if (draggingIndex < overIndex) {
      if (index > draggingIndex && index <= overIndex) return `translateY(${-h}px)`
    } else if (draggingIndex > overIndex) {
      if (index >= overIndex && index < draggingIndex) return `translateY(${h}px)`
    }
    return undefined
  }

  return { draggingId, startDrag, itemTransform }
}
