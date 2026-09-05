import { useMemo, useState } from 'react'
import { Check, Copy, Search, Terminal } from 'lucide-react'
import { copyToClipboard } from '../utils/clipboard'

export type CliReport = { version: 1; agent: 'antigravity'; command: string; output: string }
type Quota = { group: string; window: string; remaining: number; reset: string }

export function parseCliReport(value: string): CliReport | null {
  try {
    const v: unknown = JSON.parse(value)
    if (!v || typeof v !== 'object') return null
    const r = v as Record<string, unknown>
    return r.version === 1 && r.agent === 'antigravity' && typeof r.command === 'string'
      && typeof r.output === 'string' ? r as CliReport : null
  } catch { return null }
}

export function parseQuotas(output: string): Quota[] {
  const lines = output.trim().split(/\r?\n/).filter(Boolean)
  const rows: Quota[] = []
  for (const line of lines) {
    const match = line.match(/^(.+?)\s+(Weekly|Five Hour) Limit Remaining\s+(\d+(?:\.\d+)?)%\s+(\S+)\s*$/)
    if (!match || Number(match[3]) > 100 || !Number.isFinite(Date.parse(match[4]))) return []
    rows.push({ group: match[1].trim(), window: match[2] === 'Weekly' ? '每周额度' : '5 小时额度', remaining: Number(match[3]), reset: match[4] })
  }
  return rows
}

// Exact old agy quota reports only; ordinary prose and incomplete streams are unchanged.
export function normalizeLegacyCliReport(content: string): string {
  return parseQuotas(content).length ? '```kivio-cli-report\n' + JSON.stringify({
    version: 1, agent: 'antigravity', command: 'usage', output: content,
  }) + '\n```' : content
}

const titles: Record<string, string> = {
  usage: '模型配额', quota: '模型配额', help: '命令指南', skills: '可用技能',
  agents: '自定义 Agent', model: '当前模型', effort: '推理强度', config: 'CLI 配置',
  settings: 'CLI 配置', permissions: '权限配置', hooks: 'Hooks 配置', credits: 'AI Credits', changelog: '更新记录',
}

function CopyButton({ value, label = '复制原始输出' }: { value: string; label?: string }) {
  const [copied, setCopied] = useState(false)
  return <button type="button" aria-label={copied ? '已复制' : label} title={label}
    className="shrink-0 rounded-md p-1.5 text-neutral-400 hover:bg-black/5 hover:text-neutral-700 focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-500 dark:hover:bg-white/10 dark:hover:text-neutral-100"
    onClick={async () => { if (await copyToClipboard(value)) setCopied(true) }}>
    {copied ? <Check size={14} /> : <Copy size={14} />}
  </button>
}

function resetLabel(reset: string): string {
  return new Intl.DateTimeFormat('zh-CN', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }).format(new Date(reset))
}

function reportRows(output: string): { name: string; value: string }[] {
  try {
    const value: unknown = JSON.parse(output)
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      return Object.entries(value).map(([name, v]) => ({ name, value: typeof v === 'string' ? v : JSON.stringify(v, null, 2) }))
    }
  } catch { /* native text format */ }
  return output.split(/\r?\n/).filter(line => line.trim()).map(line => {
    const match = line.match(/^(.+?)(?:\t+| — |:\s+)(.*)$/)
    return match ? { name: match[1], value: match[2].replace(/\t/g, ' · ') } : { name: line, value: '' }
  })
}

