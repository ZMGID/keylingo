// Git 面板数据 hook：status 刷新（签名守卫防恒等重渲）、选中文件 diff、
// history 分页、变更操作单飞行锁、workspace-activity 失效。
import { useCallback, useEffect, useRef, useState } from 'react'
import { dockApi } from './api'
import { appendHistoryPage, gitStatusSignature } from './gitReviewModel'
import type { GitCommitItem, GitDiffResult, GitDiffStatFile, GitMutationResult, GitRepoState } from './types'
import { workspaceActivity } from './workspaceActivity'

const HISTORY_PAGE_SIZE = 50
const FALLBACK_POLL_MS = 10_000

export type CommitDiffSlot = GitDiffResult | 'loading' | 'error'

export type UseGitReviewOptions = {
  workdir: string
  active: boolean
}

export function useGitReview({ workdir, active }: UseGitReviewOptions) {
  const [status, setStatus] = useState<GitRepoState | null>(null)
  const [statusLoading, setStatusLoading] = useState(false)
  const [viewError, setViewError] = useState('')
  const [mutationError, setMutationError] = useState('')
  const [busy, setBusy] = useState(false)
  const [commits, setCommits] = useState<GitCommitItem[]>([])
  const [historyHasMore, setHistoryHasMore] = useState(false)
  const [historyLoading, setHistoryLoading] = useState(false)
  const [fileDiff, setFileDiff] = useState<{ key: string; result: GitDiffResult } | null>(null)
  const [fileDiffLoading, setFileDiffLoading] = useState(false)
  const [commitDiffs, setCommitDiffs] = useState<Record<string, CommitDiffSlot>>({})
  /** 逐文件行数统计（改动列表行内徽标）：path → {additions, deletions}。 */
  const [fileStats, setFileStats] = useState<Record<string, GitDiffStatFile>>({})

  const workdirRef = useRef(workdir)
  workdirRef.current = workdir
  const activeRef = useRef(active)
  activeRef.current = active
  const statusRef = useRef(status)
  statusRef.current = status
  const signatureRef = useRef('')
  const busyRef = useRef(false)
  const dirtyRef = useRef(false)
  const lastRevisionRef = useRef(-1)
  const fileDiffEpochRef = useRef(0)
  const fileStatsKeyRef = useRef('')

  /** 拉逐文件 numstat；内容没变则保持原引用（配合签名守卫，只在状态变化时被调用）。 */
  const loadFileStats = useCallback(async (currentWorkdir: string) => {
    try {
      const stat = await dockApi.gitDiffStat(currentWorkdir)
      if (workdirRef.current !== currentWorkdir) return
      const key = stat.files.map((f) => `${f.path}:${f.additions}:${f.deletions}`).join(';')
      if (key === fileStatsKeyRef.current) return
      fileStatsKeyRef.current = key
      const map: Record<string, GitDiffStatFile> = {}
      for (const file of stat.files) map[file.path] = file
      setFileStats(map)
    } catch {
      // 行数徽标是增强信息，失败静默，下轮 refresh 自愈。
    }
  }, [])

  const refresh = useCallback(async (options: { silent?: boolean; force?: boolean } = {}) => {
    const { silent = false, force = false } = options
    const currentWorkdir = workdirRef.current
    if (!currentWorkdir) return
    if (!silent) setStatusLoading(true)
    try {
      const next = await dockApi.gitStatus(currentWorkdir)
      if (workdirRef.current !== currentWorkdir) return
      const signature = gitStatusSignature(next)
      if (force || signature !== signatureRef.current) {
        signatureRef.current = signature
        setStatus(next)
        if (next.status === 'ready') {
          void loadFileStats(currentWorkdir)
        } else {
          fileStatsKeyRef.current = ''
          setFileStats({})
        }
      }
      setViewError('')
    } catch (err) {
      if (workdirRef.current !== currentWorkdir) return
      setViewError(err instanceof Error ? err.message : String(err))
    } finally {
      if (!silent && workdirRef.current === currentWorkdir) setStatusLoading(false)
    }
  }, [loadFileStats])

  // workdir 变化：清空全部 git 状态并立即重拉。
  useEffect(() => {
    signatureRef.current = ''
    lastRevisionRef.current = -1
    dirtyRef.current = false
    setStatus(null)
    setViewError('')
    setMutationError('')
    setCommits([])
    setHistoryHasMore(false)
    setFileDiff(null)
    setCommitDiffs({})
    fileStatsKeyRef.current = ''
    setFileStats({})
    if (workdir && active) void refresh()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workdir])

  // active 由 false → true：冲刷脏标记。
  useEffect(() => {
    if (!active || !workdir) return
    if (!statusRef.current) void refresh({ silent: true })
    if (dirtyRef.current) {
      dirtyRef.current = false
      void refresh({ silent: true })
    }
  }, [active, workdir, refresh])

  // workspace-activity：fs 或 git 任一置位即静默刷新；revision 回退（watcher 重建/重置）强制刷新。
  useEffect(() => {
    if (!workdir) return
    return workspaceActivity.subscribe(workdir, (activity) => {
      const regressed = activity.revision < lastRevisionRef.current
      lastRevisionRef.current = Math.max(lastRevisionRef.current, activity.revision)
      if (!activity.fs && !activity.git) return
      if (!activeRef.current) {
        dirtyRef.current = true
        return
      }
      void refresh({ silent: true, force: regressed })
    })
  }, [workdir, refresh])

  // watcher 不可用（浏览器预览）时的 10s 兜底轮询。
  useEffect(() => {
    if (!active || !workdir || workspaceActivity.isAvailable()) return
    const timer = window.setInterval(() => void refresh({ silent: true }), FALLBACK_POLL_MS)
    return () => window.clearInterval(timer)
  }, [active, workdir, refresh])

  /** 选中文件 diff：working_tree 为主；有 upstream 时并行拉 branch diff，working_tree 为空则回退。 */
  const selectFileDiff = useCallback(async (path: string | null) => {
    const epoch = ++fileDiffEpochRef.current
    if (!path) {
      setFileDiff(null)
      setFileDiffLoading(false)
      return
    }
    const currentWorkdir = workdirRef.current
    if (!currentWorkdir) return
    setFileDiffLoading(true)
    const hasUpstream = Boolean(statusRef.current?.upstream)
    const [workingTree, branch] = await Promise.allSettled([
      dockApi.gitDiff(currentWorkdir, 'working_tree', path),
      hasUpstream ? dockApi.gitDiff(currentWorkdir, 'branch', path) : Promise.resolve(null),
    ])
    if (fileDiffEpochRef.current !== epoch || workdirRef.current !== currentWorkdir) return
    const workingResult = workingTree.status === 'fulfilled' ? workingTree.value : null
    const branchResult = branch.status === 'fulfilled' ? branch.value : null
    const picked =
      workingResult && (workingResult.patch || !branchResult)
        ? workingResult
        : (branchResult ?? workingResult)
    if (picked) {
      setFileDiff({ key: `${currentWorkdir}:${path}`, result: picked })
    } else {
      setFileDiff(null)
      setViewError(
        workingTree.status === 'rejected'
          ? String(workingTree.reason instanceof Error ? workingTree.reason.message : workingTree.reason)
          : 'diff failed',
      )
    }
    setFileDiffLoading(false)
  }, [])

  const loadHistory = useCallback(async (append: boolean) => {
    const currentWorkdir = workdirRef.current
    if (!currentWorkdir || historyLoading) return
    setHistoryLoading(true)
    try {
      const skip = append ? commits.length : 0
      const result = await dockApi.gitLog(currentWorkdir, HISTORY_PAGE_SIZE, skip)
      if (workdirRef.current !== currentWorkdir) return
      setCommits((prev) => (append ? appendHistoryPage(prev, result.commits) : result.commits))
      setHistoryHasMore(result.hasMore)
    } catch (err) {
      if (workdirRef.current !== currentWorkdir) return
      setViewError(err instanceof Error ? err.message : String(err))
    } finally {
      if (workdirRef.current === currentWorkdir) setHistoryLoading(false)
    }
  }, [commits.length, historyLoading])

  const loadCommitDiff = useCallback(async (sha: string) => {
    const currentWorkdir = workdirRef.current
    if (!currentWorkdir) return
    setCommitDiffs((prev) => (prev[sha] === 'loading' ? prev : { ...prev, [sha]: 'loading' }))
    try {
      const result = await dockApi.gitCommitDiff(currentWorkdir, sha)
      if (workdirRef.current !== currentWorkdir) return
      setCommitDiffs((prev) => ({ ...prev, [sha]: result }))
    } catch {
      if (workdirRef.current !== currentWorkdir) return
      setCommitDiffs((prev) => ({ ...prev, [sha]: 'error' }))
    }
  }, [])

  /**
   * 变更操作单飞行：同一时刻只允许一个在飞；ok:false 时抛出（message/stderr 优先），
   * 成功则先应用随响应返回的新 status，再静默刷新兜底。
   */
  const runMutation = useCallback(
    async (action: () => Promise<GitMutationResult>): Promise<void> => {
      if (busyRef.current) return
      busyRef.current = true
      setBusy(true)
      setMutationError('')
      try {
        const result = await action()
        if (!result.ok) {
          throw new Error(result.message || result.stderr || 'git operation failed')
        }
        const signature = gitStatusSignature(result.state)
        if (signature !== signatureRef.current) {
          signatureRef.current = signature
          setStatus(result.state)
        }
        void refresh({ silent: true })
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        setMutationError(message)
        throw err
      } finally {
        busyRef.current = false
        setBusy(false)
      }
    },
    [refresh],
  )

  const clearMutationError = useCallback(() => setMutationError(''), [])

  return {
    status,
    statusLoading,
    viewError,
    mutationError,
    clearMutationError,
    busy,
    commits,
    historyHasMore,
    historyLoading,
    loadHistory,
    fileDiff,
    fileDiffLoading,
    selectFileDiff,
    commitDiffs,
    loadCommitDiff,
    fileStats,
    refresh,
    runMutation,
  }
}
