// 轻量 Diff 渲染：文件分组卡片 + hunk 行级着色。大 patch 按文件默认折叠（点击展开），
// 不做 IntersectionObserver —— 折叠态本来就不渲染 hunk 行，成本已可控。
import { useMemo, useState } from 'react'
import { ChevronDown, ChevronRight, FileCode } from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'
import { countDiffStats, parseDiff, type DiffFile } from './diffParse'

type DiffViewProps = {
  patch: string
  truncated?: boolean
  lang: Lang
  /** patch 为空时的占位文案；不传则不渲染空态。 */
  emptyText?: string
}

function DiffFileCard({ file, defaultOpen, lang }: { file: DiffFile; defaultOpen: boolean; lang: Lang }) {
  const t = i18n[lang]
  const [open, setOpen] = useState(defaultOpen)
  const { adds, dels } = useMemo(() => countDiffStats(file), [file])
  const displayPath = file.newPath || file.oldPath

  return (
    <div className="overflow-hidden rounded-md border border-neutral-200/80 dark:border-neutral-700/60">
      <button
        type="button"
        className="flex w-full items-center gap-1.5 bg-neutral-100/60 px-2 py-1.5 text-left transition-colors hover:bg-neutral-100 dark:bg-neutral-800/40 dark:hover:bg-neutral-800/70"
        onClick={() => setOpen((prev) => !prev)}
      >
        {open ? (
          <ChevronDown size={13} strokeWidth={2} className="shrink-0 text-neutral-400" />
        ) : (
          <ChevronRight size={13} strokeWidth={2} className="shrink-0 text-neutral-400" />
        )}
        <FileCode size={13} strokeWidth={1.75} className="shrink-0 text-neutral-400" />
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-neutral-700 dark:text-neutral-200">
          {file.isDeleted ? file.oldPath : displayPath}
        </span>
        {file.isNew && (
          <span className="shrink-0 rounded bg-emerald-500/15 px-1 text-[10px] text-emerald-700 dark:text-emerald-300">
            {t.dockDiffNewFile}
          </span>
        )}
        {file.isDeleted && (
          <span className="shrink-0 rounded bg-red-500/15 px-1 text-[10px] text-red-700 dark:text-red-300">
            {t.dockDiffDeletedFile}
          </span>
        )}
        {file.isBinary && (
          <span className="shrink-0 rounded bg-neutral-500/15 px-1 text-[10px] text-neutral-500 dark:text-neutral-400">
            {t.dockDiffBinary}
          </span>
        )}
        <span className="shrink-0 font-mono text-[10px]">
          {adds > 0 && <span className="text-emerald-600 dark:text-emerald-400">+{adds}</span>}
          {adds > 0 && dels > 0 && ' '}
          {dels > 0 && <span className="text-red-600 dark:text-red-400">−{dels}</span>}
        </span>
      </button>
      {open && !file.isBinary && (
        <div className="custom-scrollbar overflow-x-auto">
          {file.hunks.map((hunk, hunkIndex) => (
            <div key={hunkIndex}>
              <div className="bg-sky-500/10 px-2 py-0.5 font-mono text-[10px] text-sky-700 dark:text-sky-300">
                {hunk.header}
              </div>
              <pre className="font-mono text-[11px] leading-[1.5]">
                {hunk.lines.map((line, lineIndex) => (
                  <div
                    key={lineIndex}
                    className={
                      line.type === 'add'
                        ? 'bg-emerald-500/10 text-emerald-800 dark:text-emerald-200'
                        : line.type === 'del'
                          ? 'bg-red-500/10 text-red-800 dark:text-red-200'
                          : 'text-neutral-600 dark:text-neutral-400'
                    }
                  >
                    <span className="inline-block w-4 shrink-0 select-none text-center opacity-50">
                      {line.type === 'add' ? '+' : line.type === 'del' ? '−' : ' '}
                    </span>
                    {line.content}
                  </div>
                ))}
              </pre>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

export function DiffView({ patch, truncated = false, lang, emptyText }: DiffViewProps) {
  const t = i18n[lang]
  const files = useMemo(() => parseDiff(patch), [patch])

  if (files.length === 0 && !truncated) {
    return emptyText ? (
      <div className="px-3 py-6 text-center text-[12px] text-neutral-400 dark:text-neutral-500">
        {emptyText}
      </div>
    ) : null
  }

  return (
    <div className="flex flex-col gap-2">
      {truncated && (
        <div className="rounded-md bg-amber-500/10 px-2 py-1.5 text-[11px] text-amber-700 dark:text-amber-300">
          {t.dockDiffTruncated}
        </div>
      )}
      {files.map((file, index) => (
        <DiffFileCard
          key={`${file.oldPath}->${file.newPath}:${index}`}
          file={file}
          lang={lang}
          defaultOpen={files.length <= 3}
        />
      ))}
    </div>
  )
}
