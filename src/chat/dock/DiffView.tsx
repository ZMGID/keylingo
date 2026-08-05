// 轻量 Diff 渲染：文件分组卡片 + hunk 行级着色 + 行号 + 词级差异高亮。
// 大 patch 按文件默认折叠（点击展开），不做 IntersectionObserver ——
// 折叠态本来就不渲染 hunk 行，成本已可控。
import { useMemo, useState } from 'react'
import { ChevronDown, ChevronRight, FileCode } from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'
import { countDiffStats, intralineRanges, parseDiff, type DiffFile, type DiffLine } from './diffParse'

type DiffViewProps = {
  patch: string
  truncated?: boolean
  lang: Lang
  /** patch 为空时的占位文案；不传则不渲染空态。 */
  emptyText?: string
}

/** 行内容：有词级差异区间时中段加深色底。 */
function LineContent({ line, range }: { line: DiffLine; range?: [number, number] }) {
  if (!range) return <>{line.content || ' '}</>
  const [start, end] = range
  const mark = line.type === 'add' ? 'rounded-[2px] bg-emerald-500/25' : 'rounded-[2px] bg-red-500/25'
  return (
    <>
      {line.content.slice(0, start)}
      <span className={mark}>{line.content.slice(start, end)}</span>
      {line.content.slice(end) || ' '}
    </>
  )
}

function DiffFileCard({ file, defaultOpen, lang }: { file: DiffFile; defaultOpen: boolean; lang: Lang }) {
  const t = i18n[lang]
  const [open, setOpen] = useState(defaultOpen)
  const { adds, dels } = useMemo(() => countDiffStats(file), [file])
  const hunkRanges = useMemo(() => file.hunks.map((hunk) => intralineRanges(hunk.lines)), [file])
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
              {/* 自合成的伪 diff 头（@@ @@）没信息量，只有真 @@ -a +b 头才显示分隔条。 */}
              {/^@@ -\d/.test(hunk.header) && hunkIndex > 0 && (
                <div className="bg-sky-500/10 px-2 py-0.5 font-mono text-[10px] text-sky-700 dark:text-sky-300">
                  {hunk.header}
                </div>
              )}
              {/* w-max + min-w-full：横向滚动时行底色跟着最长行铺满，不然短行右侧缺色。 */}
              <pre className="w-max min-w-full font-mono text-[11px] leading-[1.6]">
                {hunk.lines.map((line, lineIndex) => (
                  <div
                    key={lineIndex}
                    className={
                      line.type === 'add'
                        ? 'flex bg-emerald-500/10 text-emerald-900 dark:text-emerald-100'
                        : line.type === 'del'
                          ? 'flex bg-red-500/10 text-red-900 dark:text-red-100'
                          : 'flex text-neutral-600 dark:text-neutral-400'
                    }
                  >
                    <span
                      className={`w-9 shrink-0 select-none border-r pr-1.5 text-right tabular-nums ${
                        line.type === 'add'
                          ? 'border-emerald-500/20 text-emerald-700/60 dark:text-emerald-300/50'
                          : line.type === 'del'
                            ? 'border-red-500/20 text-red-700/60 dark:text-red-300/50'
                            : 'border-neutral-500/15 text-neutral-400/80 dark:text-neutral-500/80'
                      }`}
                    >
                      {(line.type === 'del' ? line.oldNo : line.newNo) ?? ''}
                    </span>
                    <span className="min-w-0 flex-1 whitespace-pre pl-2">
                      <LineContent line={line} range={hunkRanges[hunkIndex]?.get(lineIndex)} />
                    </span>
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
