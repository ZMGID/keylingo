export const CX = 120
export const CY = 120
export const BODY_R = 78
export const EYE_POINTS = 48
export const BLOB_BLUE = '#1d6bf0'
export const BLOB_EYE = '#f3efe6'
export const BLOB_POKE_RED = '#e23b2e'

function mixHex(a: string, b: string, t: number): string {
  const u = clamp(t, 0, 1)
  const n = (h: string, i: number) => parseInt(h.slice(1 + i * 2, 3 + i * 2), 16)
  const m = (i: number) => Math.round(n(a, i) + (n(b, i) - n(a, i)) * u)
  return `#${[0, 1, 2].map((i) => m(i).toString(16).padStart(2, '0')).join('')}`
}

/** 跟生成过程对齐的几张脸。几何是 Kivio 自己的 stadium，不是 xAI 眼环。 */
export type BlobMood = 'idle' | 'think' | 'search' | 'work' | 'speak' | 'error'

const DT = 1 / 120
// 显式标注:两表三元取用后是同一类型,元组才能直接 spread 进 stepSpring(TS2556)。
type SpringTable = Record<'spin' | 'x' | 'y' | 'squash' | 'blink' | 'gaze' | 'morph' | 'boost', readonly [number, number]>
const SPR: SpringTable = {
  spin: [5, 0.9],
  x: [3.5, 1],
  y: [4, 1],
  squash: [10, 0.8],
  blink: [26, 1],
  gaze: [13, 1],
  morph: [7, 1],
  boost: [9, 0.85],
}

/** 闲置约 20fps，弹簧放慢，避免三帧切完看起来像跳。 */
const SPR_IDLE: SpringTable = {
  spin: [2.0, 1],
  x: [1.6, 1],
  y: [1.8, 1],
  squash: [3.0, 1],
  blink: [9, 1],
  gaze: [2.6, 1],
  morph: [2.2, 1],
  boost: [3.2, 0.9],
}

const PLAY: Record<BlobMood, number[]> = {
  idle: [0, 8, 3, 10, 1, 9],
  think: [8, 15, 14, 12, 5],
  search: [9, 3, 12, 17, 2],
  work: [7, 15, 10, 16],
  speak: [10, 1, 11],
  error: [7, 16],
}
const HOLD: Record<BlobMood, [number, number]> = {
  idle: [5000, 18000],
  think: [2000, 3600],
  search: [1000, 1800],
  work: [1800, 3200],
  speak: [2800, 5000],
  error: [2200, 3800],
}
const BLINK: Record<BlobMood, [number, number] | null> = {
  idle: [4000, 16000],
  think: [3500, 7000],
  search: [1600, 4000],
  work: [2800, 5500],
  speak: [3000, 7000],
  error: [3500, 7000],
}
const WINK = new Set<BlobMood>(['idle', 'speak'])
const HOP_SEGS = [
  { h: 48, d: 0.5 },
  { h: 28, d: 0.382 },
  { h: 14, d: 0.27 },
  { h: 6, d: 0.177 },
]
const HOP_DUR = HOP_SEGS.reduce((s, x) => s + x.d, 0)

type Spring = { x: number; v: number; t: number }
type Pt = [number, number]

const lerp = (a: number, b: number, t: number) => a + (b - a) * t
const clamp = (n: number, a: number, b: number) => Math.min(b, Math.max(a, n))
const k2 = (n: number) => (n < 0.5 ? 4 * n * n * n : 1 - (-2 * n + 2) ** 3 / 2)

function spring(x: number): Spring {
  return { x, v: 0, t: x }
}

function stepSpring(s: Spring, freq: number, damp: number, dt: number) {
  s.v += (-2 * damp * freq * s.v - freq * freq * (s.x - s.t)) * dt
  s.x += s.v * dt
  if (!Number.isFinite(s.x) || !Number.isFinite(s.v)) {
    s.x = s.t
    s.v = 0
  }
}

