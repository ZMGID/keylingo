import { describe, expect, it } from 'vitest'
import {
  conversationOwnerLabel,
  dayBucket,
  dayBucketLabel,
  formatCompactAge,
  formatRelativeTime,
  libraryTimestamp,
  shortModelName,
} from './format'
import type { ChatProject, ChatSet, ConversationListItem } from '../types'
import { i18n } from '../../settings/i18n'

function item(partial: Partial<ConversationListItem> = {}): ConversationListItem {
  return {
    id: 'c1',
    title: 't',
    preview: '',
    provider_id: 'p',
    model: 'm',
    message_count: 1,
    created_at: 100,
    updated_at: 200,
    pinned: false,
    ...partial,
  }
}

describe('libraryTimestamp', () => {
  it('uses created_at when sorting by created', () => {
    expect(libraryTimestamp(item(), 'created')).toBe(100)
  })

  it('uses updated_at for other sorts', () => {
    expect(libraryTimestamp(item(), 'updated')).toBe(200)
    expect(libraryTimestamp(item(), 'title')).toBe(200)
    expect(libraryTimestamp(item(), 'messages')).toBe(200)
  })

  it('falls back when the preferred field is missing', () => {
    expect(libraryTimestamp(item({ created_at: 0 }), 'created')).toBe(200)
    expect(libraryTimestamp(item({ updated_at: 0 }), 'updated')).toBe(100)
  })
})

describe('formatCompactAge', () => {
  const now = 1_700_000_000

  it('returns empty for missing timestamps', () => {
    expect(formatCompactAge(0, now)).toBe('')
  })

  it('uses compact m / h / d buckets', () => {
    expect(formatCompactAge(now - 10, now)).toBe('1m')
    expect(formatCompactAge(now - 120, now)).toBe('2m')
    expect(formatCompactAge(now - 7200, now)).toBe('2h')
    expect(formatCompactAge(now - 3 * 86400, now)).toBe('3d')
  })

  it('falls back to unpadded calendar form outside a week', () => {
    const nowSameYear = Math.floor(new Date(2023, 11, 1, 12, 0, 0).getTime() / 1000)
    const sameYear = Math.floor(new Date(2023, 5, 15, 12, 0, 0).getTime() / 1000)
    expect(formatCompactAge(sameYear, nowSameYear)).toBe('6/15')

    const older = Math.floor(new Date(2020, 0, 2, 12, 0, 0).getTime() / 1000)
    expect(formatCompactAge(older, nowSameYear)).toBe('20/1/2')
  })
})

describe('formatRelativeTime', () => {
  const t = i18n.zh
  const now = 1_700_000_000

  it('returns empty for missing timestamps', () => {
    expect(formatRelativeTime(0, t, now)).toBe('')
  })

  it('uses just-now / minutes / hours / days buckets', () => {
    expect(formatRelativeTime(now - 10, t, now)).toBe(t.chatLibJustNow)
    expect(formatRelativeTime(now - 120, t, now)).toBe(
      t.chatLibMinutesAgo.replace('{n}', '2'),
    )
    expect(formatRelativeTime(now - 7200, t, now)).toBe(
      t.chatLibHoursAgo.replace('{n}', '2'),
    )
    expect(formatRelativeTime(now - 3 * 86400, t, now)).toBe(
      t.chatLibDaysAgo.replace('{n}', '3'),
    )
  })

  it('falls back to calendar form outside a week', () => {
    // same year → MM-DD
    const sameYear = new Date('2023-06-15T12:00:00Z').getTime() / 1000
    const nowSameYear = new Date('2023-12-01T12:00:00Z').getTime() / 1000
    expect(formatRelativeTime(sameYear, t, nowSameYear)).toMatch(/^\d{2}-\d{2}$/)

    // different year → YYYY-MM-DD
    const older = new Date('2020-01-02T12:00:00Z').getTime() / 1000
    expect(formatRelativeTime(older, t, nowSameYear)).toMatch(/^\d{4}-\d{2}-\d{2}$/)
  })
})

describe('shortModelName', () => {
  it('strips provider prefixes and ellipsizes long ids', () => {
    expect(shortModelName('')).toBe('—')
    expect(shortModelName('openai/gpt-4.1')).toBe('gpt-4.1')
    expect(shortModelName('a'.repeat(30))).toBe(`${'a'.repeat(20)}…`)
  })
})

describe('conversationOwnerLabel', () => {
  const t = i18n.zh
  const projects: ChatProject[] = [
    { id: 'proj_1', name: 'Alpha', root_path: '/a', created_at: 1, updated_at: 1 } as ChatProject,
  ]
  const sets: ChatSet[] = [
    { id: 'set_1', name: 'Writing', created_at: 1, updated_at: 1 } as ChatSet,
  ]

  it('prefers set over project', () => {
    expect(
      conversationOwnerLabel(
        item({ set_id: 'set_1', project_id: 'proj_1', folder: 'Alpha' }),
        projects,
        sets,
        t,
      ),
    ).toBe(`${t.chatSetPrefix} · Writing`)
  })

  it('falls back to project name then folder', () => {
    expect(
      conversationOwnerLabel(item({ project_id: 'proj_1' }), projects, sets, t),
    ).toBe('Alpha')
    expect(
      conversationOwnerLabel(item({ folder: 'Loose' }), projects, sets, t),
    ).toBe('Loose')
    expect(conversationOwnerLabel(item(), projects, sets, t)).toBe('')
  })

  it('accepts camelCase set/project ids from the wire', () => {
    expect(
      conversationOwnerLabel(
        item({ setId: 'set_1' } as Partial<ConversationListItem>),
        projects,
        sets,
        t,
      ),
    ).toBe(`${t.chatSetPrefix} · Writing`)
  })
})

describe('dayBucket', () => {
  // 2024-06-15 12:00 local-ish via fixed unix seconds for the local timezone of the runner.
  // Use local Date construction so the test is timezone-stable.
  const nowDate = new Date(2024, 5, 15, 12, 0, 0)
  const nowSec = Math.floor(nowDate.getTime() / 1000)
  const startOfToday = new Date(2024, 5, 15).getTime() / 1000

  it('classifies today / yesterday / week / older', () => {
    expect(dayBucket(startOfToday + 3600, nowSec)).toBe('today')
    expect(dayBucket(startOfToday - 3600, nowSec)).toBe('yesterday')
    expect(dayBucket(startOfToday - 3 * 86400, nowSec)).toBe('week')
    expect(dayBucket(startOfToday - 10 * 86400, nowSec)).toBe('older')
  })

  it('labels each bucket', () => {
    const t = i18n.zh
    expect(dayBucketLabel('today', t)).toBe(t.chatLibGroupToday)
    expect(dayBucketLabel('yesterday', t)).toBe(t.chatLibGroupYesterday)
    expect(dayBucketLabel('week', t)).toBe(t.chatLibGroupThisWeek)
    expect(dayBucketLabel('older', t)).toBe(t.chatLibGroupOlder)
  })
})
