import type { ChatMessageSegment, ToolCallRecord } from './types'
import { foldToolName, hasAskUserStructuredContent, isAskUserToolName } from './askUserTools'
import { normalizeToolCallStatus } from './toolStatus'
import { isArtifactPresentationToolCall } from './artifactPresentation'

export function segmentToolCallId(segment: ChatMessageSegment): string {
  return segment.tool_call_id ?? segment.toolCallId ?? ''
}

export function toolRecordRawName(toolCall: ToolCallRecord): string {
  return toolCall.tool_name || toolCall.toolName || toolCall.name || ''
}

/**
 * 展示/分类用的规范工具名：小写 + 去分隔符后查别名。
 * dsh 在 Windows 上把 bash 报成 `pwsh`；claude 是 PascalCase 的 `Bash` / `Read`。
 */
const TOOL_NAME_ALIASES: Record<string, string> = {
  webfetch: 'web_fetch',
  websearch: 'web_search',
  todowrite: 'todo_write',
  multiedit: 'edit',
  notebookedit: 'edit',
  askuserquestion: 'ask_user',
  pwsh: 'bash',
  powershell: 'bash',
  cmd: 'bash',
  shell: 'bash',
  joboutput: 'run_command',
  joblist: 'run_command',
  jobkill: 'run_command',
  getgoal: 'todo_write',
  creategoal: 'todo_write',
  updategoal: 'todo_write',
  readimage: 'read',
  runcode: 'bash',
  subagent: 'agent',
  subagentfork: 'agent',
  listagents: 'agent',
  sendmessage: 'agent',
  interruptagent: 'agent',
  workflow: 'agent',
  ralph: 'agent',
  cordisdefine: 'run_command',
  cordisrun: 'run_command',
  cordisstop: 'run_command',
  cordisundefine: 'run_command',
}

function foldedToolName(toolCall: ToolCallRecord): string {
  return toolRecordRawName(toolCall).toLowerCase().replace(/[_\-\s]/g, '')
}

export function canonicalToolName(toolCall: ToolCallRecord): string {
  const folded = foldedToolName(toolCall)
  if (folded === 'strreplaceeditor') {
    const command = recordString(parsedToolArguments(toolCall)?.command)
    if (command === 'view') return 'read'
    if (command === 'create') return 'write'
    return 'edit'
  }
  if (folded.startsWith('cordisinspect')) return 'read'
  return TOOL_NAME_ALIASES[folded] ?? toolRecordRawName(toolCall).toLowerCase()
}

/** Edit/Write 类工具名（含外部 CLI 的 Edit/Write/MultiEdit/NotebookEdit 变体）。 */
function isFileWriteToolName(toolCall: ToolCallRecord): boolean {
  const name = canonicalToolName(toolCall)
  return name === 'write' || name === 'write_file' || name === 'edit' || name === 'edit_file'
}

