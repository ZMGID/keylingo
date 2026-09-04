import {
  BODY_R,
  CX,
  CY,
  EYE_POINTS,
  bodyPoints,
  centroid,
  faceBlinks,
  facePoints,
  lerpPoly,
  polyPath,
  type BodyShape,
  type FaceName,
  type Pt,
} from './kivioBlobShapes'

export { CX, CY, BODY_R, EYE_POINTS, polyPath }
export type { BodyShape, FaceName }

export const BLOB_BLUE = '#1d6bf0'
export const BLOB_EYE = '#f3efe6'
export const BLOB_POKE_RED = '#e23b2e'
const BLOB_ERROR = '#c45c2a'

function mixHex(a: string, b: string, t: number): string {
  const u = clamp(t, 0, 1)
  const n = (h: string, i: number) => parseInt(h.slice(1 + i * 2, 3 + i * 2), 16)
  const m = (i: number) => Math.round(n(a, i) + (n(b, i) - n(a, i)) * u)
  return `#${[0, 1, 2].map((i) => m(i).toString(16).padStart(2, '0')).join('')}`
}

/**
 * 跟生成过程对齐的心情。idle 闲着；think / search / work / speak 是一轮生成里的四个阶段；
 * error 翻车；done 是刚收工那两秒的小得意；wait 是停下来等用户（问问题 / 等审批）。
 */
export type BlobMood = 'idle' | 'think' | 'search' | 'work' | 'speak' | 'error' | 'done' | 'wait'

const DT = 1 / 120
// 显式标注:两表三元取用后是同一类型,元组才能直接 spread 进 stepSpring(TS2556)。
type SpringTable = Record<
  'spin' | 'x' | 'y' | 'squash' | 'blink' | 'gaze' | 'morph' | 'body' | 'boost',
  readonly [number, number]
>
const SPR: SpringTable = {
  spin: [5, 0.9],
  x: [3.5, 1],
  y: [4, 1],
  squash: [10, 0.8],
  blink: [26, 1],
  gaze: [13, 1],
  morph: [7, 1],
  body: [6, 0.92],
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
  body: [1.9, 1],
  boost: [3.2, 0.9],
}

/** 每个心情轮播的脸。第 0 张是进入该心情时先摆的那张。 */
const FACE_PLAY: Record<BlobMood, FaceName[]> = {
  idle: ['neutral', 'dots', 'neutral', 'smirk', 'neutral', 'peek', 'sleepy', 'neutral', 'hmm'],
  think: ['lookUp', 'hmm', 'lines', 'neutral', 'lookUp', 'focus'],
  search: ['wide', 'peek', 'dots', 'wide', 'neutral'],
  work: ['focus', 'lines', 'focus', 'neutral', 'dots'],
  speak: ['neutral', 'dots', 'happy', 'neutral', 'hmm'],
  error: ['dizzy', 'worry', 'tiny', 'dizzy', 'lines'],
  done: ['happy', 'sparkle', 'happy', 'content'],
  wait: ['wide', 'neutral', 'wide', 'hmm'],
}
const FACE_HOLD: Record<BlobMood, [number, number]> = {
  idle: [5000, 18000],
  think: [2000, 3600],
  search: [1000, 1800],
  work: [1800, 3200],
  speak: [2800, 5000],
  error: [2200, 3800],
  done: [700, 1100],
  wait: [2600, 4800],
}

/** 身体形态轮播：大多数时候是圆，状态借一个形态来说话。 */
const BODY_PLAY: Record<BlobMood, BodyShape[]> = {
  idle: ['circle'],
  think: ['cloud', 'cloud', 'circle', 'cloud'],
  search: ['egg', 'egg', 'circle'],
  work: ['squircle', 'squircle', 'circle', 'squircle'],
  speak: ['bubble', 'circle', 'bubble', 'bubble'],
  error: ['puddle'],
  done: ['circle'],
  wait: ['circle', 'egg'],
}
const BODY_HOLD: Record<BlobMood, [number, number]> = {
  idle: [1e9, 1e9],
  think: [3500, 7000],
  search: [2500, 5000],
  work: [3000, 6500],
  speak: [3000, 6000],
  error: [1e9, 1e9],
  done: [1e9, 1e9],
  wait: [4000, 8000],
}
/** 闲置偶尔随机变一下形态玩（连同 hop 一起对外汇报，让空态标题能接一句嘴）。 */
const IDLE_ANTIC_SHAPES: BodyShape[] = ['squircle', 'cloud', 'egg']
export type BlobAntic = BodyShape | 'hop'

