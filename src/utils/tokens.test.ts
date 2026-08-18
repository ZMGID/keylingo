import { describe, expect, it } from 'vitest'
import { formatTokens, formatTokensCompact, formatTokensK } from './tokens'

/**
 * 千位收成 K 的边界。1000 这一档最容易写错（`> 1000` 会让 1000 本身漏成 "1000"），
 * 消息用量条 / 上下文用量条 / 子 agent 卡片都吃这一份口径。
 */
describe('formatTokensK', () => {
  it('不足千位原样显示', () => {
    expect(formatTokensK(0)).toBe('0')
    expect(formatTokensK(999)).toBe('999')
  })

  it('从 1000 起收成一位小数 + 大写 K', () => {
    expect(formatTokensK(1000)).toBe('1.0K')
    expect(formatTokensK(1038)).toBe('1.0K')
    expect(formatTokensK(1500)).toBe('1.5K')
    expect(formatTokensK(38897)).toBe('38.9K')
  })

  it('与小写版只差后缀（Lens / 压缩分隔线仍用小写）', () => {
    expect(formatTokens(38897)).toBe('38.9k')
    expect(formatTokensK(38897)).toBe(formatTokens(38897).toUpperCase())
  })
})

describe('formatTokensCompact', () => {
  it('不足千位原样显示', () => {
    expect(formatTokensCompact(0)).toBe('0')
    expect(formatTokensCompact(999)).toBe('999')
  })

  it('从 1000 起收成 k，整数档不带小数', () => {
    expect(formatTokensCompact(1000)).toBe('1k')
    expect(formatTokensCompact(1038)).toBe('1k')
    expect(formatTokensCompact(1500)).toBe('1.5k')
    expect(formatTokensCompact(38897)).toBe('38.9k')
  })

  it('从 1_000_000 起收成 m', () => {
    expect(formatTokensCompact(1_000_000)).toBe('1m')
    expect(formatTokensCompact(1_250_000)).toBe('1.3m')
  })
})
