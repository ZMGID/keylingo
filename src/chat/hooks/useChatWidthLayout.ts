import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from 'react'
import type { Virtualizer } from '@tanstack/react-virtual'
import type { ScrollFollowHandle } from '../scroll/useScrollFollow'

// Bound the measurement cache while dragging a window or sidebar. Mounted rows
// still report their exact heights within each bucket through ResizeObserver.
const WIDTH_BUCKET_PX = 32
const widthBucket = (width: number) => Math.max(280, Math.round(width / WIDTH_BUCKET_PX) * WIDTH_BUCKET_PX)

type ReadingAnchor = {
  row: HTMLElement
  index: number | null
  top: number
}

/** Width caches may change identity; the visible message must keep its position. */
export function useChatWidthLayout(
  content: HTMLElement | null,
  viewport: HTMLElement | null,
  follow: ScrollFollowHandle,
  navigationLocked: RefObject<boolean>,
) {
  const [contentWidth, setContentWidth] = useState(704)
  const widthRef = useRef(contentWidth)
  const anchorRef = useRef<ReadingAnchor | null>(null)
  const cancelAnchor = useCallback(() => { anchorRef.current = null }, [])

  const prepareWidthChange = useCallback((width: number) => {
    if (widthBucket(width) === widthRef.current || !content || !viewport
      || follow.isFollowing() || navigationLocked.current || anchorRef.current) return
    const bounds = viewport.getBoundingClientRect()
    // Row observers can run before the content-width observer. Capture before
    // their height deltas compensate scrollTop in the previous width layout.
    for (const row of content.querySelectorAll<HTMLElement>('[data-chat-reading-row]')) {
      const rect = row.getBoundingClientRect()
      if (rect.bottom <= bounds.top || rect.top >= bounds.bottom) continue
      anchorRef.current = {
        row,
        index: row.dataset.index === undefined ? null : Number(row.dataset.index),
        top: rect.top - bounds.top,
      }
      break
    }
  }, [content, follow, navigationLocked, viewport])

  useLayoutEffect(() => {
    if (!content) return
    const updateWidth = (width: number) => {
      const next = widthBucket(width)
      if (next === widthRef.current) return
      prepareWidthChange(width)
      widthRef.current = next
      setContentWidth(next)
    }
    const style = getComputedStyle(content)
    updateWidth(content.clientWidth - parseFloat(style.paddingLeft || '0') - parseFloat(style.paddingRight || '0'))
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width
      if (typeof width === 'number') updateWidth(width)
    })
    observer.observe(content)
    return () => observer.disconnect()
  }, [content, prepareWidthChange])

  useLayoutEffect(() => {
    if (!viewport) return
    // A new reading action takes precedence over the pending layout correction.
    for (const event of ['wheel', 'touchstart', 'pointerdown', 'keydown']) {
      viewport.addEventListener(event, cancelAnchor, { capture: true, passive: true })
    }
    return () => {
      for (const event of ['wheel', 'touchstart', 'pointerdown', 'keydown']) {
        viewport.removeEventListener(event, cancelAnchor, true)
      }
      cancelAnchor()
    }
  }, [cancelAnchor, viewport])

  const restoreAnchor = useCallback((instance: Virtualizer<HTMLDivElement, HTMLDivElement>) => {
    const anchor = anchorRef.current
    if (!anchor || !viewport) return
    if (follow.isFollowing() || navigationLocked.current || !anchor.row.isConnected) {
      cancelAnchor()
      return
    }
    const root = content?.querySelector<HTMLElement>('[data-chat-rows-root]')
    if (!root) { cancelAnchor(); return }
    const viewportTop = viewport.getBoundingClientRect().top
    const origin = root.getBoundingClientRect().top - viewportTop + viewport.scrollTop
    // Ref measurements can update TanStack before React commits its new row
    // transforms. Use the measured model so both settle before the next paint.
    const index = anchor.row.dataset.index
    const totalSize = instance.getTotalSize()
    const start = index === undefined
      ? totalSize
      : instance.measurementsCache[Number(index)]?.start
    if (start === undefined) { cancelAnchor(); return }
    const target = Math.max(0, origin + start - anchor.top)
    if (Math.abs(viewport.scrollTop - target) > 0.5) {
      follow.markLayoutCompensation()
      follow.scrollToOffset(target)
    }
    const actualTop = anchor.row.getBoundingClientRect().top - viewportTop
    const modelCommitted = Math.abs(actualTop + viewport.scrollTop - origin - start) <= 1
    // Keep the anchor through the re-render caused by ref measurements. Once
    // transforms match, browser clamping at either document edge is final too.
    if (modelCommitted) cancelAnchor()
  }, [cancelAnchor, content, follow, navigationLocked, viewport])

  return { contentWidth, anchorRef, prepareWidthChange, restoreAnchor }
}
