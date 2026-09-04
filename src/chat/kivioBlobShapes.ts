/**
 * 墨团的几何字典：身体形态 + 眼睛词汇。
 *
 * 所有形状都是**定点数多边形**（身体 `BODY_POINTS`、单眼 `EYE_POINTS`），第 0 点钉在最高点、
 * 顺时针走——这样任意两张脸 / 两种身体都能逐点 lerp 出平滑变形，不需要 path 插值库。
 * 坐标系是 240×240 的 viewBox，圆心 (120,120)，身体半径 78；眼睛用局部坐标（0,0 为眼心），
 * 由 `placeEye` 落到脸上。
 *
 * 这里只有纯函数和查表，不碰时间；节拍、弹簧、状态机都在 kivioBlobSim.ts。
 */

export const CX = 120
export const CY = 120
export const BODY_R = 78
export const BODY_POINTS = 72
export const EYE_POINTS = 48

export type Pt = [number, number]

const TAU = Math.PI * 2
const sgn = (n: number) => (n < 0 ? -1 : 1)

/** 以最高点为起点、顺时针闭合：最高点有并列时取最靠中线的那个。 */
export function alignTop(pts: Pt[]): Pt[] {
  if (pts.length < 3) return pts
  let area = 0
  for (let i = 0; i < pts.length; i++) {
    const a = pts[i]
    const b = pts[(i + 1) % pts.length]
    area += a[0] * b[1] - b[0] * a[1]
  }
  // SVG y 朝下：视觉顺时针 = 有向面积为正。
  const ordered = area < 0 ? [...pts].reverse() : pts
  let best = 0
  for (let i = 1; i < ordered.length; i++) {
    const p = ordered[i]
    const q = ordered[best]
    if (p[1] < q[1] - 1e-6 || (Math.abs(p[1] - q[1]) <= 1e-6 && Math.abs(p[0]) < Math.abs(q[0]))) best = i
  }
  return ordered.slice(best).concat(ordered.slice(0, best))
}

/** 把任意顶点列表按周长等距重采样成 n 点（十字 / 星形这类折线形状用）。 */
export function resampleClosed(verts: Pt[], n: number): Pt[] {
  const segs: number[] = []
  let total = 0
  for (let i = 0; i < verts.length; i++) {
    const a = verts[i]
    const b = verts[(i + 1) % verts.length]
    const d = Math.hypot(b[0] - a[0], b[1] - a[1])
    segs.push(d)
    total += d
  }
  const out: Pt[] = []
  let seg = 0
  let walked = 0
  for (let i = 0; i < n; i++) {
    const target = (i / n) * total
    while (seg < segs.length - 1 && walked + segs[seg] < target) {
      walked += segs[seg]
      seg++
    }
    const a = verts[seg]
    const b = verts[(seg + 1) % verts.length]
    const t = segs[seg] > 0 ? (target - walked) / segs[seg] : 0
    out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
  }
  return out
}

/** 超椭圆：n=2 是圆，n≈3.2 是胶囊感的药丸，n≥4 是圆角方。 */
export function superellipse(rx: number, ry: number, n: number, count: number): Pt[] {
  const pts: Pt[] = []
  const e = 2 / n
  for (let i = 0; i < count; i++) {
    const a = (i / count) * TAU - Math.PI / 2
    const c = Math.cos(a)
    const s = Math.sin(a)
    pts.push([rx * sgn(c) * Math.abs(c) ** e, ry * sgn(s) * Math.abs(s) ** e])
  }
  return pts
}

function rotate(pts: Pt[], tilt: number): Pt[] {
  if (!tilt) return pts
  const c = Math.cos(tilt)
  const s = Math.sin(tilt)
  return pts.map(([x, y]) => [x * c - y * s, x * s + y * c])
}

function polar(count: number, radiusAt: (a: number) => number): Pt[] {
  const pts: Pt[] = []
  for (let i = 0; i < count; i++) {
    const a = (i / count) * TAU - Math.PI / 2
    const r = radiusAt(a)
    pts.push([r * Math.cos(a), r * Math.sin(a)])
  }
  return pts
}

// ---------------------------------------------------------------------------
// 身体
// ---------------------------------------------------------------------------

/**
 * 身体形态。默认永远是圆；其余按状态借用（思考=云、干活=圆角方、检索=竖起来的蛋、
 * 说话=带尾巴的气泡、出错=摊成一滩 + 抖的时候炸毛），闲置偶尔随机换一个玩。
 */
