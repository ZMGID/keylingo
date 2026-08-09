import { useEffect, useState, type ReactNode } from 'react'

type IdleWindow = Window & {
  requestIdleCallback?: (callback: () => void, options?: { timeout: number }) => number
  cancelIdleCallback?: (handle: number) => void
}

/**
 * Heavy chat content is allowed to hydrate after the surrounding text becomes
 * interactive. The intrinsic size keeps virtual rows stable while the island
 * is waiting for an idle slice.
 */
export function ChatHeavyIsland({
  children,
  fallback,
  minHeight = 96,
  delayMs = 120,
  eager = false,
}: {
  children: ReactNode
  fallback: ReactNode
  minHeight?: number
  delayMs?: number
  eager?: boolean
}) {
  const [hydrated, setHydrated] = useState(eager)

  useEffect(() => {
    if (eager) return
    let cancelled = false
    const idleWindow = window as IdleWindow
    let idleId: number | undefined
    let timeoutId: number | undefined
    const hydrate = () => {
      if (!cancelled) setHydrated(true)
    }

    if (idleWindow.requestIdleCallback && import.meta.env.MODE !== 'test') {
      idleId = idleWindow.requestIdleCallback(hydrate, { timeout: delayMs })
    } else {
      timeoutId = window.setTimeout(hydrate, delayMs)
    }

    return () => {
      cancelled = true
      if (idleId !== undefined) idleWindow.cancelIdleCallback?.(idleId)
      if (timeoutId !== undefined) window.clearTimeout(timeoutId)
    }
  }, [delayMs])

  return (
    <div
      data-chat-heavy-island="true"
      data-chat-heavy-hydrated={hydrated ? 'true' : 'false'}
      style={{
        contentVisibility: 'auto',
        containIntrinsicSize: `${minHeight}px`,
        minHeight: hydrated ? undefined : minHeight,
      }}
    >
      {hydrated ? children : fallback}
    </div>
  )
}
