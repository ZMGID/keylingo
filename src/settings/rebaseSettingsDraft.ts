import type { Settings } from '../api/tauri'
import { adoptFreshPluginManagedServers } from './connectorCatalog'
import { stableStringify } from './utils'

function same(a: unknown, b: unknown): boolean {
  return stableStringify(a) === stableStringify(b)
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/** 草稿相对基线没改 → 用其它页面 / 后端刚写入的 fresh；改过 → 留下草稿。数组整段取舍。 */
function rebaseValue(snapshot: unknown, draft: unknown, fresh: unknown): unknown {
  if (Array.isArray(draft) || Array.isArray(snapshot) || Array.isArray(fresh)) {
    return same(draft, snapshot) ? fresh : draft
  }
  if (isPlainObject(snapshot) || isPlainObject(draft) || isPlainObject(fresh)) {
    const snap = isPlainObject(snapshot) ? snapshot : {}
    const dr = isPlainObject(draft) ? draft : {}
    const fr = isPlainObject(fresh) ? fresh : {}
    const keys = new Set([...Object.keys(snap), ...Object.keys(dr), ...Object.keys(fr)])
    const out: Record<string, unknown> = { ...fr }
    for (const key of keys) {
      out[key] = rebaseValue(snap[key], dr[key], fr[key])
    }
    return out
  }
  return same(draft, snapshot) ? fresh : draft
}

/**
 * 设置页 keep-alive 草稿 vs 其它面写入的三方合并。
 * snapshot = 上次落盘/加载基线，draft = 设置页当前，fresh = 缓存/后端。
 */
export function rebaseSettingsDraft(
  snapshot: Settings,
  draft: Settings,
  fresh: Settings,
): Settings {
  if (same(draft, snapshot)) return fresh
  const rebased = rebaseValue(snapshot, draft, fresh) as Settings
  const servers = adoptFreshPluginManagedServers(
    rebased.chatTools?.servers ?? [],
    fresh.chatTools?.servers ?? [],
  )
  if (servers === (rebased.chatTools?.servers ?? [])) return rebased
  const chatTools = rebased.chatTools
  if (!chatTools) {
    return rebased
  }
  return {
    ...rebased,
    chatTools: { ...chatTools, servers },
  }
}

function parseSnapshot(raw: string): Settings | null {
  if (!raw) return null
  try {
    return JSON.parse(raw) as Settings
  } catch {
    return null
  }
}

/** persist / subscribe：有基线则三方合并，否则至少换上插件 MCP。 */
export function rebaseDraftAgainstCache(
  snapshotRaw: string,
  draft: Settings,
  fresh: Settings | null,
): Settings {
  if (!fresh) {
    return draft
  }
  const snapshot = parseSnapshot(snapshotRaw)
  if (!snapshot) {
    const servers = adoptFreshPluginManagedServers(
      draft.chatTools?.servers ?? [],
      fresh.chatTools?.servers ?? [],
    )
    if (servers === (draft.chatTools?.servers ?? [])) return draft
    return {
      ...draft,
      chatTools: { ...(draft.chatTools as Settings['chatTools']), servers },
    }
  }
  return rebaseSettingsDraft(snapshot, draft, fresh)
}
