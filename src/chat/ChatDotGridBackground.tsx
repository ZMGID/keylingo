import { useEffect, useRef } from 'react'
import { isWindows } from './platform'
import { prefersReducedMotion } from './utils'

const GRID_SPACING = 20
const DOT_RADIUS = 1
const PATTERN_MIN_SEC = 7
const PATTERN_MAX_SEC = 12
// Windows WebView2 的 GPU 进程对全幅 canvas 更敏感；10fps 仍是慢扫，CPU 大约砍半。
const TARGET_FRAME_MS = isWindows ? 1000 / 10 : 1000 / 20
const MAX_CANVAS_DPR = 1.5
const MAX_CANVAS_DPR_WINDOWS = 1
const ALPHA_BUCKETS = 12

type Dot = {
  x: number
  y: number
  normX: number
  normY: number
  diag: number
  diagRev: number
  ringDistance: number
  base: number
  phase: number
  speed: number
  fade: number
  focus: number
}

type PatternId =
  | 'band-lr'
  | 'band-rl'
  | 'band-tb'
  | 'band-bt'
  | 'band-diag'
  | 'band-diag-rev'
  | 'ring-out'
  | 'ring-in'
  | 'wave-h'
  | 'wave-v'

const PATTERN_IDS: PatternId[] = [
  'band-lr',
  'band-rl',
  'band-tb',
  'band-bt',
  'band-diag',
  'band-diag-rev',
  'ring-out',
  'ring-in',
  'wave-h',
  'wave-v',
]

type ActivePattern = {
  id: PatternId
  startedAtSec: number
  durationSec: number
}

function seededUnit(seed: number): number {
  return ((seed >>> 0) % 1000) / 1000
}

function randomBetween(min: number, max: number): number {
  return min + Math.random() * (max - min)
}

function pickNextPattern(previous?: PatternId): PatternId {
  const pool = previous ? PATTERN_IDS.filter((id) => id !== previous) : PATTERN_IDS
  return pool[Math.floor(Math.random() * pool.length)]
}

function buildDots(width: number, height: number): Dot[] {
  const dots: Dot[] = []
  const cx = width * 0.5
  const cy = height * 0.42
  const maxR = Math.hypot(width, height) * 0.55
  for (let y = GRID_SPACING / 2; y < height; y += GRID_SPACING) {
    for (let x = GRID_SPACING / 2; x < width; x += GRID_SPACING) {
      const gx = Math.floor(x / GRID_SPACING)
      const gy = Math.floor(y / GRID_SPACING)
      const seed = (gx * 73856093) ^ (gy * 19349663)
      const depth = seededUnit(seed)
      const rhythm = seededUnit(seed * 48271)
      const normX = x / width
      const normY = y / height
      dots.push({
        x,
        y,
        normX,
        normY,
        diag: (normX + normY) * 0.5,
        diagRev: (normX + (1 - normY)) * 0.5,
        ringDistance: Math.hypot(x - cx, y - cy) / maxR,
        base: 0.08 + depth * 0.14,
        phase: rhythm * Math.PI * 2,
        speed: 0.3 + depth * 0.5,
        fade: centerFade(x, y, width, height),
        focus: contentFocus(x, y, width, height),
      })
    }
  }
  return dots
}

function centerFade(x: number, y: number, width: number, height: number): number {
  const nx = (x - width * 0.5) / (width * 0.425)
  const ny = (y - height * 0.42) / (height * 0.375)
  const distance = nx * nx + ny * ny
  return Math.max(0, Math.min(1, 1 - distance * 0.95))
}

function gaussianBand(value: number, center: number, sigma: number): number {
  const delta = value - center
  return Math.exp(-(delta * delta) / (2 * sigma * sigma))
}

function contentFocus(x: number, y: number, width: number, height: number): number {
  const yNorm = y / height
  const xNorm = x / width
  const vertical = 0.55 + 0.45 * Math.exp(-Math.pow((yNorm - 0.4) / 0.34, 2))
  const horizontal = 0.6 + 0.4 * Math.exp(-Math.pow((xNorm - 0.5) / 0.42, 2))
  return Math.max(vertical, horizontal * 0.85)
}

function patternProgress(localSec: number, durationSec: number): number {
  const travel = 1.32
  const start = -0.16
  return start + (localSec / durationSec) * travel
}

function linearBand(
  norm: number,
  localSec: number,
  durationSec: number,
  direction: 1 | -1,
  trailOffset: number,
): number {
  const base = patternProgress(localSec, durationSec)
  const center = direction === 1 ? base : 1.16 - base
  const main = gaussianBand(norm, center, 0.085)
  const trail = gaussianBand(norm, center + trailOffset * direction, 0.1) * 0.4
  return Math.min(1, main + trail)
}

