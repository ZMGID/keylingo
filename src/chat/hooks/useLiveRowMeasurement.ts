import { useCallback, useRef } from 'react'
import type { VirtualItem, Virtualizer } from '@tanstack/react-virtual'
import {
  layoutScopedVirtualKey,
  measureChatVirtualRow,
  measureSettledChatRow,
} from '../messageListVirtualization'

/** Keep reused rows measured across live→history and width-layout handoffs. */
export function useLiveRowMeasurement(layoutKey: string, liveRowKey: string | null) {
  const pending = useRef<{ key: string; height: number } | null>(null)
  const observer = useRef<ResizeObserver | null>(null)
  const rowLayouts = useRef(new WeakMap<HTMLDivElement, { layout: string; key: VirtualItem['key'] }>())
  const liveKey = liveRowKey === null ? null : layoutScopedVirtualKey(layoutKey, liveRowKey)

  const liveRowRef = useCallback((element: HTMLDivElement | null) => {
    // Ref detach owns cleanup, including unmount. An effect cleanup here would
    // disconnect the observer during StrictMode's effect replay without a reattach.
    observer.current?.disconnect()
    observer.current = null
    // React detaches this ref before attaching the historical ref. Keep the
    // estimate through that boundary, including when persistence arrives later.
    if (!element || liveKey === null) return
    const measurement = { key: liveKey, height: measureChatVirtualRow(element, undefined) }
    pending.current = measurement
    if (typeof ResizeObserver === 'undefined') return
    observer.current = new ResizeObserver((entries) => {
      measurement.height = measureChatVirtualRow(element, entries[0])
    })
    observer.current.observe(element)
  }, [liveKey])

  const getLiveRowSize = useCallback((rowKey: string): number | undefined => {
    const measurement = pending.current
    return measurement?.key === layoutScopedVirtualKey(layoutKey, rowKey) && measurement.height > 0
      ? measurement.height
      : undefined
  }, [layoutKey])

  // Reattach the ref on a width change without remounting the message subtree.
  const measureRow = useCallback((
    element: HTMLDivElement | null,
    instance: Virtualizer<HTMLDivElement, HTMLDivElement>,
  ) => {
    if (!element) {
      instance.measureElement(null)
      return
    }
    const key = instance.options.getItemKey(instance.indexFromElement(element))
    const previous = rowLayouts.current.get(element)
    const layoutChanged = previous !== undefined && previous.layout !== layoutKey
    rowLayouts.current.set(element, { layout: layoutKey, key })
    if (previous && previous.key !== key && instance.elementsCache.get(previous.key) === element) {
      instance.elementsCache.delete(previous.key)
    }
    const settling = pending.current?.key === key
    if (settling) pending.current = null
    if (settling || layoutChanged) {
      // React keeps the DOM across both live→history and width changes. The
      // new layout key has different estimates, but unchanged line breaks mean
      // RO may never fire. Correct the height before paint, even during scroll.
      measureSettledChatRow(element, instance)
    } else {
      instance.measureElement(element)
    }
  }, [layoutKey])

  return { liveRowRef, getLiveRowSize, measureRow }
}
