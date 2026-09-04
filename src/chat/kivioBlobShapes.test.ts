import { describe, expect, it } from 'vitest'
import {
  BODY_POINTS,
  BODY_R,
  BODY_SHAPES,
  CX,
  CY,
  EYE_POINTS,
  FACE_NAMES,
  alignTop,
  bodyPoints,
  faceBlinks,
  facePoints,
  resampleClosed,
  superellipse,
  type Pt,
} from './kivioBlobShapes'

function signedArea(pts: Pt[]): number {
  let area = 0
  for (let i = 0; i < pts.length; i++) {
    const a = pts[i]
    const b = pts[(i + 1) % pts.length]
    area += a[0] * b[1] - b[0] * a[1]
  }
  return area / 2
}

describe('kivioBlobShapes', () => {
  it('每种身体都是 BODY_POINTS 点、顺时针、起点在最高处、包着圆心', () => {
    for (const shape of BODY_SHAPES) {
      const pts = bodyPoints(shape)
      expect(pts.length).toBe(BODY_POINTS)
      expect(signedArea(pts)).toBeGreaterThan(0)
      const minY = Math.min(...pts.map((p) => p[1]))
      expect(pts[0][1]).toBeCloseTo(minY, 6)
      // 身体体量在圆的 ±35% 内：换形态不该忽大忽小。
      const area = signedArea(pts)
      expect(area).toBeGreaterThan(Math.PI * BODY_R * BODY_R * 0.65)
      expect(area).toBeLessThan(Math.PI * BODY_R * BODY_R * 1.35)
      expect(pts.some((p) => p[0] < CX) && pts.some((p) => p[0] > CX)).toBe(true)
      expect(pts.some((p) => p[1] < CY) && pts.some((p) => p[1] > CY)).toBe(true)
    }
  })

  it('云有平底、气泡左下带尾巴、方块比圆更贴边', () => {
    const cloud = bodyPoints('cloud')
    const maxY = Math.max(...cloud.map((p) => p[1]))
    // 底边：贴着最低点 5% R 以内的点要有一排，横向铺开超过 1.3R。
    const base = cloud.filter((p) => p[1] > maxY - BODY_R * 0.05)
    expect(base.length).toBeGreaterThanOrEqual(12)
    const xs = base.map((p) => p[0])
    expect(Math.max(...xs) - Math.min(...xs)).toBeGreaterThan(BODY_R * 1.3)

    const bubble = bodyPoints('bubble')
    const tail = bubble.reduce((best, p) => (Math.hypot(p[0] - CX, p[1] - CY) > Math.hypot(best[0] - CX, best[1] - CY) ? p : best))
    expect(tail[0]).toBeLessThan(CX)
    expect(tail[1]).toBeGreaterThan(CY)
    expect(Math.hypot(tail[0] - CX, tail[1] - CY)).toBeGreaterThan(BODY_R * 1.15)

    const squircle = bodyPoints('squircle')
    const diag = squircle.reduce((best, p) => {
      const d = Math.abs(p[0] - CX) + Math.abs(p[1] - CY)
      return d > best ? d : best
    }, 0)
    // 圆的 |x|+|y| 峰值是 √2·R≈1.414R；n=4.2 的圆角方在 45° 处是 2·0.92R·cos45°^(2/4.2)≈1.56R。
    expect(diag).toBeGreaterThan(BODY_R * 1.5)
  })

  it('每张脸左右各 EYE_POINTS 点，分居中线两侧，顺时针', () => {
    for (const name of FACE_NAMES) {
      const [left, right] = facePoints(name)
      expect(left.length).toBe(EYE_POINTS)
      expect(right.length).toBe(EYE_POINTS)
      expect(Math.max(...left.map((p) => p[0]))).toBeLessThan(CX)
      expect(Math.min(...right.map((p) => p[0]))).toBeGreaterThan(CX)
      expect(signedArea(left)).toBeGreaterThan(0)
      expect(signedArea(right)).toBeGreaterThan(0)
      for (const p of [...left, ...right]) {
        expect(Number.isFinite(p[0]) && Number.isFinite(p[1])).toBe(true)
      }
    }
  })

  it('对称脸右眼是左眼的镜像；smirk / hmm 左右不同', () => {
    const [l, r] = facePoints('neutral')
    const mirrored = l.map(([x, y]) => [2 * CX - x, y] as Pt)
    const rSet = new Set(r.map(([x, y]) => `${x.toFixed(3)},${y.toFixed(3)}`))
    const hits = mirrored.filter(([x, y]) => rSet.has(`${x.toFixed(3)},${y.toFixed(3)}`)).length
    expect(hits).toBe(EYE_POINTS)

    const [sl, sr] = facePoints('smirk')
    expect(signedArea(sl)).toBeLessThan(signedArea(sr) * 0.5)
    const [hl, hr] = facePoints('hmm')
    expect(signedArea(hr)).toBeLessThan(signedArea(hl))
  })

  it('闭着 / 非眼皮语义的脸不参与眨眼', () => {
    expect(faceBlinks('neutral')).toBe(true)
    expect(faceBlinks('wide')).toBe(true)
    for (const name of ['happy', 'content', 'sleepy', 'dizzy', 'sparkle', 'lines'] as const) {
      expect(faceBlinks(name)).toBe(false)
    }
  })

  it('alignTop 把逆时针输入翻成顺时针并从最高点起步', () => {
    const ccw: Pt[] = [[0, 0], [0, 10], [10, 10], [10, 0]]
    const out = alignTop(ccw)
    expect(signedArea(out)).toBeGreaterThan(0)
    expect(out[0][1]).toBe(0)
    expect(Math.abs(out[0][0])).toBe(0)
  })

  it('resampleClosed 等距重采样保留周长', () => {
    const square: Pt[] = [[-1, -1], [1, -1], [1, 1], [-1, 1]]
    const out = resampleClosed(square, 16)
    expect(out.length).toBe(16)
    let perim = 0
    for (let i = 0; i < out.length; i++) {
      const a = out[i]
      const b = out[(i + 1) % out.length]
      perim += Math.hypot(b[0] - a[0], b[1] - a[1])
    }
    expect(perim).toBeCloseTo(8, 6)
    expect(superellipse(1, 1, 2, 4).map((p) => p.map((n) => Math.round(n * 1000) / 1000))).toEqual([
      [0, -1],
      [1, 0],
      [0, 1],
      [-1, 0],
    ])
  })
})
