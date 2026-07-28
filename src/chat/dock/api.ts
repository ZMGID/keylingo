// Right Dock API 调用封装。模式镜像 src/chat/api.ts：
// Tauri 运行时走 invoke，纯浏览器（npm run dev:ui）返回空树 / not_repo mock，保证预览不炸。
import { invoke } from '@tauri-apps/api/core'
import { isTauriRuntime } from '../utils'
import {
  normalizeDockFsListResult,
  normalizeDockFsSearchResult,
  normalizeGitBranchesResult,
  normalizeGitDiffResult,
  normalizeGitDiffStat,
  normalizeGitLogResult,
  normalizeGitMutationResult,
  normalizeGitRepoState,
  type DockFsEntryKind,
  type DockFsListResult,
  type DockFsMutationResult,
  type DockFsOpenResult,
  type DockFsSearchResult,
  type GitBranchesResult,
  type GitDiffResult,
  type GitDiffStat,
  type GitLogResult,
  type GitMutationResult,
  type GitRepoState,
} from './types'

const MOCK_NOT_REPO: GitRepoState = {
  repoRoot: '',
  head: '',
  upstream: null,
  ahead: 0,
  behind: 0,
  stashCount: 0,
  entries: [],
  status: 'not_repo',
  error: null,
}

const MOCK_EMPTY_DIFF: GitDiffResult = {
  baseRef: '',
  headRef: '',
  mode: 'working_tree',
  files: [],
  patch: '',
  stat: '',
  truncated: false,
  binaryFiles: [],
}

const MOCK_MUTATION: GitMutationResult = {
  ok: false,
  state: MOCK_NOT_REPO,
  stdout: '',
  stderr: '',
  message: 'Dock requires the desktop app',
}