const BLINK: Record<BlobMood, [number, number] | null> = {
  idle: [4000, 16000],
  think: [3500, 7000],
  search: [1600, 4000],
  work: [2800, 5500],
  speak: [3000, 7000],
  error: [3500, 7000],
  done: null,
  wait: [2200, 4800],
}
const WINK = new Set<BlobMood>(['idle', 'speak', 'done'])
const HOP_SEGS = [
  { h: 48, d: 0.5 },
  { h: 28, d: 0.382 },
  { h: 14, d: 0.27 },
  { h: 6, d: 0.177 },
]
const HOP_DUR = HOP_SEGS.reduce((s, x) => s + x.d, 0)

type Spring = { x: number; v: number; t: number }

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

/**
 * 多边形组的变形器：from → to 走一根 0→1 的弹簧。中途改目标时把**当前插值结果**烘成新的
 * from，不会像直接换 to 那样跳一帧（身体换形态很大块，跳一下很明显）。
 */
class PolyMorph {
  from: Pt[][]
  to: Pt[][]
  key: string
  s = spring(1)

  constructor(key: string, pts: Pt[][]) {
    this.key = key
    this.from = pts
    this.to = pts
  }

  retarget(key: string, pts: Pt[][]) {
    if (key === this.key) return
    const k = k2(clamp(this.s.x, 0, 1))
    this.from = this.to.map((poly, i) => lerpPoly(this.from[i], poly, k))
    this.to = pts
    this.key = key
    this.s.x = 0
    this.s.v = 0
    this.s.t = 1
  }

  snap(key: string, pts: Pt[][]) {
    this.key = key
    this.from = pts
    this.to = pts
    this.s.x = 1
    this.s.v = 0
    this.s.t = 1
  }

  progress(): number {
    return k2(clamp(this.s.x, 0, 1))
  }

  current(): Pt[][] {
    const k = this.progress()
    if (k >= 1) return this.to
    return this.to.map((poly, i) => lerpPoly(this.from[i], poly, k))
  }

  settled(): boolean {
    return this.s.x > 0.97
  }
}

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
  /** 正在等用户回答 / 审批：停下来看着你。 */
  waiting?: boolean
  /** 刚收工（StreamStatusLine 记的短窗口）：小得意一下。 */
  done?: boolean
}): BlobMood {
  if (input.error && !input.active) return 'error'
  if (!input.active) return input.done ? 'done' : 'idle'
  if (input.error) return 'error'
  if (input.waiting) return 'wait'
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
  /** 闲置随机变的形态；null = 圆。 */
  antic: BodyShape | null
  anticUntil: number
}