function computePatternGlow(
  id: PatternId,
  dot: Dot,
  localSec: number,
  durationSec: number,
): number {
  switch (id) {
    case 'band-lr':
      return linearBand(dot.normX, localSec, durationSec, 1, -0.11) * dot.focus
    case 'band-rl':
      return linearBand(dot.normX, localSec, durationSec, -1, 0.11) * dot.focus
    case 'band-tb':
      return linearBand(dot.normY, localSec, durationSec, 1, -0.11) * dot.focus
    case 'band-bt':
      return linearBand(dot.normY, localSec, durationSec, -1, 0.11) * dot.focus
    case 'band-diag': {
      const center = patternProgress(localSec, durationSec)
      const main = gaussianBand(dot.diag, center, 0.07)
      const trail = gaussianBand(dot.diag, center - 0.09, 0.085) * 0.38
      return Math.min(1, (main + trail) * dot.focus)
    }
    case 'band-diag-rev': {
      const center = patternProgress(localSec, durationSec)
      const main = gaussianBand(dot.diagRev, center, 0.07)
      const trail = gaussianBand(dot.diagRev, center - 0.09, 0.085) * 0.38
      return Math.min(1, (main + trail) * dot.focus)
    }
    case 'ring-out': {
      const center = patternProgress(localSec, durationSec)
      const main = gaussianBand(dot.ringDistance, center, 0.09)
      const trail = gaussianBand(dot.ringDistance, center - 0.08, 0.1) * 0.35
      return Math.min(1, (main + trail) * dot.focus)
    }
    case 'ring-in': {
      const center = 1.16 - patternProgress(localSec, durationSec)
      const main = gaussianBand(dot.ringDistance, center, 0.09)
      const trail = gaussianBand(dot.ringDistance, center + 0.08, 0.1) * 0.35
      return Math.min(1, (main + trail) * dot.focus)
    }
    case 'wave-h': {
      const phase = dot.normX * Math.PI * 5 - localSec * 2.4
      const wave = Math.pow(Math.max(0, Math.sin(phase)), 2.2)
      const drift = gaussianBand(dot.normX, patternProgress(localSec, durationSec), 0.22) * 0.35
      return Math.min(1, (wave * 0.65 + drift) * dot.focus)
    }
    case 'wave-v': {
      const phase = dot.normY * Math.PI * 5 - localSec * 2.4
      const wave = Math.pow(Math.max(0, Math.sin(phase)), 2.2)
      const drift = gaussianBand(dot.normY, patternProgress(localSec, durationSec), 0.22) * 0.35
      return Math.min(1, (wave * 0.65 + drift) * dot.focus)
    }
    default:
      return 0
  }
}

function resolvePattern(nowSec: number, active: ActivePattern): { pattern: ActivePattern; localSec: number } {
  const elapsed = nowSec - active.startedAtSec
  if (elapsed < active.durationSec) {
    return { pattern: active, localSec: elapsed }
  }

  const next: ActivePattern = {
    id: pickNextPattern(active.id),
    startedAtSec: nowSec,
    durationSec: randomBetween(PATTERN_MIN_SEC, PATTERN_MAX_SEC),
  }
  return { pattern: next, localSec: 0 }
}

function readDarkMode(): boolean {
  return document.documentElement.classList.contains('dark')
}

