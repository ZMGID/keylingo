export type SettledMarkdownCacheEntry = {
  normalized: string
  hasHeavySyntax: boolean
  blockCount: number
}

const MAX_ENTRIES = 96
const entries = new Map<string, SettledMarkdownCacheEntry>()

function touch(key: string, entry: SettledMarkdownCacheEntry): SettledMarkdownCacheEntry {
  entries.delete(key)
  entries.set(key, entry)
  while (entries.size > MAX_ENTRIES) {
    const oldest = entries.keys().next().value
    if (oldest === undefined) break
    entries.delete(oldest)
  }
  return entry
}

export function getSettledMarkdownCacheEntry(
  content: string,
  build: () => SettledMarkdownCacheEntry,
): SettledMarkdownCacheEntry {
  const cached = entries.get(content)
  if (cached) return touch(content, cached)
  return touch(content, build())
}

export function clearSettledMarkdownCache(): void {
  entries.clear()
}

export function settledMarkdownCacheSize(): number {
  return entries.size
}