export function polyPath(pts: Pt[]): string {
  return `M${pts.map((p) => `${p[0].toFixed(2)} ${p[1].toFixed(2)}`).join('L')}Z`
}

function lerpPoly(a: Pt[], b: Pt[], t: number): Pt[] {
  return a.map((p, i) => [lerp(p[0], b[i][0], t), lerp(p[1], b[i][1], t)])
}

function centroid(pts: Pt[]): Pt {
  let x = 0
  let y = 0
  for (const p of pts) {
    x += p[0]
    y += p[1]
  }
  const n = pts.length || 1
  return [x / n, y / n]
}

function stadium(rx: number, ry: number, tilt: number, ox: number, oy: number, side: number): Pt[] {
  const pts: Pt[] = []
  for (let i = 0; i < EYE_POINTS; i++) {
    const a = (i / EYE_POINTS) * Math.PI * 2 - Math.PI / 2
    const c = Math.cos(a)
    const s = Math.sin(a)
    const px = rx * Math.sign(c) * Math.abs(c) ** 0.62
    const py = ry * Math.sign(s) * Math.abs(s) ** 0.62
    const x = px * Math.cos(tilt) - py * Math.sin(tilt)
    const y = px * Math.sin(tilt) + py * Math.cos(tilt)
    pts.push([CX + side * ox + x, CY + oy + y])
  }
  return pts
}

/** 原创 stadium 眼。语义对齐 grok 播放列表，坐标不搬 xAI。 */
const EYE: Record<number, { rx: number; ry: number; tilt: number; x: number; y: number; tiltR?: number }> = {
  0: { rx: 9.4, ry: 24.5, tilt: 0.46, x: 23, y: -2 },
  1: { rx: 14.5, ry: 15.5, tilt: 0.28, x: 23, y: 2 },
  2: { rx: 11.2, ry: 26.5, tilt: 0.16, x: 25, y: -8 },
  3: { rx: 13.2, ry: 13.8, tilt: 0.5, x: 21, y: -11 },
  4: { rx: 10.4, ry: 20, tilt: 0.55, x: 22, y: 3, tiltR: -0.55 },
  5: { rx: 16, ry: 3.4, tilt: 0.18, x: 23, y: 5 },
  6: { rx: 13.4, ry: 28, tilt: 0.1, x: 24, y: -6 },
  7: { rx: 11, ry: 18, tilt: 0.72, x: 22, y: 7 },
  8: { rx: 8.2, ry: 22.5, tilt: 0.5, x: 24, y: -1 },
  9: { rx: 12.2, ry: 18.5, tilt: 0.22, x: 28, y: -4 },
  10: { rx: 12.6, ry: 16.2, tilt: 0.38, x: 22, y: 4 },
  11: { rx: 14.2, ry: 11.8, tilt: 0.22, x: 22, y: 1 },
  12: { rx: 10.1, ry: 22.4, tilt: 0.08, x: 26, y: -10 },
  13: { rx: 15.2, ry: 2.8, tilt: 0.12, x: 22, y: 6 },
  14: { rx: 11.1, ry: 14.2, tilt: 0.62, x: 20, y: 2, tiltR: -0.38 },
  15: { rx: 8.5, ry: 16.4, tilt: 0.4, x: 21, y: -3 },
  16: { rx: 10.2, ry: 17.2, tilt: 0.85, x: 20, y: 5 },
  17: { rx: 15.1, ry: 20.2, tilt: 0.18, x: 24, y: -6 },
}

function eyesOf(id: number): [Pt[], Pt[]] {
  const e = EYE[id] ?? EYE[0]
  const tL = e.tilt
  const tR = e.tiltR ?? e.tilt
  return [stadium(e.rx, e.ry, tL, e.x, e.y, -1), stadium(e.rx, e.ry, tR, e.x, e.y, 1)]
}

