import type { Root } from 'hast'

interface PendingParse {
  key: string
  resolve: (tree: Root) => void
  reject: (error: Error) => void
  guard: ReturnType<typeof setTimeout>
}

interface MarkdownAstResponse {
  id: number
  tree?: Root
  error?: string
}

const CACHE_LIMIT = 64
const PARSE_TIMEOUT_MS = 30_000
const cache = new Map<string, Root>()
const pending = new Map<number, PendingParse>()
let worker: Worker | null = null
let sequence = 0

function cacheKey(content: string, citationNumbers: readonly number[]): string {
  return `${citationNumbers.join(',')}\0${content}`
}

function cacheTree(key: string, tree: Root): Root {
  cache.delete(key)
  cache.set(key, tree)
  while (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    cache.delete(oldest)
  }
  return tree
}

function rejectPending(error: Error) {
  for (const parse of pending.values()) {
    clearTimeout(parse.guard)
    parse.reject(error)
  }
  pending.clear()
}

function resetWorker(error: Error) {
  rejectPending(error)
  worker?.terminate()
  worker = null
}

function ensureWorker(): Worker {
  if (worker) return worker
  const next = new Worker(new URL('./markdownAstWorker.ts', import.meta.url), { type: 'module' })
  next.onmessage = (event: MessageEvent<MarkdownAstResponse>) => {
    const { id, tree, error } = event.data
    const parse = pending.get(id)
    if (!parse) return
    clearTimeout(parse.guard)
    pending.delete(id)
    if (error) {
      parse.reject(new Error(error))
      return
    }
    if (!tree) {
      parse.reject(new Error('Markdown worker 返回了空语法树'))
      return
    }
    parse.resolve(cacheTree(parse.key, tree))
  }
  next.onerror = (event) => {
    resetWorker(new Error(`Markdown worker 异常：${event.message || '未知错误'}`))
  }
  next.onmessageerror = () => {
    resetWorker(new Error('Markdown worker 返回了无法解析的数据'))
  }
  worker = next
  return next
}

export function getCachedMarkdownAst(
  content: string,
  citationNumbers: readonly number[],
): Root | null {
  const key = cacheKey(content, citationNumbers)
  const tree = cache.get(key)
  if (!tree) return null
  cache.delete(key)
  cache.set(key, tree)
  return tree
}

export function parseMarkdownAst(
  content: string,
  citationNumbers: readonly number[],
): Promise<Root> {
  const key = cacheKey(content, citationNumbers)
  const cached = cache.get(key)
  if (cached) return Promise.resolve(cacheTree(key, cached))
  const id = ++sequence
  return new Promise<Root>((resolve, reject) => {
    const guard = setTimeout(() => {
      pending.delete(id)
      reject(new Error('Markdown worker 解析超时'))
    }, PARSE_TIMEOUT_MS)
    pending.set(id, { key, resolve, reject, guard })
    try {
      ensureWorker().postMessage({ id, content, citationNumbers })
    } catch (err) {
      clearTimeout(guard)
      pending.delete(id)
      reject(err instanceof Error ? err : new Error(String(err)))
    }
  })
}

export function clearMarkdownAstCacheForTests() {
  cache.clear()
}
