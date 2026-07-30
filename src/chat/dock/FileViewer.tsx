// 文件查看器：文件树点击文件后就地预览（行号 + 轻量语法高亮，virtua 虚拟化），
// 可切换编辑（纯文本 textarea）保存。读取/保存走 dock_fs_read / dock_fs_write。
import { memo, useEffect, useState } from 'react'
import { VList } from 'virtua'
import { ExternalLink, Loader2, Pencil, X } from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'
import { Button, IconButton } from '../../components/Button'
import { highlightCode } from '../ChatMarkdown'
import { dockApi } from './api'
import { basenameOf } from './fileTreeModel'

/** 扩展名直接当语言 id 用：highlightCode 的规则表本来就按 ts/js/py/rs/css… 分支，
 *  未知扩展名落到通用规则（注释/字符串/数字），不会出错。 */
function languageForFile(name: string): string {
  const lower = name.toLowerCase()
  return lower.includes('.') ? lower.slice(lower.lastIndexOf('.') + 1) : ''
}

/** 单行渲染：只有可见行会真正执行高亮（virtua 惰性渲染子组件）。
 *  ponytail: 逐行独立高亮，跨行块注释会降级成普通文本；查看器够用。 */
const CodeLine = memo(function CodeLine({
  line,
  language,
  lineNo,
  gutterCh,
}: {
  line: string
  language: string
  lineNo: number
  gutterCh: number
}) {
  return (
    <div className="flex font-mono text-[11.5px] leading-5">
      <span
        className="shrink-0 select-none pr-3 text-right tabular-nums text-neutral-300 dark:text-neutral-600"
        style={{ width: `${gutterCh + 2}ch` }}
      >
        {lineNo}
      </span>
      <span className="whitespace-pre pr-4 text-neutral-800 dark:text-neutral-200">
        {line ? highlightCode(line, language) : ' '}
      </span>
    </div>
  )
})

type FileViewerProps = {
  workdir: string
  path: string
  lang: Lang
  onClose: () => void
}

export function FileViewer({ workdir, path, lang, onClose }: FileViewerProps) {
  const t = i18n[lang]
  const [content, setContent] = useState<string | null>(null)
  const [error, setError] = useState('')
  const [mode, setMode] = useState<'view' | 'edit'>('view')
  const [draft, setDraft] = useState('')
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState('')

  useEffect(() => {
    let cancelled = false
    setContent(null)
    setError('')
    setMode('view')
    setSaveError('')
    dockApi
      .fsRead(workdir, path)
      .then((result) => {
        if (!cancelled) setContent(result.content)
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err))
      })
    return () => {
      cancelled = true
    }
  }, [workdir, path])

  // Escape 关闭（编辑态交给 textarea 自己，避免误丢草稿）。
  useEffect(() => {
    if (mode !== 'view') return
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [mode, onClose])

  const save = async () => {
    setSaving(true)
    setSaveError('')
    try {
      await dockApi.fsWrite(workdir, path, draft)
      setContent(draft)
      setMode('view')
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  const language = languageForFile(path)
  const lines = content !== null ? content.split('\n') : []
  const gutterCh = String(Math.max(lines.length, 1)).length

  return (
    <div className="absolute inset-0 z-10 flex flex-col bg-[var(--theme-surface-soft)] dark:bg-[#262629]">
      {/* 头部：文件名 + 路径 + 操作 */}
      <div className="flex shrink-0 items-center gap-1.5 border-b border-neutral-200/70 px-2 py-1.5 dark:border-neutral-700/50">
        <span className="shrink-0 text-[12.5px] font-medium text-neutral-800 dark:text-neutral-100">
          {basenameOf(path)}
        </span>
        {path !== basenameOf(path) ? (
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-neutral-400 dark:text-neutral-500">
            {path}
          </span>
        ) : (
          <span className="min-w-0 flex-1" />
        )}
        {mode === 'view' && content !== null && (
          <IconButton label={t.dockViewerEdit} size="sm" variant="ghost" onClick={() => { setDraft(content); setMode('edit') }}>
            <Pencil size={13} />
          </IconButton>
        )}
        <IconButton
          label={t.dockOpen}
          size="sm"
          variant="ghost"
          onClick={() => void dockApi.fsOpenPath(workdir, path, 'open').catch(() => {})}
        >
          <ExternalLink size={13} />
        </IconButton>
        <IconButton label={t.dockViewerClose} size="sm" variant="ghost" onClick={onClose}>
          <X size={13} />
        </IconButton>
      </div>

      {/* 正文 */}
      {error ? (
        <div className="flex flex-1 items-center justify-center px-4 text-center text-[12px] text-neutral-400">
          {error}
        </div>
      ) : content === null ? (
        <div className="flex flex-1 items-center justify-center gap-2 text-[12px] text-neutral-400">
          <Loader2 size={13} className="animate-spin" />
          {t.dockLoading}
        </div>
      ) : mode === 'edit' ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <textarea
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            spellCheck={false}
            className="custom-scrollbar min-h-0 flex-1 resize-none bg-transparent px-3 py-2 font-mono text-[11.5px] leading-5 text-neutral-800 outline-none dark:text-neutral-200"
          />
          <div className="flex shrink-0 items-center gap-1.5 border-t border-neutral-200/70 px-2 py-1.5 dark:border-neutral-700/50">
            {saveError && (
              <span className="min-w-0 flex-1 truncate text-[11px] text-red-500 dark:text-red-400" title={saveError}>
                {saveError}
              </span>
            )}
            <Button size="sm" variant="ghost" className="ml-auto" disabled={saving} onClick={() => setMode('view')}>
              {t.dockViewerCancel}
            </Button>
            <Button size="sm" variant="primary" disabled={saving} onClick={() => void save()}>
              {saving ? <Loader2 size={13} className="animate-spin" /> : t.dockViewerSave}
            </Button>
          </div>
        </div>
      ) : (
        <div className="custom-scrollbar min-h-0 flex-1 overflow-x-auto py-1">
          <VList className="custom-scrollbar h-full">
            {lines.map((line, index) => (
              <CodeLine key={index} line={line} language={language} lineNo={index + 1} gutterCh={gutterCh} />
            ))}
          </VList>
        </div>
      )}
    </div>
  )
}
