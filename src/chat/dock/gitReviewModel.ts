// Git 面板的纯逻辑（从 useGitReview 抽出以便单测）。
import type { GitCommitItem, GitRepoState, GitStatusEntry } from './types'

/** refresh 的响应签名：签名相同则跳过 setState，避免 10s 轮询/事件刷新打出的恒等重渲。 */
export function gitStatusSignature(state: GitRepoState | null): string {
  if (!state) return 'null'
  const head = [
    state.status,
    state.repoRoot,
    state.head,
    state.upstream ?? '',
    state.ahead,
    state.behind,
    state.stashCount,
    state.error ?? '',
  ].join('|')
  const entries = state.entries
    .map((entry) =>
      [
        entry.path,
        entry.oldPath ?? '',
        entry.indexStatus,
        entry.worktreeStatus,
        entry.kind,
        entry.staged ? 1 : 0,
        entry.conflicted ? 1 : 0,
        entry.untracked ? 1 : 0,
      ].join(','),
    )
    .join(';')
  return `${head}#${entries}`
}

export type PartitionedStatusEntries = {
  conflicted: GitStatusEntry[]
  staged: GitStatusEntry[]
  unstaged: GitStatusEntry[]
}

/** Changes 视图分组：冲突单列（其 staged 标志在冲突期间不可靠），已暂存 / 未暂存两组。 */
export function partitionStatusEntries(entries: GitStatusEntry[]): PartitionedStatusEntries {
  const conflicted: GitStatusEntry[] = []
  const staged: GitStatusEntry[] = []
  const unstaged: GitStatusEntry[] = []
  for (const entry of entries) {
    if (entry.conflicted) conflicted.push(entry)
    else if (entry.staged) staged.push(entry)
    else unstaged.push(entry)
  }
  return { conflicted, staged, unstaged }
}

/** 历史分页追加：sha 去重，保持既有顺序。 */
export function appendHistoryPage(existing: GitCommitItem[], page: GitCommitItem[]): GitCommitItem[] {
  if (page.length === 0) return existing
  const seen = new Set(existing.map((commit) => commit.sha))
  const fresh = page.filter((commit) => !seen.has(commit.sha))
  if (fresh.length === 0) return existing
  return [...existing, ...fresh]
}

export type StatusLetter = 'M' | 'A' | 'D' | 'R' | 'C' | 'U'

/** 行首状态字母徽章。优先冲突(U)/未跟踪(A)，否则取 index/worktree 中较显眼的一个。
 *  未跟踪文件显示为 A（新增），避免 `?` 像“未知/错误”。 */
export function statusLetter(entry: GitStatusEntry): StatusLetter {
  if (entry.conflicted) return 'U'
  if (entry.untracked) return 'A'
  const codes = `${entry.indexStatus}${entry.worktreeStatus}`
  if (codes.includes('R')) return 'R'
  if (codes.includes('C')) return 'C'
  if (codes.includes('A')) return 'A'
  if (codes.includes('D')) return 'D'
  if (codes.includes('U')) return 'U'
  return 'M'
}

/** 相对时间（提交列表用）。zh/en 两套文案。 */
export function relativeTime(isoDate: string, lang: 'zh' | 'en', now = Date.now()): string {
  const timestamp = Date.parse(isoDate)
  if (!Number.isFinite(timestamp)) return isoDate
  const seconds = Math.max(0, Math.floor((now - timestamp) / 1000))
  const zh = lang === 'zh'
  if (seconds < 60) return zh ? '刚刚' : 'just now'
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return zh ? `${minutes} 分钟前` : `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return zh ? `${hours} 小时前` : `${hours}h ago`
  const days = Math.floor(hours / 24)
  if (days < 30) return zh ? `${days} 天前` : `${days}d ago`
  const date = new Date(timestamp)
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}
