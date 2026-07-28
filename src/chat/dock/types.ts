// Right Dock 类型定义与容错 normalizer。
// 后端（src-tauri/src/dock/*）serde 输出 camelCase，但 normalizer 同时接受 snake_case，
// 与 kivio 现有 normalize 风格（见 src/chat/api.ts normalizeAgentRuntime）一致。

export type DockFsEntryKind = 'file' | 'dir'

export type DockFsEntry = {
  /** 相对 workdir 的路径（POSIX 分隔符）。根目录为 ''。 */
  path: string
  kind: DockFsEntryKind
  hidden: boolean
}

export type DockFsListResult = {
  entries: DockFsEntry[]
  hasMore: boolean
}

export type DockFsSearchResult = {
  entries: DockFsEntry[]
  truncated: boolean
}

export type DockFsMutationResult = {
  path: string
  kind: DockFsEntryKind
  fromPath?: string
}

export type DockFsOpenResult = {
  path: string
  absolutePath: string
  kind: DockFsEntryKind
  mode: 'open' | 'reveal'
}

export type GitStatusEntry = {
  path: string
  oldPath?: string | null
  indexStatus: string
  worktreeStatus: string
  kind: string
  staged: boolean
  conflicted: boolean
  untracked: boolean
}

export type GitRepoState = {
  repoRoot: string
  head: string
  upstream: string | null
  ahead: number
  behind: number
  stashCount: number
  entries: GitStatusEntry[]
  status: 'ready' | 'not_repo' | 'error'
  error?: string | null
}

export type GitDiffResult = {
  baseRef: string
  headRef: string
  mode: 'branch' | 'working_tree' | string
  files: string[]
  patch: string
  stat: string
  truncated: boolean
  binaryFiles: string[]
}

export type GitCommitItem = {
  sha: string
  shortSha: string
  subject: string
  authorName: string
  authorDate: string
  refs: string[]
}

export type GitLogResult = {
  commits: GitCommitItem[]
  hasMore: boolean
}

export type GitBranchItem = {
  name: string
  current: boolean
  upstream: string | null
  ahead: number
  behind: number
}

export type GitBranchesResult = {
  branches: GitBranchItem[]
}

/** 状态条 diff 徽标：工作区相对 HEAD 的 numstat 汇总。 */
export type GitDiffStatFile = {
  path: string
  additions: number
  deletions: number
}

export type GitDiffStat = {
  filesChanged: number
  additions: number
  deletions: number
  files: GitDiffStatFile[]
}

/** 变更类命令统一返回：state 为操作后的全新 GitRepoState。 */
export type GitMutationResult = {
  ok: boolean
  state: GitRepoState
  stdout: string
  stderr: string
  message: string
}

export type WorkspaceActivityEvent = {
  workdir: string
  revision: number
  fs: boolean
  git: boolean
  changedPaths: string[]
  truncated: boolean
}

// ---------- normalizers ----------

type RawRecord = Record<string, unknown>

function asRecord(value: unknown): RawRecord {
  return typeof value === 'object' && value !== null ? (value as RawRecord) : {}
}

function pick<T>(record: RawRecord, camel: string, snake: string, fallback: T): T {
  const value = record[camel] ?? record[snake]
  return value === undefined || value === null ? fallback : (value as T)
}

function pickString(record: RawRecord, camel: string, snake: string, fallback = ''): string {
  const value = pick<unknown>(record, camel, snake, fallback)
  return typeof value === 'string' ? value : String(value ?? fallback)
}

function pickNumber(record: RawRecord, camel: string, snake: string, fallback = 0): number {
  const value = pick<unknown>(record, camel, snake, fallback)
  const num = Number(value)
  return Number.isFinite(num) ? num : fallback
}

function pickBoolean(record: RawRecord, camel: string, snake: string, fallback = false): boolean {
  const value = pick<unknown>(record, camel, snake, fallback)
  return value === true || value === 'true' || value === 1
}

function pickStringArray(record: RawRecord, camel: string, snake: string): string[] {
  const value = pick<unknown>(record, camel, snake, [])
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : []
}

export function normalizeDockFsEntry(raw: unknown): DockFsEntry {
  const record = asRecord(raw)
  const kind = pickString(record, 'kind', 'kind', 'file')
  return {
    path: pickString(record, 'path', 'path'),
    kind: kind === 'dir' ? 'dir' : 'file',
    hidden: pickBoolean(record, 'hidden', 'hidden'),
  }
}

export function normalizeDockFsListResult(raw: unknown): DockFsListResult {
  const record = asRecord(raw)
  const entries = Array.isArray(record.entries) ? record.entries.map(normalizeDockFsEntry) : []
  return { entries, hasMore: pickBoolean(record, 'hasMore', 'has_more') }
}

export function normalizeDockFsSearchResult(raw: unknown): DockFsSearchResult {
  const record = asRecord(raw)
  const entries = Array.isArray(record.entries) ? record.entries.map(normalizeDockFsEntry) : []
  return { entries, truncated: pickBoolean(record, 'truncated', 'truncated') }
}

