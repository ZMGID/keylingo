import { memo, useEffect, useImperativeHandle, useRef, forwardRef, type CSSProperties } from 'react'
import {
  BLOB_BLUE,
  BLOB_EYE,
  BLOB_REST,
  BODY_R,
  CX,
  CY,
  KivioBlobSim,
  blobScheduleMs,
  type BlobMood,
} from './kivioBlobSim'
import { prefersReducedMotion } from './utils'

export type { BlobMood }

export interface KivioBlobHandle {
  pokeAt: (clientX?: number) => void
}

interface KivioBlobProps {
  size?: number
  mood?: BlobMood
  /** 切会话覆盖层盖住时停表，避免和点阵脉冲叠两套动画。 */
  paused?: boolean
  /** 变化时眨一眼 + 瞟一下（空态换文案）。连点升温不走这条。 */
  pulse?: string | number | null
  onPoke?: (streak: number) => void
}

function writeAttr(el: Element, name: string, value: string, prev: string): string {
  if (prev === value) return prev
  el.setAttribute(name, value)
  return value
}

export const KivioBlob = memo(forwardRef<KivioBlobHandle, KivioBlobProps>(function KivioBlob(
  { size = 28, mood = 'idle', paused = false, pulse, onPoke }: KivioBlobProps,
  ref,
) {
  const hostRef = useRef<HTMLSpanElement>(null)
  const rigRef = useRef<SVGGElement>(null)
  const bodyRef = useRef<SVGCircleElement>(null)
  const eye0Ref = useRef<SVGPathElement>(null)
  const eye1Ref = useRef<SVGPathElement>(null)
  const simRef = useRef<KivioBlobSim | null>(null)
  const armRef = useRef<(now?: number) => void>(() => {})
  const pausedRef = useRef(paused)
  const pulseSkipRef = useRef(true)
  const onPokeRef = useRef(onPoke)
  onPokeRef.current = onPoke
  pausedRef.current = paused
  if (!simRef.current) {
    simRef.current = new KivioBlobSim({ reducedMotion: prefersReducedMotion() })
  }

  const pokeAt = (clientX?: number) => {
    const sim = simRef.current
    const host = hostRef.current
    if (!sim) return
    let lookX: number | undefined
    if (clientX != null && host) {
      const rect = host.getBoundingClientRect()
      if (rect.width > 0) lookX = ((clientX - rect.left) / rect.width) * 2 - 1
    }
    const streak = sim.poke(performance.now(), lookX)
    onPokeRef.current?.(streak)
    armRef.current()
  }
  const pokeAtRef = useRef(pokeAt)
  pokeAtRef.current = pokeAt
  useImperativeHandle(ref, () => ({ pokeAt: (clientX) => pokeAtRef.current(clientX) }), [])

  useEffect(() => {
    simRef.current?.setMood(mood, performance.now())
    armRef.current()
  }, [mood])

  useEffect(() => {
    if (pulseSkipRef.current) {
      pulseSkipRef.current = false
      return
    }
    simRef.current?.nudge(performance.now())
    armRef.current()
  }, [pulse])

  useEffect(() => {
    armRef.current()
  }, [paused])

  useEffect(() => {
    const sim = simRef.current
    const host = hostRef.current
    const rig = rigRef.current
    const body = bodyRef.current
    const eye0 = eye0Ref.current
    const eye1 = eye1Ref.current
    if (!sim || !host || !rig || !body || !eye0 || !eye1) return

    const last = { rig: '', body: '', fill: '', d0: '', t0: '', d1: '', t1: '' }
    const apply = (now: number) => {
      const p = sim.sample(now)
      last.rig = writeAttr(rig, 'transform', p.rig, last.rig)
      last.body = writeAttr(body, 'transform', p.body, last.body)
      last.fill = writeAttr(body, 'fill', p.fill, last.fill)
      last.d0 = writeAttr(eye0, 'd', p.eyes[0].d, last.d0)
      last.t0 = writeAttr(eye0, 'transform', p.eyes[0].transform, last.t0)
      last.d1 = writeAttr(eye1, 'd', p.eyes[1].d, last.d1)
      last.t1 = writeAttr(eye1, 'transform', p.eyes[1].transform, last.t1)
    }

    apply(performance.now())
    if (prefersReducedMotion()) return

    let raf = 0
    let timeout = 0
    let onScreen = true
    let unfocused = false
    let stopped = false

    const clearTimers = () => {
      if (raf) cancelAnimationFrame(raf)
      if (timeout) clearTimeout(timeout)
      raf = 0
      timeout = 0
    }

    const arm = (now = performance.now()) => {
      clearTimers()
      if (stopped) return
      const delay = blobScheduleMs({
        reducedMotion: false,
        hidden: document.hidden,
        onScreen,
        unfocused,
        covered: pausedRef.current,
        highFps: sim.wantsHighFps(now),
        idleWakeMs: sim.nextIdleWakeMs(now),
      })
      if (delay == null) return
      const paint = (t: number) => {
        raf = 0
        apply(t)
        arm(t)
      }
      if (delay === 0) {
        raf = requestAnimationFrame(paint)
      } else {
        timeout = window.setTimeout(() => {
          timeout = 0
          raf = requestAnimationFrame(paint)
        }, delay)
      }
    }

    const onVis = () => {
      if (document.hidden) clearTimers()
      else arm()
    }
    const onBlur = () => {
      unfocused = true
      clearTimers()
    }
    const onFocus = () => {
      unfocused = false
      arm()
    }
    document.addEventListener('visibilitychange', onVis)
    window.addEventListener('blur', onBlur)
    window.addEventListener('focus', onFocus)

    const io = new IntersectionObserver((entries) => {
      onScreen = entries.some((entry) => entry.isIntersecting)
      if (onScreen) arm()
      else clearTimers()
    })
    armRef.current = arm
    io.observe(host)
    arm()

    return () => {
      stopped = true
      armRef.current = () => {}
      clearTimers()
      document.removeEventListener('visibilitychange', onVis)
      window.removeEventListener('blur', onBlur)
      window.removeEventListener('focus', onFocus)
      io.disconnect()
    }
  }, [])

  return (
    <span
      ref={hostRef}
      role="button"
      tabIndex={0}
      className="inline-flex shrink-0 cursor-pointer select-none"
      style={{ '--kv-stream-logo-size': `${size}px`, width: size, height: size } as CSSProperties}
      onPointerDown={(event) => {
        if (event.button !== 0) return
        event.preventDefault()
        pokeAt(event.clientX)
      }}
      onKeyDown={(event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return
        event.preventDefault()
        pokeAt()
      }}
    >
      <svg
        className="kv-stream-logo"
        viewBox="0 0 240 240"
        width={size}
        height={size}
        overflow="visible"
      >
        <g ref={rigRef} transform={BLOB_REST.rig}>
          <circle
            ref={bodyRef}
            cx={CX}
            cy={CY}
            r={BODY_R}
            fill={BLOB_BLUE}
            transform={BLOB_REST.body}
          />
          <path ref={eye0Ref} fill={BLOB_EYE} d={BLOB_REST.eyes[0].d} transform={BLOB_REST.eyes[0].transform} />
          <path ref={eye1Ref} fill={BLOB_EYE} d={BLOB_REST.eyes[1].d} transform={BLOB_REST.eyes[1].transform} />
        </g>
      </svg>
    </span>
  )
}))
