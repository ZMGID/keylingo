import { type ComponentType, type ReactNode, memo, useEffect, useMemo, useRef, useState } from 'react'
import {
  AlertCircle,
  Bot,
  Brain,
  CheckCircle2,
  ChevronDown,
  CircleSlash,
  Copy,
  Download,
  Eye,
  FileCode2,
  FilePen,
  FilePlus2,
  FileSearch,
  FileText,
  FolderInput,
  FolderOpen,
  FolderPlus,
  ImagePlus,
  ListChecks,
  Loader2,
  MessageCircleQuestion,
  Plug,
  Save,
  ScrollText,
  Search,
  SquareTerminal,
  Trash2,
  Wrench,
  XCircle,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { AgentTodoItem, AgentTodoState, AgentTodoStatus, ToolCallRecord, ToolCallStatus } from './types'
import { normalizeToolCallStatus } from './toolStatus'
import { formatToolResultPreview } from './toolResultPreview'
import { toolRecordRawName as toolRawName } from './segments'
import { knowledgeSearchHits, type KbHitView } from './knowledgeBaseHits'
import { AskUserBlock } from './AskUserBlock'
import { ChatMarkdown } from './ChatMarkdown'
import { WebSearchIcon } from '../settings/NavIcons'

export interface ToolCallBlockProps {
  toolCall: ToolCallRecord
  defaultOpen?: boolean
}

interface FileMutationFile {
  path: string
  operation: string
  bytesWritten?: number
  bytes_written?: number
  additions: number
  removals: number
  diff?: string
}

interface FileMutationStructuredContent {
  ok?: boolean
  operation: string
  targetTouched?: boolean
  target_touched?: boolean
  resolvedPath?: string | null
  resolved_path?: string | null
  files?: FileMutationFile[]
  bytesWritten?: number
  bytes_written?: number
  additions?: number
  removals?: number
  diff?: string
  warnings?: string[]
  diagnostics?: unknown[]
}

function compactText(text: string, max = 220): string {
  const cleaned = text.replace(/\s+/g, ' ').trim()
  if (cleaned.length <= max) return cleaned
  return `${cleaned.slice(0, max).trimEnd()}...`
}

function previewValue(value: unknown, max = 220): string {
  if (value == null) return ''
  if (typeof value === 'string') return compactText(value, max)
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  try {
    return compactText(JSON.stringify(value, null, 2), max)
  } catch {
    return compactText(String(value), max)
  }
}

function parsedArguments(toolCall: ToolCallRecord): Record<string, unknown> | null {
  const value = toolCall.arguments ?? toolCall.args ?? toolCall.input
  if (!value) return null
  if (typeof value === 'object' && !Array.isArray(value)) return value as Record<string, unknown>
  if (typeof value !== 'string') return null
  try {
    const parsed = JSON.parse(value)
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : null
  } catch {
    return null
  }
}

function toolGlyph(toolCall: ToolCallRecord): LucideIcon | ComponentType<{ size?: number; strokeWidth?: number; className?: string }> {
  const raw = toolRawName(toolCall)
  switch (raw) {
    case 'read':
    case 'read_file':
      return FileText
    case 'write':
    case 'write_file':
      return FilePlus2
    case 'edit':
    case 'edit_file':
      return FilePen
    case 'bash':
    case 'run_command':
      return SquareTerminal
    case 'grep':
    case 'search_files':
      return Search
    case 'find':
    case 'glob':
    case 'glob_files':
      return FileSearch
    case 'ls':
    case 'list_dir':
      return FolderOpen
    case 'delete':
      return Trash2
    case 'move':
      return FolderInput
    case 'copy':
      return Copy
    case 'create_dir':
      return FolderPlus
    case 'stat':
    case 'stat_path':
      return FileSearch
    case 'run_python':
      return FileCode2
    case 'web_search':
      return WebSearchIcon
    case 'web_fetch':
      return Download
    case 'skill':
    case 'skill_activate':
      return ScrollText
    case 'todo_write':
    case 'todo_update':
      return ListChecks
    case 'memory_read':
    case 'memory_search':
    case 'memory_modify':
      return Brain
    case 'save_assistant':
      return Save
    case 'mixer_vision':
      return Eye
    case 'mixer_generate_image':
      return ImagePlus
    case 'agent':
      return Bot
    case 'ask_user':
      return MessageCircleQuestion
    default:
      break
  }
  const isMcp =
    toolCall.source === 'mcp' ||
    ((toolCall.server_name || toolCall.serverName) &&
      toolCall.source !== 'native' &&
      toolCall.source !== 'skill' &&
      toolCall.source !== 'mixer')
  if (isMcp) return Plug
  return Wrench
}

function isAskUserTool(toolCall: ToolCallRecord): boolean {
  return toolRawName(toolCall) === 'ask_user'
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}

function todoStatusLabel(status?: string): string {
  switch (status) {
    case 'completed':
      return '已完成'
    case 'in_progress':
      return '进行中'
    case 'pending':
      return '待处理'
    default:
      return status ? compactText(status, 24) : ''
  }
}

function normalizeTodoItem(value: unknown): AgentTodoItem | null {
  const item = objectValue(value)
  if (!item) return null
  const id = typeof item.id === 'string' ? item.id.trim() : ''
  const content = typeof item.content === 'string' ? item.content.trim() : ''
  const status = typeof item.status === 'string' ? item.status : ''
  if (!id && !content) return null
  return {
    id,
    content,
    status: (status === 'completed' || status === 'in_progress' || status === 'pending'
      ? status
      : 'pending') as AgentTodoStatus,
  }
}

function normalizeTodoItems(value: unknown): AgentTodoItem[] {
  if (!Array.isArray(value)) return []
  return value
    .map((item) => normalizeTodoItem(item))
    .filter((item): item is AgentTodoItem => Boolean(item))
}

function todoCounts(items?: AgentTodoItem[]): { completed: number; total: number } | null {
  if (!items?.length) return null
  return {
    completed: items.filter((item) => item.status === 'completed').length,
    total: items.length,
  }
}

function formatTodoCounts(items?: AgentTodoItem[]): string {
  const counts = todoCounts(items)
  return counts ? `${counts.completed}/${counts.total}` : ''
}

function structuredTodoState(toolCall: ToolCallRecord): AgentTodoState | null {
  const structured = objectValue(toolCall.structured_content ?? toolCall.structuredContent)
  const todoState = objectValue(structured?.todoState)
  if (!todoState) return null
  return {
    items: normalizeTodoItems(todoState.items),
    updated_at: typeof todoState.updated_at === 'number' ? todoState.updated_at : undefined,
    updatedAt: typeof todoState.updatedAt === 'number' ? todoState.updatedAt : undefined,
  }
}

function stringArrayValue(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return value.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
}

function stringValue(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

interface SubagentView {
  name: string
  agentType?: string
  model?: string
  depth: number
  status: string
  result?: string
  error?: string
  preview?: string
  steps: string[]
  usage?: { inputTokens?: number; outputTokens?: number; totalTokens?: number }
}

/** Parse the optional token usage from a final sub-agent structured result.
 *  Live `subagentProgress` has no usage; only the completed `{type:"subagent"}`
 *  payload carries it. */
function subagentUsage(value: unknown): SubagentView['usage'] {
  const usage = objectValue(value)
  if (!usage) return undefined
  const input = typeof usage.inputTokens === 'number' ? usage.inputTokens : undefined
  const output = typeof usage.outputTokens === 'number' ? usage.outputTokens : undefined
  const total = typeof usage.totalTokens === 'number' ? usage.totalTokens : undefined
  if (input == null && output == null && total == null) return undefined
  return { inputTokens: input, outputTokens: output, totalTokens: total }
}

/** Compact token count, e.g. 1234 → "1.2k", 999 → "999". */
function formatTokenCount(value?: number): string {
  if (value == null || !Number.isFinite(value) || value < 0) return ''
  if (value < 1000) return String(Math.round(value))
  const thousands = value / 1000
  return `${thousands >= 100 ? Math.round(thousands) : thousands.toFixed(1)}k`
}

/** One-line token summary like `↑1.2k ↓340 · 1.5k tokens`. Empty when no usage. */
function subagentUsageLine(usage: SubagentView['usage']): string {
  if (!usage) return ''
  const parts: string[] = []
  const input = formatTokenCount(usage.inputTokens)
  const output = formatTokenCount(usage.outputTokens)
  if (input) parts.push(`↑${input}`)
  if (output) parts.push(`↓${output}`)
  const total = formatTokenCount(usage.totalTokens)
  const head = parts.join(' ')
  if (head && total) return `${head} · ${total} tokens`
  if (head) return head
  if (total) return `${total} tokens`
  return ''
}

/** Parse sub-agent state (P3) from a tool record's structured content: either
 *  the final `{ type: "subagent", ... }` result or the live `subagentProgress`
 *  merged in from `chat-subagent` events. */
function structuredSubagent(toolCall: ToolCallRecord): SubagentView | null {
  const structured = objectValue(toolCall.structured_content ?? toolCall.structuredContent)
  if (!structured) return null
  const isFinal = structured.type === 'subagent'
  const progress = objectValue(structured.subagentProgress)
  if (!isFinal && !progress) return null
  return {
    name: stringValue(progress?.name) || stringValue(structured.name) || 'subagent',
    agentType: stringValue(structured.agentType) || undefined,
    model: stringValue(structured.model) || stringValue(progress?.model) || undefined,
    depth: numberValue(progress?.depth ?? structured.depth),
    status: stringValue(progress?.status) || stringValue(structured.status) || 'running',
    result: stringValue(structured.result) || undefined,
    error: stringValue(structured.error) || undefined,
    preview: stringValue(progress?.preview) || undefined,
    steps: stringArrayValue(progress?.steps),
    usage: subagentUsage(structured.usage),
  }
}

function isSubAgentRecord(toolCall: ToolCallRecord): boolean {
  if (structuredSubagent(toolCall)) return true
  return toolCall.source === 'native' && toolRawName(toolCall) === 'agent'
}

/** Sub-agent type / display name fall back to the spawn arguments while the run
 *  is live (structured content only carries agentType in the final result). */
function subagentAgentType(view: SubagentView | null, args: Record<string, unknown> | null): string {
  return view?.agentType || stringValue(args?.subagent_type) || ''
}

function subagentName(view: SubagentView | null, args: Record<string, unknown> | null): string {
  return (
    view?.name ||
    stringValue(args?.name) ||
    stringValue(args?.subagent_type) ||
    'subagent'
  )
}

function subagentPrompt(args: Record<string, unknown> | null): string {
  return stringValue(args?.prompt)
}

function subagentStatusLine(view: SubagentView | null, status: ToolCallStatus): string {
  if (status === 'completed') return '已完成'
  if (status === 'error') return view?.error ? compactText(view.error, 160) : '运行失败'
  if (status === 'cancelled') return '已取消'
  if (status === 'running') {
    const lastStep = view?.steps?.length ? view.steps[view.steps.length - 1] : ''
    if (lastStep) return compactText(lastStep, 160)
    if (view?.preview) return compactText(view.preview, 160)
    return '运行中…'
  }
  return '准备运行…'
}

function CardEyebrow({ running = false }: { running?: boolean }) {
  // 品牌标记：7×7 实心方块（黑/白随主题）。运行中像终端光标一样步进闪烁，呼应官网运行轨迹方块。
  return (
    <span
      className={`inline-block h-[7px] w-[7px] shrink-0 bg-neutral-900 dark:bg-neutral-100 ${running ? 'kv-card-eyebrow--running' : ''}`}
      aria-hidden="true"
    />
  )
}

/** 卡片状态标记：完成用中性灰 ✓（不用应用全局那枚 terracotta ✓，保持 STYLE.md 纯灰阶
 *  调色板）；出错/取消/跳过沿用 StatusIcon 的中性图标；运行态不在此渲染（由 eyebrow 闪烁 +
 *  状态文字流光表达）。 */
function CardStatusMark({ status }: { status: ToolCallStatus }) {
  if (status === 'completed' || status === 'success') {
    return <CheckCircle2 size={13} strokeWidth={1.9} className="shrink-0 text-neutral-500 dark:text-neutral-400" />
  }
  return <StatusIcon status={status} />
}

/** Mono-gray metadata chip in a consult-card header (agent type, model, count…). */
function CardChip({ children, tabular = false }: { children: ReactNode; tabular?: boolean }) {
  return (
    <span
      className={`shrink-0 font-mono text-[11px] text-neutral-400 dark:text-neutral-500${
        tabular ? ' tabular-nums' : ''
      }`}
    >
      {children}
    </span>
  )
}

/** Labeled section in a consult-card body (Task / Result / Question / Query…). */
function CardSection({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <div className="mb-0.5 font-mono text-[9.5px] uppercase tracking-[0.16em] text-neutral-400 dark:text-neutral-500">
        {label}
      </div>
      {children}
    </div>
  )
}

/** Shared boxed "consult card" shell for standalone delegations/consultations
 *  (SUBAGENT / ADVISOR / KNOWLEDGE). Owns the eyebrow + uppercase mono label +
 *  status mark + status line + collapse; callers supply identity chips, metric
 *  chips, and the expandable body. Header order matches the original hand-rolled
 *  cards: eyebrow · LABEL · identity chips · status mark · metric chips ·
 *  status line · chevron. */
function ConsultCard({
  label,
  status,
  identityChips,
  metricChips,
  statusLine,
  children,
}: {
  label: string
  status: ToolCallStatus
  identityChips?: ReactNode
  metricChips?: ReactNode
  statusLine?: string
  children?: ReactNode
}) {
  const [open, setOpen] = useState(false)
  const hasBody = Boolean(children)
  const running = status === 'running'
  return (
    <div className="not-prose mb-2 rounded-md border border-neutral-200 bg-neutral-50/70 px-3 py-2 leading-5 text-neutral-500 transition-colors duration-200 hover:bg-neutral-100/70 dark:border-white/10 dark:bg-white/[0.02] dark:text-neutral-400 dark:hover:bg-white/[0.045]">
      <button
        type="button"
        onClick={() => { if (hasBody) setOpen((v) => !v) }}
        aria-expanded={hasBody ? open : undefined}
        className={`flex w-full max-w-full min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-left ${hasBody ? '' : 'cursor-default'}`}
        data-tauri-drag-region="false"
      >
        <CardEyebrow running={running} />
        <span className="shrink-0 font-mono text-[11px] font-semibold uppercase tracking-[0.14em] text-neutral-800 dark:text-neutral-100">
          {label}
        </span>
        {identityChips}
        {!running && (
          <span className="shrink-0">
            <CardStatusMark status={status} />
          </span>
        )}
        {metricChips}
        {statusLine && (
          <span
            className={`min-w-0 truncate font-mono text-[11px] ${
              running ? 'reasoning-shimmer-text' : 'text-neutral-400 dark:text-neutral-500'
            }`}
          >
            {statusLine}
          </span>
        )}
        {hasBody && (
          <ChevronDown
            size={12}
            strokeWidth={2}
            className={`ml-auto shrink-0 text-neutral-400 transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] dark:text-neutral-500 ${open ? 'rotate-180' : ''}`}
          />
        )}
      </button>

      {hasBody && open && (
        <div className="chat-motion-search-reveal mt-2 space-y-2 border-t border-neutral-200 pt-2 text-[12.5px] dark:border-white/10">
          {children}
        </div>
      )}
    </div>
  )
}

function SubAgentCard({ toolCall }: ToolCallBlockProps) {
  const status = normalizeToolCallStatus(toolCall.status)
  const view = useMemo(() => structuredSubagent(toolCall), [toolCall])
  const args = useMemo(() => parsedArguments(toolCall), [toolCall])

  const agentType = subagentAgentType(view, args)
  const name = subagentName(view, args)
  const model = view?.model || ''
  const duration = formatDuration(getDuration(toolCall))
  const statusLine = subagentStatusLine(view, status)
  const prompt = subagentPrompt(args)
  const result = view?.result || ''
  const error = view?.error || (toolCall.error ? compactToolError(toolCall.error) : '')
  const steps = view?.steps ?? []
  const preview = view?.preview || ''
  const usageLine = status !== 'running' ? subagentUsageLine(view?.usage) : ''

  const hasBody = Boolean(prompt || steps.length || preview || result || error)
  const displayName = name && name !== agentType ? name : ''

  return (
    <ConsultCard
      label="SUBAGENT"
      status={status}
      identityChips={
        <>
          {agentType && <CardChip>{agentType}</CardChip>}
          {displayName && <CardChip>{displayName}</CardChip>}
          {model && <CardChip>{model}</CardChip>}
        </>
      }
      metricChips={
        <>
          {duration && <CardChip tabular>{duration}</CardChip>}
          {usageLine && <CardChip tabular>{usageLine}</CardChip>}
        </>
      }
      statusLine={statusLine}
    >
      {hasBody && (
        <>
          {prompt && (
            <CardSection label="Task">
              <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
                {compactText(prompt, 600)}
              </div>
            </CardSection>
          )}
          {status === 'running' && (steps.length > 0 || preview) && (
            <div className="border-l border-neutral-300 pl-2.5 dark:border-white/15">
              {steps.length > 0 && (
                <div className="space-y-0.5 font-mono text-[10.5px] text-neutral-500 dark:text-neutral-400">
                  {steps.map((step, index) => (
                    <div key={`${index}-${step}`} className="truncate">
                      {step}
                    </div>
                  ))}
                </div>
              )}
              {preview && (
                <div className="mt-0.5 whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
                  {preview}
                </div>
              )}
            </div>
          )}
          {status !== 'running' && result && (
            <CardSection label="Result">
              <ChatMarkdown content={result} />
            </CardSection>
          )}
          {error && (
            <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
              {error}
            </div>
          )}
        </>
      )}
    </ConsultCard>
  )
}

function numberValue(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

interface AdvisorView {
  model?: string
  question?: string
  advice?: string
}

/** Parse the advisor tool's structured content ({type:"advisor", model,
 *  question, advice}). Returns null for non-advisor records. */
function structuredAdvisor(toolCall: ToolCallRecord): AdvisorView | null {
  const structured = objectValue(toolCall.structured_content ?? toolCall.structuredContent)
  if (!structured || structured.type !== 'advisor') return null
  return {
    model: stringValue(structured.model) || undefined,
    question: stringValue(structured.question) || undefined,
    advice: stringValue(structured.advice) || undefined,
  }
}

function isAdvisorRecord(toolCall: ToolCallRecord): boolean {
  if (structuredAdvisor(toolCall)) return true
  return toolCall.source === 'native' && toolRawName(toolCall) === 'advisor'
}

/** Dedicated card for an `advisor` consultation: a standalone card whose body
 *  (question + advice) is collapsible, collapsed by default to stay compact. */
function AdvisorCard({ toolCall }: ToolCallBlockProps) {
  const status = normalizeToolCallStatus(toolCall.status)
  const view = useMemo(() => structuredAdvisor(toolCall), [toolCall])
  const args = useMemo(() => parsedArguments(toolCall), [toolCall])

  const model = view?.model || ''
  const question = view?.question || stringValue(args?.question)
  const advice = view?.advice || ''
  const error = toolCall.error ? compactToolError(toolCall.error) : ''
  const duration = formatDuration(getDuration(toolCall))
  const statusLine =
    status === 'running' ? '咨询中…' : status === 'error' ? (error || '咨询失败') : ''

  const hasBody = Boolean(question || advice || error)

  return (
    <ConsultCard
      label="ADVISOR"
      status={status}
      identityChips={model ? <CardChip>{model}</CardChip> : undefined}
      metricChips={duration ? <CardChip tabular>{duration}</CardChip> : undefined}
      statusLine={statusLine}
    >
      {hasBody && (
        <>
          {question && (
            <CardSection label="Question">
              <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
                {compactText(question, 600)}
              </div>
            </CardSection>
          )}
          {advice && (
            <CardSection label="Advice">
              <ChatMarkdown content={advice} />
            </CardSection>
          )}
          {!advice && error && (
            <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
              {error}
            </div>
          )}
        </>
      )}
    </ConsultCard>
  )
}

function normalizeFileMutationFile(value: unknown): FileMutationFile | null {
  const file = objectValue(value)
  if (!file) return null
  const path = typeof file.path === 'string' ? file.path.trim() : ''
  if (!path) return null
  return {
    path,
    operation: typeof file.operation === 'string' ? file.operation : 'edit',
    bytesWritten: numberValue(file.bytesWritten),
    bytes_written: numberValue(file.bytes_written),
    additions: numberValue(file.additions),
    removals: numberValue(file.removals),
    diff: typeof file.diff === 'string' ? file.diff : '',
  }
}

// `write`/`edit` are the current Pi-style names. `write_file`/`edit_file` are
// legacy aliases from before the Pi-style rename: persisted conversations still
// carry their ToolCallRecords and must keep rendering.
function isFileMutationTool(rawName: string): boolean {
  return ['write', 'edit', 'write_file', 'edit_file'].includes(rawName)
}

function KnowledgeHits({ hits }: { hits: KbHitView[] }) {
  return (
    <div className="space-y-1.5">
      {hits.map((hit, idx) => (
        <div
          key={`${hit.n}-${idx}`}
          className="rounded-md border border-black/[0.08] bg-black/[0.02] p-2 dark:border-white/[0.1] dark:bg-white/[0.03]"
        >
          <div className="flex items-center gap-1.5 text-[10.5px] font-medium text-neutral-500 dark:text-neutral-400">
            <span className="shrink-0 rounded bg-indigo-500/15 px-1 text-indigo-500">[{hit.n}]</span>
            <span className="min-w-0 truncate">
              {hit.docName}
              {hit.headingPath ? ` · ${hit.headingPath}` : ''}
            </span>
            <span className="ml-auto shrink-0 tabular-nums text-neutral-400 dark:text-neutral-500">
              {hit.score.toFixed(2)}
            </span>
          </div>
          <div className="mt-1 line-clamp-4 whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
            {hit.text}
          </div>
        </div>
      ))}
    </div>
  )
}

function isKnowledgeSearchRecord(toolCall: ToolCallRecord): boolean {
  return toolCall.source === 'native' && toolRawName(toolCall) === 'knowledge_search'
}

/** Dedicated card for a `knowledge_search` (RAG) consultation: same consult-card
 *  shell as SUBAGENT/ADVISOR. Body shows the query plus the retrieved [n]
 *  passages (KnowledgeHits) or, when nothing matched, the plain result text. */
function KnowledgeCard({ toolCall }: ToolCallBlockProps) {
  const status = normalizeToolCallStatus(toolCall.status)
  const args = useMemo(() => parsedArguments(toolCall), [toolCall])
  const hits = useMemo(() => knowledgeSearchHits(toolCall), [toolCall])

  const query = stringValue(args?.query)
  const duration = formatDuration(getDuration(toolCall))
  const resultText = getResultPreview(toolCall)
  const error = toolCall.error ? compactToolError(toolCall.error) : ''
  const count = hits?.length ?? 0
  const statusLine =
    status === 'running' ? '检索中…' : status === 'error' ? (error || '检索失败') : ''

  const hasBody = Boolean(query || hits || (status !== 'running' && resultText) || error)

  return (
    <ConsultCard
      label="KNOWLEDGE"
      status={status}
      identityChips={count > 0 ? <CardChip>{count} 段</CardChip> : undefined}
      metricChips={duration ? <CardChip tabular>{duration}</CardChip> : undefined}
      statusLine={statusLine}
    >
      {hasBody && (
        <>
          {query && (
            <CardSection label="Query">
              <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
                {compactText(query, 300)}
              </div>
            </CardSection>
          )}
          {hits ? (
            <KnowledgeHits hits={hits} />
          ) : (
            status !== 'running' && resultText && (
              <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
                {resultText}
              </div>
            )
          )}
          {!hits && error && (
            <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
              {error}
            </div>
          )}
        </>
      )}
    </ConsultCard>
  )
}

function isPythonRecord(toolCall: ToolCallRecord): boolean {
  return toolCall.source === 'native' && toolRawName(toolCall) === 'run_python'
}

/** Dedicated card for `run_python`: same consult-card shell as SUBAGENT/ADVISOR.
 *  Body shows the executed code and stdout/stderr. Generated files/images are
 *  rendered separately at the message level (GeneratedImageArtifacts /
 *  GeneratedFileArtifacts), so here they only surface as a count chip. */
function PythonCard({ toolCall }: ToolCallBlockProps) {
  const status = normalizeToolCallStatus(toolCall.status)
  const args = useMemo(() => parsedArguments(toolCall), [toolCall])

  const code = stringValue(args?.code)
  const output = getResultPreview(toolCall)
  const error = toolCall.error ? compactToolError(toolCall.error) : ''
  const duration = formatDuration(getDuration(toolCall))
  const artifactCount = toolCall.artifacts?.length ?? 0
  const statusLine =
    status === 'running' ? '运行中…' : status === 'error' ? (error || '运行失败') : ''

  const hasBody = Boolean(code || output || error)

  return (
    <ConsultCard
      label="PYTHON"
      status={status}
      identityChips={artifactCount > 0 ? <CardChip>{artifactCount} 个产物</CardChip> : undefined}
      metricChips={duration ? <CardChip tabular>{duration}</CardChip> : undefined}
      statusLine={statusLine}
    >
      {hasBody && (
        <>
          {code && (
            <CardSection label="Code">
              <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-md border border-black/[0.08] bg-black/[0.02] p-2 font-mono text-[11px] text-neutral-600 dark:border-white/[0.1] dark:bg-white/[0.03] dark:text-neutral-300">
                {code.length > 4000 ? `${code.slice(0, 4000)}…` : code}
              </pre>
            </CardSection>
          )}
          {output && (
            <CardSection label="Output">
              <div className="whitespace-pre-wrap break-words font-mono text-[11px] text-neutral-500 dark:text-neutral-400">
                {output}
              </div>
            </CardSection>
          )}
          {error && !output && (
            <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
              {error}
            </div>
          )}
        </>
      )}
    </ConsultCard>
  )
}

function structuredFileMutation(toolCall: ToolCallRecord): FileMutationStructuredContent | null {
  const rawName = toolRawName(toolCall)
  if (!isFileMutationTool(rawName)) return null

  const structured = objectValue(toolCall.structured_content ?? toolCall.structuredContent)
  if (!structured) return null
  if (objectValue(structured.toolDraft)) return null
  const operation = typeof structured.operation === 'string' ? structured.operation : rawName
  const files = Array.isArray(structured.files)
    ? structured.files
      .map((file) => normalizeFileMutationFile(file))
      .filter((file): file is FileMutationFile => Boolean(file))
    : []
  const resolvedPath = typeof structured.resolvedPath === 'string'
    ? structured.resolvedPath
    : typeof structured.resolved_path === 'string'
      ? structured.resolved_path
      : null

  const hasMutationShape = Boolean(
    typeof structured.ok === 'boolean' ||
    typeof structured.operation === 'string' ||
    typeof structured.targetTouched === 'boolean' ||
    typeof structured.target_touched === 'boolean' ||
    resolvedPath ||
    files.length > 0 ||
    typeof structured.diff === 'string' ||
    typeof structured.additions === 'number' ||
    typeof structured.removals === 'number' ||
    Array.isArray(structured.warnings) ||
    Array.isArray(structured.diagnostics),
  )
  if (!hasMutationShape) return null
  return {
    ok: typeof structured.ok === 'boolean' ? structured.ok : true,
    operation,
    targetTouched: typeof structured.targetTouched === 'boolean' ? structured.targetTouched : undefined,
    target_touched: typeof structured.target_touched === 'boolean' ? structured.target_touched : undefined,
    resolvedPath,
    resolved_path: resolvedPath,
    files,
    bytesWritten: numberValue(structured.bytesWritten),
    bytes_written: numberValue(structured.bytes_written),
    additions: numberValue(structured.additions),
    removals: numberValue(structured.removals),
    diff: typeof structured.diff === 'string' ? structured.diff : '',
    warnings: stringArrayValue(structured.warnings),
    diagnostics: Array.isArray(structured.diagnostics) ? structured.diagnostics : [],
  }
}

function fileMutationStats(mutation: FileMutationStructuredContent): string {
  return `+${mutation.additions ?? 0} -${mutation.removals ?? 0}`
}

function fileMutationTarget(mutation: FileMutationStructuredContent): string {
  if (mutation.files?.length === 1) return mutation.files[0]?.path || ''
  if (mutation.files?.length) return `${mutation.files.length} 个文件`
  return mutation.resolvedPath || mutation.resolved_path || ''
}

function fileMutationPreview(mutation: FileMutationStructuredContent): string {
  const target = fileMutationTarget(mutation)
  const stats = mutation.files?.length ? fileMutationStats(mutation) : ''
  return [target, stats].filter(Boolean).join(' · ')
}

function FileMutationDetails({ mutation }: { mutation: FileMutationStructuredContent }) {
  const files = mutation.files ?? []
  const warnings = mutation.warnings ?? []
  const diagnostics = mutation.diagnostics ?? []
  const diff = (mutation.diff || files.map((file) => file.diff).filter(Boolean).join('\n')).trim()

  return (
    <div className="space-y-1.5">
      {files.length > 0 && (
        <div>
          <div className="text-[10.5px] font-medium text-neutral-400 dark:text-neutral-500">
            文件变更
          </div>
          <div className="space-y-0.5 text-neutral-500 dark:text-neutral-400">
            {files.map((file, index) => (
              <div key={`${file.path}-${index}`} className="flex min-w-0 items-center gap-1.5">
                <span className="shrink-0 text-neutral-400 dark:text-neutral-500">
                  {fileOperationLabel(file.operation)}
                </span>
                <span className="min-w-0 truncate">{file.path}</span>
                <span className="shrink-0 tabular-nums text-[#C56646] dark:text-[#E39A78]">
                  +{file.additions}
                </span>
                <span className="shrink-0 tabular-nums text-red-500/80">
                  -{file.removals}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
      {warnings.length > 0 && (
        <div>
          <div className="text-[10.5px] font-medium text-neutral-400 dark:text-neutral-500">
            警告
          </div>
          <div className="whitespace-pre-wrap break-words text-amber-600 dark:text-amber-300">
            {warnings.join('\n')}
          </div>
        </div>
      )}
      {diagnostics.length > 0 && (
        <div>
          <div className="text-[10.5px] font-medium text-neutral-400 dark:text-neutral-500">
            诊断
          </div>
          <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
            {previewValue(diagnostics, 900)}
          </div>
        </div>
      )}
      {diff && (
        <div>
          <div className="text-[10.5px] font-medium text-neutral-400 dark:text-neutral-500">
            Diff
          </div>
          <pre className="custom-scrollbar max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-md bg-black/[0.035] px-2 py-1.5 font-mono text-[10.5px] leading-4 text-neutral-600 dark:bg-white/[0.055] dark:text-neutral-300">
            {diff}
          </pre>
        </div>
      )}
    </div>
  )
}

function fileOperationLabel(operation: string): string {
  switch (operation) {
    case 'create':
      return '新增'
    case 'overwrite':
      return '覆盖'
    case 'edit':
      return '修改'
    case 'delete':
      return '删除'
    case 'noop':
      return '无变更'
    default:
      return operation || '变更'
  }
}

function fileToolArgumentPreview(toolCall: ToolCallRecord, args: Record<string, unknown> | null): string {
  const rawName = toolRawName(toolCall)
  const path = typeof args?.path === 'string' ? args.path.trim() : ''
  const query = typeof args?.query === 'string'
    ? args.query.trim()
    : typeof args?.pattern === 'string'
      ? args.pattern.trim()
      : ''
  const glob = typeof args?.glob === 'string' ? args.glob.trim() : ''
  if (rawName === 'write' || rawName === 'write_file') {
    return path ? path : '写入文件'
  }
  if (rawName === 'edit' || rawName === 'edit_file') {
    const edits = Array.isArray(args?.edits) ? args.edits : null
    if (edits) {
      const label = edits.length === 1 ? '1 处编辑' : `${edits.length} 处编辑`
      return [path, label].filter(Boolean).join(' · ')
    }
    // Legacy single-edit records (old_string/new_string) from persisted conversations.
    const oldString = typeof args?.old_string === 'string' ? compactText(args.old_string, 80) : ''
    return [path, oldString ? `替换 ${oldString}` : ''].filter(Boolean).join(' · ')
  }
  if (rawName === 'grep' || rawName === 'search_files') {
    const scope = glob
      ? [path || '.', glob].join(' + ')
      : path
    if (!query && !scope) return ''
    const scopeLabel = scope ? compactText(scope, 120) : ''
    const queryLabel = query ? `搜索 ${compactText(query, 80)}` : ''
    return [queryLabel, scopeLabel].filter(Boolean).join(' · ')
  }
  return ''
}

function formatDuration(ms?: number): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return ''
  if (ms < 1000) return `${Math.round(ms)}ms`
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)}s`
  return `${Math.round(ms / 1000)}s`
}

function getDuration(toolCall: ToolCallRecord): number | undefined {
  if (toolCall.duration_ms != null) return toolCall.duration_ms
  if (toolCall.durationMs != null) return toolCall.durationMs

  const startedAt = toolCall.started_at ?? toolCall.startedAt
  const completedAt = toolCall.completed_at ?? toolCall.completedAt
  if (startedAt == null || completedAt == null) return undefined

  const delta = completedAt - startedAt
  return delta > 0 && delta < 10_000 ? delta * 1000 : delta
}

function getToolName(toolCall: ToolCallRecord): string {
  const raw = toolRawName(toolCall) || 'Tool'

  if (raw === 'skill' || raw === 'skill_activate') return 'Activate skill'
  // 全英文、原形动词（不加 -ed）：直接用工具本名的动词形式，如 Read / Run / Grep / Glob。
  // 目标（文件名 + 行号 / 命令 / pattern）由 getToolTarget 追加。
  switch (raw) {
    case 'read':
    case 'read_file':
      return 'Read'
    case 'write':
    case 'write_file':
      return 'Write'
    case 'edit':
    case 'edit_file':
      return 'Edit'
    case 'bash':
    case 'run_command':
      return 'Run'
    case 'grep':
    case 'search_files':
      return 'Grep'
    case 'glob':
    case 'glob_files':
    case 'find':
      return 'Glob'
    case 'ls':
    case 'list_dir':
      return 'List'
    case 'stat':
    case 'stat_path':
      return 'Stat'
    case 'delete':
      return 'Delete'
    case 'move':
      return 'Move'
    case 'copy':
      return 'Copy'
    case 'create_dir':
      return 'Create dir'
    default:
      break
  }
  if (raw === 'run_python') return 'Python'
  if (raw === 'web_search') {
    // 结果里带了实际使用的搜索服务名（后端 structured_content.provider），显示为「Web search · Ollama」。
    const structured = objectValue(toolCall.structured_content ?? toolCall.structuredContent) ?? {}
    const provider = stringValue(structured.provider)
    return provider ? `Web search · ${provider}` : 'Web search'
  }
  if (raw === 'web_fetch') return 'Fetch'
  if (raw === 'knowledge_search') return 'Knowledge search'
  if (raw === 'mixer_vision') return 'Vision'
  if (raw === 'mixer_generate_image') return 'Generate image'
  if (raw === 'todo_write' || raw === 'todo_update') return 'Update todos'
  return raw
}

function firstString(...values: unknown[]): string {
  for (const value of values) {
    if (typeof value === 'string' && value.trim()) return value.trim()
  }
  return ''
}

/** 只取路径最后一段（Cursor 行内用文件名而非全路径）：`src/a/Lens.tsx` → `Lens.tsx`。 */
function basename(path: string): string {
  const trimmed = path.trim().replace(/[\\/]+$/, '')
  const idx = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  return idx >= 0 ? trimmed.slice(idx + 1) : trimmed
}

/** read 的行号范围 `L起-止`：优先用结果里的真实 start/end（structured_content 保留了
 *  ReadFileResult），流式未出结果时回退到请求窗口 offset/limit；整文件读取则为空。 */
function readLineLabel(toolCall: ToolCallRecord, args: Record<string, unknown> | null): string {
  const structured = objectValue(toolCall.structured_content ?? toolCall.structuredContent)
  const start = numberValue(structured?.start_line ?? structured?.startLine)
  const end = numberValue(structured?.end_line ?? structured?.endLine)
  if (start > 0 && end > 0) return `L${start}-${end}`
  const offset = numberValue(args?.offset)
  const limit = numberValue(args?.limit)
  if (offset > 0) return limit > 0 ? `L${offset}-${offset + limit - 1}` : `L${offset}+`
  return ''
}

/** 折叠行的「目标」：以输入参数为主（文件名 / 命令 / pattern / url），不含动词、不含结果。
 *  Cursor 风格 —— 行内只呈现「动词 + 目标」，其余细节（结果、diff、错误）放展开区。 */
function getToolTarget(toolCall: ToolCallRecord): string {
  const raw = toolRawName(toolCall)
  const args = parsedArguments(toolCall)
  const path = firstString(args?.path, args?.relative_path, args?.relativePath)
  const primary = ((): string => {
    switch (raw) {
      case 'read':
      case 'read_file':
        return [basename(path), readLineLabel(toolCall, args)].filter(Boolean).join(' ')
      case 'write':
      case 'write_file':
      case 'edit':
      case 'edit_file': {
        const mutation = structuredFileMutation(toolCall)
        if (mutation && (mutation.files?.length ?? 0) > 1) {
          return `${mutation.files!.length} files`
        }
        const target = (mutation && fileMutationTarget(mutation)) || path
        return target.includes('/') || target.includes('\\') ? basename(target) : target
      }
      case 'bash':
      case 'run_command':
        return compactText(firstString(args?.description, args?.command), 160)
      case 'grep':
      case 'search_files':
        return compactText(firstString(args?.query, args?.pattern), 140)
      case 'glob':
      case 'glob_files':
      case 'find': {
        const pattern = compactText(firstString(args?.pattern, args?.glob), 140)
        const dir = firstString(args?.path)
        return dir && dir !== '.' ? `${pattern} in ${basename(dir)}` : pattern
      }
      case 'ls':
      case 'list_dir':
      case 'stat':
      case 'stat_path':
      case 'delete':
      case 'create_dir':
        return path
      case 'move':
      case 'copy': {
        const from = firstString(args?.source, args?.from, args?.src, args?.path)
        const to = firstString(args?.destination, args?.to, args?.dest, args?.target)
        return [from, to].filter(Boolean).join(' → ')
      }
      case 'web_fetch':
        return compactText(firstString(args?.url), 140)
      case 'web_search':
      case 'knowledge_search':
        return compactText(firstString(args?.query), 140)
      case 'todo_write': {
        const counts = formatTodoCounts(normalizeTodoItems(args?.todos))
        return counts
      }
      case 'todo_update':
        return compactText(firstString(args?.content, args?.id), 120)
      case 'mixer_vision': {
        const count = numberValue(args?.images)
        return count > 0 ? `${count} image${count > 1 ? 's' : ''}` : ''
      }
      case 'mixer_generate_image':
        return compactText(firstString(args?.prompt), 140)
      default:
        return ''
    }
  })()
  if (primary) return primary
  // run_python 只显示动词「Python」，不追加目标；其余工具（含参数尚未解析出的情况）
  // 回退到已有的输入参数摘要（todo / mixer / skill / MCP 及流式占位 argumentPreview）。
  if (raw === 'run_python') return ''
  return getArgumentPreview(toolCall)
}

function getArgumentPreview(toolCall: ToolCallRecord): string {
  const rawName = toolRawName(toolCall)
  const args = parsedArguments(toolCall)
  if (rawName === 'todo_write') {
    const todos = normalizeTodoItems(args?.todos)
    const counts = formatTodoCounts(todos)
    return counts ? `清单 ${counts}` : todos.length ? `清单 ${todos.length} 项` : '替换 Todo 清单'
  }
  if (rawName === 'todo_update') {
    const content = typeof args?.content === 'string' ? compactText(args.content, 120) : ''
    const status = typeof args?.status === 'string' ? todoStatusLabel(args.status) : ''
    const id = typeof args?.id === 'string' ? compactText(args.id, 80) : ''
    const target = content || id
    return ['更新条目', status, target].filter(Boolean).join(' · ')
  }
  const fileMutation = structuredFileMutation(toolCall)
  if (fileMutation) {
    return fileMutationPreview(fileMutation)
  }
  const fileArgsPreview = fileToolArgumentPreview(toolCall, args)
  if (fileArgsPreview) return fileArgsPreview
  if (rawName === 'mixer_vision') {
    const imageCount = typeof args?.images === 'number' ? args.images : null
    const provider = typeof args?.provider === 'string' ? args.provider : ''
    const model = typeof args?.model === 'string' ? args.model : ''
    const imageLabel = imageCount == null
      ? '图片'
      : `图片 ${imageCount} 张`
    const modelLabel = [provider, model].filter(Boolean).join(' / ')
    return modelLabel ? `${imageLabel} · ${modelLabel}` : imageLabel
  }
  if (rawName === 'mixer_generate_image') {
    const prompt = typeof args?.prompt === 'string' ? compactText(args.prompt, 140) : ''
    const size = typeof args?.size === 'string' && args.size ? args.size : ''
    const quality = typeof args?.quality === 'string' && args.quality ? args.quality : ''
    const count = typeof args?.n === 'number' && Number.isFinite(args.n) ? `${args.n} 张` : ''
    return [prompt, size, quality, count].filter(Boolean).join(' · ')
  }
  return (
    toolCall.argument_preview ||
    toolCall.argumentPreview ||
    toolCall.argumentsPreview ||
    previewValue(toolCall.arguments ?? toolCall.args ?? toolCall.input)
  )
}

function getResultPreview(toolCall: ToolCallRecord): string {
  const rawName = toolRawName(toolCall)
  if (rawName === 'todo_write' || rawName === 'todo_update') {
    if (normalizeToolCallStatus(toolCall.status) !== 'completed') return ''
    const counts = formatTodoCounts(structuredTodoState(toolCall)?.items)
    return counts ? `已同步 ${counts}` : '已同步'
  }
  const fileMutation = structuredFileMutation(toolCall)
  if (fileMutation) {
    if (fileMutation.ok === false) {
      return `未完成 ${fileMutationPreview(fileMutation)}`
    }
    return `已应用 ${fileMutationPreview(fileMutation)}`
  }
  const raw =
    toolCall.result_preview ||
    toolCall.resultPreview ||
    previewValue(toolCall.result ?? toolCall.output)
  if (!raw) return ''
  return formatToolResultPreview(raw)
}

function stripPythonFailurePrefix(message: string): string {
  return message
    .replace(/^Python\s*(?:执行失败|语法错误|执行超时|沙盒调用失败)(?:（[^）]+）)?[：:]\s*/i, '')
    .trim()
}

function cleanPythonExceptionSnippet(message: string): string {
  const normalized = stripPythonFailurePrefix(message).replace(/\s+/g, ' ').trim()
  const stackBoundary = normalized.search(
    /\s+(?=Traceback \(most recent call last\):|File\s+"|File\s+'|await CodeRunner\(|coroutine =|new_error@|[0-9]+@wasm-function|\^+)/,
  )
  const clipped = stackBoundary >= 0 ? normalized.slice(0, stackBoundary) : normalized
  return compactText(clipped, 260)
}

function extractPythonException(message: string): string {
  const cleaned = message
    .replace(/\bstderr:\s*/gi, '\n')
    .replace(/\bstdout:\s*/gi, '\n')
  const stackNoise = /(pyodide\.asm\.js|wasm-function|new_error@|_pyodide)/i
  const exceptionName = /^[A-Za-z_][\w.]*(?:Error|Exception|Warning|Interrupt|Exit|Fault|Found|Denied|Timeout)\b/
  const lines = cleaned
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
  const tracebackLine = [...lines]
    .reverse()
    .find((line) => exceptionName.test(line) && !stackNoise.test(line) && !line.startsWith('PythonError: Traceback'))
  if (tracebackLine) return cleanPythonExceptionSnippet(tracebackLine)

  const inlineMatches = [
    ...cleaned.matchAll(
      /\b([A-Za-z_][\w.]*(?:Error|Exception|Warning|Interrupt|Exit|Fault|Found|Denied|Timeout)\b(?::\s*[^。\r\n]+)?)/g,
    ),
  ]
    .map((match) => cleanPythonExceptionSnippet(match[1] || ''))
    .filter((value) => value && !stackNoise.test(value) && !value.startsWith('PythonError: Traceback'))
  const inline = inlineMatches.reverse()[0]
  return inline || ''
}

function compactToolError(error: string): string {
  const lower = error.toLowerCase()
  if (
    lower.includes('pyodide.asm.js') ||
    lower.includes('wasm-function') ||
    lower.includes('traceback (most recent call last)') ||
    lower.includes('pythonerror: traceback') ||
    lower.includes('_pyodide/')
  ) {
    const exception = extractPythonException(error)
    if (exception) return `Python 执行失败：${exception}`
    return 'Python 执行失败。详情已隐藏，请查看最终回答。'
  }
  return compactText(error, 260)
}

function StatusIcon({ status }: { status: ToolCallStatus }) {
  // 仅在「实时」由非完成态切到完成态时让 ✓ 弹入；历史消息（挂载即完成态）不 pop，
  // 避免切换会话时大量历史工具的 ✓ 同时弹动造成视觉噪声。
  const prevStatusRef = useRef(status)
  const isDone = status === 'completed' || status === 'success'
  const wasDone = prevStatusRef.current === 'completed' || prevStatusRef.current === 'success'
  const justCompleted = isDone && !wasDone
  const isBad = status === 'error' || status === 'cancelled' || status === 'skipped'
  const wasBad =
    prevStatusRef.current === 'error' ||
    prevStatusRef.current === 'cancelled' ||
    prevStatusRef.current === 'skipped'
  const justFailed = isBad && !wasBad
  useEffect(() => {
    prevStatusRef.current = status
  }, [status])
  if (status === 'running') {
    return <Loader2 className="shrink-0 animate-spin" size={14} />
  }
  if (isDone) {
    return (
      <CheckCircle2
        className={`shrink-0 text-[#C56646] dark:text-[#E39A78]${justCompleted ? ' chat-motion-pop' : ''}`}
        size={14}
        strokeWidth={1.9}
      />
    )
  }
  if (status === 'error') {
    return <AlertCircle className={`shrink-0 text-neutral-400 dark:text-neutral-500${justFailed ? ' chat-motion-pop' : ''}`} size={14} strokeWidth={1.9} />
  }
  if (status === 'skipped') {
    return <CircleSlash className={`shrink-0${justFailed ? ' chat-motion-pop' : ''}`} size={14} strokeWidth={1.9} />
  }
  if (status === 'cancelled') {
    return <XCircle className={`shrink-0${justFailed ? ' chat-motion-pop' : ''}`} size={14} strokeWidth={1.9} />
  }
  return <Wrench className="shrink-0" size={14} strokeWidth={1.85} />
}

function ToolTypeIcon({ toolCall, status }: { toolCall: ToolCallRecord; status: ToolCallStatus }) {
  // 行首按工具类型显示图标；运行/完成/待执行用工具图标本身，出错/跳过/取消保留专用状态图标。
  const prevStatusRef = useRef(status)
  const isDone = status === 'completed' || status === 'success'
  const wasDone = prevStatusRef.current === 'completed' || prevStatusRef.current === 'success'
  const justCompleted = isDone && !wasDone
  const isBad = status === 'error' || status === 'cancelled' || status === 'skipped'
  const wasBad =
    prevStatusRef.current === 'error' ||
    prevStatusRef.current === 'cancelled' ||
    prevStatusRef.current === 'skipped'
  const justFailed = isBad && !wasBad
  useEffect(() => {
    prevStatusRef.current = status
  }, [status])
  if (status === 'error') {
    return <AlertCircle className={`shrink-0 text-neutral-400 dark:text-neutral-500${justFailed ? ' chat-motion-pop' : ''}`} size={14} strokeWidth={1.9} />
  }
  if (status === 'skipped') {
    return <CircleSlash className={`shrink-0${justFailed ? ' chat-motion-pop' : ''}`} size={14} strokeWidth={1.9} />
  }
  if (status === 'cancelled') {
    return <XCircle className={`shrink-0${justFailed ? ' chat-motion-pop' : ''}`} size={14} strokeWidth={1.9} />
  }
  const Glyph = toolGlyph(toolCall)
  if (status === 'running') {
    return (
      <Glyph
        className="shrink-0 text-neutral-400 dark:text-neutral-500 animate-pulse"
        size={14}
        strokeWidth={1.9}
      />
    )
  }
  if (isDone) {
    return (
      <Glyph
        className={`shrink-0 text-neutral-400 dark:text-neutral-500${justCompleted ? ' chat-motion-pop' : ''}`}
        size={14}
        strokeWidth={1.9}
      />
    )
  }
  return (
    <Glyph
      className="shrink-0 text-neutral-400 dark:text-neutral-500"
      size={14}
      strokeWidth={1.9}
    />
  )
}

function DefaultToolCallBlock({
  toolCall,
  defaultOpen = false,
}: ToolCallBlockProps) {
  const status = normalizeToolCallStatus(toolCall.status)
  const [open, setOpen] = useState(defaultOpen)

  const toolName = getToolName(toolCall)
  const target = useMemo(() => getToolTarget(toolCall), [toolCall])
  const fileMutation = useMemo(() => structuredFileMutation(toolCall), [toolCall])
  const knowledgeHits = useMemo(() => knowledgeSearchHits(toolCall), [toolCall])
  const argumentPreview = useMemo(() => getArgumentPreview(toolCall), [toolCall])
  const resultPreview = useMemo(() => getResultPreview(toolCall), [toolCall])
  const error = toolCall.error ? compactToolError(toolCall.error) : ''
  const hasFileMutationDetails = Boolean(
    fileMutation && (
      fileMutation.files?.length ||
      fileMutation.diff ||
      fileMutation.warnings?.length ||
      fileMutation.diagnostics?.length
    ),
  )
  const hasDetails = Boolean(argumentPreview || resultPreview || error || hasFileMutationDetails || knowledgeHits)

  return (
    <div className="not-prose mb-1 text-[12.5px] leading-5 text-neutral-500 dark:text-neutral-400">
      <button
        type="button"
        onClick={() => {
          if (hasDetails) setOpen((value) => !value)
        }}
        aria-expanded={hasDetails ? open : undefined}
        className={`max-w-full min-w-0 inline-flex items-center gap-1.5 rounded-md py-0 transition-colors ${
          hasDetails
            ? 'hover:text-neutral-700 dark:hover:text-neutral-200'
            : 'cursor-default'
        }`}
      >
        <ToolTypeIcon toolCall={toolCall} status={status} />
        <span
          className={`shrink-0 font-medium text-neutral-700 dark:text-neutral-200${
            status === 'running' ? ' chat-motion-tool-shimmer' : ''
          }`}
        >
          {toolName || '工具'}
        </span>
        {target && (
          <span className="min-w-0 truncate text-neutral-400 dark:text-neutral-500">
            {target}
          </span>
        )}
        {hasDetails && (
          <ChevronDown
            size={12}
            strokeWidth={2}
            className={`shrink-0 transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] ${open ? 'rotate-180' : ''}`}
          />
        )}
      </button>

      {hasDetails && (
        <div className={`chat-motion-reveal ${open ? 'is-open' : ''}`} aria-hidden={!open}>
          <div className="mt-1.5 ml-1.5 space-y-1.5 border-l border-black/[0.08] pl-2.5 dark:border-white/[0.1]">
            {argumentPreview && (
              <div>
                <div className="text-[10.5px] font-medium text-neutral-400 dark:text-neutral-500">
                  {'参数'}
                </div>
                <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
                  {argumentPreview}
                </div>
              </div>
            )}
            {open && resultPreview && !knowledgeHits && (
              <div>
                <div className="text-[10.5px] font-medium text-neutral-400 dark:text-neutral-500">
                  {'结果'}
                </div>
                <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
                  {resultPreview}
                </div>
              </div>
            )}
            {open && knowledgeHits && <KnowledgeHits hits={knowledgeHits} />}
            {fileMutation && hasFileMutationDetails && (
              <FileMutationDetails mutation={fileMutation} />
            )}
            {error && (
              <div className="whitespace-pre-wrap break-words text-neutral-500 dark:text-neutral-400">
                {error}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}

function ToolCallBlockComponent(props: ToolCallBlockProps) {
  if (isAskUserTool(props.toolCall)) {
    return <AskUserBlock toolCall={props.toolCall} />
  }
  if (isSubAgentRecord(props.toolCall)) {
    return <SubAgentCard {...props} />
  }
  if (isAdvisorRecord(props.toolCall)) {
    return <AdvisorCard {...props} />
  }
  if (isKnowledgeSearchRecord(props.toolCall)) {
    return <KnowledgeCard {...props} />
  }
  if (isPythonRecord(props.toolCall)) {
    return <PythonCard {...props} />
  }
  return <DefaultToolCallBlock {...props} />
}

// memo：折叠某块 / 流式更新时，props 不变的工具块跳过重渲染（尤其 knowledge_search 的大段源卡片）
export const ToolCallBlock = memo(ToolCallBlockComponent)