export function CliCommandReport({ report }: { report: CliReport }) {
  const [query, setQuery] = useState('')
  const quota = useMemo(() => ['usage', 'quota'].includes(report.command) ? parseQuotas(report.output) : [], [report])
  const rows = useMemo(() => reportRows(report.output), [report.output])
  const searchable = ['help', 'skills', 'agents'].includes(report.command)
  const visible = rows.filter(row => `${row.name} ${row.value}`.toLowerCase().includes(query.toLowerCase()))
  const groups = [...new Set(quota.map(row => row.group))]
  return <section aria-label={titles[report.command] ?? `/${report.command}`} className="not-prose my-3 w-full max-w-[600px] overflow-hidden rounded-xl border border-black/[0.07] bg-[var(--bg-primary)] text-sm text-neutral-800 dark:border-white/10 dark:text-neutral-100">
    <header className="flex items-center gap-2 border-b border-black/5 px-4 py-3 dark:border-white/10">
      <Terminal size={16} className="text-neutral-400" />
      <h3 className="m-0 flex-1 text-sm font-semibold">{titles[report.command] ?? '命令结果'}</h3>
      <span className="font-mono text-[11px] text-neutral-400">/{report.command}</span>
      <CopyButton value={report.output} />
    </header>
    {quota.length > 0 ? <div className="space-y-5 px-4 py-4">
      {groups.map(group => <div key={group}>
        <h4 className="mb-3 text-xs font-semibold text-neutral-500 dark:text-neutral-400">{group}</h4>
        <div className="grid grid-cols-1 gap-4 min-[460px]:grid-cols-2">
          {quota.filter(row => row.group === group).map(row => <div key={row.window}>
            <div className="mb-2 flex items-baseline justify-between gap-2">
              <span className="text-xs">{row.window}</span>
              <span className="text-xs text-neutral-400">剩余 <strong className="ml-1 text-lg font-semibold tabular-nums text-neutral-800 dark:text-neutral-100">{row.remaining}<span className="text-xs">%</span></strong></span>
            </div>
            <div role="progressbar" aria-label={`${group} ${row.window}剩余`} aria-valuenow={row.remaining} aria-valuemin={0} aria-valuemax={100}
              className="h-1.5 overflow-hidden rounded-full bg-neutral-100 dark:bg-neutral-800">
              <div className={`h-full rounded-full ${row.remaining <= 10 ? 'bg-amber-500' : 'bg-emerald-500 dark:bg-emerald-400'}`} style={{ width: `${row.remaining}%` }} />
            </div>
            <p className="mb-0 mt-2 text-[11px] text-neutral-400">重置于 <time dateTime={row.reset} title={row.reset}>{resetLabel(row.reset)}</time></p>
          </div>)}
        </div>
      </div>)}
      <p className="m-0 text-[10px] text-neutral-400">查询时的配额快照 · 重置时间按本地时区显示</p>
    </div> : <>
      {searchable && <div className="flex items-center gap-2 border-b border-black/5 px-4 py-2 dark:border-white/10">
        <Search size={14} className="text-neutral-400" />
        <input aria-label="筛选命令结果" placeholder="搜索名称或说明…" value={query} onChange={event => setQuery(event.target.value)} className="min-w-0 flex-1 bg-transparent py-1 text-xs outline-none placeholder:text-neutral-400" />
        <span className="text-[11px] tabular-nums text-neutral-400">{visible.length} 项</span>
      </div>}
      <div className="custom-scrollbar max-h-96 overflow-y-auto px-4">
        {visible.map((row, index) => <div key={`${row.name}:${index}`} className="flex gap-2 border-b border-black/5 py-3 last:border-0 dark:border-white/5">
          <div className="min-w-0 flex-1">
            <div className={`break-words ${report.command === 'model' || report.command === 'effort' ? 'text-base font-semibold' : 'text-xs font-medium'}`}>{row.name}</div>
            {row.value && <div className="mt-1 whitespace-pre-wrap break-words text-xs leading-relaxed text-neutral-500 dark:text-neutral-400">{row.value}</div>}
          </div>
          {searchable && (report.command !== 'help' || row.name.startsWith('/')) && <CopyButton value={report.command === 'skills' && !row.name.startsWith('/') ? `/${row.name}` : row.name} label={`复制 ${row.name}`} />}
        </div>)}
        {!visible.length && <p className="py-4 text-center text-xs text-neutral-400">没有匹配的结果</p>}
      </div>
    </>}
    <details className="border-t border-black/5 bg-black/[0.015] dark:border-white/10 dark:bg-white/[0.02]">
      <summary className="cursor-pointer px-4 py-2 text-[11px] text-neutral-400">原始输出</summary>
      <pre className="custom-scrollbar m-0 max-h-56 overflow-auto whitespace-pre-wrap break-words px-4 pb-3 text-[11px] text-neutral-500">{report.output}</pre>
    </details>
  </section>
}