export type BodyShape = 'circle' | 'squircle' | 'cloud' | 'egg' | 'bubble' | 'puddle' | 'burst'

export const BODY_SHAPES: readonly BodyShape[] = ['circle', 'squircle', 'cloud', 'egg', 'bubble', 'puddle', 'burst']

function bodyLocal(shape: BodyShape): Pt[] {
  const R = BODY_R
  switch (shape) {
    case 'squircle':
      // n=4 超椭圆面积 ≈3.71R²，缩到 0.92 让视觉体量和圆（πR²）一致。
      return superellipse(R * 0.92, R * 0.92, 4.2, BODY_POINTS)
    case 'cloud': {
      // 经典画法：几个圆的并集 + 平底。沿射线取各圆的最远交点就是并集轮廓（圆心都包着原点，
      // 射线必有交点）。径向鼓包那套出来的是五边形，不像云。
      const lumps: [number, number, number][] = [
        [0, -0.1, 0.62],
        [-0.42, 0.12, 0.5],
        [0.4, 0.05, 0.56],
        [0.18, -0.32, 0.45],
      ]
      // 圆并集本身只有圆 56% 的体量，整体放大 1.2 拉回 ~80%，换形态时不会忽然缩一号。
      const floor = R * 0.6
      const grow = 1.2
      return polar(BODY_POINTS, (a) => {
        const dx = Math.cos(a)
        const dy = Math.sin(a)
        let best = 0
        for (const [cx, cy, cr] of lumps) {
          const dot = dx * cx * R + dy * cy * R
          const disc = dot * dot - (cx * cx + cy * cy) * R * R + cr * cr * R * R
          if (disc >= 0) best = Math.max(best, dot + Math.sqrt(disc))
        }
        return best
      }).map(([x, y]) => [x * grow, (y > 0 ? floor * Math.tanh(y / floor) : y) * grow])
    }
    case 'egg':
      // 顶部收窄、整体拉高：竖起耳朵的样子。
      return superellipse(R * 0.95, R * 1.05, 2.2, BODY_POINTS).map(([x, y]) => {
        const top = (-y / (R * 1.05) + 1) / 2 // 1 = 顶, 0 = 底
        return [x * (1 - 0.2 * top), y]
      })
    case 'bubble':
      // 圆 + 左下一个高斯尾巴 = 对话气泡。
      return polar(BODY_POINTS, (a) => {
        let d = a - (Math.PI * 3) / 4
        d = Math.atan2(Math.sin(d), Math.cos(d))
        const tail = Math.exp(-((d / 0.2) ** 2))
        return R * (0.97 + 0.3 * tail)
      })
    case 'puddle':
      return superellipse(R * 1.16, R * 0.8, 2.6, BODY_POINTS).map(([x, y]) => [x, y + R * 0.15])
    case 'burst':
      return polar(BODY_POINTS, (a) => {
        const phase = ((9 * (a + Math.PI / 2)) / TAU) % 1
        const tri = 1 - 2 * Math.abs(phase - 0.5)
        return R * (0.9 + 0.15 * tri)
      })
    case 'circle':
    default:
      return superellipse(R, R, 2, BODY_POINTS)
  }
}

const BODY_CACHE = new Map<BodyShape, Pt[]>()

export function bodyPoints(shape: BodyShape): Pt[] {
  let pts = BODY_CACHE.get(shape)
  if (!pts) {
    pts = alignTop(bodyLocal(shape)).map(([x, y]) => [CX + x, CY + y])
    BODY_CACHE.set(shape, pts)
  }
  return pts
}

// ---------------------------------------------------------------------------
// 眼睛
// ---------------------------------------------------------------------------

/**
 * 单眼的形状词汇（局部坐标，眼心在 0,0）：
 * - pill / dot：睁着的眼，靠 rx/ry/n 调胖瘦；
 * - archUp（^）闭眼笑；archDown（u）抬头看 / 满足；
 * - halfMoon：上眼皮压下来一半，困 / 无聊；
 * - cross：x_x，晕；star：✦，亮了；
 */
export type EyeKind = 'pill' | 'dot' | 'archUp' | 'archDown' | 'halfMoon' | 'cross' | 'star'

