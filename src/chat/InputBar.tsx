import { cloneElement, isValidElement, memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { getCurrentWindow } from '@tauri-apps/api/window'
import {
  ArrowUp,
  Archive,
  Check,
  ChevronDown,
  CircleHelp,
  Eraser,
  Folder,
  FolderPlus,
  Layers,
  ListChecks,
  MessageSquarePlus,
  Network,
  Paperclip,
  Plus,
  Search,
  Settings,
  Sparkles,
  Square,
  Terminal,
  TextQuote,
  Wrench,
  X,
} from 'lucide-react'
import { ChatAttachments } from './ChatAttachments'
import { PastedTextEditorModal } from './PastedTextEditorModal'
import { SourcesButton } from './SourcesButton'
import { onComposerInsert, onComposerTextInsert } from './composerInsert'
import { draftKey, getComposerDraft, migrateNewChatDraft, setComposerDraft } from './composerDraft'
import { applyComposerAutoHeight } from './composerAutoHeight'
import { AssistantPicker } from './AssistantPicker'
import { MultiModelSelector } from './MultiModelSelector'
import { GitStatusPill } from './dock/GitStatusPill'
import { GitDiffChip } from './dock/GitDiffChip'
import { AgentTodoIndicator } from './AgentTodoIndicator'
import { Button, IconButton } from '../components/Button'
import { useT, type I18n, type Lang } from '../settings/i18n'
import { api, type ChatToolDefinition, type ChatMcpServer } from '../api/tauri'
import { chatApi } from './api'
import type { AgentPlanMode, AgentPlanState, AgentTodoState, ChatAssistant, ChatProject, ChatSet, ModelRef, PendingAttachment, WebSearchMode } from './types'
import {
  buildSlashCommands,
  commandMatches,
  shouldOpenSlashPopover,
  matchComposerSlashCommand,
  type SlashCommandDefinition,
  type SlashSkill,
} from './slashCommands'
import { mapExternalCliSlashCommands, externalCliAgentLabel } from './externalCliSlashCommands'
import type { ModeOption, ModeTone } from './permissionModes'
import { isTauriRuntime } from './utils'

const IMAGE_EXTENSIONS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'tiff', 'tif', 'heic', 'heif']
// 粘贴文本超过该字符数时不再写入输入框，转为内存虚拟 txt 附件（默认 3000，可配置阈值）。
const PASTE_TEXT_ATTACHMENT_THRESHOLD = 3000

function isAttachableClipboardFile(file: File): boolean {
  return Boolean(file.name?.trim()) || file.size > 0
}

function undoAccidentalFilenamePaste(
  textarea: HTMLTextAreaElement,
  valueBeforePaste: string,
  clipText: string,
  selectionStart: number,
  selectionEnd: number,
  setValue: (value: string) => void,
) {
  if (!clipText.trim()) return

  const currentValue = textarea.value
  const expectedAfterPaste = `${valueBeforePaste.slice(0, selectionStart)}${clipText}${valueBeforePaste.slice(selectionEnd)}`
  if (currentValue !== expectedAfterPaste) return

  const cleaned = `${valueBeforePaste.slice(0, selectionStart)}${valueBeforePaste.slice(selectionEnd)}`
  setValue(cleaned)
  requestAnimationFrame(() => {
    textarea.value = cleaned
    textarea.selectionStart = selectionStart
    textarea.selectionEnd = selectionStart
    applyComposerAutoHeight(textarea)
  })
}

function shouldComposerAutoFocus(activeElement: Element | null): boolean {
  if (!activeElement || activeElement === document.body || activeElement === document.documentElement) {
    return true
  }
  // Never steal focus from a text field the user may be typing in — the composer
  // itself, the search box, a conversation-rename field, another textarea, or any
  // contenteditable.
  if (
    activeElement instanceof HTMLTextAreaElement ||
    activeElement instanceof HTMLInputElement ||
    (activeElement instanceof HTMLElement && activeElement.isContentEditable)
  ) {
    return false
  }
  // Any other focused element is non-typeable (button / link / div). On macOS the
  // webview auto-focuses the first tabbable control — the sidebar toggle — when the
  // window opens, leaving it ring-highlighted instead of the composer. Treat that
  // (and any such default focus) as safe to move to the composer.
  return true
}

function isExternalMcpTool(tool: ChatToolDefinition): boolean {
  return tool.source !== 'skill' && tool.source !== 'native'
}

// MCP 官方标志（Model Context Protocol，路径取自官方 logo，viewBox 180）。
// 描边用 currentColor 跟随主题，粗细换算到与 lucide 18px 图标视重一致。
function projectPathLabel(project: ChatProject): string {
  const rootPath = project.root_path ?? project.rootPath ?? ''
  if (!rootPath) return ''
  const normalized = rootPath.replace(/\\/g, '/')
  return normalized.split('/').filter(Boolean).pop() ?? rootPath
}

function pathTail(path: string): string {
  const normalized = path.replace(/\\/g, '/')
  return normalized.split('/').filter(Boolean).pop() ?? path
}

function joinPath(parent: string, name: string): string {
  const sep = parent.includes('\\') && !parent.includes('/') ? '\\' : '/'
  return `${parent.replace(/[\\/]+$/, '')}${sep}${name}`
}

function projectUpdatedAt(project: ChatProject): number {
  return project.updated_at ?? project.updatedAt ?? project.created_at ?? project.createdAt ?? 0
}

function nextBlankProjectName(projects: ChatProject[], t: I18n): string {
  const names = new Set(projects.map((project) => project.name))
  const base = t.chatDefaultProjectName
  if (!names.has(base)) return base
  for (let index = 2; index < 100; index += 1) {
    const name = `${base} ${index}`
    if (!names.has(name)) return name
  }
  return `${base} ${Date.now()}`
}

type SlashCommandId =
  | 'help'
  | 'plan'
  | 'orchestrate'
  | 'new'
  | 'compact'
  | 'clear'
  | 'settings'
  | 'tools'
  | 'attach'
type LocalSlashCommand = SlashCommandDefinition & { id: SlashCommandId; kind: 'action' }

interface ActiveSlashToken {
  start: number
  end: number
  query: string
}

const LOCAL_SLASH_COMMANDS: LocalSlashCommand[] = [
  {
    id: 'help',
    slash: '/help',
    title: '/help',
    description: 'Show commands',
    category: 'Local',
    kind: 'action',
    keywords: ['help', 'commands', '帮助', '命令'],
  },
  {
    id: 'plan',
    slash: '/plan',
    title: '/plan',
    description: 'Enter plan mode',
    category: 'Local',
    kind: 'action',
    keywords: ['plan', 'act', 'mode', '计划', '模式', '切换'],
  },
  {
    id: 'orchestrate',
    slash: '/orchestrate',
    title: '/orchestrate',
    description: 'Enter orchestrate mode (proactive subagents)',
    category: 'Local',
    kind: 'action',
    keywords: ['orchestrate', 'agent', 'subagent', 'fanout', 'mode', '编排', 'subagents', '子代理', '模式', '切换'],
  },
  {
    id: 'new',
    slash: '/new',
    title: '/new',
    description: 'Start a new chat',
    category: 'Local',
    kind: 'action',
    keywords: ['new', 'chat', 'conversation', '新建', '新对话'],
  },
  {
    id: 'compact',
    slash: '/compact',
    title: '/compact',
    description: 'Compress context',
    category: 'Local',
    kind: 'action',
    keywords: ['compact', 'compress', 'context', '压缩', '上下文'],
  },
  {
    id: 'clear',
    slash: '/clear',
    title: '/clear',
    description: 'Clear current chat',
    category: 'Local',
    kind: 'action',
    keywords: ['clear', 'delete', 'reset', '清空', '删除', '重置'],
  },
  {
    id: 'settings',
    slash: '/settings',
    title: '/settings',
    description: 'Open chat settings',
    category: 'Local',
    kind: 'action',
    keywords: ['settings', 'config', '设置', '配置'],
  },
  {
    id: 'tools',
    slash: '/tools',
    title: '/tools',
    description: 'Show tool status',
    category: 'Local',
    kind: 'action',
    keywords: ['tools', 'mcp', 'skill', '工具', '技能'],
  },
  {
    id: 'attach',
    slash: '/attach',
    title: '/attach',
    description: 'Add files or images',
    category: 'Local',
    kind: 'action',
    keywords: ['attach', 'file', 'image', '附件', '文件', '图片'],
  },
]

function slashCommandIcon(command: SlashCommandDefinition) {
  if (command.kind === 'skill') {
    return Sparkles
  }
  if (command.kind === 'cli') {
    return Terminal
  }
  switch (command.id as SlashCommandId) {
    case 'help':
      return CircleHelp
    case 'plan':
      return ListChecks
    case 'orchestrate':
      return Network
    case 'new':
      return MessageSquarePlus
    case 'compact':
      return Archive
    case 'clear':
      return Eraser
    case 'settings':
      return Settings
    case 'tools':
      return Wrench
    case 'attach':
      return Paperclip
    default:
      return Sparkles
  }
}

// pill 颜色呼应输入框边框：Act=neutral、Plan=emerald、Orchestrate=violet。
// 档位表由 Chat 传入（内置三档 / 本地 CLI 档位），这里只按 tone 取样式。
const MODE_PILL_CLASS: Record<ModeTone, { idle: string; iconColor: string }> = {
  neutral: {
    idle: 'text-neutral-600 hover:bg-neutral-200/60 dark:text-neutral-300 dark:hover:bg-neutral-700/55',
    iconColor: 'text-neutral-500 dark:text-neutral-300',
  },
  emerald: {
    idle: 'text-emerald-600 hover:bg-emerald-500/10 dark:text-emerald-400 dark:hover:bg-emerald-400/10',
    iconColor: 'text-emerald-500 dark:text-emerald-400',
  },
  violet: {
    idle: 'text-violet-600 hover:bg-violet-500/10 dark:text-violet-400 dark:hover:bg-violet-400/10',
    iconColor: 'text-violet-500 dark:text-violet-400',
  },
}

function findActiveSlashToken(value: string, cursor: number): ActiveSlashToken | null {
  if (cursor < 0 || cursor > value.length) return null

  let start = cursor
  while (start > 0 && !/\s/.test(value[start - 1])) {
    start -= 1
  }

  const token = value.slice(start, cursor)
  if (!token.startsWith('/')) return null
  if (start > 0 && !/\s/.test(value[start - 1])) return null
  if (token.slice(1).includes('/')) return null

  return {
    start,
    end: cursor,
    query: token.slice(1),
  }
}

function imageExtensionForMime(mimeType: string): string {
  switch (mimeType.toLowerCase()) {
    case 'image/jpeg':
      return 'jpg'
    case 'image/gif':
      return 'gif'
    case 'image/webp':
      return 'webp'
    case 'image/bmp':
      return 'bmp'
    case 'image/tiff':
      return 'tiff'
    case 'image/heic':
      return 'heic'
    case 'image/heif':
      return 'heif'
    case 'image/png':
    default:
      return 'png'
  }
}

function readFileAsBase64(file: File, readError: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = typeof reader.result === 'string' ? reader.result : ''
      resolve(result.split(',')[1] ?? '')
    }
    reader.onerror = () => reject(reader.error ?? new Error(readError))
    reader.readAsDataURL(file)
  })
}