function recordObject(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function recordString(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function parsedToolArguments(toolCall: ToolCallRecord): Record<string, unknown> | null {
  const value = toolCall.arguments ?? toolCall.args ?? toolCall.input
  if (typeof value !== 'string') return recordObject(value)
  try {
    return recordObject(JSON.parse(value))
  } catch {
    return null
  }
}

export interface DiffStats {
  additions: number
  removals: number
}

/** Edit/Write 类工具的 `+N -N`：优先读后端 structured_content 的真实 additions/removals；
 *  外部 CLI 的记录没有统计时，按参数新旧文本（old_string/new_string/edits[]/content）的
 *  行数估算。非文件写入工具、未完成的调用返回 null。
 *  ponytail: replace_all 的多处命中会被低估成一处，参数即所见，够用。 */
export function toolCallDiffStats(toolCall: ToolCallRecord): DiffStats | null {
  if (!isFileWriteToolName(toolCall)) return null
  if (normalizeToolCallStatus(toolCall.status) !== 'completed') return null

  const structured = recordObject(toolCall.structured_content ?? toolCall.structuredContent)
  if (structured && !recordObject(structured.toolDraft)) {
    const files = Array.isArray(structured.files) ? structured.files.map(recordObject) : []
    if (typeof structured.additions === 'number' || typeof structured.removals === 'number' || files.length) {
      let additions = typeof structured.additions === 'number' ? structured.additions : 0
      let removals = typeof structured.removals === 'number' ? structured.removals : 0
      if (!additions && !removals) {
        for (const file of files) {
          additions += typeof file?.additions === 'number' ? file.additions : 0
          removals += typeof file?.removals === 'number' ? file.removals : 0
        }
      }
      return { additions, removals }
    }
  }

  const args = parsedToolArguments(toolCall)
  const lines = (text: string) => (text ? text.split('\n').length : 0)
  let additions = 0
  let removals = 0
  const edits = Array.isArray(args?.edits) ? args.edits : null
  if (edits) {
    for (const edit of edits) {
      const pair = recordObject(edit)
      additions += lines(recordString(pair?.new_string))
      removals += lines(recordString(pair?.old_string))
    }
  } else if (
    recordString(args?.old_string)
    || recordString(args?.new_string)
    || recordString(args?.old_str)
    || recordString(args?.new_str)
  ) {
    additions = lines(recordString(args?.new_string) || recordString(args?.new_str))
    removals = lines(recordString(args?.old_string) || recordString(args?.old_str))
  } else {
    additions = lines(recordString(args?.content) || recordString(args?.text))
  }
  return additions || removals ? { additions, removals } : null
}

/**
 * 用户在生成中「立刻引导」插进来的那句话（后端合成的 display-only 记录，见
 * `chat/agent/steering.rs`）。渲染成一条用户小气泡而不是工具卡。
 *
 * 三条判据全要 —— 这张卡呈现的是「用户说过的话」，冒充它比冒充一张搜索卡严重：
 * 只有 native 通道、保留工具名、structured type 三者同时对上才认。
 */
export function isUserSteerToolCall(toolCall: ToolCallRecord): boolean {
  if (toolCall.source !== 'native') return false
  if (toolRecordRawName(toolCall) !== 'user_steer') return false
  const structured = toolCall.structured_content ?? toolCall.structuredContent
  if (!structured || typeof structured !== 'object') return false
  return (structured as { type?: unknown }).type === 'user_steer'
}

/** 插话卡里那句话（空串 = 不是插话卡 / 载荷缺字段）。 */
export function userSteerText(toolCall: ToolCallRecord): string {
  const structured = toolCall.structured_content ?? toolCall.structuredContent
  if (!structured || typeof structured !== 'object') return ''
  const text = (structured as { text?: unknown }).text
  return typeof text === 'string' ? text : ''
}

export function isUserFollowUpToolCall(toolCall: ToolCallRecord): boolean {
  if (toolCall.source !== 'native') return false
  if (toolRecordRawName(toolCall) !== 'user_follow_up') return false
  const structured = toolCall.structured_content ?? toolCall.structuredContent
  if (!structured || typeof structured !== 'object') return false
  return (structured as { type?: unknown }).type === 'user_follow_up'
}

export function userFollowUpText(toolCall: ToolCallRecord): string {
  const structured = toolCall.structured_content ?? toolCall.structuredContent
  if (!structured || typeof structured !== 'object') return ''
  const text = (structured as { text?: unknown }).text
  return typeof text === 'string' ? text : ''
}

function structuredStringField(
  toolCall: ToolCallRecord,
  snake: string,
  camel: string,
): string | null {
  const structured = toolCall.structured_content ?? toolCall.structuredContent
  if (!structured || typeof structured !== 'object') return null
  const value = (structured as Record<string, unknown>)[snake]
    ?? (structured as Record<string, unknown>)[camel]
  return typeof value === 'string' ? value : null
}

/** 插话卡上的前端队列 id。蛇/驼峰都认，协议层 Value 原样穿过。 */
export function userSteerId(toolCall: ToolCallRecord): string | null {
  if (!isUserSteerToolCall(toolCall)) return null
  return structuredStringField(toolCall, 'steer_id', 'steerId')
}

export function userFollowUpId(toolCall: ToolCallRecord): string | null {
  if (!isUserFollowUpToolCall(toolCall)) return null
  return structuredStringField(toolCall, 'follow_up_id', 'followUpId')
}

/** 外部 CLI 的子代理工具调用：claude 新版报 `Agent`、旧版报 `Task`；dsh 报
 *  `subagent` / `subagent_fork` / `workflow` / `ralph`（source 恒为 `external_cli`）。
 *  精确匹配整名，MCP/native 不受影响。 */
export function isExternalSubagentToolCall(toolCall: ToolCallRecord): boolean {
  if (toolCall.source !== 'external_cli') return false
  const name = foldedToolName(toolCall)
  return name === 'agent'
    || name === 'task'
    || name === 'subagent'
    || name === 'subagentfork'
    || name === 'workflow'
    || name === 'ralph'
}

/** Tool calls that render as their own dedicated, always-visible card in the
 *  timeline (never folded into the "调用 N 次工具" group): sub-agents (`agent`),
 *  advisor consultations, and ask-user prompts. Matched by structured content type
 *  first, then by the native tool name for the still-streaming case (before
 *  structured content arrives). */
export function isStandaloneToolCard(toolCall: ToolCallRecord): boolean {
  // 用户插话：把它折进「调用 N 次工具」等于把用户自己说的话藏起来，同 ask_user 的理由。
  if (isUserSteerToolCall(toolCall) || isUserFollowUpToolCall(toolCall)) return true
  const structured = toolCall.structured_content ?? toolCall.structuredContent
  if (structured && typeof structured === 'object') {
    const type = (structured as { type?: unknown }).type
    if (type === 'subagent' || type === 'advisor') return true
    // 问用户：载荷里是 `askUser`（没有 `type` 字段）。它记的是「问了什么 + 你选了什么」，
    // 折进「调用 N 次工具」里等于把一次人为决定藏起来 —— 那是这条对话里最该看见的东西。
    if (hasAskUserStructuredContent(structured)) return true
  }
  const name = toolRecordRawName(toolCall)
  // 外部 CLI 报的是自己的工具名，所以这条判据不能只认 native。
  if (isAskUserToolName(name)) return true
  // claude 的 `ExitPlanMode` 是计划审批，不是问用户卡，但同样是一次人为决定。
  if (foldToolName(name) === 'exitplanmode') return true
  // 外部 CLI 的子代理（claude 的 Agent/Task）：一次完整的委派，同内置 agent 独立成卡，
  // 折进「调用 N 次工具」等于把派活这件事藏起来。
  if (isExternalSubagentToolCall(toolCall)) return true
  if (toolCall.source !== 'native') return false
  return name === 'agent' || name === 'advisor' || isArtifactPresentationToolCall(toolCall)
}

/** tool record 的唯一 id（兼容多种字段命名）。 */
export function toolRecordId(toolCall: ToolCallRecord): string {
  return toolCall.id || toolCall.toolCallId || toolCall.call_id || toolCall.callId || ''
}

export function segmentStepNumber(segment: ChatMessageSegment): number | null | undefined {
  return segment.step_number ?? segment.stepNumber
}

function segmentDisplayRank(segment: ChatMessageSegment): number {
  if (segment.kind === 'reasoning') return 0
  if (segment.kind === 'text') return 1
  return 2
}

export function compareTimelineSegments(
  a: ChatMessageSegment,
  b: ChatMessageSegment,
): number {
  const aStepNumber = segmentStepNumber(a)
  const bStepNumber = segmentStepNumber(b)
  const sameModelStep =
    aStepNumber != null &&
    aStepNumber === bStepNumber &&
    (a.round ?? null) === (b.round ?? null) &&
    a.phase === b.phase
  if (sameModelStep) {
    const rankDelta = segmentDisplayRank(a) - segmentDisplayRank(b)
    if (rankDelta !== 0) return rankDelta
  }
  return a.order - b.order
}

/** 渲染前的「有内容」判定：reasoning/text 段空白则不渲染，也不应单独成组/打断分组。
 *  tool 段始终保留（其记录可能缺失，交由 UI 兜底）。 */
function segmentHasContent(segment: ChatMessageSegment): boolean {
  if (segment.kind === 'tool') return true
  return Boolean((segment.text ?? '').trim())
}

export type TimelineGroupItem =
  | { type: 'text'; segment: ChatMessageSegment }
  | { type: 'group'; segments: ChatMessageSegment[] }
  | { type: 'standaloneTool'; segment: ChatMessageSegment }

/**
 * 以正文(text)段为分隔，把两条正文之间连续的非 text 段（reasoning + tool）聚成一个组。
 * - 纯函数：输入有序 segments → 输出渲染项数组，便于单测。
 * - text 段单独成项（原样渲染正文），永远打断分组。
 * - `tool → text → tool` ⇒ 两个组。
 * - 空白 reasoning/text 段先过滤，避免产生空组或多余分隔。
 * - `isStandalone(segment)` 命中的 tool 段（如 advisor / subagent）像 text 一样单独成项、
 *   打断分组，交给调用方以专属卡片常驻渲染（不被折叠进「调用 N 次工具」组）。
 */
export function groupTimelineSegments(
  orderedSegments: ChatMessageSegment[],
  isStandalone?: (segment: ChatMessageSegment) => boolean,
): TimelineGroupItem[] {
  const items: TimelineGroupItem[] = []
  let current: ChatMessageSegment[] | null = null
  for (const segment of orderedSegments) {
    if (!segmentHasContent(segment)) continue
    if (segment.kind === 'text') {
      current = null
      items.push({ type: 'text', segment })
      continue
    }
    if (segment.kind === 'tool' && isStandalone?.(segment)) {
      current = null
      items.push({ type: 'standaloneTool', segment })
      continue
    }
    if (!current) {
      current = []
      items.push({ type: 'group', segments: current })
    }
    current.push(segment)
  }
  return items
}

export type ToolGroupCategory =
  | 'read'
  | 'codeSearch'
  | 'globFiles'
  | 'fileWrite'
  | 'runCommand'
  | 'webFetch'
  | 'webSearch'
  | 'listDir'
  | 'fileOps'
  | 'todo'
  | 'memory'
  | 'subAgent'
  | 'skill'
  | 'image'
  | 'notion'
  | 'mcp'
  | 'other'

/** 分组头图标用的代表类别：工具类别全集 + 纯思考组的 `'reasoning'`。 */
export type ToolGroupIcon = ToolGroupCategory | 'reasoning'

function categorizeTool(toolCall: ToolCallRecord): ToolGroupCategory {
  const raw = canonicalToolName(toolCall)
  switch (raw) {
    case 'read':
    case 'read_file':
      return 'read'
    case 'grep':
    case 'search_files':
      return 'codeSearch'
    case 'find':
    case 'glob':
    case 'glob_files':
      return 'globFiles'
    case 'write':
    case 'write_file':
    case 'edit':
    case 'edit_file':
      return 'fileWrite'
    case 'bash':
    case 'run_command':
      return 'runCommand'
    case 'web_fetch':
      return 'webFetch'
    case 'web_search':
      return 'webSearch'
    case 'ls':
    case 'list_dir':
      return 'listDir'
    case 'move':
    case 'copy':
    case 'delete':
    case 'create_dir':
    case 'stat':
    case 'stat_path':
      return 'fileOps'
    case 'todo_write':
    case 'todo_update':
      return 'todo'
    case 'memory_read':
    case 'memory_search':
    case 'memory_modify':
      return 'memory'
    case 'agent':
      return 'subAgent'
    case 'skill':
    case 'skill_activate':
      return 'skill'
    case 'mixer_vision':
    case 'mixer_generate_image':
      return 'image'
    default:
      break
  }
  const server = (toolCall.server_name || toolCall.serverName || toolCall.server_id || toolCall.serverId || '')
    .toLowerCase()
  if (server.includes('notion') || raw.toLowerCase().startsWith('notion')) {
    return 'notion'
  }
  const isMcp =
    toolCall.source === 'mcp' ||
    (Boolean(toolCall.server_name || toolCall.serverName) &&
      toolCall.source !== 'native' &&
      toolCall.source !== 'skill' &&
      toolCall.source !== 'mixer')
  if (isMcp) return 'mcp'
  return 'other'
}

/** 去重（保持首次出现顺序）并剔除 `'other'` 后的「有意义类别」集合，文案与图标共用同一判定。 */
function meaningfulCategories(categories: ToolGroupCategory[]): ToolGroupCategory[] {
  const seen = new Set<ToolGroupCategory>()
  const result: ToolGroupCategory[] = []
  for (const category of categories) {
    if (category === 'other' || seen.has(category)) continue
    seen.add(category)
    result.push(category)
  }
  return result
}

/**
 * 每个类别的「动作片段」（不带时态前缀、不带状态后缀）。
 * n = 该类别下的工具数；部分类别不带数量。
 * Codex 风格：动词 + 数量 + 宾语，由调用方加「已/正在」前缀。
 */
function categoryFragment(category: ToolGroupCategory, count: number): string {
  switch (category) {
    case 'read':
      return `读取 ${count} 个文件`
    case 'fileWrite':
      return `编辑 ${count} 个文件`
    case 'runCommand':
      return `执行 ${count} 条命令`
    case 'webFetch':
      return `读取 ${count} 个网页`
    case 'listDir':
      return `浏览 ${count} 个目录`
    case 'fileOps':
      return `处理 ${count} 个文件`
    case 'codeSearch':
      return '搜索代码'
    case 'webSearch':
      return '搜索网络'
    case 'globFiles':
      return '查找文件'
    case 'todo':
      return '更新任务清单'
    case 'memory':
      return '检索记忆'
    case 'subAgent':
      return '调度 Subagent'
    case 'skill':
      return '运行技能'
    case 'image':
      return '处理图像'
    case 'notion':
      return '检索 Notion'
    case 'mcp':
      return '调用外部工具'
    case 'other':
    default:
      return '工具调用'
  }
}

/** 代表类别：单一有意义类别时取该类别，混合/未知时回退 `'other'`（与文案选择保持一致）。 */
function representativeCategory(categories: ToolGroupCategory[]): ToolGroupCategory {
  const meaningful = meaningfulCategories(categories)
  return meaningful.length === 1 ? meaningful[0] : 'other'
}

export interface ToolGroupSummary {
  text: string
  status: 'running' | 'error' | 'done'
  /** 折叠头图标用的代表类别。 */
  icon: ToolGroupIcon
  /** 组内涉及的「有意义类别」列表（去重、保持首次出现顺序、剔除 `'other'`）。
   *  混合类别时用于在摘要后排一行各类工具图标；纯 reasoning 组为 `[]`。 */
  categories: ToolGroupIcon[]
  /** 组内所有 Edit/Write 的 `+N -N` 总计；没有文件写入（或行数全为 0）时为 null。 */
  diffStats: DiffStats | null
}

/**
 * 为一个分组生成 Codex 风格的自然语言摘要：动词 + 数量 + 宾语。
 * - 纯 reasoning 组：done → `思考`；running → `正在思考…`。
 * - 有意义类别 1 个：单个动作片段；2 个：用「和」连接；0 个或 ≥3 个：`调用 N 次工具`。
 * - done 时片段直接用原形（不加「已」）；running 时前缀「正在」且整体以「…」结尾。
 * - 失败（仅 done 态）：整体末尾追加 `，N 项失败`。
 * `status` 字段保留供 MessageBubble 做流光/失败判定。
 */
export function summarizeToolGroup(
  segments: ChatMessageSegment[],
  toolCalls: ToolCallRecord[],
  toolCallById?: ReadonlyMap<string, ToolCallRecord>,
): ToolGroupSummary {
  const toolSegments = segments.filter((segment) => segment.kind === 'tool')
  // 「步数」按工具步计；纯 reasoning 组（无工具）回退到总段数。
  const stepCount = toolSegments.length || segments.length
  const matchedTools: ToolCallRecord[] = []
  for (const segment of toolSegments) {
    const id = segmentToolCallId(segment)
    const record = toolCallById
      ? toolCallById.get(id)
      : toolCalls.find((tool) => toolRecordId(tool) === id)
    if (record) matchedTools.push(record)
  }

  const categories = matchedTools.map((tool) => categorizeTool(tool))
  const meaningful = meaningfulCategories(categories)

  // 图标代表类别：无工具段（纯 reasoning 组）→ 'reasoning'；否则取代表类别。
  const icon: ToolGroupIcon = toolSegments.length
    ? representativeCategory(categories)
    : 'reasoning'

  const running = matchedTools.some((tool) => normalizeToolCallStatus(tool.status) === 'running')
  const failed = matchedTools.filter((tool) => normalizeToolCallStatus(tool.status) === 'error').length

  const status: ToolGroupSummary['status'] = running ? 'running' : failed > 0 ? 'error' : 'done'

  // 选出本组的「动作片段」数组（不带时态前缀）。
  const fragments = buildGroupFragments(categories, meaningful, toolSegments.length, stepCount)

  // running 时每个片段前缀「正在」且整体以「…」结尾；done 时片段直接用原形（不加「已」）。
  let text: string
  if (running) {
    text = `${fragments.map((fragment) => `正在${fragment}`).join('和')}…`
  } else {
    text = fragments.join('和')
    if (failed > 0) {
      text = `${text}，${failed} 项失败`
    }
  }

  return {
    text,
    status,
    icon,
    categories: meaningful,
    diffStats: matchedTools.reduce<DiffStats | null>((total, tool) => {
      const stats = toolCallDiffStats(tool)
      if (!stats) return total
      return {
        additions: (total?.additions ?? 0) + stats.additions,
        removals: (total?.removals ?? 0) + stats.removals,
      }
    }, null),
  }
}

/**
 * 选出一个分组的「动作片段」数组（不带时态前缀/状态后缀）。
 * - 纯 reasoning 组（无 tool 段）：`['思考']`。
 * - 有意义类别 m===0（全 other/未知）：`['调用 N 次工具']`。
 * - m===1：该类别片段（带其自身工具数）。
 * - m===2：两个片段（各带自身工具数）。
 * - m>=3：`['调用 N 次工具']`（类别太多不逐一列，图标排已展示种类）。
 */
function buildGroupFragments(
  categories: ToolGroupCategory[],
  meaningful: ToolGroupCategory[],
  toolSegmentCount: number,
  stepCount: number,
): string[] {
  if (toolSegmentCount === 0) return ['思考']
  if (meaningful.length === 0 || meaningful.length >= 3) {
    return [`调用 ${stepCount} 次工具`]
  }
  // 按类别统计工具数。
  const counts = new Map<ToolGroupCategory, number>()
  for (const category of categories) {
    counts.set(category, (counts.get(category) ?? 0) + 1)
  }
  return meaningful.map((category) => categoryFragment(category, counts.get(category) ?? 0))
}
