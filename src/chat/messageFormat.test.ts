import { describe, expect, it } from 'vitest'
import { formatAssistantMessageTime } from './messageFormat'

describe('formatAssistantMessageTime', () => {
  // 2026-01-01T12:00:00Z
  const ts = 1767268800

  it('shows only the clock time for same-day messages', () => {
    const now = new Date(ts * 1000)
    expect(formatAssistantMessageTime(ts, now)).toMatch(/^\d{1,2}:\d{2} (AM|PM)$/)
  })

  it('shows month + day without year within the same year', () => {
    const now = new Date('2026-06-15T00:00:00')
    expect(formatAssistantMessageTime(ts, now)).toMatch(/^Jan 1, \d{1,2}:\d{2} (AM|PM)$/)
  })

  it('shows the year only across years', () => {
    const now = new Date('2027-06-15T00:00:00')
    expect(formatAssistantMessageTime(ts, now)).toMatch(/^Jan 1, 2026, \d{1,2}:\d{2} (AM|PM)$/)
  })
})