const EYE_CACHE: Record<number, [Pt[], Pt[]]> = {}
for (const id of Object.keys(EYE).map(Number)) EYE_CACHE[id] = eyesOf(id)

type BlinkKey = { at: number; v: number }

export function queueBlink(q: BlinkKey[], now: number, random: () => number, stretch = 1) {
  const s = stretch
  q.push(
    { at: now, v: 0.05 },
    { at: now + 70 * s, v: 0.05 },
    { at: now + 150 * s, v: 1.08 },
    { at: now + 300 * s, v: 1 },
  )
  if (random() < 0.14) q.push({ at: now + 370 * s, v: 0.05 }, { at: now + 480 * s, v: 1 })
}

export function consumeBlink(q: BlinkKey[], now: number): number | null {
  let key: number | null = null
  while (q.length && now >= q[0].at) key = q.shift()!.v
  return key
}

export function hopY(hopAt: number, now: number, scale = 1, timeScale = 1): number | null {
  if (hopAt < 0) return 0
  const et = (now - hopAt) / 1000 / Math.max(timeScale, 0.01)
  if (et >= HOP_DUR) return null
  let en = 0
  for (const seg of HOP_SEGS) {
    if (et < en + seg.d) {
      const bn = (et - en) / seg.d
      return -4 * seg.h * bn * (1 - bn) * 0.5 * scale
    }
    en += seg.d
  }
  return 0
}

/** 偏长间隔，避免每次切换都像节拍器。 */
function spanMs(min: number, max: number, u: number): number {
  const t = 1 - (1 - u) * (1 - u)
  return min + (max - min) * t
}

export function isSearchToolName(name: string): boolean {
  const n = name.toLowerCase()
  return n.includes('search') || n.includes('webfetch') || n.includes('web_fetch') || n.includes('knowledge')
}

export function resolveBlobMood(input: {
  active: boolean
  error?: boolean
  contentLen?: number
  reasoningStreaming?: boolean
  runningToolNames?: string[]
}): BlobMood {
  if (input.error && !input.active) return 'error'
  if (!input.active) return 'idle'
  if (input.error) return 'error'
  const running = input.runningToolNames ?? []
  if (running.some(isSearchToolName)) return 'search'
  if (running.length > 0) return 'work'
  if ((input.contentLen ?? 0) > 0 && !input.reasoningStreaming) return 'speak'
  return 'think'
}

interface Pose {
  spin: number
  tx: number
  ty: number
  squash: number
  lid: number
  boost: number
}

interface IdleBias {
  spin: number
  tx: number
  ty: number
  squash: number
}

interface PoseCtx {
  nodUntil: number
  nodEnd: number
  impulseAt: number
  shakeUntil: number
  hopUntil: number
  biasUntil: number
  bias: IdleBias
}

function nextIdleBias(
  rand: (a: number, b: number) => number,
  sign: () => number,
): IdleBias & { hold: [number, number]; hop: boolean } {
  const r = rand(0, 1)
  if (r < 0.38) {
    return { spin: 0, tx: 0, ty: 0, squash: 1, hold: [4000, 14000], hop: false }
  }
  if (r < 0.7) {
    const d = sign()
    return {
      spin: d * rand(6, 14),
      tx: d * rand(2.5, 7),
      ty: rand(-1.5, 1.2),
      squash: 1,
      hold: [3500, 12000],
      hop: false,
    }
  }
  if (r < 0.84) {
    return { spin: rand(-3, 3), tx: 0, ty: -rand(1.5, 4), squash: 1.02, hold: [2800, 9000], hop: false }
  }
  if (r < 0.93) {
    return { spin: 0, tx: 0, ty: rand(1.5, 4), squash: 0.984, hold: [3500, 11000], hop: false }
  }
  const d = sign()
  return {
    spin: d * rand(4, 10),
    tx: d * rand(2, 5),
    ty: -2,
    squash: 1,
    hold: [4000, 13000],
    hop: true,
  }
}

