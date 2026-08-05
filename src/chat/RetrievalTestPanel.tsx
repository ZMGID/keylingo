// 检索测试台（D1）：选库 + 输入查询 → 走与 knowledge_search 完全相同的检索核心，
// 分阶段展示向量/关键词命中、融合、重排(含降级)、阈值/去重淘汰与耗时。
import { useState } from 'react'
import { AlertCircle, Check, Copy, Search } from 'lucide-react'
import { copyToClipboard } from '../utils/clipboard'
import {
  kbRetrievalTest,
  type KnowledgeLibrary,
  type RetrievalResponse,
  type RetrievalCandidate,
  type RetrievalDecision,
} from './knowledgeBase'

const DECISION_LABEL: Record<RetrievalDecision, string> = {
  kept: '入选',
  duplicate: '去重',
  below_threshold: '低于阈值',
  truncated: '截断',
}

const DECISION_CLASS: Record<RetrievalDecision, string> = {
  kept: 'bg-emerald-100 text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300',
  duplicate: 'bg-amber-100 text-amber-700 dark:bg-amber-950/40 dark:text-amber-300',
  below_threshold: 'bg-neutral-200 text-neutral-600 dark:bg-neutral-800 dark:text-neutral-400',
  truncated: 'bg-neutral-200 text-neutral-600 dark:bg-neutral-800 dark:text-neutral-400',
}

function num(n: number | undefined, digits = 4): string {
  return n == null ? '—' : n.toFixed(digits)
}

// 片段单元格：hover 显示复制按钮，点击复制原文并给对勾反馈。
function SnippetCell({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  const handleCopy = () => {
    void copyToClipboard(text).then((ok) => {
      if (!ok) return
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1500)
    })
  }
  return (
    <td className="min-w-[240px] px-2.5 py-2 text-neutral-600 dark:text-neutral-400">
      <div className="group/snip flex items-start gap-1.5">
        <span className="line-clamp-2 flex-1">{text}</span>
        <button
          type="button"
          onClick={handleCopy}
          className="shrink-0 rounded p-1 text-neutral-400 opacity-0 transition-opacity hover:bg-black/[0.05] hover:text-neutral-600 focus-visible:opacity-100 group-hover/snip:opacity-100 dark:hover:bg-white/[0.08] dark:hover:text-neutral-200"
          title={copied ? '已复制' : '复制片段'}
          aria-label={copied ? '已复制' : '复制片段'}
        >
          {copied ? <Check size={13} className="text-emerald-500" /> : <Copy size={13} />}
        </button>
      </div>
    </td>
  )
}

function rank(n: number | undefined): string {
  return n == null ? '—' : String(n + 1)
}