function nextIdleBias(
  rand: (a: number, b: number) => number,
  sign: () => number,
): IdleBias & { hold: [number, number]; hop: boolean; antic: BodyShape | null } {
  // 档位：38% 原地不动、28% 歪一点、12% 上浮、8% 下沉、8% 蹦一下、6% 变形态。
  const r = rand(0, 1)
  if (r < 0.38) {
    return { spin: 0, tx: 0, ty: 0, squash: 1, hold: [4000, 14000], hop: false, antic: null }
  }
  if (r < 0.66) {
    const d = sign()
    return {
      spin: d * rand(6, 14),
      tx: d * rand(2.5, 7),
      ty: rand(-1.5, 1.2),
      squash: 1,
      hold: [3500, 12000],
      hop: false,
      antic: null,
    }
  }
  if (r < 0.78) {
    return { spin: rand(-3, 3), tx: 0, ty: -rand(1.5, 4), squash: 1.02, hold: [2800, 9000], hop: false, antic: null }
  }
  if (r < 0.86) {
    return { spin: 0, tx: 0, ty: rand(1.5, 4), squash: 0.984, hold: [3500, 11000], hop: false, antic: null }
  }
  if (r < 0.94) {
    const d = sign()
    return {
      spin: d * rand(4, 10),
      tx: d * rand(2, 5),
      ty: -2,
      squash: 1,
      hold: [4000, 13000],
      hop: true,
      antic: null,
    }
  }
  // 变个形态玩一会，再慢慢变回圆（每次挑档 6%，档位 3–10 秒一换 ⇒ 大约两分钟一回）。
  const shape = IDLE_ANTIC_SHAPES[Math.min(IDLE_ANTIC_SHAPES.length - 1, Math.floor(rand(0, IDLE_ANTIC_SHAPES.length)))]
  return {
    spin: shape === 'cloud' ? rand(-4, 4) : sign() * rand(3, 8),
    tx: 0,
    ty: shape === 'cloud' ? -3 : 0,
    squash: 1,
    hold: [3500, 7500],
    hop: false,
    antic: shape,
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
    // 云是横着飘的，别歪太多；左右慢慢晃 + 轻微上浮。
    spin = -6 + Math.sin(mt * 0.35) * 5
    tx = Math.sin(mt * 0.3) * 8
    ty = -2 + Math.sin(mt * 0.6) * 4
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
  } else if (mood === 'done') {
    // 果冻式小蹦：左右晃 + 上下弹 + 拉伸压扁。
    const et = Math.sin(mt * 2.4)
    spin = Math.sin(mt * 1.2) * 4
    tx = Math.sin(mt * 1.1) * 2.5
    ty = -Math.abs(et) * 3.5
    squash = 1 + et * 0.025
    boost = 1.06
  } else if (mood === 'wait') {
    // 歪头看着你，偶尔慢慢摆一下。
    spin = 13 + Math.sin(mt * 0.45) * 3
    tx = 3
    ty = -1 + Math.sin(mt * 0.7) * 0.8
    squash = 1.01
    boost = 1.03
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
  if (mood === 'done') return { x: rand(-0.3, 0.3) * 16, y: -rand(0.1, 0.5) * 10, hold: [900, 1600] as [number, number] }
  if (mood === 'wait') return { x: rand(-0.15, 0.15) * 16, y: -rand(0.5, 0.9) * 10, hold: [1800, 3600] as [number, number] }
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
  bodyD: string
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
  face: FaceName
  body: BodyShape
  spin: number
  blink: number
  heat: number
  pokes: number
}

export class KivioBlobSim {
  mood: BlobMood = 'idle'
  /** 闲置小动作（变形态 / 蹦一下）的通知口，空态标题拿它接一句嘴。 */
  onAntic: ((kind: BlobAntic) => void) | null = null
  private random: () => number
  private reduced: boolean
  private spin = spring(0)
  private tx = spring(0)
  private ty = spring(0)
  private squash = spring(1)
  private blink = spring(1)
  private gazeX = spring(0)
  private gazeY = spring(0)
  private boost = spring(1)
  private face = new PolyMorph('neutral', facePoints('neutral'))
  private body = new PolyMorph('circle', [bodyPoints('circle')])
  private faceIdx = 0
  private bodyIdx = 0
  private t0 = 0
  private last = 0
  private faceUntil = 0
  private bodyUntil = 0
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
  /** 被戳时身体的临时形态（压扁 / 装方 / 炸毛）。 */
  private pokeShape: BodyShape | null = null
  private pokeShapeUntil = 0
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
    antic: null,
    anticUntil: 0,
  }

  constructor(opts: { random?: () => number; reducedMotion?: boolean } = {}) {
    this.random = opts.random ?? Math.random
    this.reduced = opts.reducedMotion ?? false
  }

  setMood(mood: BlobMood, now = 0) {
    const changed = mood !== this.mood || !this.inited
    this.mood = mood
    const faces = FACE_PLAY[mood]
    const bodies = BODY_PLAY[mood]
    this.faceIdx = 0
    this.bodyIdx = 0
    if (changed) {
      this.face.retarget(faces[0], facePoints(faces[0]))
      this.body.retarget(bodies[0], [bodyPoints(bodies[0])])
    }
    this.faceUntil = now + (mood === 'idle' ? this.span(...FACE_HOLD[mood]) : this.rand(...FACE_HOLD[mood]))
    this.bodyUntil = now + this.rand(...BODY_HOLD[mood])
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
      antic: null,
      anticUntil: 0,
    }
    this.blinkQ = []
    if (mood === 'done' && changed) {
      // 收工先蹦一下。
      this.hopAt = now
    } else if (faceBlinks(faces[0])) {
      queueBlink(this.blinkQ, now, this.random, mood === 'idle' ? 2.6 : 1)
    }
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
    // 戳一下压扁；戳多了装方；再戳炸毛。
    if (this.pokeCount >= 7) {
      this.pokeShape = 'burst'
      this.pokeShapeUntil = now + 1400
    } else if (this.pokeCount >= 4) {
      this.pokeShape = 'squircle'
      this.pokeShapeUntil = now + 1800
    } else {
      this.pokeShape = 'puddle'
      this.pokeShapeUntil = now + (this.mood === 'idle' ? 520 : 300)
    }
    if (this.pokeCount >= 7) this.face.retarget('dizzy', facePoints('dizzy'))
    else if (this.pokeCount >= 4) this.face.retarget('lines', facePoints('lines'))
    else if (this.pokeCount >= 2) this.face.retarget('smirk', facePoints('smirk'))
    else this.face.retarget('flat', facePoints('flat'))
    this.faceUntil = now + this.rand(1400, 2600)
    if (this.pokeCount >= 5) this.ctx.shakeUntil = now + 420
    if (this.mood === 'idle') {
      this.ctx.bias = {
        spin: dir * this.rand(8, 16),
        tx: dir * this.rand(4, 9),
        ty: -3,
        squash: 1.02,
      }
      this.ctx.biasUntil = now + this.span(2500, 8000)
      // 被戳就别继续装云了。
      this.ctx.antic = null
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
      face: this.face.key as FaceName,
      body: this.body.key as BodyShape,
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
    if (this.pokeShape && now < this.pokeShapeUntil + 600) return true
    if (this.mood === 'idle') return false
    if (!this.face.settled() || !this.body.settled()) return true
    if (now < this.ctx.nodEnd) return true
    if (this.mood === 'error') return now < this.ctx.shakeUntil
    return true
  }

  /** 闲置按呼吸节拍醒；出错只睡到下一次抖动。 */
  nextIdleWakeMs(now: number): number {
    if (this.mood === 'idle') return BLOB_IDLE_BREATHE_MS
    const wakes = [this.blinkUntil, this.faceUntil, this.bodyUntil]
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

    if (now >= this.faceUntil) {
      const list = FACE_PLAY[this.mood]
      this.faceIdx = (this.faceIdx + 1) % list.length
      this.face.retarget(list[this.faceIdx], facePoints(list[this.faceIdx]))
      this.faceUntil = now + (this.mood === 'idle' ? this.span(...FACE_HOLD[this.mood]) : this.rand(...FACE_HOLD[this.mood]))
    }
    if (now >= this.bodyUntil) {
      const list = BODY_PLAY[this.mood]
      this.bodyIdx = (this.bodyIdx + 1) % list.length
      this.bodyUntil = now + this.rand(...BODY_HOLD[this.mood])
    }
    if (this.pokeShape && now >= this.pokeShapeUntil) this.pokeShape = null
    if (this.ctx.antic && now >= this.ctx.anticUntil) this.ctx.antic = null
    const bodyShape = this.bodyTarget(now)
    this.body.retarget(bodyShape, [bodyPoints(bodyShape)])

    // 闭着的脸（笑弯 / x_x / 星星 / 一条线）不眨；但节拍照走，否则 nextIdleWakeMs 会拿到过期时间。
    const blinkable = faceBlinks(this.face.key as FaceName)
    const cad = BLINK[this.mood]
    if (cad && now >= this.blinkUntil) {
      if (blinkable) queueBlink(this.blinkQ, now, this.random, this.mood === 'idle' ? 2.6 : 1)
      this.blinkUntil = now + (this.mood === 'idle' ? this.span(...cad) : this.rand(...cad))
    }
    const key = consumeBlink(this.blinkQ, now)
    this.blink.t = blinkable ? key ?? (this.blinkQ.length ? this.blink.t : pose.lid) : 1

    if (now >= this.gazeUntil) {
      const gz = nextGaze(this.mood, (a, b) => this.rand(a, b), () => this.sign())
      this.gazeX.t = gz.x
      this.gazeY.t = gz.y
      this.gazeUntil = now + (this.mood === 'idle' ? this.span(...gz.hold) : this.rand(...gz.hold))
    }
    if (WINK.has(this.mood) && now >= this.winkUntil) {
      if (blinkable) {
        this.winkAt = now
        this.winkEye = this.random() < 0.5 ? 0 : 1
        this.winkDur = this.mood === 'idle' ? 700 : 320
      }
      this.winkUntil = now + (this.mood === 'idle' ? this.span(8000, 24000) : this.rand(4500, 10000))
    }
    const hopEvery = hopCadence(this.mood)
    if (hopEvery && now >= this.ctx.hopUntil && this.hopAt < 0) {
      this.hopAt = now
      this.ctx.hopUntil = now + (this.mood === 'idle' ? this.span(...hopEvery) : this.rand(...hopEvery))
      if (this.mood === 'idle') this.onAntic?.('hop')
    }
    if (this.mood === 'idle' && !this.reduced && now >= this.ctx.biasUntil) {
      const next = nextIdleBias((a, b) => this.rand(a, b), () => this.sign())
      this.ctx.bias = { spin: next.spin, tx: next.tx, ty: next.ty, squash: next.squash }
      this.ctx.biasUntil = now + this.span(...next.hold)
      if (next.hop && this.hopAt < 0) {
        this.hopAt = now
        this.onAntic?.('hop')
      }
      if (next.antic) {
        this.ctx.antic = next.antic
        this.ctx.anticUntil = now + this.rand(...next.hold)
        this.onAntic?.(next.antic)
      }
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
      stepSpring(this.face.s, ...spr.morph, step)
      stepSpring(this.body.s, ...(this.pokeShape ? SPR.body : spr.body), step)
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
      this.boost.x = 1
      this.hopAt = -1
      const faces = FACE_PLAY[this.mood]
      this.face.snap(faces[0], facePoints(faces[0]))
      this.body.snap('circle', [bodyPoints('circle')])
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
      this.mood === 'idle' ? 0.36 + this.pokeHeat * 0.5 : this.mood === 'done' ? 0.7 : 1,
      this.mood === 'idle' ? 2.1 : 1,
    )
    if (hop == null) {
      this.hopAt = -1
      hop = 0
    }
    const polys = this.face.current()
    const k = this.face.progress()
    const gx = this.gazeX.x
    const gy = this.gazeY.x
    const pulse = 1 + (this.mood === 'idle' ? 0.03 : 0.07) * Math.sin(k * Math.PI)
    const boost = this.boost.x
    const eyes = [0, 1].map((i) => {
      let lid = this.blink.x
      if (blinkable && i === this.winkEye && now < this.winkAt + this.winkDur) {
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
      bodyD: polyPath(this.body.current()[0]),
      fill: this.mood === 'error'
        ? BLOB_ERROR
        : this.pokeHeat <= 0
          ? BLOB_BLUE
          : mixHex(BLOB_BLUE, BLOB_POKE_RED, this.pokeHeat),
      eyes,
    }
  }

  /** 此刻身体该是什么形：被戳 > 出错抖动炸毛 > 闲置小动作 > 心情轮播。 */
  private bodyTarget(now: number): BodyShape {
    if (this.pokeShape) return this.pokeShape
    if (this.mood === 'error') return now < this.ctx.shakeUntil ? 'burst' : 'puddle'
    if (this.mood === 'idle') return this.ctx.antic ?? 'circle'
    const list = BODY_PLAY[this.mood]
    return list[this.bodyIdx % list.length]
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
