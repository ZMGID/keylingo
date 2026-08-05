// Git 面板：Changes（分组列表 + 提交框）/ History（虚拟化提交列表 + 内联 diff）两视图。
import { useEffect, useMemo, useState } from 'react'
import { VList } from 'virtua'
import {
  Check,
  ChevronDown,
  ChevronRight,
  FolderSearch,
  GitBranch,
  GitCommitHorizontal,
  Loader2,
  Minus,
  Plus,
  RefreshCw,
  Undo2,
  X,
} from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'
import { IconButton } from '../../components/Button'
import { dockApi } from './api'
import { ConfirmDialog } from './ConfirmDialog'
import { DiffView } from './DiffView'
import { DockContextMenu, type DockMenuAnchor, type DockMenuItem } from './DockContextMenu'
import { partitionStatusEntries, relativeTime, statusLetter, type StatusLetter } from './gitReviewModel'
import { useGitReview } from './useGitReview'
import type { GitBranchItem, GitStatusEntry } from './types'

const STATUS_BADGE_CLASS: Record<StatusLetter, string> = {
  M: 'bg-amber-500/15 text-amber-700 dark:text-amber-300',
  A: 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300',
  D: 'bg-red-500/15 text-red-700 dark:text-red-300',
  R: 'bg-sky-500/15 text-sky-700 dark:text-sky-300',
  C: 'bg-sky-500/15 text-sky-700 dark:text-sky-300',
  U: 'bg-purple-500/15 text-purple-700 dark:text-purple-300',
}

function StatusBadge({ letter }: { letter: StatusLetter }) {
  return (
    <span
      className={`flex h-4 w-4 shrink-0 items-center justify-center rounded font-mono text-[10px] font-semibold ${STATUS_BADGE_CLASS[letter]}`}
    >
      {letter}
    </span>
  )
}

function entryDisplayPath(entry: GitStatusEntry): string {
  return entry.oldPath ? `${entry.oldPath} → ${entry.path}` : entry.path
}

type GitPanelProps = {
  workdir: string
  active: boolean
  lang: Lang
  /** 「在文件树中定位」：切到文件树 tab 并展开定位。 */
  onRevealInTree?: (path: string) => void
}