export interface EyeSpec {
  kind: EyeKind
  /** 半宽 / 半高（局部单位；240 viewBox 里身体半径是 78）。 */
  rx: number
  ry: number
  /** 弧度，左眼原值、右眼默认取反（镜像）。 */
  tilt?: number
  /** 眼心相对脸中线的偏移：x 只写正值，右眼自动镜像；y 朝下为正。 */
  x: number
  y: number
  /** 超椭圆指数（pill/dot 用）。 */
  n?: number
}

function eyeLocal(spec: EyeSpec): Pt[] {
  const { rx, ry } = spec
  switch (spec.kind) {
    case 'dot':
      return superellipse(rx, ry, spec.n ?? 2.3, EYE_POINTS)
    case 'archUp':
    case 'archDown': {
      // 抛物线拱：外弧 y=-h(1-(x/w)²)，内弧往下压 t，两端收细。
      const half = EYE_POINTS / 2
      const w = rx
      const h = ry
      const t = Math.max(3, ry * 0.7)
      const pts: Pt[] = []
      for (let i = 0; i < half; i++) {
        const x = -w + (2 * w * i) / (half - 1)
        pts.push([x, -h * (1 - (x / w) ** 2)])
      }
      for (let i = half - 1; i >= 0; i--) {
        const x = -w + (2 * w * i) / (half - 1)
        pts.push([x, -h * (1 - (x / w) ** 2) + t * (1 - 0.4 * (x / w) ** 2)])
      }
      const shift = h * 0.5 - t * 0.3
      const out = pts.map(([x, y]) => [x, y + shift] as Pt)
      return spec.kind === 'archDown' ? out.map(([x, y]) => [x, -y] as Pt) : out
    }
    case 'halfMoon': {
      // 平顶 + 半圆底。
      const top = EYE_POINTS / 3
      const pts: Pt[] = []
      const flatY = -ry * 0.2
      for (let i = 0; i < top; i++) pts.push([-rx + (2 * rx * i) / (top - 1), flatY])
      const arc = EYE_POINTS - top
      for (let i = 0; i < arc; i++) {
        const th = (i / arc) * Math.PI
        pts.push([rx * Math.cos(th), flatY + ry * 1.2 * Math.sin(th)])
      }
      return pts
    }
    case 'cross': {
      const L = rx
      const t = Math.max(2.5, ry)
      const plus: Pt[] = [
        [-t, -L], [t, -L], [t, -t], [L, -t], [L, t], [t, t],
        [t, L], [-t, L], [-t, t], [-L, t], [-L, -t], [-t, -t],
      ]
      return resampleClosed(rotate(plus, Math.PI / 4), EYE_POINTS)
    }
    case 'star': {
      const outer = rx
      const inner = rx * 0.36
      const verts: Pt[] = []
      for (let i = 0; i < 8; i++) {
        const a = (i / 8) * TAU - Math.PI / 2
        const r = i % 2 === 0 ? outer : inner
        verts.push([r * Math.cos(a), r * Math.sin(a) * (ry / rx)])
      }
      return resampleClosed(verts, EYE_POINTS)
    }
    case 'pill':
    default:
      return superellipse(rx, ry, spec.n ?? 3.2, EYE_POINTS)
  }
}

/** 左眼 side=-1，右眼 side=1（x 与 tilt 镜像）。 */
export function placeEye(spec: EyeSpec, side: -1 | 1): Pt[] {
  const tilt = (spec.tilt ?? 0) * side
  const local = alignTop(rotate(eyeLocal(spec), tilt))
  return local.map(([x, y]) => [CX + side * spec.x + x, CY + spec.y + y])
}

/**
 * 一张脸 = 左右各一只眼。右眼不写就镜像左眼。`noBlink` 的脸是本来就闭着 / 不是眼皮语义
 * 的（笑弯、x_x、星星、一条线），眨眼缩放只会让它们抽一下。
 */
export interface FaceSpec {
  left: EyeSpec
  right?: EyeSpec
  noBlink?: boolean
}

export type FaceName =
  | 'neutral'
  | 'dots'
  | 'wide'
  | 'tiny'
  | 'lines'
  | 'focus'
  | 'worry'
  | 'smirk'
  | 'happy'
  | 'content'
  | 'sleepy'
  | 'dizzy'
  | 'sparkle'
  | 'lookUp'
  | 'hmm'
  | 'peek'
  | 'flat'

