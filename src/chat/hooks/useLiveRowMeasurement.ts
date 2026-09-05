import { useCallback, useRef } from 'react'
import type { Virtualizer } from '@tanstack/react-virtual'
import {
  layoutScopedVirtualKey,
  measureChatVirtualRow,
  measureSettledChatRow,
} from '../messageListVirtualization'

/** One temporary estimate follows the stable row key from live content to history. */
export function useLiveRowMeasurement(layoutKey: string, liveRowKey: string | null) {
  const pending = useRef<{ key: string; height: number } | null>(null)
  const observer = useRef<ResizeObserver | null>(null)
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

  const measureRow = useCallback((
    element: HTMLDivElement | null,
    instance: Virtualizer<HTMLDivElement, HTMLDivElement>,
  ) => {
    if (element && pending.current?.key === instance.options.getItemKey(instance.indexFromElement(element))) {
      pending.current = null
      // The seed may predate collapsed reasoning or the final footer. Replace
      // it before paint even when TanStack would defer measurement during scroll.
      measureSettledChatRow(element, instance)
    } else {
      instance.measureElement(element)
    }
  }, [])

  return { liveRowRef, getLiveRowSize, measureRow }
}
