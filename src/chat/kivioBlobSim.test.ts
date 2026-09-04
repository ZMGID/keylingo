import { describe, expect, it } from 'vitest'
import {
  blobScheduleMs,
  BLOB_BLUE,
  BLOB_IDLE_WAKE_MIN_MS,
  BLOB_IDLE_BREATHE_MS,
  BLOB_POKE_RED,
  BODY_R,
  consumeBlink,
  CX,
  CY,
  EYE_POINTS,
  KivioBlobSim,
  polyPath,
  queueBlink,
  resolveBlobMood,
} from './kivioBlobSim'
import { BODY_POINTS } from './kivioBlobShapes'

describe('kivioBlobSim', () => {
  it('queueBlink 走 70ms 眯 → 150ms 过冲 → 300ms 睁开', () => {
    const q: { at: number; v: number }[] = []
    queueBlink(q, 1000, () => 1)
    expect(q.map((k) => [k.at - 1000, k.v])).toEqual([
      [0, 0.05],
      [70, 0.05],
      [150, 1.08],
      [300, 1],
    ])
    expect(consumeBlink(q, 1000)).toBe(0.05)
    expect(consumeBlink(q, 1149)).toBe(0.05)
    expect(consumeBlink(q, 1150)).toBe(1.08)
    expect(consumeBlink(q, 1300)).toBe(1)
    expect(consumeBlink(q, 2000)).toBeNull()
  })

  it('14% 连眨会再塞两帧', () => {
    const q: { at: number; v: number }[] = []
    queueBlink(q, 0, () => 0)
    expect(q).toHaveLength(6)
    expect(q[4]).toEqual({ at: 370, v: 0.05 })
    expect(q[5]).toEqual({ at: 480, v: 1 })
  })

  it('眼环是左右各 48 点，分居圆心两侧', () => {
    const sim = new KivioBlobSim({ random: () => 0.5, reducedMotion: true })
    const paint = sim.sample(0)
    const parse = (d: string) =>
      [...d.matchAll(/(-?\d+\.\d+)/g)].map((m) => Number(m[1]))
    const left = parse(paint.eyes[0].d)
    const right = parse(paint.eyes[1].d)
    expect(left.length).toBe(EYE_POINTS * 2)
    expect(right.length).toBe(EYE_POINTS * 2)
    const leftXs = left.filter((_, i) => i % 2 === 0)
    const rightXs = right.filter((_, i) => i % 2 === 0)
    expect(Math.max(...leftXs)).toBeLessThan(CX)
    expect(Math.min(...rightXs)).toBeGreaterThan(CX)
    expect(polyPath([[1, 2], [3, 4]])).toBe('M1.00 2.00L3.00 4.00Z')
  })

  it('思考态弹簧把 spin 拉向负角', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('think', 0)
    let paint = sim.sample(0)
    for (let t = 16; t <= 800; t += 16) paint = sim.sample(t)
    expect(sim.debug().mood).toBe('think')
    expect(sim.debug().spin).toBeLessThan(-2)
    expect(paint.rig).toContain('rotate(')
  })

  it('reduced motion 钉死 pose', () => {
    const sim = new KivioBlobSim({ random: () => 0.5, reducedMotion: true })
    sim.setMood('think', 0)
    const paint = sim.sample(800)
    expect(sim.debug().spin).toBe(0)
    expect(sim.debug().blink).toBe(1)
    expect(paint.rig.startsWith('translate(0.00 0.00)')).toBe(true)
  })

  it('闲置眨眼结束后不钉满帧，改走呼吸节拍', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('idle', 0)
    for (let t = 0; t <= 1200; t += 16) sim.sample(t)
    expect(sim.wantsHighFps(1200)).toBe(false)
    expect(sim.nextIdleWakeMs(1200)).toBe(BLOB_IDLE_BREATHE_MS)
    sim.setMood('think', 1200)
    expect(sim.wantsHighFps(1200)).toBe(true)
  })

  it('出错间隙不钉满帧', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('error', 0)
    for (let t = 0; t <= 2500; t += 16) sim.sample(t)
    expect(sim.wantsHighFps(2500)).toBe(false)
    expect(sim.nextIdleWakeMs(2500)).toBeGreaterThan(100)
  })

  it('blobScheduleMs：隐藏/屏外/失焦/覆盖停表，闲置走呼吸节拍，忙碌跟 vsync', () => {
    expect(blobScheduleMs({ reducedMotion: true, hidden: false, onScreen: true, highFps: true })).toBeNull()
    expect(blobScheduleMs({ reducedMotion: false, hidden: true, onScreen: true, highFps: true })).toBeNull()
    expect(blobScheduleMs({ reducedMotion: false, hidden: false, onScreen: false, highFps: true })).toBeNull()
    expect(blobScheduleMs({ reducedMotion: false, hidden: false, onScreen: true, unfocused: true, highFps: true })).toBeNull()
    expect(blobScheduleMs({ reducedMotion: false, hidden: false, onScreen: true, covered: true, highFps: true })).toBeNull()
    expect(blobScheduleMs({
      reducedMotion: false,
      hidden: false,
      onScreen: true,
      highFps: false,
      idleWakeMs: 9000,
    })).toBe(9000)
    expect(blobScheduleMs({
      reducedMotion: false,
      hidden: false,
      onScreen: true,
      highFps: false,
      idleWakeMs: 1,
    })).toBe(BLOB_IDLE_WAKE_MIN_MS)
    expect(blobScheduleMs({ reducedMotion: false, hidden: false, onScreen: true, highFps: true })).toBe(0)
  })

  it('闲置有呼吸起伏', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('idle', 0)
    let paint = sim.sample(0)
    for (let t = 16; t <= 900; t += 16) paint = sim.sample(t)
    expect(paint.body.includes('scale(1 1.000)')).toBe(false)
    expect(paint.rig.startsWith('translate(0.00 0.00)')).toBe(false)
  })

  it('闲置会把重心慢慢挪过去', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('idle', 0)
    for (let t = 0; t <= 10000; t += 16) sim.sample(t)
    expect(Math.abs(sim.debug().spin)).toBeGreaterThan(3)
  })

  it('poke 会眨眼并跳一下', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('idle', 0)
    sim.sample(0)
    sim.poke(200)
    expect(sim.wantsHighFps(200)).toBe(true)
    const paint = sim.sample(280)
    expect(paint.rig.startsWith('translate(0.00 0.00)')).toBe(false)
  })

  it('连点升温变红，换文案 nudge 不脸红', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('idle', 0)
    sim.sample(0)
    expect(sim.sample(10).fill.toLowerCase()).toBe(BLOB_BLUE)
    sim.nudge(20)
    expect(sim.debug().heat).toBe(0)
    expect(sim.sample(30).fill.toLowerCase()).toBe(BLOB_BLUE)
    expect(sim.poke(100)).toBe(1)
    expect(sim.debug().heat).toBeGreaterThan(0)
    expect(sim.sample(120).fill.toLowerCase()).not.toBe(BLOB_BLUE)
    for (let i = 1; i < 6; i++) sim.poke(100 + i * 80)
    expect(sim.debug().pokes).toBe(6)
    expect(sim.debug().heat).toBe(1)
    expect(sim.sample(600).fill.toLowerCase()).toBe(BLOB_POKE_RED)
    sim.poke(6000)
    expect(sim.debug().pokes).toBe(1)
  })

  it('resolveBlobMood 跟生成阶段走', () => {
    expect(resolveBlobMood({ active: false })).toBe('idle')
    expect(resolveBlobMood({ active: false, error: true })).toBe('error')
    expect(resolveBlobMood({ active: true })).toBe('think')
    expect(resolveBlobMood({ active: true, runningToolNames: ['web_search'] })).toBe('search')
    expect(resolveBlobMood({ active: true, runningToolNames: ['read_file'] })).toBe('work')
    expect(resolveBlobMood({ active: true, contentLen: 40, reasoningStreaming: false })).toBe('speak')
    expect(resolveBlobMood({ active: true, contentLen: 40, reasoningStreaming: true })).toBe('think')
  })

  it('resolveBlobMood：等用户压过工具态，收工窗口只在非活动时生效', () => {
    expect(resolveBlobMood({ active: true, waiting: true, runningToolNames: ['ask_user'] })).toBe('wait')
    expect(resolveBlobMood({ active: false, done: true })).toBe('done')
    expect(resolveBlobMood({ active: false, done: true, error: true })).toBe('error')
    expect(resolveBlobMood({ active: true, done: true })).toBe('think')
  })

  it('身体是一条 BODY_POINTS 点的闭合路径，闲置钉在圆上', () => {
    const sim = new KivioBlobSim({ random: () => 0.5, reducedMotion: true })
    const paint = sim.sample(0)
    const nums = [...paint.bodyD.matchAll(/(-?\d+\.\d+)/g)]
    expect(nums.length).toBe(BODY_POINTS * 2)
    expect(paint.bodyD.startsWith('M')).toBe(true)
    expect(paint.bodyD.endsWith('Z')).toBe(true)
    expect(sim.debug().body).toBe('circle')
    // 圆：所有点到圆心距离 ≈ BODY_R。
    for (let i = 0; i < nums.length; i += 2) {
      const dx = Number(nums[i][1]) - CX
      const dy = Number(nums[i + 1][1]) - CY
      expect(Math.hypot(dx, dy)).toBeCloseTo(BODY_R, 0)
    }
  })

  it('掷中时：思考变云、干活变方、说话带尾巴的气泡；出错必摊成一滩', () => {
    const shapeAfter = (mood: 'think' | 'work' | 'speak' | 'error', random: number) => {
      const sim = new KivioBlobSim({ random: () => random })
      sim.setMood(mood, 0)
      for (let t = 0; t <= 1600; t += 16) sim.sample(t)
      return sim.debug().body
    }
    expect(shapeAfter('think', 0.1)).toBe('cloud')
    expect(shapeAfter('work', 0.1)).toBe('squircle')
    expect(shapeAfter('speak', 0.1)).toBe('bubble')
    expect(shapeAfter('error', 0.1)).toBe('puddle')
    expect(shapeAfter('error', 0.9)).toBe('puddle')
  })

  it('没掷中：生成中大多数时候就是个圆，不变云也不变气泡', () => {
    for (const mood of ['think', 'speak', 'work', 'search'] as const) {
      const sim = new KivioBlobSim({ random: () => 0.9 })
      sim.setMood(mood, 0)
      const seen = new Set<string>()
      for (let t = 0; t <= 20000; t += 50) {
        sim.sample(t)
        seen.add(sim.debug().body)
      }
      expect([...seen]).toEqual(['circle'])
    }
  })

  it('身体变形是渐进的：切心情后一帧不会直接跳成目标形状', () => {
    const sim = new KivioBlobSim({ random: () => 0.1 })
    sim.setMood('idle', 0)
    sim.sample(0)
    const circle = sim.sample(16).bodyD
    sim.setMood('work', 32)
    const mid = sim.sample(48).bodyD
    let settled = mid
    for (let t = 64; t <= 2400; t += 16) settled = sim.sample(t).bodyD
    expect(mid).not.toBe(circle)
    expect(mid).not.toBe(settled)
    expect(sim.debug().body).toBe('squircle')
  })

  it('进入各心情先摆播放列表第 0 张脸；收工是笑脸 + 蹦一下', () => {
    const face = (mood: 'think' | 'search' | 'error' | 'done' | 'wait') => {
      const sim = new KivioBlobSim({ random: () => 0.5 })
      sim.setMood(mood, 0)
      sim.sample(0)
      return sim.debug().face
    }
    expect(face('think')).toBe('lookUp')
    expect(face('search')).toBe('wide')
    expect(face('error')).toBe('dizzy')
    expect(face('done')).toBe('happy')
    expect(face('wait')).toBe('wide')
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('idle', 0)
    sim.sample(0)
    sim.setMood('done', 100)
    expect(sim.wantsHighFps(100)).toBe(true)
    const paint = sim.sample(260)
    expect(paint.rig.startsWith('translate(0.00 0.00)')).toBe(false)
  })

  it('闭着的脸不眨眼：笑弯的眼皮缩放钉在 1', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('done', 0)
    for (let t = 0; t <= 600; t += 16) sim.sample(t)
    expect(sim.debug().face).toBe('happy')
    expect(sim.debug().blink).toBeCloseTo(1, 2)
  })

  it('戳一下压扁、戳多了装方、再戳炸毛，过后变回圆', () => {
    const sim = new KivioBlobSim({ random: () => 0.5 })
    sim.setMood('idle', 0)
    sim.sample(0)
    sim.poke(100)
    sim.sample(116)
    expect(sim.debug().body).toBe('puddle')
    expect(sim.debug().face).toBe('flat')
    for (let i = 1; i < 4; i++) sim.poke(100 + i * 80)
    sim.sample(400)
    expect(sim.debug().body).toBe('squircle')
    for (let i = 4; i < 7; i++) sim.poke(100 + i * 80)
    sim.sample(700)
    expect(sim.debug().body).toBe('burst')
    expect(sim.debug().face).toBe('dizzy')
    for (let t = 716; t <= 3200; t += 16) sim.sample(t)
    expect(sim.debug().body).toBe('circle')
  })

  it('闲置随机小动作会通知 onAntic（变形态 / 蹦）', () => {
    // random 钉在 0.97：nextIdleBias 落进「变形态」档，形状取列表末位。
    const sim = new KivioBlobSim({ random: () => 0.97 })
    const seen: string[] = []
    sim.onAntic = (kind) => seen.push(kind)
    sim.setMood('idle', 0)
    for (let t = 0; t <= 30000; t += 50) sim.sample(t)
    expect(seen.length).toBeGreaterThan(0)
    expect(seen.every((k) => k === 'egg' || k === 'hop')).toBe(true)
    expect(seen).toContain('egg')
  })

  it('reduced motion 下身体永远是圆、脸不变形', () => {
    const sim = new KivioBlobSim({ random: () => 0.5, reducedMotion: true })
    sim.setMood('think', 0)
    for (let t = 0; t <= 800; t += 16) sim.sample(t)
    expect(sim.debug().body).toBe('circle')
    expect(sim.debug().face).toBe('lookUp')
  })
})