export const dockApi = {
  /** 解析当前会话/项目的有效工作目录。 */
  async resolveCwd(conversationId: string | null | undefined, projectId?: string | null): Promise<string> {
    if (!isTauriRuntime()) return ''
    if (!conversationId && !projectId) return ''
    return invoke<string>('dock_resolve_cwd', {
      conversationId: conversationId ?? null,
      projectId: projectId ?? null,
    })
  },

  async fsList(
    workdir: string,
    path = '',
    maxResults = 1000,
    showHidden = false,
  ): Promise<DockFsListResult> {
    if (!isTauriRuntime()) return { entries: [], hasMore: false }
    const raw = await invoke('dock_fs_list', { workdir, path, maxResults, showHidden })
    return normalizeDockFsListResult(raw)
  },

  async fsSearch(
    workdir: string,
    query: string,
    maxResults = 200,
    showHidden = false,
  ): Promise<DockFsSearchResult> {
    if (!isTauriRuntime()) return { entries: [], truncated: false }
    const raw = await invoke('dock_fs_search', { workdir, query, maxResults, showHidden })
    return normalizeDockFsSearchResult(raw)
  },

  async fsCreate(workdir: string, path: string, kind: DockFsEntryKind): Promise<DockFsMutationResult> {
    if (!isTauriRuntime()) throw new Error('Dock requires the desktop app')
    return invoke<DockFsMutationResult>('dock_fs_create', { workdir, path, kind })
  },

  async fsRename(workdir: string, fromPath: string, toPath: string): Promise<DockFsMutationResult> {
    if (!isTauriRuntime()) throw new Error('Dock requires the desktop app')
    return invoke<DockFsMutationResult>('dock_fs_rename', { workdir, fromPath, toPath })
  },

  async fsDelete(workdir: string, path: string): Promise<DockFsMutationResult> {
    if (!isTauriRuntime()) throw new Error('Dock requires the desktop app')
    return invoke<DockFsMutationResult>('dock_fs_delete', { workdir, path })
  },

  async fsOpenPath(workdir: string, path: string, mode: 'open' | 'reveal'): Promise<DockFsOpenResult> {
    if (!isTauriRuntime()) throw new Error('Dock requires the desktop app')
    return invoke<DockFsOpenResult>('dock_fs_open_path', { workdir, path, mode })
  },

  /** 整体替换监听目标集。 */
  async workspaceWatchSet(workdirs: string[]): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('dock_workspace_watch_set', { workdirs })
  },

  // ---- 终端面板（PTY 会话）----

  async terminalCreate(workdir: string, cols: number, rows: number): Promise<{ sessionId: string }> {
    if (!isTauriRuntime()) throw new Error('Terminal requires the desktop app')
    return invoke<{ sessionId: string }>('dock_terminal_create', { workdir, cols, rows })
  },

  async terminalWrite(sessionId: string, data: string): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('dock_terminal_write', { sessionId, data })
  },

  async terminalResize(sessionId: string, cols: number, rows: number): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('dock_terminal_resize', { sessionId, cols, rows })
  },

  async terminalClose(sessionId: string): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('dock_terminal_close', { sessionId })
  },

  async gitStatus(workdir: string): Promise<GitRepoState> {
    if (!isTauriRuntime()) return MOCK_NOT_REPO
    const raw = await invoke('dock_git_status', { workdir })
    return normalizeGitRepoState(raw)
  },

  async gitDiff(
    workdir: string,
    mode: 'branch' | 'working_tree' = 'working_tree',
    path?: string,
  ): Promise<GitDiffResult> {
    if (!isTauriRuntime()) return MOCK_EMPTY_DIFF
    const raw = await invoke('dock_git_diff', { workdir, mode, path: path ?? null })
    return normalizeGitDiffResult(raw)
  },

  async gitLog(workdir: string, limit = 50, skip = 0): Promise<GitLogResult> {
    if (!isTauriRuntime()) return { commits: [], hasMore: false }
    const raw = await invoke('dock_git_log', { workdir, limit, skip })
    return normalizeGitLogResult(raw)
  },

  async gitCommitDiff(workdir: string, commit: string, path?: string): Promise<GitDiffResult> {
    if (!isTauriRuntime()) return MOCK_EMPTY_DIFF
    const raw = await invoke('dock_git_commit_diff', { workdir, commit, path: path ?? null })
    return normalizeGitDiffResult(raw)
  },

  async gitBranches(workdir: string): Promise<GitBranchesResult> {
    if (!isTauriRuntime()) return { branches: [] }
    const raw = await invoke('dock_git_branches', { workdir })
    return normalizeGitBranchesResult(raw)
  },

  async gitDiffStat(workdir: string): Promise<GitDiffStat> {
    if (!isTauriRuntime()) return { filesChanged: 0, additions: 0, deletions: 0, files: [] }
    const raw = await invoke('dock_git_diff_stat', { workdir })
    return normalizeGitDiffStat(raw)
  },

  // ---- 变更类（均返回 { ok, state, stdout, stderr, message }）----

  async gitStage(workdir: string, path: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_stage', { workdir, path }))
  },

  async gitStageAll(workdir: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_stage_all', { workdir }))
  },

  async gitUnstage(workdir: string, path: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_unstage', { workdir, path }))
  },

  async gitUnstageAll(workdir: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_unstage_all', { workdir }))
  },

  async gitDiscard(workdir: string, path: string, oldPath?: string | null): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_discard', { workdir, path, oldPath: oldPath ?? null }))
  },

  async gitDiscardAll(workdir: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_discard_all', { workdir }))
  },

  async gitCommit(workdir: string, message: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_commit', { workdir, message }))
  },

  async gitSwitchBranch(workdir: string, branch: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_switch_branch', { workdir, branch }))
  },

  async gitCreateBranch(workdir: string, branch: string, startPoint?: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_create_branch', { workdir, branch, startPoint: startPoint ?? null }))
  },

  async gitInit(workdir: string, branch?: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_init', { workdir, branch: branch ?? null }))
  },

  async gitAddToGitignore(workdir: string, path: string): Promise<GitMutationResult> {
    if (!isTauriRuntime()) return MOCK_MUTATION
    return normalizeGitMutationResult(await invoke('dock_git_add_to_gitignore', { workdir, path }))
  },
}