export interface InputBarProps {
  /**
   * 返回 false 表示发送未被接受；此时保留输入草稿和附件。
   * 长任务应在消息正式进入发送流程时调用 onAccepted，让输入框立即清空，无需等待整轮生成完成。
   */
  onSend: (
    content: string,
    attachments: PendingAttachment[],
    options?: { onAccepted?: () => void },
  ) => boolean | void | Promise<boolean | void>
  disabled?: boolean
  /**
   * 生成中（`disabled`）时的排队入口。传了它 = 运行中不再吞掉发送，而是把这条排进队列，
   * 本轮结束后自动发出（也可以在队列条上点「立刻引导」注入当前运行）。
   * 传了它还会放开附件相关的门（选择 / 粘贴 / 拖入）——那些是纯本地状态，运行中做没有风险；
   * 斜杠命令 / 项目 / 模式 / 知识库仍按 `disabled` 锁住（它们要打后端）。
   */
  onQueue?: (content: string, attachments: PendingAttachment[]) => void
  onCancel?: () => void
  cancelVisible?: boolean
  cancelling?: boolean
  onOpenSettings?: () => void
  onOpenTools?: () => void
  onNewChat?: () => void | Promise<void>
  onCompactContext?: () => void | Promise<void>
  onClearChat?: () => void | Promise<void>
  enabledTools?: ChatToolDefinition[]
  toolsDisabledReason?: string
  toolStatusHint?: string
  sendDisabledReason?: string
  agentPlanState?: AgentPlanState | null
  agentTodoState?: AgentTodoState | null
  onAgentPlanModeChange?: (mode: AgentPlanMode) => void | Promise<void>
  enabledSkills?: SlashSkill[]
  onOpenSkillSettings?: () => void
  selectedProject?: ChatProject | null
  // 当前会话自身所属的项目（id + 名）。用于在没有 selectedProject（导航态）时，
  // 让项目按钮仍反映"这条会话属于哪个项目"——例如从「最近」打开一条项目内的对话。
  conversationProject?: { id: string; name: string } | null
  onSelectProject?: (project: ChatProject | null) => void | Promise<void>
  showProjectEntry?: boolean
  /** 导航态选中的集；与项目互斥。空对话时显示 Start in / 状态条。 */
  selectedSet?: ChatSet | null
  onSelectSet?: (set: ChatSet | null) => void | Promise<void>
  /** 当前生效的专家(无则为空);显示在底部栏 */
  currentAssistant?: { id: string; name: string } | null
  onOpenAssistantCenter?: () => void
  /** 从底栏弹层选择专家：应用到当前会话，或（无会话时）以该专家开新对话；null=清除 */
  onSelectAssistant?: (assistant: ChatAssistant | null) => void | Promise<void>
  autoFocus?: boolean
  /** footer：贴底（有消息时）；inline：嵌入居中区域（空对话欢迎页） */
  layout?: 'footer' | 'inline'
  /** 外部 CLI 模式：斜杠命令直通 Agent，不展示 Kivio 弹层 */
  usesExternalRuntime?: boolean
  /** Kivio Chat：不提供 /plan /orchestrate / 技能斜杠（那些是 Agent 能力） */
  usesChatRuntime?: boolean
  externalAgentName?: string | null
  conversationId?: string | null
  /** 本会话挂载的知识库 id；缺省时 knowledge_search 检索全部库 */
  knowledgeBaseIds?: string[]
  onChangeKnowledgeBaseIds?: (ids: string[]) => void | Promise<void>
  /** 强制检索：开启后要求模型每次回答前必先调 knowledge_search。 */
  forceKnowledgeSearch?: boolean
  onToggleForceKnowledgeSearch?: () => void | Promise<void>
  /** 已配置的 MCP 服务器；底栏「来源」弹层切换各服务器 enabled(是否加载) */
  mcpServers?: ChatMcpServer[]
  onToggleMcpServer?: (serverId: string) => void | Promise<void>
  /** 网络搜索全局开关（nativeTools.webSearch）；在「来源」弹层内切换 */
  webSearchMode?: WebSearchMode
  onSetWebSearchMode?: (mode: WebSearchMode) => void | Promise<void>
  builtinWebSearchSupported?: boolean
  /** 多答模型集（会话级 reply_models / replyModels；0/1 个=单模型，≥2=一问多答） */
  replyModels?: ModelRef[]
  onChangeReplyModels?: (models: ModelRef[]) => void | Promise<void>
  /** 上下文用量指示器：由 Chat 注入 <ContextIndicator>，渲染在底栏右侧 Act 右边 */
  contextSlot?: ReactNode
  /** 底栏模式胶囊的档位表，由 Chat 算好传入（内置会话 = Kivio 三档；本地 CLI 会话 =
   *  该 CLI 自己的沙盒/权限档位）。空表 = 无档位可选，胶囊整个不渲染。 */
  modeOptions?: ModeOption[]
  modeValue?: string
  onModeChange?: (value: string) => void | Promise<void>
  /** dsh Agent 模式胶囊，画在权限胶囊左边。空表 = 不渲染。 */
  presetOptions?: ModeOption[]
  presetValue?: string
  onPresetChange?: (value: string) => void | Promise<void>
  /** 已有对话内容后锁定（dsh 只允许空白 agent 换 preset）。 */
  presetLocked?: boolean
  presetLockedReason?: string
  /** Git 状态胶囊：Dock 解析出的工作目录；空则不渲染 */
  gitWorkdir?: string | null
  gitLang?: Lang
  /** 胶囊卡片里「在 Git 面板中打开」：打开 Dock 并切到 Git 页 */
  onOpenGitPanel?: () => void
  /** 功能栏右侧的用量注入区（会话输入/缓存命中/输出）：渲染在模式胶囊与上下文指示器左侧 */
  usageSlot?: ReactNode
}