export function ChatDotGridBackground() {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const frameRef = useRef<number>()
  const frameTimerRef = useRef<number>()
  const dotsRef = useRef<Dot[]>([])
  const bucketsRef = useRef<Dot[][]>(Array.from({ length: ALPHA_BUCKETS }, () => []))
  const sizeRef = useRef({ width: 0, height: 0, dpr: 0 })
  const patternRef = useRef<ActivePattern | null>(null)
  const darkRef = useRef(readDarkMode())
  const reducedMotionRef = useRef(prefersReducedMotion())

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return

    // ponytail: 不开 desynchronized —— 这块画布按 10–20fps 跑（TARGET_FRAME_MS），
    // 低延迟直通路径一点收益没有，却会在「transparent(true) 窗口 + 圆角 overflow:hidden 祖先」
    // 下被 WebView2 降级成不透明黑层，把整个主区涂黑。
    const ctx = canvas.getContext('2d', { alpha: true })
    if (!ctx) return
    const buckets = bucketsRef.current
    let disposed = false
    let unfocused = false
    let offscreen = false

    const isPaused = () =>
      disposed || document.hidden || unfocused || offscreen || reducedMotionRef.current

    const clearScheduledFrame = () => {
      if (frameRef.current) {
        window.cancelAnimationFrame(frameRef.current)
        frameRef.current = undefined
      }
      if (frameTimerRef.current) {
        window.clearTimeout(frameTimerRef.current)
        frameTimerRef.current = undefined
      }
    }

    const resize = () => {
      const parent = canvas.parentElement
      if (!parent) return
      const width = Math.max(1, Math.floor(parent.clientWidth))
      const height = Math.max(1, Math.floor(parent.clientHeight))
      const maxDpr = isWindows ? MAX_CANVAS_DPR_WINDOWS : MAX_CANVAS_DPR
      const dpr = Math.min(window.devicePixelRatio || 1, maxDpr)
      if (
        sizeRef.current.width === width &&
        sizeRef.current.height === height &&
        sizeRef.current.dpr === dpr
      ) {
        return
      }

      sizeRef.current = { width, height, dpr }
      canvas.width = Math.floor(width * dpr)
      canvas.height = Math.floor(height * dpr)
      canvas.style.width = `${width}px`
      canvas.style.height = `${height}px`
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      dotsRef.current = buildDots(width, height)
    }

    const draw = (time: number) => {
      const width = canvas.clientWidth
      const height = canvas.clientHeight
      if (width <= 0 || height <= 0) return

      ctx.clearRect(0, 0, width, height)
      const dark = darkRef.current
      const reducedMotion = reducedMotionRef.current
      const nowSec = time * 0.001

      if (!patternRef.current) {
        patternRef.current = {
          id: pickNextPattern(),
          startedAtSec: nowSec,
          durationSec: randomBetween(PATTERN_MIN_SEC, PATTERN_MAX_SEC),
        }
      }

      const { pattern, localSec } = resolvePattern(nowSec, patternRef.current)
      patternRef.current = pattern

      for (const bucket of buckets) {
        bucket.length = 0
      }

      for (const dot of dotsRef.current) {
        const band = reducedMotion
          ? 0
          : computePatternGlow(pattern.id, dot, localSec, pattern.durationSec)
        const pulse = reducedMotion ? 0 : Math.sin(nowSec * dot.speed + dot.phase) * 0.012
        const alpha = (dot.base * (0.48 + band * 0.52) + band * 0.34 + pulse) * dot.fade
        if (alpha <= 0.01) continue

        const bucketIndex = Math.max(1, Math.min(ALPHA_BUCKETS - 1, Math.round(alpha * (ALPHA_BUCKETS - 1))))
        buckets[bucketIndex].push(dot)
      }

      const channel = dark ? '255, 255, 255' : '0, 0, 0'
      for (let bucketIndex = 1; bucketIndex < buckets.length; bucketIndex += 1) {
        const dots = buckets[bucketIndex]
        if (dots.length === 0) continue

        ctx.beginPath()
        for (const dot of dots) {
          ctx.moveTo(dot.x + DOT_RADIUS, dot.y)
          ctx.arc(dot.x, dot.y, DOT_RADIUS, 0, Math.PI * 2)
        }
        ctx.fillStyle = `rgba(${channel}, ${bucketIndex / (ALPHA_BUCKETS - 1)})`
        ctx.fill()
      }
    }

    const startAnimation = () => {
      clearScheduledFrame()
      if (isPaused()) {
        if (!disposed && !document.hidden && !offscreen) draw(performance.now())
        return
      }
      frameRef.current = window.requestAnimationFrame(loop)
    }

    const loop = (time: number) => {
      draw(time)
      frameTimerRef.current = window.setTimeout(() => {
        if (!isPaused()) {
          frameRef.current = window.requestAnimationFrame(loop)
        }
      }, TARGET_FRAME_MS)
    }

    resize()
    patternRef.current = null
    startAnimation()

    const resizeObserver = new ResizeObserver(() => {
      resize()
      startAnimation()
    })
    resizeObserver.observe(canvas.parentElement ?? canvas)

    const themeObserver = new MutationObserver(() => {
      darkRef.current = readDarkMode()
      if (reducedMotionRef.current || document.hidden) draw(performance.now())
    })
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['class'],
    })

    const motionMedia = window.matchMedia('(prefers-reduced-motion: reduce)')
    const onMotionChange = () => {
      reducedMotionRef.current = motionMedia.matches
      startAnimation()
    }
    motionMedia.addEventListener('change', onMotionChange)

    const onVisibilityChange = () => {
      if (document.hidden) {
        clearScheduledFrame()
        return
      }
      startAnimation()
    }
    document.addEventListener('visibilitychange', onVisibilityChange)

    const onWindowBlur = () => {
      unfocused = true
      clearScheduledFrame()
    }
    const onWindowFocus = () => {
      unfocused = false
      startAnimation()
    }
    window.addEventListener('blur', onWindowBlur)
    window.addEventListener('focus', onWindowFocus)

    const intersectionObserver = new IntersectionObserver((entries) => {
      offscreen = entries.every((entry) => !entry.isIntersecting)
      if (offscreen) clearScheduledFrame()
      else startAnimation()
    })
    intersectionObserver.observe(canvas)

    return () => {
      disposed = true
      resizeObserver.disconnect()
      themeObserver.disconnect()
      intersectionObserver.disconnect()
      motionMedia.removeEventListener('change', onMotionChange)
      document.removeEventListener('visibilitychange', onVisibilityChange)
      window.removeEventListener('blur', onWindowBlur)
      window.removeEventListener('focus', onWindowFocus)
      clearScheduledFrame()
      patternRef.current = null
      dotsRef.current = []
      for (const bucket of buckets) {
        bucket.length = 0
      }
      sizeRef.current = { width: 0, height: 0, dpr: 0 }
      canvas.width = 0
      canvas.height = 0
    }
  }, [])

  return (
    <canvas
      ref={canvasRef}
      className="chat-empty-hero-dot-canvas"
      aria-hidden="true"
    />
  )
}
