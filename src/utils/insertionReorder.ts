import { useEffect, useRef, useState } from 'react'

/**
 * 插入线式拖拽重排：
 * - **源行原地保留**（变淡作占位，不位移，所以列表不会塌一个洞）；
 * - 一张跟着指针走的浮起小卡片表示"你正在搬的是这个"；
 * - 目标位置画一条插入线。
 *
 * 没有拖动把手：**按住行本身**移动超过 `DRAG_THRESHOLD` 才进入拖拽，否则算普通点击
 * （所以 pointerdown 时不能 preventDefault，会连点击和聚焦一起废掉）。真的拖过之后
 * 吞掉紧随其后的那次 click，否则放手瞬间会顺手把这一行选中。
 * 行上带 `data-no-drag` 的控件（菜单、新建等）不触发拖拽。
 *
 * 与 `usePointerReorder`（供应商列表那套：其余行上下让位、要求等高）是两条路，别合并：
 * 这套按每行真实位置做命中判定，**不要求行高相等**，可展开的分组行、高度不齐的对话行
 * 都能直接用，也不需要「拖拽时先全折叠」那种 hack。
 *
 * 位置一律用 `offsetTop`/`offsetHeight`（相对滚动内容，不随滚动变化），
 * 不用 `getBoundingClientRect`（视口坐标，边缘自动滚动时会整批失效）。
 */
export type InsertionReorderOptions = {
  /** 滚动容器；也是行位置的参照系，需要 `position: relative`。 */
  listRef: React.RefObject<HTMLElement | null>
  /** 行元素选择器，行上须带 `data-reorder-id`。 */
  rowSelector: string
  /**
   * 放手时回调，`toIndex` 是移除拖动项之后的最终下标。原地不动时不触发。
   * `scopeId` 是最近的 `[data-reorder-scope]` 祖先的值（没有则为空串）——
   * 侧栏靠它区分「拖的是哪个集/项目里的对话」，否则多个分组同时展开时行会互串。
   */
  onDrop: (id: string, toIndex: number, scopeId: string) => void
}

type Row = { id: string; top: number; height: number }

const DRAG_THRESHOLD = 5

export function useInsertionReorder({ listRef, rowSelector, onDrop }: InsertionReorderOptions) {
  const [draggingId, setDraggingId] = useState<string | null>(null)
  /** 插入位置（0..n），null = 不显示线。 */
  const [dropSlot, setDropSlot] = useState<number | null>(null)
  /** 浮起卡片的视口坐标。 */
  const [ghostPos, setGhostPos] = useState<{ x: number; y: number } | null>(null)
  const rowsRef = useRef<Row[]>([])

  useEffect(() => {
    if (!draggingId) return
    const prev = document.body.style.userSelect
    document.body.style.userSelect = 'none'
    return () => {
      document.body.style.userSelect = prev
    }
  }, [draggingId])

  /** 挂在行的 onPointerDown 上：先只是「预备」，越过阈值才真进入拖拽。 */
  const startDrag = (e: React.PointerEvent<HTMLElement>, id: string) => {
    if (e.button !== 0) return
    const target = e.target as HTMLElement
    if (target.closest('[data-no-drag]')) return
    const list = listRef.current
    if (!list) return

    const rowEl = target.closest(rowSelector) as HTMLElement | null
    const scopeEl = (target.closest('[data-reorder-scope]') as HTMLElement | null) ?? list
    const scopeId = scopeEl === list ? '' : (scopeEl.dataset.reorderScope ?? '')
    if (!rowEl) return

    const startY = e.clientY
    const startX = e.clientX
    let lastY = startY
    let lastX = startX
    let active = false
    let rows: Row[] = []
    let fromIndex = -1
    let raf = 0

    const slotFor = (y: number) => {
      for (let i = 0; i < rows.length; i += 1) {
        if (y < rows[i].top + rows[i].height / 2) return i
      }
      return rows.length
    }

    /** 指针在滚动内容里的 y。 */
    const contentY = () => lastY - list.getBoundingClientRect().top + list.scrollTop

    const update = () => {
      setGhostPos({ x: lastX, y: lastY })
      setDropSlot(slotFor(contentY()))
    }

    // 悬在上下边缘时持续滚动（指针不动也要滚，所以走 rAF）
    const autoScroll = () => {
      const rect = list.getBoundingClientRect()
      const zone = 24
      let delta = 0
      if (lastY < rect.top + zone) delta = -Math.min(10, Math.ceil((rect.top + zone - lastY) / 4))
      else if (lastY > rect.bottom - zone)
        delta = Math.min(10, Math.ceil((lastY - (rect.bottom - zone)) / 4))
      if (delta) {
        const before = list.scrollTop
        list.scrollTop += delta
        if (list.scrollTop !== before) update()
      }
      raf = requestAnimationFrame(autoScroll)
    }

    const begin = () => {
      // 拖拽期间没有任何行改变布局（源行只变淡、不位移），所以开局快照一次就够。
      rows = Array.from(scopeEl.querySelectorAll<HTMLElement>(rowSelector)).map((el) => ({
        id: el.dataset.reorderId ?? '',
        top: el.offsetTop,
        height: el.offsetHeight,
      }))
      fromIndex = rows.findIndex((row) => row.id === id)
      if (rows.length === 0 || fromIndex < 0) return false
      rowsRef.current = rows // 插入线的位置从这份快照算
      active = true
      setDraggingId(id)
      update()
      raf = requestAnimationFrame(autoScroll)
      return true
    }

    const onMove = (ev: PointerEvent) => {
      lastY = ev.clientY
      lastX = ev.clientX
      if (!active) {
        if (Math.abs(lastY - startY) < DRAG_THRESHOLD && Math.abs(lastX - startX) < DRAG_THRESHOLD) {
          return
        }
        if (!begin()) {
          cleanup()
          return
        }
      }
      ev.preventDefault()
      update()
    }

    function cleanup() {
      cancelAnimationFrame(raf)
      document.removeEventListener('pointermove', onMove)
      document.removeEventListener('pointerup', onUp)
      document.removeEventListener('pointercancel', onUp)
    }

    function onUp() {
      const wasActive = active
      const slot = wasActive ? slotFor(contentY()) : -1
      cleanup()
      setDraggingId(null)
      setDropSlot(null)
      setGhostPos(null)
      if (!wasActive) return
      // 真拖过就不该顺手把这一行选中。
      document.addEventListener(
        'click',
        (ev) => {
          ev.stopPropagation()
          ev.preventDefault()
        },
        { capture: true, once: true },
      )
      rowsRef.current = rows
      // 插入位是「移除前」的槽位；移除拖动项后，落在它后面的槽位要减一。
      const toIndex = slot > fromIndex ? slot - 1 : slot
      if (toIndex !== fromIndex) onDrop(id, toIndex, scopeId)
    }

    document.addEventListener('pointermove', onMove)
    document.addEventListener('pointerup', onUp)
    document.addEventListener('pointercancel', onUp)
  }

  /** 插入线的 top（相对滚动内容）；不在拖拽中返回 null。 */
  const lineTop = (() => {
    if (dropSlot === null || !draggingId) return null
    const rows = rowsRef.current
    if (rows.length === 0) return null
    if (dropSlot <= 0) return rows[0].top
    const prev = rows[Math.min(dropSlot, rows.length) - 1]
    return prev.top + prev.height
  })()

  return { draggingId, lineTop, ghostPos, startDrag }
}