function applyPose(mood: BlobMood, mt: number, now: number, ctx: PoseCtx, rand: (a: number, b: number) => number): Pose {
  let spin = 0
  let tx = 0
  let ty = 0
  let squash = 1
  let lid = 1
  let boost = 1
  if (mood === 'idle') {
    const br = Math.sin(mt * 0.48) * 0.62 + Math.sin(mt * 0.91) * 0.38
    spin = Math.sin(mt * 0.13) * 2 + ctx.bias.spin
    tx = Math.sin(mt * 0.11) * 1.6 + ctx.bias.tx
    ty = br * 2.2 + ctx.bias.ty
    squash = 1 + br * 0.016 + (ctx.bias.squash - 1)
    if (now < ctx.shakeUntil) {
      spin += Math.sin(now * 0.055) * 8
      tx += Math.sin(now * 0.08) * 5
    }
  } else if (mood === 'think') {
    spin = -12 + Math.sin(mt * 0.35) * 7
    tx = Math.sin(mt * 0.3) * 8
    ty = Math.sin(mt * 0.6) * 4
  } else if (mood === 'search') {
    const et = Math.sin(mt * 1.3)
    spin = et * 16
    tx = et * 10
    ty = Math.sin(mt * 1.7) * 4
  } else if (mood === 'work') {
    const et = Math.sin(mt * Math.PI * 2 * 1.6)
    spin = 5 + et * 3.5
    tx = 4
    ty = 2 + Math.max(0, et) * 4
    squash = 1 - Math.max(0, et) * 0.03
  } else if (mood === 'speak') {
    spin = 10 + Math.sin(mt * 0.5) * 2
    tx = 3
    ty = -2.5 + Math.sin(mt * 0.8) * 1.1
    squash = 1.018
    boost = 1.05
    if (now >= ctx.nodUntil) {
      ctx.nodUntil = now + rand(1800, 3200)
      ctx.nodEnd = now + 380
    }
    if (now < ctx.nodEnd) {
      const et = 1 - (ctx.nodEnd - now) / 380
      ty += Math.sin(et * Math.PI) * 6
      spin += Math.sin(et * Math.PI) * 3
    }
  } else {
    if (now >= ctx.impulseAt) {
      ctx.shakeUntil = now + 420
      ctx.impulseAt = now + rand(1800, 3200)
    }
    spin = now < ctx.shakeUntil ? Math.sin(now * 0.05) * 6 : 0
    ty = 4
    squash = 0.972
    lid = 0.92
  }
  return { spin, tx, ty, squash, lid, boost }
}

function nextGaze(mood: BlobMood, rand: (a: number, b: number) => number, sign: () => number) {
  if (mood === 'idle') {
    if (rand(0, 1) < 0.52) {
      return { x: sign() * rand(0.22, 0.72) * 11, y: rand(-0.4, 0.32) * 7, hold: [2000, 10000] as [number, number] }
    }
    return { x: 0, y: 0, hold: [3000, 14000] as [number, number] }
  }
  if (mood === 'think') return { x: sign() * rand(0.5, 1) * 16, y: -rand(0.4, 1) * 10, hold: [1500, 2800] as [number, number] }
  if (mood === 'search') return { x: sign() * rand(0.7, 1) * 16, y: rand(-1, 1) * 10, hold: [550, 1150] as [number, number] }
  if (mood === 'work') return { x: rand(-0.4, 0.4) * 16, y: rand(0.4, 1) * 10, hold: [1200, 2400] as [number, number] }
  if (mood === 'speak') return { x: rand(-0.3, 0.3) * 16, y: rand(-0.25, 0.25) * 10, hold: [2200, 4200] as [number, number] }
  return { x: rand(-0.2, 0.2) * 16, y: 0.2 * 10, hold: [1800, 3200] as [number, number] }
}