export const FACES: Record<FaceName, FaceSpec> = {
  /** 默认：两颗竖着的软药丸。 */
  neutral: { left: { kind: 'pill', rx: 12, ry: 19, n: 2.8, x: 24, y: -2 } },
  /** 圆点，呆一点。 */
  dots: { left: { kind: 'dot', rx: 13.5, ry: 14, x: 23, y: 0 } },
  /** 瞪大：惊 / 等你回答。 */
  wide: { left: { kind: 'dot', rx: 16, ry: 17.5, n: 2.2, x: 24, y: -3 } },
  /** 缩成小点：局促 / 心虚。 */
  tiny: { left: { kind: 'dot', rx: 8, ry: 8.5, x: 22, y: 2 } },
  /** 两条线：眯着，嘴上不说心里有数。 */
  lines: { left: { kind: 'pill', rx: 15, ry: 3.6, n: 3.6, x: 23, y: 3 }, noBlink: true },
  /** 内倾 / \：专注、来劲了。 */
  focus: { left: { kind: 'pill', rx: 10, ry: 17, n: 3, tilt: 0.34, x: 22, y: 0 } },
  /** 外倾 \ /：担心、不好意思。 */
  worry: { left: { kind: 'pill', rx: 10, ry: 17, n: 3, tilt: -0.3, x: 23, y: 1 } },
  /** 一只眯一只睁：搞鬼。 */
  smirk: {
    left: { kind: 'pill', rx: 14, ry: 3.6, n: 3.6, x: 23, y: 3 },
    right: { kind: 'dot', rx: 13, ry: 14, x: 24, y: -2 },
  },
  /** ^ ^ 闭眼笑。 */
  happy: { left: { kind: 'archUp', rx: 15, ry: 10, x: 23, y: -1 }, noBlink: true },
  /** u u 抬头满足。 */
  content: { left: { kind: 'archDown', rx: 14, ry: 9, x: 23, y: -3 }, noBlink: true },
  /** 半睁：困 / 无聊。 */
  sleepy: { left: { kind: 'halfMoon', rx: 14, ry: 12, x: 23, y: 1 }, noBlink: true },
  /** x_x */
  dizzy: { left: { kind: 'cross', rx: 13, ry: 4.4, x: 23, y: 0 }, noBlink: true },
  /** ✦ ✦ */
  sparkle: { left: { kind: 'star', rx: 16, ry: 16, x: 24, y: -2 }, noBlink: true },
  /** 眼睛挪到上面：想事情。 */
  lookUp: { left: { kind: 'dot', rx: 11, ry: 12, x: 22, y: -11 } },
  /** 一只正常一只半眯：嗯？ */
  hmm: {
    left: { kind: 'pill', rx: 12, ry: 19, n: 2.8, x: 24, y: -2 },
    right: { kind: 'pill', rx: 12, ry: 9, n: 3, x: 24, y: 0 },
  },
  /** 往一边挤：偷看。 */
  peek: { left: { kind: 'pill', rx: 10, ry: 16, n: 3, x: 28, y: -4 } },
  /** 压扁一点的默认：被戳 / 被摊平时用。 */
  flat: { left: { kind: 'pill', rx: 13, ry: 12, n: 3, x: 24, y: 4 } },
}

export const FACE_NAMES = Object.keys(FACES) as FaceName[]

const FACE_CACHE = new Map<FaceName, [Pt[], Pt[]]>()

export function facePoints(name: FaceName): [Pt[], Pt[]] {
  let pts = FACE_CACHE.get(name)
  if (!pts) {
    const spec = FACES[name] ?? FACES.neutral
    pts = [placeEye(spec.left, -1), placeEye(spec.right ?? spec.left, 1)]
    FACE_CACHE.set(name, pts)
  }
  return pts
}

export function faceBlinks(name: FaceName): boolean {
  return !(FACES[name] ?? FACES.neutral).noBlink
}

export function polyPath(pts: Pt[]): string {
  return `M${pts.map((p) => `${p[0].toFixed(2)} ${p[1].toFixed(2)}`).join('L')}Z`
}

export function lerpPoly(a: Pt[], b: Pt[], t: number): Pt[] {
  return a.map((p, i) => [p[0] + (b[i][0] - p[0]) * t, p[1] + (b[i][1] - p[1]) * t])
}

export function centroid(pts: Pt[]): Pt {
  let x = 0
  let y = 0
  for (const p of pts) {
    x += p[0]
    y += p[1]
  }
  const n = pts.length || 1
  return [x / n, y / n]
}