export function normalizeGitStatusEntry(raw: unknown): GitStatusEntry {
  const record = asRecord(raw)
  return {
    path: pickString(record, 'path', 'path'),
    oldPath: pick<string | null>(record, 'oldPath', 'old_path', null),
    indexStatus: pickString(record, 'indexStatus', 'index_status', ' '),
    worktreeStatus: pickString(record, 'worktreeStatus', 'worktree_status', ' '),
    kind: pickString(record, 'kind', 'kind', 'file'),
    staged: pickBoolean(record, 'staged', 'staged'),
    conflicted: pickBoolean(record, 'conflicted', 'conflicted'),
    untracked: pickBoolean(record, 'untracked', 'untracked'),
  }
}

export function normalizeGitRepoState(raw: unknown): GitRepoState {
  const record = asRecord(raw)
  const status = pickString(record, 'status', 'status', 'error')
  return {
    repoRoot: pickString(record, 'repoRoot', 'repo_root'),
    head: pickString(record, 'head', 'head'),
    upstream: pick<string | null>(record, 'upstream', 'upstream', null),
    ahead: pickNumber(record, 'ahead', 'ahead'),
    behind: pickNumber(record, 'behind', 'behind'),
    stashCount: pickNumber(record, 'stashCount', 'stash_count'),
    entries: Array.isArray(record.entries) ? record.entries.map(normalizeGitStatusEntry) : [],
    status: status === 'ready' || status === 'not_repo' ? status : 'error',
    error: pick<string | null>(record, 'error', 'error', null),
  }
}

export function normalizeGitDiffResult(raw: unknown): GitDiffResult {
  const record = asRecord(raw)
  return {
    baseRef: pickString(record, 'baseRef', 'base_ref'),
    headRef: pickString(record, 'headRef', 'head_ref'),
    mode: pickString(record, 'mode', 'mode', 'working_tree'),
    files: pickStringArray(record, 'files', 'files'),
    patch: pickString(record, 'patch', 'patch'),
    stat: pickString(record, 'stat', 'stat'),
    truncated: pickBoolean(record, 'truncated', 'truncated'),
    binaryFiles: pickStringArray(record, 'binaryFiles', 'binary_files'),
  }
}

export function normalizeGitCommitItem(raw: unknown): GitCommitItem {
  const record = asRecord(raw)
  return {
    sha: pickString(record, 'sha', 'sha'),
    shortSha: pickString(record, 'shortSha', 'short_sha'),
    subject: pickString(record, 'subject', 'subject'),
    authorName: pickString(record, 'authorName', 'author_name'),
    authorDate: pickString(record, 'authorDate', 'author_date'),
    refs: pickStringArray(record, 'refs', 'refs'),
  }
}

export function normalizeGitLogResult(raw: unknown): GitLogResult {
  const record = asRecord(raw)
  return {
    commits: Array.isArray(record.commits) ? record.commits.map(normalizeGitCommitItem) : [],
    hasMore: pickBoolean(record, 'hasMore', 'has_more'),
  }
}

export function normalizeGitBranchItem(raw: unknown): GitBranchItem {
  const record = asRecord(raw)
  return {
    name: pickString(record, 'name', 'name'),
    current: pickBoolean(record, 'current', 'current'),
    upstream: pick<string | null>(record, 'upstream', 'upstream', null),
    ahead: pickNumber(record, 'ahead', 'ahead'),
    behind: pickNumber(record, 'behind', 'behind'),
  }
}

export function normalizeGitBranchesResult(raw: unknown): GitBranchesResult {
  const record = asRecord(raw)
  return {
    branches: Array.isArray(record.branches) ? record.branches.map(normalizeGitBranchItem) : [],
  }
}

export function normalizeGitDiffStat(raw: unknown): GitDiffStat {
  const record = asRecord(raw)
  const files = Array.isArray(record.files) ? record.files : []
  return {
    filesChanged: pickNumber(record, 'filesChanged', 'files_changed'),
    additions: pickNumber(record, 'additions', 'additions'),
    deletions: pickNumber(record, 'deletions', 'deletions'),
    files: files.map((item) => {
      const file = asRecord(item)
      return {
        path: pickString(file, 'path', 'path'),
        additions: pickNumber(file, 'additions', 'additions'),
        deletions: pickNumber(file, 'deletions', 'deletions'),
      }
    }),
  }
}

export function normalizeGitMutationResult(raw: unknown): GitMutationResult {
  const record = asRecord(raw)
  return {
    ok: pickBoolean(record, 'ok', 'ok'),
    state: normalizeGitRepoState(record.state),
    stdout: pickString(record, 'stdout', 'stdout'),
    stderr: pickString(record, 'stderr', 'stderr'),
    message: pickString(record, 'message', 'message'),
  }
}

export function normalizeWorkspaceActivityEvent(raw: unknown): WorkspaceActivityEvent {
  const record = asRecord(raw)
  return {
    workdir: pickString(record, 'workdir', 'workdir'),
    revision: pickNumber(record, 'revision', 'revision'),
    fs: pickBoolean(record, 'fs', 'fs'),
    git: pickBoolean(record, 'git', 'git'),
    changedPaths: pickStringArray(record, 'changedPaths', 'changed_paths'),
    truncated: pickBoolean(record, 'truncated', 'truncated'),
  }
}