function hopCadence(mood: BlobMood): [number, number] | null {
  if (mood === 'idle') return [22000, 56000]
  if (mood === 'search') return [4000, 7000]
  if (mood === 'work') return [6000, 9000]
  if (mood === 'think') return [8000, 12000]
  return null
}

export interface BlobPaint {
  rig: string
  body: string
  fill: string
  eyes: [{ d: string; transform: string }, { d: string; transform: string }]
}

/** 闲置用呼吸节拍（约 20fps）；忙碌跟 vsync。 */
export const BLOB_IDLE_WAKE_MIN_MS = 48
export const BLOB_IDLE_WAKE_MAX_MS = 16_000
export const BLOB_IDLE_BREATHE_MS = 48

/** `null` = 不排下一帧（隐藏 / 屏外 / 失焦 / 被覆盖 / 减弱动态）。`0` = rAF。`>0` = 睡到下一次节拍。 */
export function blobScheduleMs(opts: {
  reducedMotion: boolean
  hidden: boolean
  onScreen: boolean
  unfocused?: boolean
  covered?: boolean
  highFps: boolean
  idleWakeMs?: number
}): number | null {
  if (opts.reducedMotion || opts.hidden || !opts.onScreen || opts.unfocused || opts.covered) return null
  if (opts.highFps) return 0
  const wake = opts.idleWakeMs ?? 8000
  return Math.min(BLOB_IDLE_WAKE_MAX_MS, Math.max(BLOB_IDLE_WAKE_MIN_MS, wake))
}

export interface BlobSimDebug {
  mood: BlobMood
  eyeTo: number
  spin: number
  blink: number
  heat: number
  pokes: number
}

export class KivioBlobSim {
  mood: BlobMood = 'idle'
  private random: () => number
  private reduced: boolean
  private spin = spring(0)
  private tx = spring(0)
  private ty = spring(0)
  private squash = spring(1)
  private blink = spring(1)
  private gazeX = spring(0)
  private gazeY = spring(0)
  private morph = spring(1)
  private boost = spring(1)
  private eyeFrom = 0
  private eyeTo = 0
  private eyeIdx = 0
  private t0 = 0
  private last = 0
  private eyeUntil = 0
  private blinkUntil = 0
  private gazeUntil = 0
  private winkAt = -1e9
  private winkEye = 0
  private winkUntil = 0
  private winkDur = 320
  private hopAt = -1
  private pokeHeat = 0
  private pokeCount = 0
  private lastPokeAt = -1e9
  private heatHoldUntil = 0
  private blinkQ: BlinkKey[] = []
  private inited = false
  private ctx: PoseCtx = {
    nodUntil: 0,
    nodEnd: 0,
    impulseAt: 0,
    shakeUntil: 0,
    hopUntil: 0,
    biasUntil: 0,
    bias: { spin: 0, tx: 0, ty: 0, squash: 1 },
  }

  constructor(opts: { random?: () => number; reducedMotion?: boolean } = {}) {
    this.random = opts.random ?? Math.random
    this.reduced = opts.reducedMotion ?? false
  }

  setMood(mood: BlobMood, now = 0) {
    const changed = mood !== this.mood || !this.inited
    this.mood = mood
    const list = PLAY[mood]
    this.eyeIdx = 0
    if (changed) this.morphTo(list[0])
    this.eyeUntil = now + (mood === 'idle' ? this.span(...HOLD[mood]) : this.rand(...HOLD[mood]))
    this.blinkUntil = now + (mood === 'idle' ? this.span(1800, 6000) : this.rand(900, 2800))
    this.gazeUntil = now + (mood === 'idle' ? this.span(800, 4200) : this.rand(280, 900))
    const hopEvery = hopCadence(mood)
    this.ctx = {
      nodUntil: now + 1600,
      nodEnd: 0,
      impulseAt: now + 600,
      shakeUntil: 0,
      hopUntil: now + (hopEvery ? (mood === 'idle' ? this.span(...hopEvery) : this.rand(...hopEvery)) : 1e12),
      biasUntil: mood === 'idle' ? now + this.span(3000, 10000) : 1e12,
      bias: { spin: 0, tx: 0, ty: 0, squash: 1 },
    }
    this.blinkQ = []
    queueBlink(this.blinkQ, now, this.random, mood === 'idle' ? 2.6 : 1)
  }

