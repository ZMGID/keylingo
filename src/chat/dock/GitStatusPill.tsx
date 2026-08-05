// 工具栏的 Git 分支胶囊：显示当前分支（或「无 Git 仓库」），点击弹出小卡片
// （刷新 / 初始化仓库 / 跳转 Git 面板）。数据来自 useGitBadge。
import { useState } from 'react'
import { GitBranch, Loader2, PanelRight, Plus, RefreshCw } from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'
import { dockApi } from './api'
import { partitionStatusEntries } from './gitReviewModel'
import { useGitBadge } from './useGitBadge'

type GitStatusPillProps = {
  workdir: string
  lang: Lang
  disabled?: boolean
  onOpenGitPanel: () => void
}

export function GitStatusPill({ workdir, lang, disabled, onOpenGitPanel }: GitStatusPillProps) {
  const t = i18n[lang]
  const { state, loading, applyMutationState, refresh } = useGitBadge(workdir)
  const [open, setOpen] = useState(false)
  const [initBusy, setInitBusy] = useState(false)
  const [actionError, setActionError] = useState('')

  const isRepo = state?.status === 'ready'
  const label = state === null ? 'Git' : isRepo ? state.head || 'HEAD' : t.dockGitPillNoRepo

  const handleRefresh = async () => {
    setActionError('')
    try {
      await refresh()
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleInit = async () => {
    if (initBusy || !workdir) return
    setInitBusy(true)
    setActionError('')
    try {
      const result = await dockApi.gitInit(workdir)
      if (!result.ok) {
        throw new Error(result.message || result.stderr || t.dockGitInitFailed)
      }
      applyMutationState(result.state)
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err))
    } finally {
      setInitBusy(false)
    }
  }

  const counts = isRepo ? partitionStatusEntries(state.entries) : null
  const untrackedCount = isRepo ? state.entries.filter((entry) => entry.untracked).length : 0

  return (
    <div className="relative shrink-0 self-center" data-tauri-drag-region="false">
      <button
        type="button"
        onClick={() => setOpen((prev) => !prev)}
        onMouseDown={(event) => event.preventDefault()}
        disabled={disabled}
        className={`chat-composer-status-item text-[12px] font-medium ${
          open ? 'bg-black/[0.05] dark:bg-white/[0.07]' : ''
        } disabled:cursor-default disabled:opacity-50`}
        aria-expanded={open}
        aria-haspopup="dialog"
        title="Git"
      >
        <GitBranch strokeWidth={1.75} className="!text-emerald-600 dark:!text-emerald-400" />
        <span className="min-w-0 max-w-[140px] truncate">{label}</span>
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-30" onClick={() => setOpen(false)} aria-hidden />
          <div
            className="chat-motion-popover kv-menu absolute bottom-full left-0 z-40 mb-2 w-[248px]"
            role="dialog"
            data-tauri-drag-region="false"
          >
            <div className="flex items-center gap-1.5 border-b border-neutral-200/70 px-3 py-2 dark:border-neutral-700/60">
              <GitBranch size={13} strokeWidth={1.9} className="shrink-0 text-neutral-500 dark:text-neutral-400" />
              <span className="text-[12px] font-semibold text-neutral-800 dark:text-neutral-100">Git</span>
              <button
                type="button"
                onClick={() => void handleRefresh()}
                disabled={loading}
                className="ml-auto rounded-md p-1 text-neutral-400 transition-colors hover:bg-neutral-200/70 hover:text-neutral-600 disabled:opacity-50 dark:hover:bg-neutral-700/70 dark:hover:text-neutral-300"
                title={t.dockRefresh}
                aria-label={t.dockRefresh}
              >
                {loading ? (
                  <Loader2 size={13} strokeWidth={1.9} className="animate-spin" />
                ) : (
                  <RefreshCw size={13} strokeWidth={1.9} />
                )}
              </button>
            </div>

            {actionError && (
              <div className="border-b border-neutral-200/70 px-3 py-2 text-[11px] leading-4 text-red-600 dark:border-neutral-700/60 dark:text-red-400">
                {actionError}
              </div>
            )}

            {!isRepo ? (
              <div className="p-1.5">
                <div className="px-1.5 py-2 text-[12px] text-neutral-500 dark:text-neutral-400">
                  {!workdir ? t.dockNoWorkdir : state === null ? t.dockLoading : t.dockGitNotRepo}
                </div>
                <button
                  type="button"
                  onClick={() => void handleInit()}
                  disabled={initBusy || !workdir}
                  className="kv-menu-row text-neutral-800 transition-colors hover:bg-neutral-100 disabled:opacity-50 dark:text-neutral-200 dark:hover:bg-neutral-800"
                >
                  {initBusy ? (
                    <Loader2 size={14} strokeWidth={1.8} className="shrink-0 animate-spin" />
                  ) : (
                    <Plus size={14} strokeWidth={1.8} className="shrink-0" />
                  )}
                  <span className="min-w-0 flex-1">{t.dockGitInitRepo}</span>
                </button>
              </div>
            ) : (
              <div className="p-1.5">
                <div className="space-y-1 px-1.5 py-1.5 text-[12px] text-neutral-600 dark:text-neutral-300">
                  <div className="flex items-center gap-1.5">
                    <span className="shrink-0 text-neutral-400 dark:text-neutral-500">{t.dockGitBranchLabel}</span>
                    <span className="min-w-0 truncate font-medium text-neutral-800 dark:text-neutral-100">
                      {state.head || 'HEAD'}
                    </span>
                    {state.upstream && (state.ahead > 0 || state.behind > 0) && (
                      <span className="shrink-0 text-[11px] text-neutral-400 dark:text-neutral-500">
                        {state.ahead > 0 ? `↑${state.ahead}` : ''}
                        {state.behind > 0 ? `↓${state.behind}` : ''}
                      </span>
                    )}
                  </div>
                  {counts && (
                    <div className="text-[11px] text-neutral-400 dark:text-neutral-500">
                      {`${t.dockGitStaged} ${counts.staged.length} · ${t.dockGitUnstaged} ${counts.unstaged.length} · ${t.dockGitUntracked} ${untrackedCount}`}
                      {counts.conflicted.length > 0 && (
                        <span className="text-red-500 dark:text-red-400">{` · ${t.dockGitConflicted} ${counts.conflicted.length}`}</span>
                      )}
                    </div>
                  )}
                </div>
                <button
                  type="button"
                  onClick={() => {
                    setOpen(false)
                    onOpenGitPanel()
                  }}
                  className="kv-menu-row text-neutral-800 transition-colors hover:bg-neutral-100 dark:text-neutral-200 dark:hover:bg-neutral-800"
                >
                  <PanelRight size={14} strokeWidth={1.8} className="shrink-0" />
                  <span className="min-w-0 flex-1">{t.dockGitOpenPanel}</span>
                </button>
              </div>
            )}
          </div>
        </>
      )}
    </div>
  )
}
