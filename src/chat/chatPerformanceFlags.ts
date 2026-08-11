export type ChatPerformanceFlags = {
  tanstackVirtualizer: boolean
  liveRowExternalization: boolean
  lightweightStreamingMarkdown: boolean
  settledMarkdownCache: boolean
}

export const CHAT_PERFORMANCE_FLAG_KEYS = {
  tanstackVirtualizer: 'chat.performance.tanstackVirtualizer',
  liveRowExternalization: 'chat.performance.liveRowExternalization',
  lightweightStreamingMarkdown: 'chat.performance.lightweightStreamingMarkdown',
  settledMarkdownCache: 'chat.performance.settledMarkdownCache',
} as const

const DEFAULT_FLAGS: ChatPerformanceFlags = {
  tanstackVirtualizer: true,
  // Primary streaming path: live row OUTSIDE the virtualizer (document flow).
  // Token growth only moves scrollHeight → contentGrowth pin. Putting live in
  // the virtualizer (LiveAgent) fights Kivio's outside chrome reserve + source
  // classification and was the root of "not pinned / 往下抽".
  // Set false only to A/B the in-list experiment.
  liveRowExternalization: true,

  lightweightStreamingMarkdown: true,
  settledMarkdownCache: true,
}

let cachedFlags: ChatPerformanceFlags | null = null

function readOverride(key: string): boolean | undefined {
  const globalFlags = (globalThis as { __KIVIO_CHAT_PERF_FLAGS__?: Record<string, unknown> })
    .__KIVIO_CHAT_PERF_FLAGS__
  if (globalFlags && typeof globalFlags[key] === 'boolean') {
    return globalFlags[key] as boolean
  }
  try {
    const raw = window.localStorage.getItem(key)
    if (raw === null) return undefined
    if (raw === '1' || raw === 'true') return true
    if (raw === '0' || raw === 'false') return false
  } catch {
    // Private mode, test environments, and WebView startup can lack storage.
  }
  return undefined
}

export function getChatPerformanceFlags(): ChatPerformanceFlags {
  if (cachedFlags) return cachedFlags
  cachedFlags = {
    tanstackVirtualizer: readOverride(CHAT_PERFORMANCE_FLAG_KEYS.tanstackVirtualizer) ?? DEFAULT_FLAGS.tanstackVirtualizer,
    liveRowExternalization: readOverride(CHAT_PERFORMANCE_FLAG_KEYS.liveRowExternalization) ?? DEFAULT_FLAGS.liveRowExternalization,
    lightweightStreamingMarkdown: readOverride(CHAT_PERFORMANCE_FLAG_KEYS.lightweightStreamingMarkdown) ?? DEFAULT_FLAGS.lightweightStreamingMarkdown,
    settledMarkdownCache: readOverride(CHAT_PERFORMANCE_FLAG_KEYS.settledMarkdownCache) ?? DEFAULT_FLAGS.settledMarkdownCache,
  }
  return cachedFlags
}

export function resetChatPerformanceFlagsForTests(): void {
  cachedFlags = null
}

/** Re-read local/global diagnostic overrides without reloading the app. */
export function refreshChatPerformanceFlags(): ChatPerformanceFlags {
  cachedFlags = null
  return getChatPerformanceFlags()
}