  poke(now: number, lookX?: number): number {
    if (now - this.lastPokeAt > 4200) this.pokeCount = 0
    this.pokeCount += 1
    this.lastPokeAt = now
    this.pokeHeat = Math.min(1, this.pokeHeat + (this.pokeCount < 4 ? 0.16 : 0.22))
    this.heatHoldUntil = now + 3200
    queueBlink(this.blinkQ, now, this.random, this.mood === 'idle' ? 2.6 : 1)
    const dir = lookX != null ? (lookX < 0 ? -1 : 1) : this.sign()
    const glance = lookX != null ? Math.min(1, Math.max(0.35, Math.abs(lookX))) : this.rand(0.45, 1)
    this.gazeX.t = dir * this.rand(8, 16) * glance
    this.gazeY.t = this.rand(-6, 4)
    this.gazeUntil = now + this.rand(this.mood === 'idle' ? 1400 : 700, this.mood === 'idle' ? 2800 : 1400)
    this.hopAt = now
    this.winkAt = now
    this.winkEye = this.random() < 0.5 ? 0 : 1
    this.winkDur = this.mood === 'idle' ? 700 : 320
    if (this.pokeCount >= 4) this.morphTo(5)
    if (this.pokeCount >= 7) this.morphTo(13)
    if (this.pokeCount >= 5) this.ctx.shakeUntil = now + 420
    if (this.mood === 'idle') {
      this.ctx.bias = {
        spin: dir * this.rand(8, 16),
        tx: dir * this.rand(4, 9),
        ty: -3,
        squash: 1.02,
      }
      this.ctx.biasUntil = now + this.span(2500, 8000)
    }
    return this.pokeCount
  }

  nudge(now: number) {
    queueBlink(this.blinkQ, now, this.random, this.mood === 'idle' ? 2.6 : 1)
    this.gazeX.t = this.sign() * this.rand(6, 12)
    this.gazeY.t = this.rand(-4, 3)
    this.gazeUntil = now + this.rand(700, 1400)
  }

  debug(): BlobSimDebug {
    return {
      mood: this.mood,
      eyeTo: this.eyeTo,
      spin: this.spin.x,
      blink: this.blink.x,
      heat: this.pokeHeat,
      pokes: this.pokeCount,
    }
  }

  /** 闲置眨眼可短时拉满帧；跳和换脸走 20fps，弹簧已经按这个节拍放慢。 */
  wantsHighFps(now: number): boolean {
    if (this.reduced) return false
    if (this.blinkQ.length > 0) return true
    if (now < this.winkAt + this.winkDur) return true
    if (this.hopAt >= 0) return true
    if (now < this.ctx.shakeUntil) return true
    if (this.mood === 'idle') return false
    if (this.morph.x < 0.97) return true
    if (now < this.ctx.nodEnd) return true
    if (this.mood === 'error') return now < this.ctx.shakeUntil
    return true
  }

  /** 闲置按呼吸节拍醒；出错只睡到下一次抖动。 */
  nextIdleWakeMs(now: number): number {
    if (this.mood === 'idle') return BLOB_IDLE_BREATHE_MS
    const wakes = [this.blinkUntil, this.eyeUntil]
    if (WINK.has(this.mood)) wakes.push(this.winkUntil)
    if (this.mood === 'error') wakes.push(this.ctx.impulseAt)
    return Math.min(...wakes) - now
  }

