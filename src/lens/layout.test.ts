import { describe, expect, it } from 'vitest'
import { clamp, computeMetrics, computeSelectBar } from './layout'

describe('clamp', () => {
  it('clamps values within bounds', () => {
    expect(clamp(5, 0, 10)).toBe(5)
    expect(clamp(-1, 0, 10)).toBe(0)
    expect(clamp(99, 0, 10)).toBe(10)
  })
})

describe('computeMetrics', () => {
  it('respects minimum bounds on small viewports', () => {
    const metrics = computeMetrics(800, 600)
    expect(metrics.READY_W).toBeGreaterThanOrEqual(420)
    expect(metrics.SELECT_W).toBeGreaterThanOrEqual(440)
    expect(metrics.ANSWER_H).toBeGreaterThanOrEqual(220)
    expect(metrics.SELECT_BOTTOM_OFFSET).toBeGreaterThanOrEqual(80)
  })

  it('respects maximum bounds on large viewports', () => {
    const metrics = computeMetrics(4000, 3000)
    expect(metrics.READY_W).toBeLessThanOrEqual(560)
    expect(metrics.SELECT_W).toBeLessThanOrEqual(640)
    expect(metrics.ANSWER_H).toBeLessThanOrEqual(480)
    expect(metrics.SELECT_BOTTOM_OFFSET).toBeLessThanOrEqual(160)
  })
})

describe('computeSelectBar', () => {
  it('centers the select bar horizontally near the bottom', () => {
    const metrics = computeMetrics(1920, 1080)
    const bar = computeSelectBar(1920, 1080, metrics)
    expect(bar.width).toBe(metrics.SELECT_W)
    expect(bar.x).toBe(Math.round(1920 / 2 - metrics.SELECT_W / 2))
    expect(bar.y).toBe(Math.round(1080 - metrics.SELECT_BOTTOM_OFFSET - 56))
  })
})