export function GitPanel({ workdir, active, lang, onRevealInTree }: GitPanelProps) {
  const t = i18n[lang]
  const review = useGitReview({ workdir, active })
  const { status, busy } = review

  const [view, setView] = useState<'changes' | 'history'>('changes')
  const [commitMessage, setCommitMessage] = useState('')
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [menu, setMenu] = useState<{ anchor: DockMenuAnchor; entry: GitStatusEntry } | null>(null)
  const [discardTarget, setDiscardTarget] = useState<GitStatusEntry | null>(null)
  const [expandedSha, setExpandedSha] = useState<string | null>(null)
  const [collapsedSections, setCollapsedSections] = useState<Record<string, boolean>>({})

  // 分支下拉
  const [branchOpen, setBranchOpen] = useState(false)
  const [branches, setBranches] = useState<GitBranchItem[] | null>(null)
  const [newBranch, setNewBranch] = useState('')

  useEffect(() => {
    if (!branchOpen || branches !== null || !workdir) return
    let cancelled = false
    dockApi
      .gitBranches(workdir)
      .then((result) => {
        if (!cancelled) setBranches(result.branches)
      })
      .catch(() => {
        if (!cancelled) setBranches([])
      })
    return () => {
      cancelled = true
    }
  }, [branchOpen, branches, workdir])

  // workdir 切换时重置本地 UI 状态。
  useEffect(() => {
    setSelectedPath(null)
    setCommitMessage('')
    setBranches(null)
    setBranchOpen(false)
    setExpandedSha(null)
    setMenu(null)
    setDiscardTarget(null)
  }, [workdir])

  // History 视图首次打开时加载第一页。
  useEffect(() => {
    if (view === 'history' && review.commits.length === 0 && !review.historyLoading && active) {
      void review.loadHistory(false)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, active])

  const partitioned = useMemo(
    () => partitionStatusEntries(status?.entries ?? []),
    [status?.entries],
  )

  const mutate = (action: Parameters<typeof review.runMutation>[0]) => {
    void review.runMutation(action).catch(() => {
      // 错误已落到 review.mutationError，这里只吞掉 rejection。
    })
  }

  const handleCommit = () => {
    const message = commitMessage.trim()
    if (!message || partitioned.staged.length === 0) return
    void review
      .runMutation(() => dockApi.gitCommit(workdir, message))
      .then(() => setCommitMessage(''))
      .catch(() => {})
  }

  const handleDiscardConfirm = () => {
    const target = discardTarget
    setDiscardTarget(null)
    if (!target) return
    mutate(() => dockApi.gitDiscard(workdir, target.path, target.oldPath))
  }

  const handleSelectEntry = (entry: GitStatusEntry) => {
    const next = selectedPath === entry.path ? null : entry.path
    setSelectedPath(next)
    void review.selectFileDiff(next)
  }

  const handleToggleCommit = (sha: string) => {
    const next = expandedSha === sha ? null : sha
    setExpandedSha(next)
    if (next && !review.commitDiffs[next]) void review.loadCommitDiff(next)
  }

  const menuItems = (entry: GitStatusEntry): DockMenuItem[] => {
    const items: DockMenuItem[] = []
    if (entry.staged) {
      items.push({
        key: 'unstage',
        label: t.dockGitUnstage,
        icon: <Minus strokeWidth={1.75} />,
        onSelect: () => mutate(() => dockApi.gitUnstage(workdir, entry.path)),
      })
    } else {
      items.push({
        key: 'stage',
        label: t.dockGitStage,
        icon: <Plus strokeWidth={1.75} />,
        onSelect: () => mutate(() => dockApi.gitStage(workdir, entry.path)),
      })
      items.push({
        key: 'discard',
        label: t.dockGitDiscard,
        icon: <Undo2 strokeWidth={1.75} />,
        danger: true,
        onSelect: () => setDiscardTarget(entry),
      })
      items.push({
        key: 'gitignore',
        label: t.dockGitAddToGitignore,
        icon: <X strokeWidth={1.75} />,
        onSelect: () => mutate(() => dockApi.gitAddToGitignore(workdir, entry.path)),
      })
    }
    if (onRevealInTree) {
      items.push({
        key: 'reveal',
        label: t.dockGitRevealInTree,
        icon: <FolderSearch strokeWidth={1.75} />,
        onSelect: () => onRevealInTree(entry.path),
      })
    }
    return items
  }

  const renderSection = (
    key: string,
    title: string,
    entries: GitStatusEntry[],
    sectionActions?: React.ReactNode,
  ) => {
    if (entries.length === 0) return null
    const collapsed = collapsedSections[key] ?? false
    return (
      <div key={key}>
        <div className="sticky top-0 z-10 flex items-center gap-1 bg-[var(--theme-surface-soft)] px-2 py-1 dark:bg-[#262629]">
          <button
            type="button"
            className="flex min-w-0 flex-1 items-center gap-1 text-left"
            onClick={() => setCollapsedSections((prev) => ({ ...prev, [key]: !collapsed }))}
          >
            {collapsed ? (
              <ChevronRight size={12} strokeWidth={2} className="shrink-0 text-neutral-400" />
            ) : (
              <ChevronDown size={12} strokeWidth={2} className="shrink-0 text-neutral-400" />
            )}
            <span className="text-[11px] font-medium text-neutral-500 dark:text-neutral-400">{title}</span>
            <span className="text-[10px] text-neutral-400 dark:text-neutral-500">{entries.length}</span>
          </button>
          {sectionActions}
        </div>
        {!collapsed &&
          entries.map((entry) => {
            const letter = statusLetter(entry)
            const selected = selectedPath === entry.path
            // 行内行数徽标：numstat 按新路径键；rename 行（old => new）做兜底匹配。
            const stat =
              review.fileStats[entry.path] ??
              (entry.oldPath ? review.fileStats[`${entry.oldPath} => ${entry.path}`] : undefined)
            return (
              <div
                key={`${entry.staged ? 's' : 'u'}:${entry.path}`}
                role="button"
                tabIndex={0}
                className={`group flex w-full cursor-default items-center gap-1.5 px-2 py-1 text-left transition-colors ${
                  selected
                    ? 'bg-neutral-500/10 dark:bg-neutral-400/10'
                    : 'hover:bg-neutral-500/5 dark:hover:bg-neutral-400/5'
                }`}
                onClick={() => handleSelectEntry(entry)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault()
                    handleSelectEntry(entry)
                  }
                }}
                onContextMenu={(e) => {
                  e.preventDefault()
                  setMenu({ anchor: { left: e.clientX, top: e.clientY }, entry })
                }}
              >
                <StatusBadge letter={letter} />
                <span className="min-w-0 flex-1 truncate text-[12px] text-neutral-700 dark:text-neutral-200">
                  {entryDisplayPath(entry)}
                </span>
                {stat && (stat.additions > 0 || stat.deletions > 0) && (
                  <span className="shrink-0 text-[11px] tabular-nums">
                    {stat.additions > 0 && (
                      <span className="font-medium text-emerald-600 dark:text-emerald-400">+{stat.additions}</span>
                    )}
                    {stat.deletions > 0 && (
                      <span className="ml-1 font-medium text-red-500 dark:text-red-400">−{stat.deletions}</span>
                    )}
                  </span>
                )}
                <span className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                  {entry.staged ? (
                    <IconButton
                      label={t.dockGitUnstage}
                      size="xs"
                      variant="ghost"
                      onClick={(e) => {
                        e.stopPropagation()
                        mutate(() => dockApi.gitUnstage(workdir, entry.path))
                      }}
                    >
                      <Minus size={12} />
                    </IconButton>
                  ) : (
                    <>
                      <IconButton
                        label={t.dockGitStage}
                        size="xs"
                        variant="ghost"
                        onClick={(e) => {
                          e.stopPropagation()
                          mutate(() => dockApi.gitStage(workdir, entry.path))
                        }}
                      >
                        <Plus size={12} />
                      </IconButton>
                      <IconButton
                        label={t.dockGitDiscard}
                        size="xs"
                        variant="ghost"
                        onClick={(e) => {
                          e.stopPropagation()
                          setDiscardTarget(entry)
                        }}
                      >
                        <Undo2 size={12} />
                      </IconButton>
                    </>
                  )}
                </span>
              </div>
            )
          })}
      </div>
    )
  }

  // ---------- 空态 / 错误态 ----------

  // ponytail: 没有 workdir 就永远不会发起 gitStatus，别转圈；只有真在请求时才是加载中。
  if (workdir && !status && review.statusLoading) {
    return (
      <div className="flex flex-1 items-center justify-center gap-2 text-[12px] text-neutral-400">
        <Loader2 size={14} className="animate-spin" />
        {t.dockLoading}
      </div>
    )
  }

  if (!workdir || (status && status.status === 'not_repo')) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 px-6 text-center">
        <GitBranch size={20} strokeWidth={1.5} className="text-neutral-300 dark:text-neutral-600" />
        <div className="text-[12px] text-neutral-400 dark:text-neutral-500">
          {workdir ? t.dockGitNotRepo : t.dockNoWorkdir}
        </div>
      </div>
    )
  }

  const totalChanges = partitioned.staged.length + partitioned.unstaged.length + partitioned.conflicted.length

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* 顶栏：分支选择 + 视图切换 + 刷新 */}
      <div className="relative flex items-center gap-1 border-b border-neutral-200/70 px-2 py-1.5 dark:border-neutral-700/50">
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-1 rounded-md px-1.5 py-1 text-left transition-colors hover:bg-neutral-500/10"
          onClick={() => setBranchOpen((prev) => !prev)}
          disabled={!status || status.status !== 'ready'}
        >
          <GitBranch size={13} strokeWidth={1.75} className="shrink-0 text-neutral-400" />
          <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-neutral-700 dark:text-neutral-200">
            {status?.head || '—'}
          </span>
          {status && (status.ahead > 0 || status.behind > 0) && (
            <span className="shrink-0 text-[10px] text-neutral-400">
              {status.ahead > 0 ? `↑${status.ahead}` : ''}
              {status.behind > 0 ? `↓${status.behind}` : ''}
            </span>
          )}
          <ChevronDown size={12} strokeWidth={2} className="shrink-0 text-neutral-400" />
        </button>
        <div className="flex shrink-0 items-center rounded-md bg-neutral-500/10 p-0.5">
          {(['changes', 'history'] as const).map((item) => (
            <button
              key={item}
              type="button"
              className={`rounded px-2 py-0.5 text-[11px] transition-colors ${
                view === item
                  ? 'bg-white font-medium text-neutral-800 shadow-sm dark:bg-neutral-700 dark:text-neutral-100'
                  : 'text-neutral-500 hover:text-neutral-700 dark:text-neutral-400 dark:hover:text-neutral-200'
              }`}
              onClick={() => setView(item)}
            >
              {item === 'changes' ? t.dockGitChanges : t.dockGitHistory}
            </button>
          ))}
        </div>
        <IconButton label={t.dockRefresh} size="sm" variant="ghost" onClick={() => void review.refresh()}>
          <RefreshCw size={13} className={review.statusLoading ? 'animate-spin' : ''} />
        </IconButton>

        {branchOpen && (
          <>
            <div className="fixed inset-0 z-[150]" onClick={() => setBranchOpen(false)} />
            <div className="absolute left-1 right-1 top-full z-[160] mt-1 max-h-64 overflow-auto rounded-lg border border-neutral-200 bg-white py-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-900">
              <div className="flex items-center gap-1 px-2 py-1">
                <input
                  value={newBranch}
                  onChange={(e) => setNewBranch(e.target.value)}
                  placeholder={t.dockGitNewBranchPlaceholder}
                  className="min-w-0 flex-1 rounded-md border border-neutral-200 bg-transparent px-1.5 py-1 text-[12px] outline-none focus:border-neutral-400 dark:border-neutral-700 dark:focus:border-neutral-500"
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && newBranch.trim()) {
                      mutate(() => dockApi.gitCreateBranch(workdir, newBranch.trim()))
                      setNewBranch('')
                      setBranchOpen(false)
                    }
                  }}
                />
                <IconButton
                  label={t.dockGitNewBranch}
                  size="xs"
                  variant="ghost"
                  disabled={!newBranch.trim() || busy}
                  onClick={() => {
                    mutate(() => dockApi.gitCreateBranch(workdir, newBranch.trim()))
                    setNewBranch('')
                    setBranchOpen(false)
                  }}
                >
                  <Plus size={12} />
                </IconButton>
              </div>
              <div className="my-1 border-t border-neutral-200/80 dark:border-neutral-700" />
              {(branches ?? []).length === 0 ? (
                <div className="px-3 py-2 text-[11px] text-neutral-400">{t.dockGitBranchesEmpty}</div>
              ) : (
                (branches ?? []).map((branch) => (
                  <button
                    key={branch.name}
                    type="button"
                    className={`flex w-full items-center gap-1.5 px-3 py-1 text-left text-[12px] transition-colors hover:bg-neutral-500/10 ${
                      branch.current
                        ? 'font-medium text-neutral-900 dark:text-neutral-50'
                        : 'text-neutral-600 dark:text-neutral-300'
                    }`}
                    disabled={branch.current || busy}
                    onClick={() => {
                      setBranchOpen(false)
                      mutate(() => dockApi.gitSwitchBranch(workdir, branch.name))
                    }}
                  >
                    {branch.current ? (
                      <Check size={12} strokeWidth={2} className="shrink-0 text-emerald-500" />
                    ) : (
                      <span className="w-3 shrink-0" />
                    )}
                    <span className="min-w-0 flex-1 truncate">{branch.name}</span>
                    {(branch.ahead > 0 || branch.behind > 0) && (
                      <span className="shrink-0 text-[10px] text-neutral-400">
                        {branch.ahead > 0 ? `↑${branch.ahead}` : ''}
                        {branch.behind > 0 ? `↓${branch.behind}` : ''}
                      </span>
                    )}
                  </button>
                ))
              )}
            </div>
          </>
        )}
      </div>

      {/* 错误横幅 */}
      {(review.viewError || review.mutationError || (status && status.status === 'error')) && (
        <div className="flex items-start gap-1.5 border-b border-red-500/20 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-700 dark:text-red-300">
          <span className="min-w-0 flex-1 break-all">
            {review.mutationError || review.viewError || status?.error}
          </span>
          <button
            type="button"
            className="shrink-0 rounded p-0.5 hover:bg-red-500/20"
            onClick={() => {
              review.clearMutationError()
              void review.refresh({ silent: true })
            }}
          >
            <X size={11} />
          </button>
        </div>
      )}

      {view === 'changes' ? (
        <>
          <div className="custom-scrollbar min-h-0 flex-1 overflow-y-auto">
            {totalChanges === 0 ? (
              <div className="px-3 py-8 text-center text-[12px] text-neutral-400 dark:text-neutral-500">
                {t.dockGitEmpty}
              </div>
            ) : (
              <>
                {renderSection('conflicted', t.dockGitConflicted, partitioned.conflicted)}
                {renderSection(
                  'staged',
                  t.dockGitStaged,
                  partitioned.staged,
                  partitioned.staged.length > 0 ? (
                    <IconButton
                      label={t.dockGitUnstage}
                      size="xs"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => mutate(() => dockApi.gitUnstageAll(workdir))}
                    >
                      <Minus size={12} />
                    </IconButton>
                  ) : undefined,
                )}
                {renderSection(
                  'unstaged',
                  t.dockGitUnstaged,
                  partitioned.unstaged,
                  partitioned.unstaged.length > 0 ? (
                    <IconButton
                      label={t.dockGitStage}
                      size="xs"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => mutate(() => dockApi.gitStageAll(workdir))}
                    >
                      <Plus size={12} />
                    </IconButton>
                  ) : undefined,
                )}
              </>
            )}
          </div>

          {/* 选中文件 diff */}
          {selectedPath && (
            <div className="custom-scrollbar max-h-[40%] shrink-0 overflow-y-auto border-t border-neutral-200/70 p-2 dark:border-neutral-700/50">
              {review.fileDiffLoading ? (
                <div className="flex items-center justify-center gap-2 py-4 text-[11px] text-neutral-400">
                  <Loader2 size={12} className="animate-spin" />
                  {t.dockLoading}
                </div>
              ) : review.fileDiff ? (
                <DiffView
                  patch={review.fileDiff.result.patch}
                  truncated={review.fileDiff.result.truncated}
                  lang={lang}
                  emptyText={t.dockDiffEmpty}
                />
              ) : null}
            </div>
          )}

          {/* 提交框 */}
          <div className="shrink-0 border-t border-neutral-200/70 p-2 dark:border-neutral-700/50">
            <textarea
              value={commitMessage}
              onChange={(e) => setCommitMessage(e.target.value)}
              placeholder={t.dockGitCommitPlaceholder}
              rows={2}
              className="custom-scrollbar w-full resize-none rounded-md border border-neutral-200 bg-transparent px-2 py-1.5 text-[12px] outline-none focus:border-neutral-400 dark:border-neutral-700 dark:focus:border-neutral-500"
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) handleCommit()
              }}
            />
            <button
              type="button"
              className="mt-1.5 flex w-full items-center justify-center gap-1.5 rounded-md bg-neutral-900 px-3 py-1.5 text-[12px] font-medium text-white transition-colors hover:bg-neutral-700 disabled:cursor-not-allowed disabled:opacity-40 dark:bg-neutral-100 dark:text-neutral-900 dark:hover:bg-neutral-200"
              disabled={partitioned.staged.length === 0 || !commitMessage.trim() || busy}
              onClick={handleCommit}
            >
              {busy ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} strokeWidth={2} />}
              {t.dockGitCommit}
            </button>
          </div>
        </>
      ) : (
        <div className="min-h-0 flex-1">
          {review.commits.length === 0 && !review.historyLoading ? (
            <div className="px-3 py-8 text-center text-[12px] text-neutral-400 dark:text-neutral-500">
              {t.dockGitHistoryEmpty}
            </div>
          ) : (
            <VList className="custom-scrollbar h-full">
              {review.commits.map((commit) => {
                const expanded = expandedSha === commit.sha
                const diffSlot = review.commitDiffs[commit.sha]
                return (
                  <div key={commit.sha} className="border-b border-neutral-200/50 dark:border-neutral-700/30">
                    <button
                      type="button"
                      className="flex w-full items-start gap-2 px-2 py-1.5 text-left transition-colors hover:bg-neutral-500/5 dark:hover:bg-neutral-400/5"
                      onClick={() => handleToggleCommit(commit.sha)}
                    >
                      <GitCommitHorizontal size={13} strokeWidth={1.75} className="mt-0.5 shrink-0 text-neutral-400" />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-[12px] text-neutral-700 dark:text-neutral-200">
                          {commit.subject}
                        </span>
                        <span className="mt-0.5 flex items-center gap-1.5 text-[10px] text-neutral-400 dark:text-neutral-500">
                          <span className="font-mono">{commit.shortSha}</span>
                          <span className="truncate">{commit.authorName}</span>
                          <span className="shrink-0">{relativeTime(commit.authorDate, lang)}</span>
                        </span>
                      </span>
                      {expanded ? (
                        <ChevronDown size={12} strokeWidth={2} className="mt-1 shrink-0 text-neutral-400" />
                      ) : (
                        <ChevronRight size={12} strokeWidth={2} className="mt-1 shrink-0 text-neutral-400" />
                      )}
                    </button>
                    {expanded && (
                      <div className="px-2 pb-2">
                        {!diffSlot || diffSlot === 'loading' ? (
                          <div className="flex items-center justify-center gap-2 py-3 text-[11px] text-neutral-400">
                            <Loader2 size={12} className="animate-spin" />
                            {t.dockLoading}
                          </div>
                        ) : diffSlot === 'error' ? (
                          <button
                            type="button"
                            className="w-full py-2 text-center text-[11px] text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-300"
                            onClick={() => void review.loadCommitDiff(commit.sha)}
                          >
                            {t.dockRetry}
                          </button>
                        ) : (
                          <DiffView
                            patch={diffSlot.patch}
                            truncated={diffSlot.truncated}
                            lang={lang}
                            emptyText={t.dockDiffEmpty}
                          />
                        )}
                      </div>
                    )}
                  </div>
                )
              })}
              {(review.historyHasMore || review.historyLoading) && (
                <button
                  type="button"
                  className="flex w-full items-center justify-center gap-1.5 py-2 text-[11px] text-neutral-400 transition-colors hover:text-neutral-600 disabled:opacity-50 dark:hover:text-neutral-300"
                  disabled={review.historyLoading}
                  onClick={() => void review.loadHistory(true)}
                >
                  {review.historyLoading && <Loader2 size={11} className="animate-spin" />}
                  {t.dockGitLoadMore}
                </button>
              )}
            </VList>
          )}
        </div>
      )}

      {menu && (
        <DockContextMenu
          anchor={menu.anchor}
          items={menuItems(menu.entry)}
          onClose={() => setMenu(null)}
        />
      )}
      {discardTarget && (
        <ConfirmDialog
          lang={lang}
          title={t.dockGitDiscardTitle}
          message={discardTarget.path}
          confirmLabel={t.dockGitDiscardConfirm}
          onConfirm={handleDiscardConfirm}
          onCancel={() => setDiscardTarget(null)}
        />
      )}
    </div>
  )
}
