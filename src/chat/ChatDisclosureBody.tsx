import { useLayoutEffect, useRef, useState, type ReactNode } from 'react'

/** Animate user toggles only. Automatic stream transitions remain synchronous. */
export function ChatDisclosureBody({
  open,
  animate = true,
  keepMounted = false,
  children,
}: {
  open: boolean
  animate?: boolean
  keepMounted?: boolean
  children: ReactNode | (() => ReactNode)
}) {
  const [present, setPresent] = useState(open)
  const previousOpen = useRef(open)
  const boxRef = useRef<HTMLDivElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const animationRef = useRef<Animation | null>(null)
  const observerRef = useRef<ResizeObserver | null>(null)

  useLayoutEffect(() => {
    const box = boxRef.current
    const content = contentRef.current
    if (!box || !content) return
    box.inert = !open
    const wasOpen = previousOpen.current
    previousOpen.current = open
    if (wasOpen === open) return

    const from = animationRef.current
      ? box.getBoundingClientRect().height
      : wasOpen ? content.getBoundingClientRect().height : 0
    animationRef.current?.cancel()
    observerRef.current?.disconnect()
    animationRef.current = null
    observerRef.current = null
    const finish = () => {
      box.style.height = open ? '' : '0px'
      box.style.overflow = open ? '' : 'clip'
      box.removeAttribute('data-chat-disclosure-animating')
      observerRef.current?.disconnect()
      observerRef.current = null
      setPresent(open)
    }
    if (!animate || typeof box.animate !== 'function' || window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      finish()
      return
    }

    setPresent(true)
    box.style.overflow = 'clip'
    box.setAttribute('data-chat-disclosure-animating', 'true')
    let target = open ? content.getBoundingClientRect().height : 0
    const duration = Math.min(360, 200 + Math.abs(target - from) * 0.12)
    const deadline = performance.now() + duration
    const run = (start: number, end: number, duration: number) => {
      const animation = box.animate(
        [{ height: `${start}px` }, { height: `${end}px` }],
        { duration, easing: 'cubic-bezier(0.2, 0, 0, 1)' },
      )
      animationRef.current = animation
      animation.onfinish = () => {
        if (animationRef.current !== animation) return
        animationRef.current = null
        finish()
      }
    }
    run(from, target, duration)
    // Code and images can finish loading during expansion. Retarget from the
    // currently painted height without restarting the interaction's deadline.
    if (open && typeof ResizeObserver !== 'undefined') {
      observerRef.current = new ResizeObserver(() => {
        const next = content.getBoundingClientRect().height
        if (!animationRef.current || Math.abs(next - target) < 0.5) return
        const current = box.getBoundingClientRect().height
        animationRef.current.cancel()
        target = next
        run(current, next, Math.max(1, deadline - performance.now()))
      })
      observerRef.current.observe(content)
    }
  }, [animate, open])

  useLayoutEffect(() => () => {
    animationRef.current?.cancel()
    observerRef.current?.disconnect()
  }, [])

  return (
    <div ref={boxRef} data-chat-disclosure-body aria-hidden={!open} style={open ? undefined : { height: 0, overflow: 'clip' }}>
      <div ref={contentRef} style={{ display: 'flow-root' }}>
        {(open || present || keepMounted) && (typeof children === 'function' ? children() : children)}
      </div>
    </div>
  )
}
