import { describe, expect, it } from 'vitest'
import { buildContextBarSlices, CONTEXT_FREE_SEGMENT_ID, fullnessLabel } from './contextPanel'
import { i18n } from '../settings/i18n'

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