  sample(now: number): BlobPaint {
    if (!this.inited) {
      this.t0 = now
      this.last = now
      this.setMood(this.mood, now)
      this.winkUntil = now + (this.mood === 'idle' ? this.span(8000, 22000) : this.rand(4000, 8000))
      this.inited = true
    }
    const dt = Math.min((now - this.last) / 1000, 0.08)
    this.last = now
    const mt = (now - this.t0) / 1000
    const pose = applyPose(this.mood, mt, now, this.ctx, (a, b) => this.rand(a, b))
    this.spin.t = pose.spin
    this.tx.t = pose.tx
    this.ty.t = pose.ty
    this.squash.t = pose.squash
    this.boost.t = pose.boost
    if (this.mood === 'idle') {
      this.spin.t += this.gazeX.t * 0.48
      this.tx.t += this.gazeX.t * 0.3
      this.ty.t += this.gazeY.t * 0.16
    }

    if (now >= this.eyeUntil) {
      const list = PLAY[this.mood]
      this.eyeIdx = (this.eyeIdx + 1) % list.length
      this.morphTo(list[this.eyeIdx])
      this.eyeUntil = now + (this.mood === 'idle' ? this.span(...HOLD[this.mood]) : this.rand(...HOLD[this.mood]))
    }
    const cad = BLINK[this.mood]
    if (cad && now >= this.blinkUntil) {
      queueBlink(this.blinkQ, now, this.random, this.mood === 'idle' ? 2.6 : 1)
      this.blinkUntil = now + (this.mood === 'idle' ? this.span(...cad) : this.rand(...cad))
    }
    const key = consumeBlink(this.blinkQ, now)
    this.blink.t = key ?? (this.blinkQ.length ? this.blink.t : pose.lid)

    if (now >= this.gazeUntil) {
      const gz = nextGaze(this.mood, (a, b) => this.rand(a, b), () => this.sign())
      this.gazeX.t = gz.x
      this.gazeY.t = gz.y
      this.gazeUntil = now + (this.mood === 'idle' ? this.span(...gz.hold) : this.rand(...gz.hold))
    }
    if (WINK.has(this.mood) && now >= this.winkUntil) {
      this.winkAt = now
      this.winkEye = this.random() < 0.5 ? 0 : 1
      this.winkDur = this.mood === 'idle' ? 700 : 320
      this.winkUntil = now + (this.mood === 'idle' ? this.span(8000, 24000) : this.rand(4500, 10000))
    }
    const hopEvery = hopCadence(this.mood)
    if (hopEvery && now >= this.ctx.hopUntil && this.hopAt < 0) {
      this.hopAt = now
      this.ctx.hopUntil = now + (this.mood === 'idle' ? this.span(...hopEvery) : this.rand(...hopEvery))
    }
    if (this.mood === 'idle' && !this.reduced && now >= this.ctx.biasUntil) {
      const next = nextIdleBias((a, b) => this.rand(a, b), () => this.sign())
      this.ctx.bias = { spin: next.spin, tx: next.tx, ty: next.ty, squash: next.squash }
      this.ctx.biasUntil = now + this.span(...next.hold)
      if (next.hop && this.hopAt < 0) this.hopAt = now
    }

    const n = Math.max(1, Math.ceil(dt / DT))
    const step = dt / n
    const spr = this.mood === 'idle' ? SPR_IDLE : SPR
    for (let i = 0; i < n; i++) {
      stepSpring(this.spin, ...spr.spin, step)
      stepSpring(this.tx, ...spr.x, step)
      stepSpring(this.ty, ...spr.y, step)
      stepSpring(this.squash, ...spr.squash, step)
      stepSpring(this.blink, ...spr.blink, step)
      stepSpring(this.gazeX, ...spr.gaze, step)
      stepSpring(this.gazeY, ...spr.gaze, step)
      stepSpring(this.morph, ...spr.morph, step)
      stepSpring(this.boost, ...spr.boost, step)
    }
    if (this.reduced) {
      this.spin.x = 0
      this.tx.x = 0
      this.ty.x = 0
      this.squash.x = 1
      this.blink.x = 1
      this.gazeX.x = 0
      this.gazeY.x = 0
      this.morph.x = 1
      this.boost.x = 1
      this.hopAt = -1
    }

    if (now > this.heatHoldUntil && this.pokeHeat > 0) {
      this.pokeHeat = Math.max(0, this.pokeHeat - dt * 0.32)
      if (this.pokeHeat <= 0.01) {
        this.pokeHeat = 0
        this.pokeCount = 0
      }
    }

    let hop = hopY(
      this.hopAt,
      now,
      this.mood === 'idle' ? 0.36 + this.pokeHeat * 0.5 : 1,
      this.mood === 'idle' ? 2.1 : 1,
    )
    if (hop == null) {
      this.hopAt = -1
      hop = 0
    }
    const k = k2(clamp(this.morph.x, 0, 1))
    const from = EYE_CACHE[this.eyeFrom]
    const to = EYE_CACHE[this.eyeTo]
    const polys = [lerpPoly(from[0], to[0], k), lerpPoly(from[1], to[1], k)]
    const gx = this.gazeX.x
    const gy = this.gazeY.x
    const pulse = 1 + (this.mood === 'idle' ? 0.03 : 0.07) * Math.sin(k * Math.PI)
    const boost = this.boost.x
    const eyes = [0, 1].map((i) => {
      let lid = this.blink.x
      if (i === this.winkEye && now < this.winkAt + this.winkDur) {
        const xr = (now - this.winkAt) / this.winkDur
        const fr = xr < 0.42 ? 1 - xr / 0.42 : (xr - 0.42) / 0.58
        lid = Math.min(lid, Math.max(fr, 0.04))
      }
      const [cx, cy] = centroid(polys[i])
      const live = this.mood !== 'idle'
      const amp = live ? 1 : 0.28
      const wobX = (Math.sin(now * 42e-5 + i) * 3.6 + Math.sin(now * 0.001 + i * 2) * 1.3) * amp
      const wobY = Math.sin(now * 58e-5 + i) * 2.2 * amp
      const lookX = gx + wobX
      const lookY = gy + wobY
      const sx = (1 - Math.abs(gx) * 0.012) * boost * pulse
      const sy = clamp(lid, 0.04, 1.2) * boost * pulse
      return {
        d: polyPath(polys[i]),
        transform: `translate(${(lookX * 0.55).toFixed(2)} ${(lookY * 0.45).toFixed(2)}) translate(${cx.toFixed(2)} ${cy.toFixed(2)}) scale(${sx.toFixed(3)} ${sy.toFixed(3)}) translate(${(-cx).toFixed(2)} ${(-cy).toFixed(2)})`,
      }
    }) as BlobPaint['eyes']

    return {
      rig: `translate(${this.tx.x.toFixed(2)} ${(this.ty.x + hop).toFixed(2)}) rotate(${this.spin.x.toFixed(2)} ${CX} ${CY})`,
      body: `translate(${CX} ${CY}) scale(1 ${this.squash.x.toFixed(3)}) translate(${-CX} ${-CY})`,
      fill: this.mood === 'error'
        ? '#c45c2a'
        : this.pokeHeat <= 0
          ? BLOB_BLUE
          : mixHex(BLOB_BLUE, BLOB_POKE_RED, this.pokeHeat),
      eyes,
    }
  }

  private morphTo(to: number) {
    if (to === this.eyeTo && this.morph.x > 0.96) return
    this.eyeFrom = this.eyeTo
    this.eyeTo = to
    this.morph.x = 0
    this.morph.v = 0
    this.morph.t = 1
  }

  private rand(a: number, b: number) {
    return a + this.random() * (b - a)
  }

  private span(a: number, b: number) {
    return spanMs(a, b, this.random())
  }

  private sign() {
    return this.random() < 0.5 ? -1 : 1
  }
}

export const BLOB_REST = new KivioBlobSim({ random: () => 0.5, reducedMotion: true }).sample(0)
