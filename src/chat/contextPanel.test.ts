import { describe, expect, it } from 'vitest'
import {
  applyLiveContextUsage,
  buildContextBarSlices,
  CONTEXT_FREE_SEGMENT_ID,
  fullnessLabel,
} from './contextPanel'
import { i18n } from '../settings/i18n'
import type { ConversationContextState } from './types'

describe('buildContextBarSlices', () => {
  const t = i18n.zh

  it('includes free space slice when window is known', () => {
    const slices = buildContextBarSlices(
      [
        { id: 'conversation', label: 'Conversation', estimated_tokens: 50_000 },
        { id: 'attachments', label: 'Attachments', estimated_tokens: 10_000 },
      ],
      60_000,
      200_000,
      t,
    )
    const free = slices.find((slice) => slice.id === CONTEXT_FREE_SEGMENT_ID)
    expect(free?.tokens).toBe(140_000)
    expect(free?.widthPercent).toBeCloseTo(70, 1)
    expect(slices.reduce((sum, slice) => sum + slice.widthPercent, 0)).toBeCloseTo(100, 1)
  })

  // 外部 CLI 拿不到窗口时后端现在送 null（不再编造 200K）。此时不该出现「剩余空间」条，
  // 也不该按假分母算宽度 —— 已用段独占整条，视觉上等价于「满度未知」。
  it('omits the free slice when the window is unknown', () => {
    const slices = buildContextBarSlices(
      [{ id: 'external-session', label: 'CLI session context', estimated_tokens: 23_605 }],
      23_605,
      null,
      t,
    )
    expect(slices.find((slice) => slice.id === CONTEXT_FREE_SEGMENT_ID)).toBeUndefined()
    expect(slices).toHaveLength(1)
    expect(slices[0].widthPercent).toBeCloseTo(100, 1)
  })
})

describe('fullnessLabel', () => {
  const t = i18n.zh

  it('says the fullness is unknown when the window could not be resolved', () => {
    // 外部 CLI 既不报 usage_update.size、模型名也匹配不到静态表（如 cursor 选了不带
    // `context=` 的模型）：百分比永远算不出来，不能用「CLI 待上报」暗示用户再等。
    expect(fullnessLabel(null, true, null, t)).toBe(t.contextFullnessWindowUnknown)
    expect(fullnessLabel(null, true, null, t)).not.toBe(t.contextFullnessCliPending)
  })

  it('still says CLI pending when the window is known but usage has not arrived', () => {
    expect(fullnessLabel(null, true, 200_000, t)).toBe(t.contextFullnessCliPending)
  })

  it('falls back to the builtin estimating label off the external path', () => {
    expect(fullnessLabel(null, false, 200_000, t)).toBe(t.contextFullnessEstimated)
  })

  it('renders a percentage once both numerator and denominator exist', () => {
    expect(fullnessLabel(0.42, true, 200_000, t)).toBe(
      t.contextFullnessPercentFull.replace('{percent}', '42'),
    )
  })
})

describe('applyLiveContextUsage', () => {
  const base: ConversationContextState = {
    estimated_input_tokens: 40_000,
    context_window_tokens: 1_000_000,
    usage_ratio: 0.04,
    status: 'normal',
    token_count_source: 'cli_reported',
    context_source: 'external_cli',
    compression_count: 2,
    segments: [{ id: 'external-session', label: 'CLI session context', estimated_tokens: 40_000 }],
  }

  it('moves the numerator and the percentage during generation', () => {
    const next = applyLiveContextUsage(base, { usedTokens: 47_300, contextWindowTokens: 1_000_000 })
    expect(next?.estimated_input_tokens).toBe(47_300)
    expect(next?.estimatedInputTokens).toBe(47_300)
    expect(next?.usage_ratio).toBeCloseTo(0.0473, 6)
  })

  // 分母粘滞：claude 只在轮末那条 result 里带窗口，中途的上报不带。
  // 冲掉已知窗口会让用量条在生成过程中退回「满度未知」。
  it('keeps the known window when a live report omits it', () => {
    const next = applyLiveContextUsage(base, { usedTokens: 50_000 })
    expect(next?.context_window_tokens).toBe(1_000_000)
    expect(next?.contextWindowTokens).toBe(1_000_000)
    expect(next?.usage_ratio).toBeCloseTo(0.05, 6)

    const nullWindow = applyLiveContextUsage(base, { usedTokens: 50_000, contextWindowTokens: null })
    expect(nullWindow?.context_window_tokens).toBe(1_000_000)
  })

  it('adopts a newly reported window (the model may switch mid-session)', () => {
    const next = applyLiveContextUsage(base, { usedTokens: 50_000, contextWindowTokens: 200_000 })
    expect(next?.context_window_tokens).toBe(200_000)
    expect(next?.usage_ratio).toBeCloseTo(0.25, 6)
  })

  it('leaves the window unknown when nothing ever reported one', () => {
    const noWindow = applyLiveContextUsage(
      { ...base, context_window_tokens: null, usage_ratio: null },
      { usedTokens: 50_000 },
    )
    expect(noWindow?.context_window_tokens).toBeNull()
    expect(noWindow?.usage_ratio).toBeNull()
  })

  // 口径的单一真源在 Rust 侧：状态阈值与来源标签不在前端重算，轮末的权威快照负责刷新它们。
  it('does not invent a status or a token source of its own', () => {
    const next = applyLiveContextUsage(base, { usedTokens: 990_000 })
    expect(next?.status).toBe('normal')
    expect(next?.token_count_source).toBe('cli_reported')
    expect(next?.compression_count).toBe(2)
  })

  // 分段按比例缩放：明细只有轮末算得准，但留在旧总量上会让进度条里出现一条对不上的缝。
  it('scales the existing segments so the bar stays consistent', () => {
    const next = applyLiveContextUsage(base, { usedTokens: 80_000 })
    expect(next?.segments?.[0].estimated_tokens).toBe(80_000)
    const twoSegments = applyLiveContextUsage(
      {
        ...base,
        estimated_input_tokens: 100,
        segments: [
          { id: 'conversation', label: 'C', estimated_tokens: 75 },
          { id: 'tool_definitions', label: 'T', estimated_tokens: 25 },
        ],
      },
      { usedTokens: 200 },
    )
    expect(twoSegments?.segments?.map((segment) => segment.estimated_tokens)).toEqual([150, 50])
  })

  it('is a no-op before any context state exists', () => {
    expect(applyLiveContextUsage(null, { usedTokens: 100 })).toBeNull()
    expect(applyLiveContextUsage(undefined, { usedTokens: 100 })).toBeNull()
  })
})
