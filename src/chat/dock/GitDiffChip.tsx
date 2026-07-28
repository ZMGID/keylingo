// 状态条右端的 diff 徽标：绿 +adds / 红 −dels，无改动时不渲染，点击进 Git 面板。
// 与工具栏的 GitStatusPill 各自实例化 useGitBadge（毫秒级查询，换取互不耦合）。
import { i18n, type Lang } from '../../settings/i18n'
import { useGitBadge } from './useGitBadge'

type GitDiffChipProps = {
  workdir: string
  lang: Lang
  onOpenGitPanel: () => void
}

export function GitDiffChip({ workdir, lang, onOpenGitPanel }: GitDiffChipProps) {
  const t = i18n[lang]
  const { state, diffStat } = useGitBadge(workdir)

  if (state?.status !== 'ready' || !diffStat || diffStat.filesChanged === 0) return null

  return (
    <button
      type="button"
      className="chat-composer-status-item ml-auto text-[12px] tabular-nums"
      onClick={onOpenGitPanel}
      title={t.dockGitOpenPanel}
    >
      <span className="font-medium text-emerald-600 dark:text-emerald-400">
        +{diffStat.additions.toLocaleString()}
      </span>
      <span className="font-medium text-red-500 dark:text-red-400">
        −{diffStat.deletions.toLocaleString()}
      </span>
    </button>
  )
}
