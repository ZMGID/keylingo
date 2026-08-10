import type { ChatProject, ChatSet, ConversationLibrarySort, ConversationListItem } from '../types'
import type { I18n } from '../../settings/i18n'

/** Unix seconds → 相对时间（对话时间戳是秒）。 */
export function formatRelativeTime(sec: number, t: I18n, nowSec = Math.floor(Date.now() / 1000)): string {
  if (!sec) return ''
  const diff = Math.max(0, nowSec - sec)
  if (diff < 60) return t.chatLibJustNow
  if (diff < 3600) return t.chatLibMinutesAgo.replace('{n}', String(Math.floor(diff / 60)))
  if (diff < 86400) return t.chatLibHoursAgo.replace('{n}', String(Math.floor(diff / 3600)))
  if (diff < 86400 * 7) return t.chatLibDaysAgo.replace('{n}', String(Math.floor(diff / 86400)))
  const d = new Date(sec * 1000)
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  if (y === new Date(nowSec * 1000).getFullYear()) return `${m}-${day}`
  return `${y}-${m}-${day}`
}

/** 对话库时间列/按日分组：排序键是 created 时用创建时间，其余用更新时间。 */
export function libraryTimestamp(
  conv: ConversationListItem,
  sort: ConversationLibrarySort,
): number {
  if (sort === 'created') return conv.created_at || conv.updated_at || 0
  return conv.updated_at || conv.created_at || 0
}

export function shortModelName(model: string): string {
  if (!model) return '—'
  const base = model.split('/').pop() || model
  return base.length > 22 ? `${base.slice(0, 20)}…` : base
}

export function conversationOwnerLabel(
  conv: ConversationListItem,
  projects: ChatProject[],
  sets: ChatSet[],
  t: I18n,
): string {
  const setId = conv.set_id ?? conv.setId ?? null
  if (setId) {
    const name = sets.find((s) => s.id === setId)?.name
    if (name) return `${t.chatSetPrefix} · ${name}`
  }
  const projectId = conv.project_id ?? conv.projectId ?? null
  const project = projectId
    ? projects.find((p) => p.id === projectId)
    : projects.find((p) => conv.folder === p.name)
  return project?.name ?? conv.folder ?? ''
}

export type DayBucket = 'today' | 'yesterday' | 'week' | 'older'

export function dayBucket(sec: number, nowSec = Math.floor(Date.now() / 1000)): DayBucket {
  const n = new Date(nowSec * 1000)
  const startOfToday = new Date(n.getFullYear(), n.getMonth(), n.getDate()).getTime() / 1000
  if (sec >= startOfToday) return 'today'
  if (sec >= startOfToday - 86400) return 'yesterday'
  if (sec >= startOfToday - 7 * 86400) return 'week'
  return 'older'
}

export function dayBucketLabel(bucket: DayBucket, t: I18n): string {
  switch (bucket) {
    case 'today':
      return t.chatLibGroupToday
    case 'yesterday':
      return t.chatLibGroupYesterday
    case 'week':
      return t.chatLibGroupThisWeek
    default:
      return t.chatLibGroupOlder
  }
}