export function RetrievalTestPanel({ libraries }: { libraries: KnowledgeLibrary[] }) {
  const [selected, setSelected] = useState<string[]>([])
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<RetrievalResponse | null>(null)

  const toggle = (id: string) =>
    setSelected((cur) => (cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id]))

  const run = async () => {
    if (!query.trim() || selected.length === 0) return
    setBusy(true)
    setError(null)
    try {
      setResult(await kbRetrievalTest(selected, query.trim()))
    } catch (e) {
      setError(String(e))
      setResult(null)
    } finally {
      setBusy(false)
    }
  }

  const cands = result?.candidates ?? []

  return (
    <div key="test" className="chat-motion-tab-in mt-5 space-y-4">
      {libraries.length === 0 ? (
        <p className="text-[13px] text-neutral-500 dark:text-neutral-400">先创建并索引一个知识库，再来测试检索。</p>
      ) : (
        <>
          {/* 选库 */}
          <div className="flex flex-wrap gap-2">
            {libraries.map((l) => (
              <button
                key={l.id}
                type="button"
                onClick={() => toggle(l.id)}
                data-tauri-drag-region="false"
                className={`rounded-full border px-3 py-1 text-[12px] transition-colors ${
                  selected.includes(l.id)
                    ? 'border-[#C56646] bg-[#C56646]/10 text-[#C56646] dark:border-[#E39A78] dark:text-[#E39A78]'
                    : 'border-neutral-200 text-neutral-600 hover:border-neutral-300 dark:border-neutral-700 dark:text-neutral-300'
                }`}
              >
                {l.name}
                <span className="ml-1.5 text-[10.5px] tabular-nums text-neutral-400">{l.chunkCount}</span>
              </button>
            ))}
          </div>

          {/* 查询 */}
          <div className="flex items-stretch gap-2">
            <textarea
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) run()
              }}
              placeholder="输入查询…（⌘/Ctrl+Enter 运行）"
              rows={2}
              data-tauri-drag-region="false"
              className="min-w-0 flex-1 resize-none rounded-lg border border-neutral-200 bg-white px-3 py-2 text-[13px] leading-relaxed outline-none focus:border-[#C56646] dark:border-neutral-700 dark:bg-neutral-900"
            />
            <button
              type="button"
              onClick={run}
              disabled={busy || !query.trim() || selected.length === 0}
              data-tauri-drag-region="false"
              className="flex shrink-0 items-center gap-1.5 self-stretch rounded-lg bg-[#C56646] px-4 text-[13px] font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-40"
            >
              <Search size={14} />
              {busy ? '检索中…' : '运行'}
            </button>
          </div>

          {error && (
            <div className="flex items-center gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-[12px] text-red-700 dark:border-red-900/60 dark:bg-red-950/30 dark:text-red-300">
              <AlertCircle size={14} className="shrink-0" />
              <span className="min-w-0 flex-1 break-words">{error}</span>
            </div>
          )}

          {result && (
            <div className="space-y-3">
              {/* 概览：配置 + 耗时 + rerank 状态 */}
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[11.5px] text-neutral-500 dark:text-neutral-400">
                <span>
                  候选 {result.effectiveConfig.candidateK} · 上下文 {result.effectiveConfig.contextTopK} · 权重 向量
                  {result.effectiveConfig.weightVector}/关键词{result.effectiveConfig.weightKeyword}
                  {result.effectiveConfig.minScore > 0 && <> · 阈值 {result.effectiveConfig.minScore}</>}
                </span>
                <span className="tabular-nums">
                  嵌入 {result.timings.embedMs}ms · 检索 {result.timings.searchMs}ms · 重排 {result.timings.rerankMs}ms · 合计{' '}
                  {result.timings.totalMs}ms
                </span>
                <RerankBadge status={result.rerankStatus} />
              </div>

              {cands.length === 0 ? (
                <p className="text-[13px] text-neutral-500 dark:text-neutral-400">未召回任何候选（知识库中没有相关内容）。</p>
              ) : (
                <div className="overflow-x-auto rounded-lg border border-neutral-200 dark:border-neutral-800">
                  <table className="w-full min-w-[760px] text-left text-[11.5px]">
                    <thead className="bg-neutral-50 text-neutral-500 dark:bg-neutral-900/50 dark:text-neutral-400">
                      <tr className="whitespace-nowrap">
                        <th className="px-2.5 py-2 font-medium">状态</th>
                        <th className="px-2.5 py-2 font-medium">来源</th>
                        <th className="px-2.5 py-2 text-right font-medium">向量#</th>
                        <th className="px-2.5 py-2 text-right font-medium">向量距离</th>
                        <th className="px-2.5 py-2 text-right font-medium">关键词#</th>
                        <th className="px-2.5 py-2 text-right font-medium">融合分</th>
                        <th className="px-2.5 py-2 text-right font-medium">重排分</th>
                        <th className="px-2.5 py-2 font-medium">片段</th>
                      </tr>
                    </thead>
                    <tbody>
                      {cands.map((c: RetrievalCandidate) => (
                        <tr
                          key={`${c.kbId}:${c.chunkId}`}
                          className="border-t border-neutral-100 align-top dark:border-neutral-800/70"
                        >
                          <td className="whitespace-nowrap px-2.5 py-2">
                            <span className={`inline-block rounded px-1.5 py-0.5 text-[10.5px] ${DECISION_CLASS[c.decision]}`}>
                              {c.finalRank != null ? `#${c.finalRank + 1} ` : ''}
                              {DECISION_LABEL[c.decision]}
                            </span>
                          </td>
                          <td className="px-2.5 py-2 text-neutral-700 dark:text-neutral-300">
                            <div
                              className="max-w-[9rem] truncate"
                              title={c.docName + (c.headingPath ? ` — ${c.headingPath}` : '')}
                            >
                              {c.docName}
                              {c.headingPath && <span className="text-neutral-400"> — {c.headingPath}</span>}
                            </div>
                          </td>
                          <td className="whitespace-nowrap px-2.5 py-2 text-right tabular-nums">{rank(c.vectorRank)}</td>
                          <td className="whitespace-nowrap px-2.5 py-2 text-right tabular-nums">{num(c.vectorDistance)}</td>
                          <td className="whitespace-nowrap px-2.5 py-2 text-right tabular-nums">{rank(c.keywordRank)}</td>
                          <td className="whitespace-nowrap px-2.5 py-2 text-right tabular-nums">{num(c.fusedScore, 5)}</td>
                          <td className="whitespace-nowrap px-2.5 py-2 text-right tabular-nums">{num(c.rerankScore)}</td>
                          <SnippetCell text={c.text} />
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          )}
        </>
      )}
    </div>
  )
}

function RerankBadge({ status }: { status: RetrievalResponse['rerankStatus'] }) {
  if (status.state === 'off') return <span className="text-neutral-400">重排：关</span>
  if (status.state === 'ok') return <span className="text-emerald-600 dark:text-emerald-400">重排：已应用</span>
  return (
    <span className="text-red-600 dark:text-red-400" title={status.error}>
      重排：失败（已降级为融合序）
    </span>
  )
}