export const InputBar = memo(function InputBar({
  onSend,
  disabled,
  onQueue,
  onCancel,
  cancelVisible,
  cancelling,
  onOpenSettings,
  onOpenTools,
  onNewChat,
  onCompactContext,
  onClearChat,
  enabledTools = [],
  toolsDisabledReason,
  toolStatusHint,
  sendDisabledReason,
  agentPlanState = null,
  agentTodoState = null,
  onAgentPlanModeChange,
  enabledSkills = [],
  onOpenSkillSettings,
  selectedProject = null,
  conversationProject = null,
  onSelectProject,
  showProjectEntry = false,
  selectedSet = null,
  onSelectSet,
  currentAssistant = null,
  onOpenAssistantCenter,
  onSelectAssistant,
  autoFocus,
  layout = 'footer',
  usesExternalRuntime = false,
  usesChatRuntime = false,
  externalAgentName = null,
  conversationId = null,
  knowledgeBaseIds = [],
  onChangeKnowledgeBaseIds,
  forceKnowledgeSearch = false,
  onToggleForceKnowledgeSearch,
  mcpServers = [],
  onToggleMcpServer,
  webSearchMode = 'off',
  onSetWebSearchMode,
  builtinWebSearchSupported = false,
  replyModels = [],
  onChangeReplyModels,
  contextSlot,
  modeOptions = [],
  modeValue = '',
  onModeChange,
  presetOptions = [],
  presetValue = '',
  onPresetChange,
  presetLocked = false,
  presetLockedReason,
  gitWorkdir = null,
  gitLang,
  onOpenGitPanel,
  usageSlot,
}: InputBarProps) {
  const t = useT()
  // 生成中的排队模式：Enter 改成入队，且只锁「要打后端」的入口。附件的选择 / 粘贴 / 拖入
  // 是纯本地状态，运行中做没有风险，所以走 composerLocked（比 disabled 宽）这道门。
  const queueMode = Boolean(disabled && onQueue)
  const [sendPending, setSendPending] = useState(false)
  const composerLocked = (Boolean(disabled) || sendPending) && !queueMode
  const draftKeyValue = draftKey(conversationId)
  const [input, setInput] = useState(() => getComposerDraft(draftKeyValue)?.input ?? '')
  const [quotes, setQuotes] = useState<string[]>(() => getComposerDraft(draftKeyValue)?.quotes ?? [])
  const [attachments, setAttachments] = useState<PendingAttachment[]>(() => getComposerDraft(draftKeyValue)?.attachments ?? [])
  const [attachmentError, setAttachmentError] = useState('')
  const [editingAttachment, setEditingAttachment] = useState<PendingAttachment | null>(null)
  const [dragActive, setDragActive] = useState(false)
  const [toolPanelOpen, setToolPanelOpen] = useState(false)
  const [modeMenuOpen, setModeMenuOpen] = useState(false)
  const [presetMenuOpen, setPresetMenuOpen] = useState(false)
  const [projectMenuOpen, setProjectMenuOpen] = useState(false)
  const [projectOptions, setProjectOptions] = useState<ChatProject[]>([])
  const [projectOptionsLoading, setProjectOptionsLoading] = useState(false)
  const [projectOptionsError, setProjectOptionsError] = useState('')
  const [projectSearchQuery, setProjectSearchQuery] = useState('')
  const [projectCreating, setProjectCreating] = useState(false)
  const [slashPanelOpen, setSlashPanelOpen] = useState(false)
  const [slashSelectedIndex, setSlashSelectedIndex] = useState(0)
  const [activeSlashToken, setActiveSlashToken] = useState<ActiveSlashToken | null>(null)
  const [externalCliSlashCommands, setExternalCliSlashCommands] = useState<SlashCommandDefinition[]>([])
  const [externalCliSlashHint, setExternalCliSlashHint] = useState<string | null>(null)
  const [externalCliSlashLoading, setExternalCliSlashLoading] = useState(false)
  const [slashPanelLeft, setSlashPanelLeft] = useState(0)
  const innerRef = useRef<HTMLDivElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const slashHighlightRef = useRef<HTMLDivElement>(null)
  // 草稿持久化：会话 key 变化（切对话且未卸载）时载入对应草稿；每次内容变化写回内存 store。
  // keyRef 保证写回落到当前会话，不串到刚切走的会话。
  const draftKeyRef = useRef(draftKeyValue)
  // 发送等待期间如果「新会话占位键」迁移成真实会话 id，清理目标也要跟着迁移；
  // 但普通切会话没有发生草稿迁移时，绝不能误清新会话自己的草稿。
  const sendingDraftKeyRef = useRef<string | null>(null)
  useEffect(() => {
    if (draftKeyRef.current === draftKeyValue) return
    const prevKey = draftKeyRef.current
    draftKeyRef.current = draftKeyValue
    // 新建会话刚落库拿到 id（切 plan/orchestrate 模式等会触发）：草稿跟着搬过去，
    // 本地 state 已经是那份内容，直接返回，别当成「切到了另一条会话」把字清掉。
    if (migrateNewChatDraft(prevKey, draftKeyValue)) {
      if (sendingDraftKeyRef.current === prevKey) {
        sendingDraftKeyRef.current = draftKeyValue
      }
      return
    }
    const d = getComposerDraft(draftKeyValue)
    setInput(d?.input ?? '')
    setQuotes(d?.quotes ?? [])
    setAttachments(d?.attachments ?? [])
  }, [draftKeyValue])
  useEffect(() => {
    setComposerDraft(draftKeyRef.current, { input, quotes, attachments })
  }, [input, quotes, attachments])
  const agentPlanMode = agentPlanState?.mode ?? 'act'
  const agentPlanActive = agentPlanMode === 'plan'
  const agentOrchestrateActive = agentPlanMode === 'orchestrate'
  const projectEntryEnabled = Boolean(showProjectEntry && onSelectProject)
  // 项目按钮的显示态：优先导航选中的项目；否则回退到当前会话自身的项目（有名才算），
  // 这样从「最近」打开一条属于项目的对话时，按钮仍能显示该项目。
  const effectiveProject: { id: string; name: string } | null =
    selectedProject ?? (conversationProject?.name ? conversationProject : null)
  // 集：导航态选中的集（项目优先；两者在侧栏互斥）。
  const effectiveSet: { id: string; name: string } | null =
    effectiveProject ? null : (selectedSet ? { id: selectedSet.id, name: selectedSet.name } : null)
  // 专家入口:欢迎页与对话中都显示,未选时为「选择专家」图标,已选时高亮 + 清除按钮。
  const showAssistantEntry = Boolean(onOpenAssistantCenter)
  const modeEntryEnabled = Boolean(onModeChange) && modeOptions.length > 0
  const presetEntryEnabled = Boolean(onPresetChange) && presetOptions.length > 0
  // 状态条只放「你在哪」—— 当前项目或集。Git 分支/diff 归下面的工具栏。
  const gitStatusEnabled = Boolean(gitWorkdir && gitLang && onOpenGitPanel)
  const todoBarVisible = (agentTodoState?.items?.length ?? 0) > 0
  const statusBarVisible = Boolean(
    (projectEntryEnabled && effectiveProject) || effectiveSet,
  )
  const activeModeOption = modeOptions.find((option) => option.value === modeValue) ?? modeOptions[0]
  const activeModePillClass = MODE_PILL_CLASS[activeModeOption?.tone ?? 'neutral']
  const activePresetOption = presetOptions.find((option) => option.value === presetValue) ?? presetOptions[0]
  const activePresetPillClass = MODE_PILL_CLASS[activePresetOption?.tone ?? 'neutral']

  const closeProjectMenu = useCallback(() => {
    setProjectMenuOpen(false)
  }, [])

  const closeModeMenu = useCallback(() => {
    setModeMenuOpen(false)
  }, [])

  const closePresetMenu = useCallback(() => {
    setPresetMenuOpen(false)
  }, [])

  const attachmentsFromPaths = useCallback(
    (paths: string[]) =>
      paths.map((path) => {
        const normalized = path.replace(/\\/g, '/')
        const name = normalized.split('/').filter(Boolean).pop() || t.chatAttachmentFallbackName
        const ext = name.split('.').pop()?.toLowerCase() ?? ''
        const type: PendingAttachment['type'] = IMAGE_EXTENSIONS.includes(ext) ? 'image' : 'file'
        return {
          id: `pending-att-${crypto.randomUUID()}`,
          type,
          name,
          path,
        }
      }),
    [t],
  )

  const loadProjectOptions = useCallback(async () => {
    if (!projectEntryEnabled) return
    setProjectOptionsLoading(true)
    setProjectOptionsError('')
    try {
      setProjectOptions(await chatApi.getProjects())
    } catch (err) {
      console.error('Failed to load chat projects:', err)
      setProjectOptionsError(typeof err === 'string' ? err : err instanceof Error ? err.message : t.chatProjectLoadFailed)
    } finally {
      setProjectOptionsLoading(false)
    }
  }, [projectEntryEnabled, t])

  const toggleProjectMenu = useCallback(() => {
    if (!projectEntryEnabled || disabled) return
    setSlashPanelOpen(false)
    setToolPanelOpen(false)
    setModeMenuOpen(false)
    setProjectMenuOpen((open) => {
      const nextOpen = !open
      if (nextOpen) {
        setProjectSearchQuery('')
        void loadProjectOptions()
      }
      return nextOpen
    })
  }, [disabled, loadProjectOptions, projectEntryEnabled])

  const selectProject = useCallback(async (project: ChatProject | null) => {
    if (!onSelectProject) return
    closeProjectMenu()
    await onSelectProject(project)
    requestAnimationFrame(() => textareaRef.current?.focus({ preventScroll: true }))
  }, [closeProjectMenu, onSelectProject])

  const createBlankProject = useCallback(async () => {
    if (!onSelectProject || disabled || projectCreating) return
    setProjectOptionsError('')
    setProjectCreating(true)
    try {
      // 空白项目必须落到真实文件夹：先选父目录，再在其中 mkdir 并登记项目。
      const picked = await open({
        directory: true,
        multiple: false,
        title: t.chatPickBlankProjectLocation,
      })
      const parentPath = Array.isArray(picked) ? picked[0] : picked
      if (!parentPath) return

      const name = nextBlankProjectName(projectOptions, t)
      const rootPath = joinPath(parentPath, name)
      const project = await chatApi.createProject(name, null, null, rootPath, { ensureRootDir: true })
      setProjectOptions((prev) => [
        project,
        ...prev.filter((item) => item.id !== project.id),
      ])
      closeProjectMenu()
      await onSelectProject(project)
    } catch (err) {
      console.error('Failed to create blank chat project from input bar:', err)
      setProjectOptionsError(typeof err === 'string' ? err : err instanceof Error ? err.message : t.chatProjectCreateFailed)
    } finally {
      setProjectCreating(false)
      requestAnimationFrame(() => textareaRef.current?.focus({ preventScroll: true }))
    }
  }, [closeProjectMenu, disabled, onSelectProject, projectCreating, projectOptions, t])

  const createProjectFromFolder = useCallback(async () => {
    if (!onSelectProject || disabled || projectCreating) return
    setProjectOptionsError('')
    setProjectCreating(true)
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: t.chatPickProjectFolder,
      })
      const rootPath = Array.isArray(picked) ? picked[0] : picked
      if (!rootPath) return
      const project = await chatApi.createProject(pathTail(rootPath), null, null, rootPath)
      setProjectOptions((prev) => [
        project,
        ...prev.filter((item) => item.id !== project.id),
      ])
      closeProjectMenu()
      await onSelectProject(project)
    } catch (err) {
      console.error('Failed to create chat project from input bar:', err)
      setProjectOptionsError(typeof err === 'string' ? err : err instanceof Error ? err.message : t.chatProjectCreateFailed)
    } finally {
      setProjectCreating(false)
      requestAnimationFrame(() => textareaRef.current?.focus({ preventScroll: true }))
    }
  }, [closeProjectMenu, disabled, onSelectProject, projectCreating, t])

  const updateTextareaHeight = useCallback(() => {
    const textarea = textareaRef.current
    if (!textarea) return
    applyComposerAutoHeight(textarea)
  }, [])

  // 高度/滚动条是 input 的纯函数，统一在这里跟。原来每条改 input 的路径各自补一次
  // requestAnimationFrame(updateTextareaHeight)，草稿回填那条（见上方 draftKeyValue effect）
  // 漏了 —— 切走再切回时框子还留着上一条会话的高度且 overflowY:hidden，下半截文本看不到也滚不动。
  // 用 layout effect 而非 rAF：DOM 提交后、绘制前跑完，不闪；也不必在每个 setInput 后手动记得调。
  useLayoutEffect(updateTextareaHeight, [input, updateTextareaHeight])

  // 消息区「添加到聊天」：把选中文字作为引用卡片挂到输入框上方（发送时才拼进正文）。
  const insertQuoteFromSelection = useCallback((text: string) => {
    const trimmed = text.trim()
    if (!trimmed) return
    setQuotes((prev) => [...prev, trimmed])
    requestAnimationFrame(() => textareaRef.current?.focus({ preventScroll: true }))
  }, [])

  useEffect(() => onComposerInsert(insertQuoteFromSelection), [insertQuoteFromSelection])

  // Right Dock「插入 @ 引用」等：文本直接追加到输入框正文（与引用卡片信道并列）。
  const insertTextAtEnd = useCallback((text: string) => {
    if (!text) return
    setInput((prev) => {
      const needsSpace = prev.length > 0 && !prev.endsWith(' ') && !prev.endsWith('\n')
      return `${prev}${needsSpace ? ' ' : ''}${text}`
    })
    requestAnimationFrame(() => {
      const textarea = textareaRef.current
      if (textarea) {
        textarea.focus({ preventScroll: true })
        textarea.selectionStart = textarea.value.length
        textarea.selectionEnd = textarea.value.length
      }
    })
  }, [])

  useEffect(() => onComposerTextInsert(insertTextAtEnd), [insertTextAtEnd])

  const syncSlashToken = useCallback((value: string, cursor: number) => {
    const token = findActiveSlashToken(value, cursor)
    setActiveSlashToken(token)
    if (token && shouldOpenSlashPopover()) {
      setSlashPanelOpen(true)
      setToolPanelOpen(false)
      closeProjectMenu()
    } else {
      setSlashPanelOpen(false)
    }
  }, [closeProjectMenu])

  const allSlashCommands = useMemo(
    () => {
      if (usesExternalRuntime) return externalCliSlashCommands
      const local = usesChatRuntime
        ? LOCAL_SLASH_COMMANDS.filter((command) => command.id !== 'plan' && command.id !== 'orchestrate')
        : LOCAL_SLASH_COMMANDS
      return buildSlashCommands(local, usesChatRuntime ? [] : enabledSkills)
    },
    [enabledSkills, externalCliSlashCommands, usesChatRuntime, usesExternalRuntime],
  )

  useEffect(() => {
    if (!usesExternalRuntime || !externalAgentName) {
      setExternalCliSlashCommands([])
      setExternalCliSlashHint(null)
      setExternalCliSlashLoading(false)
      return
    }

    let cancelled = false
    setExternalCliSlashLoading(true)
    void chatApi.listExternalCliSlashCommands(externalAgentName, conversationId)
      .then((result) => {
        if (cancelled) return
        setExternalCliSlashCommands(mapExternalCliSlashCommands(externalAgentName, result.commands))
        setExternalCliSlashHint(result.message ?? null)
      })
      .catch((err) => {
        if (cancelled) return
        setExternalCliSlashCommands([])
        setExternalCliSlashHint(
          typeof err === 'string' ? err : err instanceof Error ? err.message : t.chatCliCommandsLoadFailed,
        )
      })
      .finally(() => {
        if (!cancelled) setExternalCliSlashLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [conversationId, externalAgentName, usesExternalRuntime, t])

  useEffect(() => {
    if (!slashPanelOpen || !usesExternalRuntime || !externalAgentName) return
    let cancelled = false
    void chatApi.listExternalCliSlashCommands(externalAgentName, conversationId)
      .then((result) => {
        if (cancelled) return
        setExternalCliSlashCommands(mapExternalCliSlashCommands(externalAgentName, result.commands))
        setExternalCliSlashHint(result.message ?? null)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [slashPanelOpen, conversationId, externalAgentName, usesExternalRuntime])
  const filteredSlashCommands = useMemo(
    () => allSlashCommands.filter((command) => (
      commandMatches(command, activeSlashToken?.query ?? '')
    )),
    [allSlashCommands, activeSlashToken?.query],
  )
  const slashHighlight = useMemo(
    () => matchComposerSlashCommand(input, allSlashCommands),
    [allSlashCommands, input],
  )

  const syncSlashHighlightScroll = useCallback(() => {
    const textarea = textareaRef.current
    const overlay = slashHighlightRef.current
    if (!textarea || !overlay) return
    overlay.scrollTop = textarea.scrollTop
    overlay.scrollLeft = textarea.scrollLeft
  }, [])
  useLayoutEffect(() => {
    syncSlashHighlightScroll()
  }, [input, slashHighlight, syncSlashHighlightScroll])
  const visibleProjectOptions = useMemo(() => {
    const query = projectSearchQuery.trim().toLowerCase()
    return [...projectOptions]
      .sort((a, b) => projectUpdatedAt(b) - projectUpdatedAt(a))
      .filter((project) => {
        if (!query) return true
        const rootPath = project.root_path ?? project.rootPath ?? ''
        return project.name.toLowerCase().includes(query) || rootPath.toLowerCase().includes(query)
      })
      .slice(0, 8)
  }, [projectOptions, projectSearchQuery])

  const removeActiveSlashToken = useCallback(() => {
    const token = activeSlashToken
    if (!token) {
      setInput('')
      return
    }

    setInput((prev) => {
      const next = `${prev.slice(0, token.start)}${prev.slice(token.end)}`.replace(/^\s+/, '')
      requestAnimationFrame(() => {
        const textarea = textareaRef.current
        if (!textarea) return
        textarea.selectionStart = Math.min(token.start, next.length)
        textarea.selectionEnd = Math.min(token.start, next.length)
      })
      return next
    })
  }, [activeSlashToken])

  const completeActiveSlashToken = useCallback((command: SlashCommandDefinition) => {
    const token = activeSlashToken
    if (!token) return

    const cursor = token.start + command.slash.length
    setInput((prev) => {
      const next = `${prev.slice(0, token.start)}${command.slash}${prev.slice(token.end)}`
      requestAnimationFrame(() => {
        const textarea = textareaRef.current
        if (!textarea) return
        textarea.focus({ preventScroll: true })
        textarea.selectionStart = cursor
        textarea.selectionEnd = cursor
      })
      return next
    })
    setActiveSlashToken({
      start: token.start,
      end: cursor,
      query: command.slash.slice(1),
    })
    setSlashPanelOpen(true)
  }, [activeSlashToken])

  // Skill commands complete to `/name ` (trailing space) and close the popover
  // so the user types arguments; the whole string is sent on Enter and parsed
  // by the backend slash-trigger preprocessing.
  const completeSkillSlashToken = useCallback((command: SlashCommandDefinition) => {
    const token = activeSlashToken
    if (!token) return

    const insertion = `${command.slash} `
    const cursor = token.start + insertion.length
    setInput((prev) => {
      const next = `${prev.slice(0, token.start)}${insertion}${prev.slice(token.end)}`
      requestAnimationFrame(() => {
        const textarea = textareaRef.current
        if (!textarea) return
        textarea.focus({ preventScroll: true })
        textarea.selectionStart = cursor
        textarea.selectionEnd = cursor
      })
      return next
    })
    setActiveSlashToken(null)
    setSlashPanelOpen(false)
  }, [activeSlashToken])

  const selectedSlashCommand = filteredSlashCommands[slashSelectedIndex]
    ?? filteredSlashCommands[0]

  const addAttachments = useCallback(
    (next: PendingAttachment[], options?: { imagesOnly?: boolean }) => {
      const filtered = options?.imagesOnly
        ? next.filter((attachment) => attachment.type === 'image')
        : next.filter((attachment) => attachment.name.trim() !== '')
      if (filtered.length === 0) {
        setAttachmentError(options?.imagesOnly ? t.chatDropImagesOnly : t.chatNoAddableFiles)
        return
      }

      setAttachments((prev) => {
        const existing = new Set(prev.map((attachment) => attachment.path))
        const dedupedNext = filtered.filter((attachment) => {
          if (existing.has(attachment.path)) return false
          existing.add(attachment.path)
          return true
        })
        if (dedupedNext.length === 0) {
          setAttachmentError(t.chatAttachmentAdded)
          return prev
        }
        setAttachmentError('')
        return [...prev, ...dedupedNext]
      })
      textareaRef.current?.focus()
    },
    [t],
  )

  // 编辑弹窗保存：用编辑后的内容重建内存附件数据（提交时由 api 层生成新的 File/Blob 内容）。
  const updateAttachmentContent = useCallback((id: string, content: string) => {
    setAttachments((prev) => prev.map((attachment) =>
      attachment.id === id ? { ...attachment, content } : attachment
    ))
  }, [])

  const setAgentPlanMode = useCallback(async (mode: AgentPlanMode) => {
    if (disabled || !onAgentPlanModeChange) return
    setSlashPanelOpen(false)
    setToolPanelOpen(false)
    closeProjectMenu()
    closeModeMenu()
    if (agentPlanMode !== mode) {
      await onAgentPlanModeChange(mode)
    }
    requestAnimationFrame(() => {
      textareaRef.current?.focus({ preventScroll: true })
    })
  }, [agentPlanMode, closeModeMenu, closeProjectMenu, disabled, onAgentPlanModeChange])

  // 模式胶囊选档：档位表由 Chat 决定（内置三档 / 本地 CLI 档位），这里只回传选中的 value。
  const pickMode = useCallback(async (value: string) => {
    if (disabled || !onModeChange) return
    setSlashPanelOpen(false)
    setToolPanelOpen(false)
    closeProjectMenu()
    closeModeMenu()
    closePresetMenu()
    if (value !== modeValue) {
      await onModeChange(value)
    }
    requestAnimationFrame(() => {
      textareaRef.current?.focus({ preventScroll: true })
    })
  }, [closeModeMenu, closePresetMenu, closeProjectMenu, disabled, modeValue, onModeChange])

  const toggleModeMenu = useCallback(() => {
    if (disabled || !modeEntryEnabled) return
    setSlashPanelOpen(false)
    setToolPanelOpen(false)
    closeProjectMenu()
    closePresetMenu()
    setModeMenuOpen((open) => !open)
  }, [closePresetMenu, closeProjectMenu, disabled, modeEntryEnabled])

  const pickPreset = useCallback(async (value: string) => {
    if (disabled || presetLocked || !onPresetChange) return
    setSlashPanelOpen(false)
    setToolPanelOpen(false)
    closeProjectMenu()
    closeModeMenu()
    closePresetMenu()
    if (value !== presetValue) {
      await onPresetChange(value)
    }
    requestAnimationFrame(() => {
      textareaRef.current?.focus({ preventScroll: true })
    })
  }, [closeModeMenu, closePresetMenu, closeProjectMenu, disabled, onPresetChange, presetLocked, presetValue])

  const togglePresetMenu = useCallback(() => {
    if (disabled || !presetEntryEnabled) return
    setSlashPanelOpen(false)
    setToolPanelOpen(false)
    closeProjectMenu()
    closeModeMenu()
    setPresetMenuOpen((open) => !open)
  }, [closeModeMenu, closeProjectMenu, disabled, presetEntryEnabled])

  // Shift+Tab 在胶囊当前那套档位里循环，跟看得见的控件保持一致。
  const cycleMode = useCallback(async () => {
    if (modeOptions.length === 0) return
    const index = modeOptions.findIndex((option) => option.value === modeValue)
    const next = modeOptions[(index + 1) % modeOptions.length]
    await pickMode(next.value)
  }, [modeOptions, modeValue, pickMode])

  const openAttachmentPicker = useCallback(async () => {
    if (composerLocked) return
    setToolPanelOpen(false)
    closeProjectMenu()
    setSlashPanelOpen(false)
    setAttachmentError('')
    try {
      const selected = await open({
        multiple: true,
        directory: false,
      })
      const paths = Array.isArray(selected) ? selected : selected ? [selected] : []
      if (paths.length === 0) return

      addAttachments(attachmentsFromPaths(paths))
    } catch (err) {
      console.error('Failed to add chat attachment:', err)
      setAttachmentError(
        typeof err === 'string' ? err : err instanceof Error ? err.message : t.chatAttachmentAddFailed,
      )
    }
  }, [addAttachments, attachmentsFromPaths, closeProjectMenu, composerLocked, t])

  const handleSlashCommandSelect = useCallback(async (command: SlashCommandDefinition) => {
    if (disabled) return

    if (command.kind === 'skill' || command.kind === 'cli') {
      // Complete the token; user can add args then send with Enter (CLI passthrough).
      completeSkillSlashToken(command)
      return
    }

    if (command.id === 'help') {
      setInput('/')
      setActiveSlashToken({ start: 0, end: 1, query: '' })
      setSlashPanelOpen(true)
      setToolPanelOpen(false)
      closeProjectMenu()
      requestAnimationFrame(() => {
        const textarea = textareaRef.current
        if (!textarea) return
        textarea.focus({ preventScroll: true })
        textarea.selectionStart = 1
        textarea.selectionEnd = 1
      })
      return
    }

    removeActiveSlashToken()
    setSlashPanelOpen(false)

    switch (command.id) {
      case 'plan':
        await setAgentPlanMode('plan')
        return
      case 'orchestrate':
        await setAgentPlanMode('orchestrate')
        return
      case 'new':
        setInput('')
        setAttachments([])
        setAttachmentError('')
        await onNewChat?.()
        return
      case 'compact':
        await onCompactContext?.()
        return
      case 'clear':
        setInput('')
        setAttachments([])
        setAttachmentError('')
        await onClearChat?.()
        return
      case 'settings':
        onOpenSettings?.()
        return
      case 'tools':
        if (onOpenTools) {
          onOpenTools()
        } else {
          setToolPanelOpen(true)
          closeProjectMenu()
        }
        return
      case 'attach':
        await openAttachmentPicker()
        return
    }
  }, [
    disabled,
    completeSkillSlashToken,
    onClearChat,
    onCompactContext,
    onNewChat,
    onOpenSettings,
    onOpenTools,
    openAttachmentPicker,
    removeActiveSlashToken,
    setAgentPlanMode,
    closeProjectMenu,
  ])


  const clearSentDraft = (sentDraftKey: string) => {
    setComposerDraft(sentDraftKey, { input: '', quotes: [], attachments: [] })
    // 等待发送时用户可能已经切到另一条有自己草稿的会话。只清本次提交实际归属的输入框。
    if (draftKeyRef.current !== sentDraftKey) return
    setInput('')
    setQuotes([])
    setAttachments([])
    setAttachmentError('')
    setToolPanelOpen(false)
    closeProjectMenu()
    setSlashPanelOpen(false)
    if (textareaRef.current) applyComposerAutoHeight(textareaRef.current)
  }

  const handleSend = async () => {
    const trimmed = input.trim()
    if (sendPending || (!trimmed && quotes.length === 0 && attachments.length === 0) || sendDisabledReason) return
    // 生成中：有排队入口就排队（本轮结束后自动发出），没有就照旧什么都不做。
    if (disabled && !onQueue) return
    const quotedBlock = quotes
      .map((q) => q.split('\n').map((line) => `> ${line}`).join('\n'))
      .join('\n\n')
    const content = quotedBlock
      ? (trimmed ? `${quotedBlock}\n\n${trimmed}` : quotedBlock)
      : trimmed
    if (disabled && onQueue) {
      onQueue(content, attachments)
      clearSentDraft(draftKeyRef.current)
    } else {
      sendingDraftKeyRef.current = draftKeyRef.current
      setSendPending(true)
      const sentSnapshot = {
        input,
        quotes: [...quotes],
        attachments: [...attachments],
      }
      let acceptedNotified = false
      let clearedDraftKey: string | null = null
      const notifyAccepted = () => {
        if (acceptedNotified) return
        acceptedNotified = true
        // 必须读 ref：欢迎页首发会把 `__new__` 迁到真实会话 id，冻在发送开始会清错键。
        clearedDraftKey = sendingDraftKeyRef.current ?? draftKeyRef.current
        clearSentDraft(clearedDraftKey)
        sendingDraftKeyRef.current = null
        // 后端生成仍在继续，但输入框已经可以接收下一条排队消息。
        setSendPending(false)
      }
      const restoreRejectedDraft = () => {
        if (!acceptedNotified || !clearedDraftKey) return
        const typedAfterAccept = (textareaRef.current?.value ?? '').trim().length > 0
        if (draftKeyRef.current !== clearedDraftKey || typedAfterAccept) return
        setComposerDraft(clearedDraftKey, sentSnapshot)
        setInput(sentSnapshot.input)
        setQuotes(sentSnapshot.quotes)
        setAttachments(sentSnapshot.attachments)
        if (textareaRef.current) applyComposerAutoHeight(textareaRef.current)
      }
      try {
        const accepted = await onSend(content, attachments, { onAccepted: notifyAccepted })
        if (accepted === false) {
          restoreRejectedDraft()
          return
        }
        // 兼容没有“已接收”通知的普通 onSend：Promise 完成后再按旧语义清理。
        notifyAccepted()
      } catch (error) {
        console.error('Failed to submit composer message:', error)
        restoreRejectedDraft()
      } finally {
        sendingDraftKeyRef.current = null
        setSendPending(false)
      }
    }
  }

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return

    if (e.key === 'Tab' && e.shiftKey && modeEntryEnabled && !disabled) {
      e.preventDefault()
      void cycleMode()
      return
    }

    if (slashPanelOpen) {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        if (filteredSlashCommands.length > 0) {
          setSlashSelectedIndex((index) => (index + 1) % filteredSlashCommands.length)
        }
        return
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        if (filteredSlashCommands.length > 0) {
          setSlashSelectedIndex((index) => (
            index - 1 + filteredSlashCommands.length
          ) % filteredSlashCommands.length)
        }
        return
      }
      if (e.key === 'Tab') {
        e.preventDefault()
        if (selectedSlashCommand) {
          if (selectedSlashCommand.kind === 'skill') {
            completeSkillSlashToken(selectedSlashCommand)
          } else {
            completeActiveSlashToken(selectedSlashCommand)
          }
        }
        return
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        if (selectedSlashCommand) {
          void handleSlashCommandSelect(selectedSlashCommand)
        }
        return
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        setSlashPanelOpen(false)
        return
      }
    }

    // 生成中按 Esc = 点停止。只绑在输入框上（发完焦点就在这），
    // 不接全局监听——图片查看器/右键菜单/侧边栏那一堆 Esc 关闭会跟着一块触发。
    if (e.key === 'Escape' && onCancel && cancelVisible && !cancelling) {
      e.preventDefault()
      onCancel()
      return
    }

    if (e.key !== 'Enter' || e.shiftKey) return
    e.preventDefault()
    void handleSend()
  }

  const handleInput = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const nextValue = e.target.value
    setInput(nextValue)
    // 高度/滚动条由 input 的 layout effect 统一跟，这里不再内联量一遍。
    syncSlashToken(nextValue, e.target.selectionStart)
  }

  const handleSelect = (e: React.SyntheticEvent<HTMLTextAreaElement>) => {
    const el = e.currentTarget
    syncSlashToken(el.value, el.selectionStart)
  }

  const handlePaste = async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    if (composerLocked || !isTauriRuntime()) return

    const attachableClipboardFiles = Array.from(e.clipboardData.files).filter(isAttachableClipboardFile)
    const textarea = textareaRef.current
    const clipText = e.clipboardData.getData('text/plain')
    const selectionStart = textarea?.selectionStart ?? input.length
    const selectionEnd = textarea?.selectionEnd ?? input.length
    const valueBeforePaste = textarea?.value ?? input

    // 需同步拦截的两种情形：剪贴板有 File 对象（阻止文件名文本插入），或超长纯文本
    // （转虚拟附件，阻止正文插入）。必须在任何 await 之前调用 preventDefault——
    // 事件处理是 async 的，等读取完系统文件路径再调，浏览器默认粘贴早已把文本
    // 插入输入框（事后清空又会误删用户已写内容）。
    if (attachableClipboardFiles.length > 0 || clipText.length > PASTE_TEXT_ATTACHMENT_THRESHOLD) {
      e.preventDefault()
    }

    const nativePaths: string[] = []
    try {
      const native = await api.chatReadClipboardFiles()
      if (native.success && native.files?.length) {
        nativePaths.push(...native.files.map((file) => file.path))
      }
    } catch (err) {
      console.error('Failed to read clipboard files:', err)
    }

    const hasNativeFiles = nativePaths.length > 0
    const hasClipboardFiles = attachableClipboardFiles.length > 0

    // 纯文字粘贴：短文本放行交给浏览器默认处理；超长文本已在上方同步阶段 preventDefault
    // （正文不会进输入框），这里只需生成内存虚拟 txt 附件（不落盘）。
    if (!hasNativeFiles && !hasClipboardFiles) {
      if (clipText.length > PASTE_TEXT_ATTACHMENT_THRESHOLD) {
        addAttachments([
          {
            id: `pending-att-${crypto.randomUUID()}`,
            type: 'file',
            name: t.chatPastedTextAttachmentName,
            path: `memory://${crypto.randomUUID()}`,
            content: clipText,
          },
        ])
      }
      return
    }

    if (hasNativeFiles && textarea) {
      // 等浏览器默认粘贴与 React onChange 完成后，只在内容完全等于“插入了文件名”时撤销。
      window.setTimeout(() => {
        undoAccidentalFilenamePaste(
          textarea,
          valueBeforePaste,
          clipText,
          selectionStart,
          selectionEnd,
          setInput,
        )
      }, 0)
    }

    setAttachmentError('')

    try {
      const pastedAttachments: PendingAttachment[] = []

      if (hasNativeFiles) {
        pastedAttachments.push(...attachmentsFromPaths(nativePaths))
      } else for (const [index, file] of attachableClipboardFiles.entries()) {
        const ext = file.name.split('.').pop()?.toLowerCase() ?? ''

        if (file.type.startsWith('image/') || IMAGE_EXTENSIONS.includes(ext)) {
          const imageExt = file.type.startsWith('image/')
            ? imageExtensionForMime(file.type)
            : ext
          const name = file.name || `pasted-image-${Date.now()}-${index + 1}.${imageExt}`
          const dataBase64 = await readFileAsBase64(file, t.chatClipboardImageReadFailed)
          const result = await api.chatSavePastedImage(
            name,
            file.type || `image/${imageExt}`,
            dataBase64,
          )
          if (!result.success || !result.path || !result.name) {
            throw new Error(result.error || t.chatPasteImageFailed)
          }
          pastedAttachments.push({
            id: `pending-att-${crypto.randomUUID()}`,
            type: 'image',
            name: result.name,
            path: result.path,
          })
          continue
        }

        if (file.size <= 0) continue

        const name = file.name || `pasted-file-${Date.now()}-${index + 1}.${ext}`
        const dataBase64 = await readFileAsBase64(file, t.chatClipboardImageReadFailed)
        const result = await api.chatSavePastedAttachment(name, dataBase64)
        if (!result.success || !result.path || !result.name) {
          throw new Error(result.error || t.chatPasteAttachmentFailed)
        }
        pastedAttachments.push({
          id: `pending-att-${crypto.randomUUID()}`,
          type: 'file',
          name: result.name,
          path: result.path,
        })
      }

      if (pastedAttachments.length === 0) {
        setAttachmentError(t.chatNoAddableFiles)
        return
      }

      addAttachments(pastedAttachments)
    } catch (err) {
      console.error('Failed to paste chat attachment:', err)
      setAttachmentError(
        typeof err === 'string' ? err : err instanceof Error ? err.message : t.chatPasteAttachmentFailed,
      )
    }
  }

  const removeAttachment = (id: string) => {
    setAttachments((prev) => prev.filter((attachment) => attachment.id !== id))
    setAttachmentError('')
  }

  useEffect(() => {
    if (!autoFocus || disabled) return
    requestAnimationFrame(() => {
      if (shouldComposerAutoFocus(document.activeElement)) {
        const el = textareaRef.current
        el?.focus({ preventScroll: true })
        // 恢复草稿后光标应落到末尾，而非开头，省得每次手动移到最后再输入。
        if (el) el.selectionStart = el.selectionEnd = el.value.length
      }
    })
  }, [autoFocus, disabled])

  useEffect(() => {
    if (!autoFocus || !isTauriRuntime()) return
    let cancelled = false
    let unlisten: (() => void) | undefined

    getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused || cancelled) return
      requestAnimationFrame(() => {
        if (!cancelled && !disabled && shouldComposerAutoFocus(document.activeElement)) {
          textareaRef.current?.focus({ preventScroll: true })
        }
      })
    }).then((handler) => {
      if (cancelled) {
        handler()
      } else {
        unlisten = handler
      }
    }).catch((err) => {
      console.error('Failed to listen for chat input focus changes:', err)
    })

    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [autoFocus, disabled])

  useEffect(() => {
    if (!toolPanelOpen) return
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setToolPanelOpen(false)
      }
    }
    window.addEventListener('keydown', handleEscape)
    return () => window.removeEventListener('keydown', handleEscape)
  }, [toolPanelOpen])

  useEffect(() => {
    if (!modeMenuOpen) return
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        closeModeMenu()
      }
    }
    window.addEventListener('keydown', handleEscape)
    return () => window.removeEventListener('keydown', handleEscape)
  }, [closeModeMenu, modeMenuOpen])

  useEffect(() => {
    if (!projectMenuOpen) return
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        closeProjectMenu()
      }
    }
    window.addEventListener('keydown', handleEscape)
    return () => window.removeEventListener('keydown', handleEscape)
  }, [closeProjectMenu, projectMenuOpen])

  useEffect(() => {
    if (!slashPanelOpen) return
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target
      if (!(target instanceof Element)) return
      if (target.closest('[data-chat-slash-panel="true"]')) return
      if (target.closest('[data-chat-composer="true"]')) return
      setSlashPanelOpen(false)
    }
    window.addEventListener('pointerdown', handlePointerDown)
    return () => window.removeEventListener('pointerdown', handlePointerDown)
  }, [slashPanelOpen])

  useLayoutEffect(() => {
    if (!slashPanelOpen) return

    const updateSlashPanelLeft = () => {
      const inner = innerRef.current
      const textarea = textareaRef.current
      if (!inner || !textarea) return

      const innerRect = inner.getBoundingClientRect()
      const textareaRect = textarea.getBoundingClientRect()
      setSlashPanelLeft(Math.max(0, Math.round(textareaRect.left - innerRect.left)))
    }

    updateSlashPanelLeft()
    window.addEventListener('resize', updateSlashPanelLeft)

    const resizeObserver = typeof ResizeObserver === 'undefined'
      ? null
      : new ResizeObserver(updateSlashPanelLeft)
    if (resizeObserver) {
      if (innerRef.current) resizeObserver.observe(innerRef.current)
      if (textareaRef.current) resizeObserver.observe(textareaRef.current)
    }

    return () => {
      window.removeEventListener('resize', updateSlashPanelLeft)
      resizeObserver?.disconnect()
    }
  }, [slashPanelOpen])

  useEffect(() => {
    if (!disabled) return
    setToolPanelOpen(false)
    closeProjectMenu()
    setSlashPanelOpen(false)
  }, [closeProjectMenu, disabled])

  useEffect(() => {
    setSlashSelectedIndex(0)
  }, [activeSlashToken?.query])

  useEffect(() => {
    if (slashSelectedIndex < filteredSlashCommands.length) return
    setSlashSelectedIndex(Math.max(filteredSlashCommands.length - 1, 0))
  }, [filteredSlashCommands.length, slashSelectedIndex])

  useEffect(() => {
    if (!isTauriRuntime()) return
    let cancelled = false
    let unlisten: (() => void) | undefined

    getCurrentWebview().onDragDropEvent((event) => {
      if (cancelled || composerLocked) return

      if (event.payload.type === 'enter' || event.payload.type === 'over') {
        setDragActive(true)
        setAttachmentError('')
        return
      }

      if (event.payload.type === 'leave') {
        setDragActive(false)
        return
      }

      if (event.payload.type === 'drop') {
        setDragActive(false)
        addAttachments(attachmentsFromPaths(event.payload.paths))
      }
    }).then((handler) => {
      if (cancelled) {
        handler()
      } else {
        unlisten = handler
      }
    }).catch((err) => {
      console.error('Failed to listen for chat attachment drops:', err)
    })

    return () => {
      cancelled = true
      setDragActive(false)
      unlisten?.()
    }
  }, [addAttachments, attachmentsFromPaths, composerLocked])

  const canSend = (Boolean(input.trim()) || attachments.length > 0)
    && !slashPanelOpen
    && (!disabled || queueMode)
    && !sendDisabledReason
    && !sendPending
  // 发送键与停止键共用输入行右侧那一个槽位（crossfade）。生成中**有东西可发**时把槽位还给
  // 发送键 —— 就是原本那个键、原本的样子，按下去这一条进队列。这一刻停止走 Esc（见
  // handleKeyDown）或清空输入框让停止键回来；否则用户在生成中打完字会发现没有键可按。
  const stopOwnsSendSlot = Boolean(cancelVisible && onCancel) && !(queueMode && canSend)
  const cliAgentLabel = externalCliAgentLabel(externalAgentName)

  const wrapperClass =
    layout === 'inline'
      ? 'w-full'
      : 'chat-composer-footer shrink-0 px-6 pb-8 pt-2'

  const innerClass = layout === 'inline' ? 'w-full' : 'mx-auto w-full max-w-4xl'
  const slashPanelPlacementClass = layout === 'inline'
    ? 'top-full mt-1'
    : 'bottom-full mb-1'
  const slashPanelOrigin = layout === 'inline' ? 'top left' : 'bottom left'
  const projectPanelPlacementClass = layout === 'inline'
    ? 'top-full mt-1.5'
    : 'bottom-full mb-1.5'
  const projectPanelOrigin = layout === 'inline' ? 'top left' : 'bottom left'
  // 模式菜单移到发送键旁、右对齐：原点跟随展开方向用右侧
  const modePanelOrigin = layout === 'inline' ? 'top right' : 'bottom right'
  const externalMcpTools = enabledTools.filter(isExternalMcpTool)
  const showMcpSection = externalMcpTools.length > 0 || Boolean(toolsDisabledReason)
  const mcpStatusLine = toolsDisabledReason
    || (externalMcpTools.length > 0 ? `MCP ${externalMcpTools.length}` : '')

  // 项目菜单内容：贴按钮紧凑宽度，状态条 / 工具栏入口共用同一 body。
  const projectMenuBody = (
    <>
      <div className="flex h-7 items-center gap-1.5 rounded-md px-2 text-neutral-500 dark:text-neutral-400">
        <Search size={14} strokeWidth={1.8} className="shrink-0" />
        <input
          value={projectSearchQuery}
          onChange={(event) => setProjectSearchQuery(event.target.value)}
          placeholder={t.chatSearchProjects}
          className="min-w-0 flex-1 border-0 bg-transparent text-[12px] font-semibold text-neutral-800 outline-none placeholder:text-neutral-400 dark:text-neutral-100 dark:placeholder:text-neutral-500"
        />
      </div>

      <div className="chat-popover-scroll mt-0.5 max-h-48 overflow-y-auto">
        {projectOptionsLoading ? (
          <div className="px-2 py-2.5 text-[12px] text-neutral-400 dark:text-neutral-500">
            {t.chatLoadingProjects}
          </div>
        ) : projectOptionsError ? (
          <div className="px-2 py-2 text-[12px] text-red-500 dark:text-red-400">
            {projectOptionsError}
          </div>
        ) : visibleProjectOptions.length > 0 ? (
          <div className="py-1">
            {visibleProjectOptions.map((project) => {
              const active = selectedProject?.id === project.id
              const pathLabel = projectPathLabel(project)
              return (
                <button
                  key={project.id}
                  type="button"
                  onClick={() => void selectProject(project)}
                  className={`flex min-h-[34px] w-full min-w-0 items-center gap-1.5 rounded-md px-2 text-left transition-colors ${
                    active
                      ? 'bg-neutral-100 text-neutral-950 dark:bg-neutral-800 dark:text-neutral-50'
                      : 'text-neutral-800 hover:bg-neutral-100 dark:text-neutral-200 dark:hover:bg-neutral-800'
                  }`}
                >
                  <Folder size={14} strokeWidth={1.75} className="shrink-0 text-neutral-500 dark:text-neutral-400" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[12px] font-semibold">{project.name}</span>
                    {pathLabel && (
                      <span className="block truncate text-[10px] font-medium text-neutral-400 dark:text-neutral-500">
                        {pathLabel}
                      </span>
                    )}
                  </span>
                  {active && <Check size={13} strokeWidth={2} className="shrink-0 text-neutral-500 dark:text-neutral-300" />}
                </button>
              )
            })}
          </div>
        ) : (
          <div className="px-2 py-2.5 text-[12px] leading-5 text-neutral-400 dark:text-neutral-500">
            {projectSearchQuery.trim() ? t.chatNoMatchingProjects : t.chatNoRecentProjects}
          </div>
        )}
      </div>

      <div className="mt-0.5 border-t border-neutral-200/80 pt-0.5 dark:border-neutral-800">
        {selectedProject && (
          <button
            type="button"
            onClick={() => void selectProject(null)}
            className="flex h-7 w-full items-center gap-1.5 rounded-md px-2 text-left text-[12px] font-semibold text-neutral-500 transition-colors hover:bg-neutral-100 hover:text-neutral-800 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
          >
            <Folder size={14} strokeWidth={1.75} className="shrink-0" />
            <span className="min-w-0 flex-1 truncate">{t.chatLeaveProject}</span>
          </button>
        )}
        <button
          type="button"
          onClick={() => void createBlankProject()}
          disabled={projectCreating}
          className="flex h-7 w-full items-center gap-1.5 rounded-md px-2 text-left text-[12px] font-semibold text-neutral-800 transition-colors hover:bg-neutral-100 disabled:cursor-default disabled:opacity-50 dark:text-neutral-100 dark:hover:bg-neutral-800"
        >
          <Plus size={14} strokeWidth={1.8} className="shrink-0 text-neutral-600 dark:text-neutral-300" />
          <span className="min-w-0 flex-1 truncate">
            {projectCreating ? t.chatAddingProject : t.chatNewBlankProject}
          </span>
        </button>
        <button
          type="button"
          onClick={() => void createProjectFromFolder()}
          disabled={projectCreating}
          className="flex h-7 w-full items-center gap-1.5 rounded-md px-2 text-left text-[12px] font-semibold text-neutral-800 transition-colors hover:bg-neutral-100 disabled:cursor-default disabled:opacity-50 dark:text-neutral-100 dark:hover:bg-neutral-800"
        >
          <Folder size={14} strokeWidth={1.75} className="shrink-0 text-neutral-600 dark:text-neutral-300" />
          <span className="min-w-0 flex-1 truncate">{t.chatUseExistingFolder}</span>
        </button>
      </div>
    </>
  )

  return (
    <div className={wrapperClass}>
      <div ref={innerRef} className={`relative ${innerClass}`}>
        {toolPanelOpen && (
          <>
            <div className="fixed inset-0 z-30" onClick={() => setToolPanelOpen(false)} aria-hidden />
            <div
              className={`chat-motion-popover absolute inset-x-0 z-40 overflow-hidden kv-menu ${projectPanelPlacementClass}`}
              style={{ ['--chat-popover-origin' as string]: projectPanelOrigin }}
              data-tauri-drag-region="false"
            >
              <div className="space-y-1.5 px-3 py-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-[12px] font-semibold text-neutral-800 dark:text-neutral-100">Skill</span>
                  {onOpenSkillSettings && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        setToolPanelOpen(false)
                        onOpenSkillSettings()
                      }}
                    >
                      {t.chatManage}
                    </Button>
                  )}
                </div>
                <div className="text-[11px] leading-4 text-neutral-600 dark:text-neutral-300">
                  <span className="text-neutral-500 dark:text-neutral-400">
                    {t.chatSkillsEnabledCount.replace('{n}', String(enabledSkills.length))}
                  </span>
                  {enabledSkills.length > 0 && (
                    <>
                      <span className="text-neutral-300 dark:text-neutral-600"> · </span>
                      <span className="text-neutral-700 dark:text-neutral-200">
                        {enabledSkills.map((skill) => skill.name).join('、')}
                      </span>
                    </>
                  )}
                </div>

                {showMcpSection && mcpStatusLine && (
                  <div className="border-t border-neutral-200/80 pt-1.5 text-[11px] text-neutral-500 dark:border-neutral-800 dark:text-neutral-400">
                    {mcpStatusLine}
                  </div>
                )}

                {(sendDisabledReason || toolStatusHint) && (
                  <p className="rounded-md bg-amber-50 px-2 py-1 text-[11px] leading-4 text-amber-700 dark:bg-amber-400/10 dark:text-amber-200">
                    {sendDisabledReason || toolStatusHint}
                  </p>
                )}
              </div>
            </div>
          </>
        )}
        {slashPanelOpen && (
          <div
            className={`chat-motion-popover absolute z-40 overflow-hidden kv-menu font-sans ${slashPanelPlacementClass}`}
            style={{
              ['--chat-popover-origin' as string]: slashPanelOrigin,
              ['--chat-popover-start-y' as string]: '0px',
              left: slashPanelLeft,
              width: `calc(100% - ${slashPanelLeft}px)`,
            }}
            data-chat-slash-panel="true"
            data-tauri-drag-region="false"
          >
            <div className="chat-popover-scroll max-h-[min(184px,34vh)] overflow-y-auto">
              {filteredSlashCommands.length > 0 ? (
                filteredSlashCommands.map((command, index) => {
                  const Icon = slashCommandIcon(command)
                  const selected = index === slashSelectedIndex
                  return (
                    <button
                      key={command.id}
                      type="button"
                      aria-selected={selected}
                      onMouseEnter={() => setSlashSelectedIndex(index)}
                      onMouseDown={(event) => event.preventDefault()}
                      onClick={() => void handleSlashCommandSelect(command)}
                      className={`flex h-[26px] w-full min-w-0 items-center gap-1.5 rounded-md px-2 text-left transition-colors ${
                        selected
                          ? 'bg-neutral-100 text-neutral-900 dark:bg-neutral-800 dark:text-neutral-50'
                          : 'text-neutral-700 hover:bg-neutral-50 dark:text-neutral-200 dark:hover:bg-neutral-800/70'
                      }`}
                    >
                      <Icon
                        size={13}
                        strokeWidth={1.8}
                        className="shrink-0 text-neutral-600 dark:text-neutral-300"
                      />
                      <span className="min-w-0 flex-1 truncate text-[12px] leading-none">
                        <span className="font-semibold">{command.title}</span>
                        {command.argumentHint && (
                          <span className="ml-1 text-[11px] font-normal text-neutral-400 dark:text-neutral-500">
                            {command.argumentHint}
                          </span>
                        )}
                        <span className="ml-1.5 text-[11px] font-medium text-neutral-400 dark:text-neutral-500">
                          {command.description}
                        </span>
                      </span>
                    </button>
                  )
                })
              ) : (
                <div className="flex h-[26px] items-center px-2 text-[11px] font-medium text-neutral-400 dark:text-neutral-500">
                  {usesExternalRuntime
                    ? (externalCliSlashLoading
                      ? t.chatLoadingCliCommands
                      : externalCliSlashHint ?? 'No matching CLI command')
                    : 'No matching command'}
                </div>
              )}
            </div>
          </div>
        )}
        {/* ① 状态条：「你在哪 + 在做什么 + 改了多少」—— 项目/集、当前 todo、diff 徽标。 */}
        {(statusBarVisible || todoBarVisible || gitStatusEnabled) && (
          <div className="chat-composer-status" data-tauri-drag-region="false">
            {statusBarVisible && effectiveProject && (
              <div className="relative min-w-0">
                <button
                  type="button"
                  onClick={toggleProjectMenu}
                  disabled={disabled}
                  className="chat-composer-status-item"
                  title={t.chatProjectChip.replace('{name}', effectiveProject.name)}
                  aria-haspopup="menu"
                  aria-expanded={projectMenuOpen}
                >
                  <Folder strokeWidth={1.75} />
                  <span className="min-w-0 truncate">{effectiveProject.name}</span>
                </button>
                {projectMenuOpen && projectEntryEnabled && (
                  <>
                    <div
                      className="fixed inset-0 z-30"
                      onClick={closeProjectMenu}
                      aria-hidden
                    />
                    <div
                      className={`chat-motion-popover absolute left-0 z-50 w-[min(260px,calc(100vw-24px))] overflow-visible kv-menu ${projectPanelPlacementClass}`}
                      style={{ ['--chat-popover-origin' as string]: projectPanelOrigin }}
                      data-tauri-drag-region="false"
                    >
                      {projectMenuBody}
                    </div>
                  </>
                )}
              </div>
            )}
            {statusBarVisible && effectiveSet && (
              <button
                type="button"
                onClick={() => {
                  if (disabled || !onSelectSet) return
                  void onSelectSet(null)
                }}
                disabled={disabled || !onSelectSet}
                className="chat-composer-status-item"
                title={t.chatSetChipExit.replace('{name}', effectiveSet.name)}
              >
                <Layers strokeWidth={1.75} />
                <span className="min-w-0 truncate">{effectiveSet.name}</span>
              </button>
            )}
            {todoBarVisible && (
              <AgentTodoIndicator todoState={agentTodoState} placement="status" />
            )}
            {gitStatusEnabled && gitWorkdir && gitLang && onOpenGitPanel && (
              <GitDiffChip
                workdir={gitWorkdir}
                lang={gitLang}
                onOpenGitPanel={onOpenGitPanel}
              />
            )}
          </div>
        )}

        <div
          data-chat-composer="true"
          className={`chat-composer-shell relative select-none ${modeMenuOpen ? 'z-30' : 'z-10'} rounded-xl border px-3 py-2 transition-[box-shadow,border-color] duration-[var(--kv-dur-normal)] ease-[var(--kv-ease-out)] ${
            dragActive
              ? 'border-[#5c8df7] shadow-[0_2px_12px_rgba(0,0,0,0.06)] ring-2 ring-[#5c8df7]/25 dark:border-[#5c8df7] dark:shadow-none'
              : agentPlanActive
                ? 'border-emerald-500 shadow-[0_1px_2px_rgba(0,0,0,0.04),0_4px_10px_-4px_rgba(0,0,0,0.06),0_12px_32px_-14px_rgba(0,0,0,0.14)] focus-within:border-emerald-500 focus-within:shadow-[0_1px_3px_rgba(0,0,0,0.05),0_6px_14px_-6px_rgba(0,0,0,0.07),0_18px_44px_-16px_rgba(16,185,129,0.22)] dark:border-emerald-400 dark:shadow-none dark:focus-within:border-emerald-400'
                : agentOrchestrateActive
                  ? 'border-violet-500 shadow-[0_1px_2px_rgba(0,0,0,0.04),0_4px_10px_-4px_rgba(0,0,0,0.06),0_12px_32px_-14px_rgba(0,0,0,0.14)] focus-within:border-violet-500 focus-within:shadow-[0_1px_3px_rgba(0,0,0,0.05),0_6px_14px_-6px_rgba(0,0,0,0.07),0_18px_44px_-16px_rgba(139,92,246,0.22)] dark:border-violet-400 dark:shadow-none dark:focus-within:border-violet-400'
                  : 'border-neutral-200/80 shadow-[0_1px_2px_rgba(0,0,0,0.04),0_4px_10px_-4px_rgba(0,0,0,0.06),0_12px_32px_-14px_rgba(0,0,0,0.14)] focus-within:border-neutral-300 focus-within:shadow-[0_1px_3px_rgba(0,0,0,0.05),0_6px_14px_-6px_rgba(0,0,0,0.07),0_18px_44px_-16px_rgba(0,0,0,0.20)] dark:border-neutral-700 dark:shadow-none dark:focus-within:border-neutral-600'
          }`}
        >
          {dragActive && (
            <div className="chat-motion-fade-up mb-2 rounded-2xl border border-dashed border-[#5c8df7]/70 bg-[#5c8df7]/10 px-3 py-2 text-center text-[13px] font-medium text-[#2960d8] dark:text-[#9bb8fa]">
              {t.chatDropToAttach}
            </div>
          )}
          {attachments.length > 0 && (
            <div className="chat-motion-fade-up mb-2 px-1">
              <ChatAttachments
                attachments={attachments}
                variant="composer"
                onRemove={composerLocked ? undefined : removeAttachment}
                onEditAttachment={setEditingAttachment}
              />
            </div>
          )}
          {attachmentError && (
            <div className="chat-motion-fade-up mb-2 px-1 text-[12px] text-red-500 dark:text-red-400">
              {attachmentError}
            </div>
          )}
          {quotes.length > 0 && (
            <div className="chat-motion-fade-up mb-2 flex flex-col gap-1.5">
              {quotes.map((q, i) => (
                <div key={i} className="kv-quote-chip">
                  <TextQuote size={14} className="kv-quote-chip-icon" />
                  <span className="kv-quote-chip-text">{q}</span>
                  <button
                    type="button"
                    className="kv-quote-chip-remove"
                    onClick={() => setQuotes((prev) => prev.filter((_, idx) => idx !== i))}
                    aria-label={t.chatRemoveQuote}
                    data-tauri-drag-region="false"
                  >
                    <X size={13} />
                  </button>
                </div>
              ))}
            </div>
          )}
          {(sendDisabledReason || toolStatusHint) && !attachmentError && (
            <div className="chat-motion-fade-up mb-2 px-1 text-[12px] text-amber-600 dark:text-amber-300">
              {sendDisabledReason || toolStatusHint}
            </div>
          )}
          {/* 输入行自成定位上下文。发送键靠 inset-y-0 + my-auto 求垂直居中，而这是按
              **最近的定位祖先**算的：挂在 shell 上时，上面的附件 / 引用 / 提示条的高度
              也会被算进去——加一张引用卡片，按钮就往上飘半张卡片，正好落到引用卡右下角。
              把它和 textarea 圈进同一个 relative 行，居中就只按输入行算。
              `-mr-3 pr-3`：让本行的 padding box 右缘与 shell 的 padding box 右缘重合，
              这样下面的 `right-2` 和改动前落在同一像素（shell 的 px-3 不随容器查询变，
              这 12px 是常量）。垂直方向仍旧不写死数值——shell 的 py 在容器查询里会变。 */}
          <div className="relative -mr-3 pr-3">
            <div className="relative w-[calc(100%-1.75rem)]">
              {slashHighlight && (
                <div
                  ref={slashHighlightRef}
                  aria-hidden
                  className="chat-composer-highlight py-1.5 pl-1 pr-1 text-[15px] leading-relaxed text-neutral-900 dark:text-neutral-100"
                >
                  {slashHighlight.prefix}
                  <span className="chat-composer-slash">{slashHighlight.command}</span>
                  {slashHighlight.rest}
                  {input.endsWith('\n') ? '\u200b' : null}
                </div>
              )}
              <textarea
                ref={textareaRef}
                value={input}
                readOnly={sendPending}
                aria-busy={sendPending}
                onChange={handleInput}
                onPaste={(e) => void handlePaste(e)}
                onKeyDown={handleKeyDown}
                onSelect={handleSelect}
                onScroll={syncSlashHighlightScroll}
                autoCapitalize="off"
                autoCorrect="off"
                autoComplete="off"
                spellCheck={false}
                placeholder={
                  usesExternalRuntime
                    ? t.chatCliCommandPlaceholder.replace('{agent}', cliAgentLabel)
                    : 'Ask me anything...'
                }
                rows={1}
                /* 宽度用 calc 收 28px（= 发送键 28 宽 + right-2 的 8 − 与滚动条留的 4px 呼吸）而不是
                   w-full + pr-*：滚动条长在**盒子右边缘**，padding 挡不住它，只有把盒子本身收窄
                   才能让它落到绝对定位的发送键左侧（原来 pr-10 只挡住了文字，滚动条仍压在键下）。
                   不用 margin —— w-full 是 width:100%，再加 margin 会溢出容器 28px。
                   custom-scrollbar：与全站同一根 8px 细条，否则这里是 WebView2 原生带箭头的粗条。 */
                className={`custom-scrollbar block max-h-40 min-h-[28px] w-full select-text resize-none overflow-y-hidden border-0 bg-transparent py-1.5 pl-1 pr-1 text-[15px] leading-relaxed outline-none placeholder:text-neutral-400 disabled:opacity-50 [field-sizing:content] ${
                  slashHighlight
                    ? 'is-slash-highlight'
                    : 'text-neutral-900 dark:text-neutral-100'
                }`}
              />
            </div>

            {/* 发送 / 停止：绝对定位在输入行右侧。两按钮共存于同一槽位，做 opacity+scale
                crossfade。谁占槽位见 `stopOwnsSendSlot`：生成中打了字就归发送键。 */}
            <div className="chat-composer-send-slot absolute inset-y-0 right-2 my-auto h-7 w-7">
              <button
                type="button"
                onClick={() => void handleSend()}
                disabled={!canSend}
                tabIndex={-1}
                title={sendDisabledReason || (canSend ? t.chatSend : t.chatSendHintEmpty)}
                aria-label={sendDisabledReason || t.chatSend}
                aria-hidden={stopOwnsSendSlot}
                className={`chat-composer-send absolute inset-0 flex items-center justify-center rounded-full transition-all duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-out)] ${
                  stopOwnsSendSlot
                    ? 'pointer-events-none scale-90 opacity-0'
                    : 'opacity-100'
                } ${canSend ? 'is-ready' : ''}`}
              >
                <ArrowUp size={16} strokeWidth={2.25} />
              </button>
              {onCancel ? (
                <button
                  type="button"
                  onClick={onCancel}
                  disabled={cancelling}
                  tabIndex={stopOwnsSendSlot ? undefined : -1}
                  aria-hidden={!stopOwnsSendSlot}
                  className={`absolute inset-0 flex items-center justify-center rounded-full bg-neutral-900 text-white transition-all duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] hover:bg-neutral-700 disabled:bg-neutral-300 disabled:text-neutral-500 dark:bg-neutral-100 dark:text-neutral-900 dark:hover:bg-neutral-200 dark:disabled:bg-neutral-700 dark:disabled:text-neutral-500 ${
                    stopOwnsSendSlot ? 'opacity-100' : 'pointer-events-none scale-90 opacity-0'
                  }`}
                  title={cancelling ? t.chatStopping : t.chatStopGenerating}
                  aria-label={cancelling ? t.chatStopping : t.chatStopGeneratingShort}
                >
                  <Square size={12} strokeWidth={2.4} fill="currentColor" />
                </button>
              ) : null}
            </div>
          </div>
        </div>

        {/* ③ 功能栏：移出输入框，裸露坐在窗口底色上（无背景无边框）。
            这样输入框高度只由文本决定，能收到单行 —— 原来图标在盒内，盒子被撑到 ~100px。 */}
        <div className="chat-composer-tools" data-tauri-drag-region="false">
            <IconButton
              size="sm"
              shape="circle"
              label={t.chatAddAttachment}
              onClick={() => void openAttachmentPicker()}
              disabled={disabled}
              tabIndex={-1}
              className="shrink-0 disabled:opacity-40"
            >
              <Plus size={18} strokeWidth={1.75} />
            </IconButton>

            {onChangeKnowledgeBaseIds && onSetWebSearchMode && (
              <SourcesButton
                knowledgeBaseIds={knowledgeBaseIds}
                onChangeKnowledgeBaseIds={onChangeKnowledgeBaseIds}
                forceKnowledgeSearch={forceKnowledgeSearch}
                onToggleForceKnowledgeSearch={onToggleForceKnowledgeSearch}
                mcpServers={mcpServers}
                onToggleMcpServer={onToggleMcpServer ?? (() => {})}
                webSearchMode={webSearchMode}
                onSetWebSearchMode={onSetWebSearchMode}
                builtinWebSearchSupported={builtinWebSearchSupported}
                onOpenSettings={onOpenSettings}
                disabled={disabled}
                layout={layout}
              />
            )}
            {/* 已选项目时这个入口移到状态条（那里是「当前上下文」的位置），
                工具栏只在未选项目时保留「进入项目」这个动作。 */}
            {projectEntryEnabled && !effectiveProject && (
              <div className="relative shrink-0">
                <IconButton
                  size="sm"
                  shape="circle"
                  label={t.chatEnterProject}
                  onClick={toggleProjectMenu}
                  disabled={disabled}
                  aria-expanded={projectMenuOpen}
                  aria-haspopup="menu"
                  className={`shrink-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-neutral-300/60 disabled:opacity-50 dark:focus-visible:ring-neutral-600 ${
                    projectMenuOpen
                      ? 'bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-100'
                      : ''
                  }`}
                >
                  <FolderPlus size={18} strokeWidth={1.75} />
                </IconButton>
                {projectMenuOpen && (
                  <>
                    <div
                      className="fixed inset-0 z-30"
                      onClick={closeProjectMenu}
                      aria-hidden
                    />
                    <div
                      className={`chat-motion-popover absolute left-0 z-50 w-[min(260px,calc(100vw-24px))] overflow-visible kv-menu ${projectPanelPlacementClass}`}
                      style={{ ['--chat-popover-origin' as string]: projectPanelOrigin }}
                      data-tauri-drag-region="false"
                    >
                      {projectMenuBody}
                    </div>
                  </>
                )}
              </div>
            )}
            {showAssistantEntry && onOpenAssistantCenter && (
              <AssistantPicker
                currentAssistant={currentAssistant}
                onSelect={onSelectAssistant ?? (() => {})}
                onOpenCenter={onOpenAssistantCenter}
                disabled={disabled}
                layout={layout}
              />
            )}

            {!usesExternalRuntime && onChangeReplyModels && (
              <div className="min-w-0 shrink" data-tauri-drag-region="false">
                <MultiModelSelector
                  value={replyModels}
                  onChange={(models) => void onChangeReplyModels(models)}
                  placement={layout === 'inline' ? 'down' : 'up'}
                />
              </div>
            )}

            {/* Git 分支胶囊 + diff 徽标（ml-auto 把徽标顶到右侧、挨着上下文指示器）。 */}
            {gitStatusEnabled && gitWorkdir && gitLang && onOpenGitPanel && (
              <GitStatusPill
                workdir={gitWorkdir}
                lang={gitLang}
                disabled={disabled}
                onOpenGitPanel={onOpenGitPanel}
              />
            )}

            <div className="ml-auto flex items-center gap-1.5">
            {usageSlot}
            {presetEntryEnabled && activePresetOption && (
              <div className="relative shrink-0 self-center">
                <button
                  type="button"
                  onClick={togglePresetMenu}
                  onMouseDown={(event) => event.preventDefault()}
                  disabled={disabled}
                  className={`inline-flex h-[26px] max-w-full items-center gap-0.5 rounded-full px-1.5 text-left text-[12px] font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-neutral-300/60 dark:focus-visible:ring-neutral-600 ${
                    presetMenuOpen
                      ? 'bg-neutral-200 text-neutral-800 dark:bg-neutral-700 dark:text-neutral-100'
                      : activePresetPillClass.idle
                  } disabled:cursor-default disabled:opacity-50`}
                  aria-expanded={presetMenuOpen}
                  aria-haspopup="menu"
                  title={presetLocked && presetLockedReason ? presetLockedReason : t.chatSwitchAgentPreset}
                >
                  <activePresetOption.icon
                    size={13}
                    strokeWidth={1.9}
                    className={`shrink-0 ${activePresetPillClass.iconColor}`}
                  />
                  <span className="min-w-0 truncate">{activePresetOption.label}</span>
                  <ChevronDown
                    size={12}
                    strokeWidth={2}
                    className={`shrink-0 text-neutral-400 transition-transform ${
                      presetMenuOpen ? 'rotate-180' : ''
                    }`}
                  />
                </button>
                {presetMenuOpen && (
                  <>
                    <div className="fixed inset-0 z-30" onClick={closePresetMenu} aria-hidden />
                    <div
                      className={`chat-motion-popover absolute right-0 z-40 w-[min(236px,calc(100vw-32px))] overflow-visible kv-menu ${projectPanelPlacementClass}`}
                      style={{ ['--chat-popover-origin' as string]: modePanelOrigin }}
                      data-tauri-drag-region="false"
                      role="menu"
                    >
                      {presetOptions.map((option) => {
                        const active = option.value === presetValue
                        const Icon = option.icon
                        return (
                          <button
                            key={option.value}
                            type="button"
                            role="menuitemradio"
                            aria-checked={active}
                            disabled={presetLocked && !active}
                            onClick={() => void pickPreset(option.value)}
                            className={`kv-menu-row transition-colors ${
                              active
                                ? 'bg-neutral-100 text-neutral-950 dark:bg-neutral-800 dark:text-neutral-50'
                                : 'text-neutral-800 hover:bg-neutral-100 dark:text-neutral-200 dark:hover:bg-neutral-800'
                            } disabled:cursor-default disabled:opacity-50`}
                          >
                            <Icon
                              size={14}
                              strokeWidth={1.8}
                              className={`shrink-0 ${MODE_PILL_CLASS[option.tone].iconColor}`}
                            />
                            <span className="min-w-0 flex-1 leading-tight">
                              <span className="block truncate text-[12px] font-semibold">{option.label}</span>
                              {option.description && (
                                <span className="block truncate text-[10px] font-medium text-neutral-400 dark:text-neutral-500">
                                  {option.description}
                                </span>
                              )}
                            </span>
                            {active && (
                              <Check size={13} strokeWidth={2} className="shrink-0 text-neutral-500 dark:text-neutral-300" />
                            )}
                          </button>
                        )
                      })}
                    </div>
                  </>
                )}
              </div>
            )}
            {modeEntryEnabled && activeModeOption && (
              <div className="relative shrink-0 self-center">
                <button
                  type="button"
                  onClick={toggleModeMenu}
                  onMouseDown={(event) => event.preventDefault()}
                  disabled={disabled}
                  className={`inline-flex h-[26px] max-w-full items-center gap-0.5 rounded-full px-1.5 text-left text-[12px] font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-neutral-300/60 dark:focus-visible:ring-neutral-600 ${
                    modeMenuOpen
                      ? 'bg-neutral-200 text-neutral-800 dark:bg-neutral-700 dark:text-neutral-100'
                      : activeModePillClass.idle
                  } disabled:cursor-default disabled:opacity-50`}
                  aria-expanded={modeMenuOpen}
                  aria-haspopup="menu"
                  title={t.chatSwitchMode}
                >
                  <activeModeOption.icon
                    size={13}
                    strokeWidth={1.9}
                    className={`shrink-0 ${activeModePillClass.iconColor}`}
                  />
                  <span className="min-w-0 truncate">{activeModeOption.label}</span>
                  <ChevronDown
                    size={12}
                    strokeWidth={2}
                    className={`shrink-0 text-neutral-400 transition-transform ${
                      modeMenuOpen ? 'rotate-180' : ''
                    }`}
                  />
                </button>
                {modeMenuOpen && (
                  <>
                    <div className="fixed inset-0 z-30" onClick={closeModeMenu} aria-hidden />
                    <div
                      className={`chat-motion-popover absolute right-0 z-40 w-[min(236px,calc(100vw-32px))] overflow-visible kv-menu ${projectPanelPlacementClass}`}
                      style={{ ['--chat-popover-origin' as string]: modePanelOrigin }}
                      data-tauri-drag-region="false"
                      role="menu"
                    >
                      {modeOptions.map((option) => {
                        const active = option.value === modeValue
                        const Icon = option.icon
                        return (
                          <button
                            key={option.value}
                            type="button"
                            role="menuitemradio"
                            aria-checked={active}
                            onClick={() => void pickMode(option.value)}
                            className={`kv-menu-row transition-colors ${
                              active
                                ? 'bg-neutral-100 text-neutral-950 dark:bg-neutral-800 dark:text-neutral-50'
                                : 'text-neutral-800 hover:bg-neutral-100 dark:text-neutral-200 dark:hover:bg-neutral-800'
                            }`}
                          >
                            <Icon
                              size={14}
                              strokeWidth={1.8}
                              className={`shrink-0 ${MODE_PILL_CLASS[option.tone].iconColor}`}
                            />
                            <span className="min-w-0 flex-1 leading-tight">
                              <span className="block truncate text-[12px] font-semibold">{option.label}</span>
                              {option.description && (
                                <span className="block truncate text-[10px] font-medium text-neutral-400 dark:text-neutral-500">
                                  {option.description}
                                </span>
                              )}
                            </span>
                            {active && (
                              <Check size={13} strokeWidth={2} className="shrink-0 text-neutral-500 dark:text-neutral-300" />
                            )}
                          </button>
                        )
                      })}
                    </div>
                  </>
                )}
              </div>
            )}

            {/* 注入 placement：上下文弹层贴按钮紧凑展开，仅翻转方向随 layout */}
            {isValidElement<{ placement?: 'up' | 'down' }>(contextSlot)
              ? cloneElement(contextSlot, {
                  placement: layout === 'inline' ? 'down' : 'up',
                })
              : contextSlot}

            {/* 发送 / 停止已移进输入框右端（见 textarea 同级的 chat-composer-send-slot）。 */}
            </div>
          </div>

          {/* 虚拟文本附件（粘贴长文本生成的 txt）编辑弹窗 */}
          {editingAttachment?.content !== undefined && (
            <PastedTextEditorModal
              name={editingAttachment.name}
              initialContent={editingAttachment.content}
              onSave={(content) => updateAttachmentContent(editingAttachment.id, content)}
              onClose={() => setEditingAttachment(null)}
            />
          )}
      </div>
    </div>
  )
})
