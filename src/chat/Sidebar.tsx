import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { save } from '@tauri-apps/plugin-dialog'
import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderPlus,
  Layers,
  LayoutGrid,
  MoreHorizontal,
  NotebookPen,
  Plus,
  Puzzle,
  Search,
  SquarePen,
} from 'lucide-react'
import type { ChatAssistant, ChatProject, ChatSet, ConversationListItem } from './types'
import { AgentIcon, KnowledgeIcon, McpIcon, SkillIcon } from '../settings/NavIcons'
import { ConversationList } from './ConversationList'
import { ChatSectionMenu } from './ChatSectionMenu'
import { ProjectContextMenu } from './ProjectContextMenu'
import { ProjectDialog } from './ProjectDialog'
import { SetContextMenu } from './SetContextMenu'
import { SetDialog } from './SetDialog'
import { SidebarAccountMenu } from './SidebarAccountMenu'
import { getSettingsCached } from '../api/settingsCache'
import { IconButton } from '../components/Button'
import { chatApi } from './api'
import { ChatTitlebarActions } from './ChatTitlebarActions'
import { chatTitlebarMacInsetClass, isMac, usesNativeTitlebar } from './platform'
import type { ConversationMenuAnchor } from './ConversationContextMenu'
import type { ChatUserProfile } from './types'
import { UserAvatar } from './UserAvatar'
import type { Lang } from '../settings/i18n'
import { conversationMarkdownFilename } from './conversationExport'

function resolveChatUserProfile(
  chat?: { userDisplayName?: string; userAvatar?: string } | null,
): ChatUserProfile {
  return {
    displayName: chat?.userDisplayName?.trim() || '',
    avatarUrl: chat?.userAvatar?.trim() || '',
  }
}

const modLabel = isMac ? '⌘' : 'Ctrl'

export type ExtensionsNavItem = 'assistants' | 'skill' | 'mcp' | 'knowledge' | 'notes' | 'plugins'

const extensionSubItems: Array<{
  id: ExtensionsNavItem
  label: string
  icon: (props: { size?: number; className?: string }) => React.JSX.Element
}> = [
  { id: 'assistants', label: '助手', icon: AgentIcon },
  { id: 'skill', label: 'Skill', icon: SkillIcon },
  { id: 'mcp', label: 'MCP', icon: McpIcon },
  { id: 'knowledge', label: '知识库', icon: KnowledgeIcon },
  { id: 'notes', label: '笔记', icon: (props) => <NotebookPen size={props.size} className={props.className} strokeWidth={1.75} /> },
  { id: 'plugins', label: '插件', icon: (props) => <Puzzle size={props.size} className={props.className} strokeWidth={1.75} /> },
]

const PROJECT_PREVIEW_LIMIT = 5

function conversationProjectId(conversation: ConversationListItem): string | null {
  return conversation.project_id ?? conversation.projectId ?? null
}

function conversationBelongsToProject(
  conversation: ConversationListItem,
  project: ChatProject,
): boolean {
  const projectId = conversationProjectId(conversation)
  return projectId ? projectId === project.id : conversation.folder === project.name
}

function conversationMatchesSearch(conversation: ConversationListItem, query: string): boolean {
  if (!query) return true
  return (
    conversation.title.toLowerCase().includes(query) ||
    conversation.preview.toLowerCase().includes(query)
  )
}

function projectMatchesSearch(project: ChatProject, query: string): boolean {
  if (!query) return true
  return (
    project.name.toLowerCase().includes(query) ||
    (project.root_path ?? project.rootPath ?? '').toLowerCase().includes(query)
  )
}

function findConversationProject(
  conversation: ConversationListItem,
  projects: ChatProject[],
): ChatProject | undefined {
  const projectId = conversationProjectId(conversation)
  if (projectId) return projects.find((project) => project.id === projectId)
  return projects.find((project) => conversation.folder === project.name)
}

function conversationProjectLabel(
  conversation: ConversationListItem,
  projects: ChatProject[],
): string {
  return findConversationProject(conversation, projects)?.name ?? conversation.folder ?? ''
}

interface SidebarProps {
  lang: Lang
  currentConversationId?: string
  generatingConversationIds?: ReadonlySet<string>
  optimisticConversations?: ConversationListItem[]
  selectedProject?: ChatProject | null
  onSelectProject: (project: ChatProject | null) => void
  selectedSet?: ChatSet | null
  onSelectSet: (set: ChatSet | null) => void
  onSelectConversation: (id: string) => void
  onNewConversation: () => void
  onConversationDeleted?: (id: string) => void
  onForceDropConversation?: (id: string) => void
  onOpenSettings: () => void
  onOpenExtensionsItem: (item: ExtensionsNavItem) => void
  onSelectLang: (lang: Lang) => void
  onCheckUpdate: () => void
  settingsActive?: boolean
  extensionsActive?: ExtensionsNavItem | null
  collapsed: boolean
  onToggleCollapsed: () => void
  refreshKey: number
  profileRefreshKey?: number
  searchOpen: boolean
  onSearchOpenChange: (open: boolean) => void
}

function SidebarUserFooter({
  profile,
  lang,
  settingsActive,
  onOpenSettings,
  onSelectLang,
  onCheckUpdate,
}: {
  profile: ChatUserProfile
  lang: Lang
  settingsActive: boolean
  onOpenSettings: () => void
  onSelectLang: (lang: Lang) => void
  onCheckUpdate: () => void
}) {
  const [menuRect, setMenuRect] = useState<{ left: number; top: number; width: number } | null>(null)
  const rowRef = useRef<HTMLButtonElement>(null)

  return (
    <div
      className="shrink-0 border-t border-neutral-200/60 p-1.5 dark:border-neutral-800/80"
      data-tauri-drag-region="false"
    >
      {/* 整行即触发（对齐 Claude）：不再是「名字 + 独立齿轮按钮」，设置折进菜单。 */}
      <button
        ref={rowRef}
        type="button"
        onClick={() => {
          if (menuRect) {
            setMenuRect(null)
            return
          }
          const rect = rowRef.current?.getBoundingClientRect()
          if (!rect) return
          setMenuRect({ left: rect.left, top: rect.top, width: rect.width })
        }}
        className={`flex w-full items-center gap-2 rounded-lg px-1.5 py-1 text-left transition-colors ${
          menuRect || settingsActive
            ? 'bg-black/[0.06] dark:bg-white/[0.1]'
            : 'hover:bg-black/[0.04] dark:hover:bg-white/[0.06]'
        }`}
        aria-haspopup="menu"
        aria-expanded={menuRect !== null}
      >
        <UserAvatar profile={profile} size={22} />
        <span
          className="min-w-0 flex-1 truncate text-[12.5px] text-neutral-700 dark:text-neutral-300"
          title={profile.displayName || undefined}
        >
          {profile.displayName || 'Kivio'}
        </span>
        <ChevronDown
          size={13}
          strokeWidth={2}
          className={`shrink-0 text-neutral-400 transition-transform duration-[var(--kv-dur-fast)] dark:text-neutral-500 ${
            menuRect ? 'rotate-180' : ''
          }`}
        />
      </button>

      {menuRect && (
        <SidebarAccountMenu
          triggerRect={menuRect}
          lang={lang}
          onOpenSettings={onOpenSettings}
          onSelectLang={onSelectLang}
          onCheckUpdate={onCheckUpdate}
          onClose={() => setMenuRect(null)}
        />
      )}
    </div>
  )
}

interface NavRowProps {
  icon: React.ReactNode
  label: string
  onClick?: () => void
  disabled?: boolean
  active?: boolean
  /** 图标在 hover 时的微动效（group-hover transform 工具类） */
  iconMotion?: string
}

function NavRow({ icon, label, onClick, disabled, active, iconMotion }: NavRowProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={`group flex w-full items-center gap-2.5 rounded-lg px-3 py-1.5 text-left text-[13px] transition-colors disabled:cursor-default disabled:opacity-40 ${
        active
          ? 'bg-black/[0.06] font-medium text-neutral-900 dark:bg-white/[0.1] dark:text-neutral-50'
          : 'text-neutral-800 hover:bg-black/[0.04] dark:text-neutral-200 dark:hover:bg-white/[0.06]'
      }`}
    >
      <span
        className={`flex h-5 w-5 shrink-0 items-center justify-center text-neutral-600 transition duration-300 ease-out will-change-transform group-hover:text-neutral-800 group-active:scale-90 dark:text-neutral-400 dark:group-hover:text-neutral-200 ${iconMotion ?? ''}`}
      >
        {icon}
      </span>
      <span className="min-w-0 flex-1 truncate font-medium">{label}</span>
    </button>
  )
}

function ExtensionsNav({
  activeItem,
  onSelectItem,
}: {
  activeItem?: ExtensionsNavItem | null
  onSelectItem: (item: ExtensionsNavItem) => void
}) {
  const [expanded, setExpanded] = useState(() => Boolean(activeItem))

  useEffect(() => {
    if (activeItem) setExpanded(true)
  }, [activeItem])

  const highlighted = expanded || !!activeItem

  return (
    <div className="py-0.5">
      <button
        type="button"
        onClick={() => setExpanded((open) => !open)}
        className={`group flex w-full items-center gap-2.5 rounded-lg px-3 py-1.5 text-left text-[13px] font-medium transition-colors ${
          highlighted
            ? 'bg-black/[0.06] text-neutral-900 dark:bg-white/[0.1] dark:text-neutral-50'
            : 'text-neutral-800 hover:bg-black/[0.04] dark:text-neutral-200 dark:hover:bg-white/[0.06]'
        }`}
        aria-expanded={expanded}
      >
        <span className="flex h-5 w-5 shrink-0 items-center justify-center text-neutral-600 transition duration-300 ease-out will-change-transform group-hover:text-neutral-800 group-active:scale-90 group-hover:rotate-3 group-hover:scale-110 dark:text-neutral-400 dark:group-hover:text-neutral-200">
          <LayoutGrid size={17} strokeWidth={1.75} />
        </span>
        <span className="min-w-0 flex-1 truncate">扩展</span>
        <ChevronRight
          size={14}
          strokeWidth={2}
          className={`shrink-0 text-neutral-400 transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] dark:text-neutral-500 ${
            expanded ? 'rotate-90' : ''
          }`}
        />
      </button>
      {expanded && (
        <div className="ml-[12px] mt-0.5 grid grid-cols-2 gap-x-1">
          {extensionSubItems.map((item) => {
            const active = activeItem === item.id
            const Icon = item.icon
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => onSelectItem(item.id)}
                className={`flex items-center gap-2 rounded-md py-1.5 pl-2 pr-1 text-left text-[13px] transition-colors ${
                  active
                    ? 'font-medium text-neutral-900 dark:text-neutral-100'
                    : 'text-neutral-700 hover:bg-black/[0.04] hover:text-neutral-900 dark:text-neutral-300 dark:hover:bg-white/[0.06] dark:hover:text-neutral-100'
                }`}
              >
                <span className={`flex h-4 w-4 shrink-0 items-center justify-center ${
                  active ? 'text-neutral-700 dark:text-neutral-200' : 'text-neutral-400 dark:text-neutral-500'
                }`}>
                  <Icon size={15} />
                </span>
                <span className="min-w-0 flex-1 truncate">{item.label}</span>
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}

function SearchDialog({
  query,
  results,
  currentConversationId,
  projects,
  sets,
  onQueryChange,
  onSelectConversation,
  onClose,
}: {
  query: string
  results: ConversationListItem[]
  currentConversationId?: string
  projects: ChatProject[]
  sets: ChatSet[]
  onQueryChange: (query: string) => void
  onSelectConversation: (conversation: ConversationListItem) => void
  onClose: () => void
}) {
  const dialogRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  const normalizedQuery = query.trim()

  return createPortal(
    <div
      className="fixed inset-0 z-[260] flex items-start justify-center bg-black/45 px-5 pt-[16vh] dark:bg-black/60"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <div
        ref={dialogRef}
        className="chat-motion-popover flex max-h-[62vh] w-full max-w-[560px] flex-col overflow-hidden rounded-xl border border-neutral-200 bg-white shadow-2xl shadow-black/25 dark:border-neutral-700 dark:bg-[#242426]"
        role="dialog"
        aria-modal="true"
        aria-label="搜索对话"
      >
        <div className="flex items-center gap-2 border-b border-neutral-200/80 px-3 py-2 dark:border-neutral-700/80">
          <Search size={15} strokeWidth={1.75} className="shrink-0 text-neutral-400" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            autoCapitalize="off"
            autoCorrect="off"
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => onQueryChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && results[0]) {
                if (event.nativeEvent.isComposing || event.keyCode === 229) return
                event.preventDefault()
                onSelectConversation(results[0])
              }
            }}
            placeholder="搜索对话"
            className="min-w-0 flex-1 bg-transparent text-[14px] font-medium text-neutral-900 outline-none placeholder:text-neutral-400 dark:text-neutral-100 dark:placeholder:text-neutral-500"
          />
        </div>

        <div className="px-3 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wide text-neutral-400 dark:text-neutral-500">
          {normalizedQuery ? '搜索结果' : '近期对话'}
        </div>

        <div className="custom-scrollbar min-h-0 overflow-y-auto px-1.5 pb-1.5">
          {results.length > 0 ? (
            results.map((conversation, index) => {
              const active = conversation.id === currentConversationId
              const projectLabel = conversationProjectLabel(conversation, projects)
              const setId = conversation.set_id ?? conversation.setId ?? null
              const setLabel = setId ? sets.find((s) => s.id === setId)?.name ?? '' : ''
              return (
                <button
                  key={conversation.id}
                  type="button"
                  onClick={() => onSelectConversation(conversation)}
                  style={{
                    ['--chat-motion-delay' as string]: `${Math.min(index, 12) * 18}ms`,
                  }}
                  className={`chat-motion-row group/search-result flex w-full min-w-0 items-center gap-2 rounded-md px-2.5 py-1.5 text-left transition-colors ${
                    active
                      ? 'bg-black/[0.07] dark:bg-white/[0.1]'
                      : 'hover:bg-black/[0.04] dark:hover:bg-white/[0.07]'
                  }`}
                >
                  <span
                    className={`min-w-0 flex-1 truncate text-[13px] ${
                      active
                        ? 'font-semibold text-neutral-950 dark:text-neutral-50'
                        : 'font-medium text-neutral-800 dark:text-neutral-200'
                    }`}
                    title={conversation.title}
                  >
                    {conversation.title}
                  </span>
                  {setLabel && (
                    <span className="max-w-[100px] shrink-0 truncate text-[12px] text-neutral-400 dark:text-neutral-500">
                      集 · {setLabel}
                    </span>
                  )}
                  {!setLabel && projectLabel && (
                    <span className="max-w-[100px] shrink-0 truncate text-[12px] text-neutral-400 dark:text-neutral-500">
                      {projectLabel}
                    </span>
                  )}
                </button>
              )
            })
          ) : (
            <div className="px-3 py-6 text-center text-[13px] text-neutral-400 dark:text-neutral-500">
              没有匹配的对话
            </div>
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}

export const Sidebar = memo(function Sidebar({
  lang,
  currentConversationId,
  generatingConversationIds = new Set(),
  optimisticConversations = [],
  selectedProject = null,
  onSelectProject,
  selectedSet = null,
  onSelectSet,
  onSelectConversation,
  onNewConversation,
  onConversationDeleted,
  onForceDropConversation,
  onOpenSettings,
  onOpenExtensionsItem,
  onSelectLang,
  onCheckUpdate,
  settingsActive = false,
  extensionsActive = null,
  collapsed,
  onToggleCollapsed,
  refreshKey,
  profileRefreshKey = 0,
  searchOpen,
  onSearchOpenChange,
}: SidebarProps) {
  const asideRef = useRef<HTMLElement>(null)
  // 折叠后侧栏仍挂载（用于滑出动画），用 inert 让其退出 tab 序 / 不可点击 / 不进 a11y 树。
  // useLayoutEffect：在绘制前与 JSX 里的 aria-hidden 原子地一起生效，避免短暂可聚焦窗口。
  useLayoutEffect(() => {
    const el = asideRef.current
    if (el) el.inert = collapsed
  }, [collapsed])
  const [conversations, setConversations] = useState<ConversationListItem[]>([])
  const [projects, setProjects] = useState<ChatProject[]>([])
  const [sets, setSets] = useState<ChatSet[]>([])
  const [assistants, setAssistants] = useState<ChatAssistant[]>([])
  const [searchQuery, setSearchQuery] = useState('')
  // 后端全量索引搜索结果（覆盖所有对话，不止已加载的前 80）；空查询/非 Tauri 时为空，回退客户端过滤。
  const [fullSearchResults, setFullSearchResults] = useState<ConversationListItem[]>([])
  // 侧栏三块改为横排标签页：同一时刻只显示一块（对话/集/项目）。
  const [activeTab, setActiveTab] = useState<'conversations' | 'sets' | 'projects'>('conversations')
  const [collapsedProjectIds, setCollapsedProjectIds] = useState<Set<string>>(
    () => new Set(),
  )
  const [expandedProjectConversationIds, setExpandedProjectConversationIds] = useState<Set<string>>(
    () => new Set(),
  )
  const [collapsedSetIds, setCollapsedSetIds] = useState<Set<string>>(() => new Set())
  const [expandedSetConversationIds, setExpandedSetConversationIds] = useState<Set<string>>(
    () => new Set(),
  )
  const [loading, setLoading] = useState(false)
  const [sectionMenuAnchor, setSectionMenuAnchor] = useState<ConversationMenuAnchor | null>(null)
  const [projectMenuState, setProjectMenuState] = useState<{
    projectId: string
    anchor: ConversationMenuAnchor
  } | null>(null)
  const [dialogProject, setDialogProject] = useState<ChatProject | null | undefined>(undefined)
  const [projectSaving, setProjectSaving] = useState(false)
  const [projectError, setProjectError] = useState('')
  const [setMenuState, setSetMenuState] = useState<{
    setId: string
    anchor: ConversationMenuAnchor
  } | null>(null)
  const [dialogSet, setDialogSet] = useState<ChatSet | null | undefined>(undefined)
  const [setDialogSaving, setSetDialogSaving] = useState(false)
  const [setDialogError, setSetDialogError] = useState('')
  const sectionMenuButtonRef = useRef<HTMLButtonElement>(null)
  const sidebarLoadedRef = useRef(false)
  const [userProfile, setUserProfile] = useState(() => resolveChatUserProfile())

  useEffect(() => {
    let cancelled = false
    void getSettingsCached().then((settings) => {
      if (!cancelled) setUserProfile(resolveChatUserProfile(settings.chat))
    }).catch((err) => {
      console.error('Failed to load chat user profile:', err)
    })
    return () => {
      cancelled = true
    }
  }, [profileRefreshKey])

  const loadSidebarData = useCallback(async (options?: { silent?: boolean; projectOverride?: ChatProject | null; setOverride?: ChatSet | null }) => {
    const projectForLoad = options?.projectOverride === undefined ? selectedProject : options.projectOverride
    const setForLoad = options?.setOverride === undefined ? selectedSet : options.setOverride
    const silent = options?.silent ?? false
    if (!silent) setLoading(true)
    try {
      const [projectData, setData, assistantData, conversationData] = await Promise.all([
        chatApi.getProjects(),
        chatApi.getSets(),
        chatApi.getAssistants(),
        chatApi.getConversations(0, 80),
      ])
      setProjects(projectData)
      setSets(setData)
      setAssistants(assistantData)
      setConversations(conversationData)
      if (projectForLoad && !projectData.some((project) => project.id === projectForLoad.id)) {
        onSelectProject(null)
      }
      if (setForLoad && !setData.some((set) => set.id === setForLoad.id)) {
        onSelectSet(null)
      }
    } catch (err) {
      console.error('Failed to load chat sidebar data:', err)
    } finally {
      if (!silent) setLoading(false)
    }
  }, [onSelectProject, onSelectSet, selectedProject, selectedSet])

  useEffect(() => {
    // 侧栏数据与 selectedProject 无关（loadSidebarData 始终拉全部项目+对话，仅用 selectedProject
    // 判断项目是否被删）。切项目时拉到的是相同数据，不该进 loading 态白闪一下；首次加载非静默
    // 显 loading，之后（含跨项目切换）一律静默后台刷新，消除切换对话时的侧栏闪烁。
    void loadSidebarData({ silent: sidebarLoadedRef.current })
    sidebarLoadedRef.current = true
  }, [loadSidebarData, selectedProject?.id])

  useEffect(() => {
    if (refreshKey === 0) return
    void loadSidebarData({ silent: true })
  }, [loadSidebarData, refreshKey])

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (settingsActive) return
      const mod = e.metaKey || e.ctrlKey
      if (!mod || e.key.toLowerCase() !== 'p') return
      e.preventDefault()
      openCreateProjectDialog()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [settingsActive])

  const handleRenameConversation = async (id: string, title: string) => {
    try {
      await chatApi.updateConversation(id, { title })
      await loadSidebarData({ silent: true })
    } catch (err) {
      console.error('Failed to rename conversation:', err)
    }
  }

  const handleDeleteConversation = async (id: string) => {
    if (!window.confirm('确定删除此对话？此操作无法撤销。')) return
    // B3：删"generating"会话先强制清父组件 in-flight/乐观状态，
    // 让乐观合并（visibleConversations）不再保留它。
    if (generatingConversationIds.has(id)) {
      onForceDropConversation?.(id)
      try {
        await chatApi.cancelStream(id)
      } catch (err) {
        console.error('Failed to cancel stream before delete:', err)
      }
    }
    try {
      await chatApi.deleteConversation(id)
    } catch (err) {
      console.error('Failed to delete conversation:', err)
    } finally {
      // 无论后端删除成功或抛错，都本地剔除该 id 并刷新侧栏，确保 ghost 立即消失。
      setConversations((items) => items.filter((item) => item.id !== id))
      onForceDropConversation?.(id)
      if (currentConversationId === id) {
        onConversationDeleted?.(id)
      }
      await loadSidebarData({ silent: true })
    }
  }

  const handleExportConversation = async (id: string, title: string) => {
    try {
      const path = await save({
        defaultPath: conversationMarkdownFilename(title),
        filters: [{ name: 'Markdown', extensions: ['md'] }],
      })
      if (!path) return
      await chatApi.exportConversationMarkdown(id, path, lang)
    } catch (err) {
      const prefix = lang === 'zh' ? '导出失败：' : 'Export failed: '
      const message = err instanceof Error ? err.message : String(err)
      window.alert(`${prefix}${message}`)
    }
  }

  const handleMoveConversationToProject = async (id: string, projectId: string | undefined) => {
    try {
      const conversation = await chatApi.updateConversation(id, { projectId: projectId ?? null })
      const conversationProjectId = conversation.project_id ?? conversation.projectId ?? null
      if (
        currentConversationId === id &&
        selectedProject &&
        conversationProjectId !== selectedProject.id
      ) {
        onConversationDeleted?.(id)
      }
      await loadSidebarData({ silent: true })
    } catch (err) {
      console.error('Failed to move conversation:', err)
    }
  }

  const handleMoveConversationToSet = async (id: string, setId: string | undefined) => {
    try {
      const conversation = await chatApi.updateConversation(id, { setId: setId ?? null })
      const conversationSetId = conversation.set_id ?? conversation.setId ?? null
      // 当前打开的对话被移出当前选中集，则从主视图移除（与项目逻辑一致）。
      if (currentConversationId === id && selectedSet && conversationSetId !== selectedSet.id) {
        onConversationDeleted?.(id)
      }
      await loadSidebarData({ silent: true })
    } catch (err) {
      console.error('Failed to move conversation to set:', err)
    }
  }

  const openCreateSetDialog = () => {
    setDialogSet(null)
    setSetDialogError('')
  }

  const openSetMenu = (setId: string, button: HTMLButtonElement) => {
    const rect = button.getBoundingClientRect()
    setSetMenuState({ setId, anchor: { left: rect.right - 180, top: rect.bottom + 4 } })
  }

  const handleSaveSet = async (
    name: string,
    systemPrompt: string,
    defaultAssistantId: string | null,
    color: string | null,
  ) => {
    setSetDialogSaving(true)
    setSetDialogError('')
    try {
      const set = dialogSet
        ? await chatApi.updateSet(dialogSet.id, { name, systemPrompt, defaultAssistantId, color })
        : await chatApi.createSet(name, systemPrompt, defaultAssistantId, color)
      onSelectSet(set)
      await loadSidebarData({ silent: true, setOverride: set })
      setDialogSet(undefined)
    } catch (err) {
      setSetDialogError(typeof err === 'string' ? err : (err as Error).message || '集保存失败')
    } finally {
      setSetDialogSaving(false)
    }
  }

  const handleDeleteSet = async (set: ChatSet) => {
    if (!window.confirm(`确定删除集「${set.name}」？集内的对话会移出集，不会被删除。`)) {
      return
    }
    try {
      await chatApi.deleteSet(set.id)
      if (selectedSet?.id === set.id) {
        onSelectSet(null)
        if (currentConversationId) onConversationDeleted?.(currentConversationId)
      }
      await loadSidebarData({ silent: true })
    } catch (err) {
      console.error('Failed to delete set:', err)
    }
  }

  const openSectionMenu = () => {
    // toggle：开着就关，关着就按位置打开。
    if (sectionMenuAnchor) {
      setSectionMenuAnchor(null)
      return
    }
    const button = sectionMenuButtonRef.current
    if (!button) return
    const rect = button.getBoundingClientRect()
    // 菜单右对齐到按钮，但窄侧栏里会顶到窗口左缘 → 夹住至少 8px 边距。
    setSectionMenuAnchor({ left: Math.max(8, rect.right - 200), top: rect.bottom + 4 })
  }

  function openCreateProjectDialog() {
    setDialogProject(null)
    setProjectError('')
  }

  const openProjectMenu = (projectId: string, button: HTMLButtonElement) => {
    const rect = button.getBoundingClientRect()
    setProjectMenuState({
      projectId,
      anchor: { left: rect.right - 180, top: rect.bottom + 4 },
    })
  }

  const handleSaveProject = async (name: string, rootPath?: string | null) => {
    setProjectSaving(true)
    setProjectError('')
    try {
      const project = dialogProject
        ? await chatApi.updateProject(dialogProject.id, { name, rootPath })
        : await chatApi.createProject(name, null, null, rootPath)
      onSelectProject(project)
      await loadSidebarData({ silent: true, projectOverride: project })
      setDialogProject(undefined)
    } catch (err) {
      setProjectError(typeof err === 'string' ? err : (err as Error).message || '项目保存失败')
    } finally {
      setProjectSaving(false)
    }
  }

  const handleOpenProjectFolder = async (project: ChatProject) => {
    try {
      await chatApi.openProjectFolder(project.id)
    } catch (err) {
      window.alert(typeof err === 'string' ? err : (err as Error).message || '打开项目文件夹失败')
    }
  }

  const handleDeleteProject = async (project: ChatProject) => {
    if (!window.confirm(`确定删除项目「${project.name}」？项目内的聊天会移出项目，不会被删除。`)) {
      return
    }
    try {
      await chatApi.deleteProject(project.id)
      if (selectedProject?.id === project.id) {
        onSelectProject(null)
        if (currentConversationId) onConversationDeleted?.(currentConversationId)
      }
      await loadSidebarData({ silent: true })
    } catch (err) {
      console.error('Failed to delete project:', err)
    }
  }

  const handleClearAllConversations = async () => {
    const targetConversations = selectedProject
      ? conversations.filter((conv) => conversationBelongsToProject(conv, selectedProject))
      : conversations
    if (targetConversations.length === 0) return
    const scope = selectedProject ? `项目「${selectedProject.name}」中的` : '全部'
    if (!window.confirm(`确定删除${scope} ${targetConversations.length} 个对话？此操作无法撤销。`)) return
    try {
      await Promise.all(targetConversations.map((conv) => chatApi.deleteConversation(conv.id)))
      if (currentConversationId && targetConversations.some((conv) => conv.id === currentConversationId)) {
        onConversationDeleted?.(currentConversationId)
      }
      await loadSidebarData({ silent: true })
    } catch (err) {
      console.error('Failed to clear conversations:', err)
    }
  }

  const visibleConversations = useMemo(() => {
    if (optimisticConversations.length === 0) return conversations
    const realConversationIds = new Set(conversations.map((item) => item.id))
    const visibleOptimisticConversations = optimisticConversations.filter((item) => {
      return generatingConversationIds.has(item.id) || !realConversationIds.has(item.id)
    })
    if (visibleOptimisticConversations.length === 0) return conversations
    const optimisticIds = new Set(visibleOptimisticConversations.map((item) => item.id))
    return [
      ...visibleOptimisticConversations,
      ...conversations.filter((item) => !optimisticIds.has(item.id)),
    ]
  }, [conversations, generatingConversationIds, optimisticConversations])

  const normalizedSearchQuery = searchQuery.trim().toLowerCase()

  const projectConversationMap = useMemo(() => {
    const map = new Map<string, ConversationListItem[]>()
    projects.forEach((project) => {
      map.set(
        project.id,
        visibleConversations.filter((conversation) => conversationBelongsToProject(conversation, project)),
      )
    })
    return map
  }, [projects, visibleConversations])

  const visibleProjects = projects

  const setConversationMap = useMemo(() => {
    const map = new Map<string, ConversationListItem[]>()
    sets.forEach((set) => {
      map.set(
        set.id,
        visibleConversations.filter(
          (conversation) => (conversation.set_id ?? conversation.setId) === set.id,
        ),
      )
    })
    return map
  }, [sets, visibleConversations])

  // 「最近」标签：跨集/项目的全部对话，置顶在前、再按更新时间倒序。
  const recentConversations = useMemo(
    () =>
      [...visibleConversations].sort((a, b) => {
        const pin = Number(Boolean(b.pinned)) - Number(Boolean(a.pinned))
        if (pin !== 0) return pin
        return (b.updated_at ?? 0) - (a.updated_at ?? 0)
      }),
    [visibleConversations],
  )

  // 查询变化时去后端全量索引搜（debounce 180ms）。覆盖掉出"最近 80"的老对话。
  useEffect(() => {
    if (!searchOpen || !normalizedSearchQuery) {
      setFullSearchResults([])
      return
    }
    let cancelled = false
    const handle = setTimeout(() => {
      void chatApi
        .searchConversations(searchQuery, 30)
        .then((items) => {
          if (!cancelled) setFullSearchResults(items)
        })
        .catch(() => {
          if (!cancelled) setFullSearchResults([])
        })
    }, 180)
    return () => {
      cancelled = true
      clearTimeout(handle)
    }
  }, [searchOpen, normalizedSearchQuery, searchQuery])

  const searchResults = useMemo(() => {
    if (!normalizedSearchQuery) {
      return visibleConversations.slice(0, 9)
    }
    // Tauri：后端全量结果优先；为空/mock 时回退到已加载列表的客户端过滤（也覆盖后端结果到达前的瞬间）。
    if (fullSearchResults.length > 0) {
      return fullSearchResults
    }
    return visibleConversations
      .filter((conversation) => {
        const project = findConversationProject(conversation, projects)
        return (
          conversationMatchesSearch(conversation, normalizedSearchQuery) ||
          (project ? projectMatchesSearch(project, normalizedSearchQuery) : false) ||
          (conversation.folder ?? '').toLowerCase().includes(normalizedSearchQuery)
        )
      })
      .slice(0, 9)
  }, [normalizedSearchQuery, projects, visibleConversations, fullSearchResults])

  const clearableConversationCount = selectedProject
    ? conversations.filter((conv) => conversationBelongsToProject(conv, selectedProject)).length
    : conversations.length

  const allVisibleProjectsCollapsed = visibleProjects.length > 0 &&
    visibleProjects.every((project) => collapsedProjectIds.has(project.id))

  const allVisibleSetsCollapsed =
    sets.length > 0 && sets.every((set) => collapsedSetIds.has(set.id))

  const menuSet = setMenuState ? sets.find((set) => set.id === setMenuState.setId) : undefined

  const closeSearch = useCallback(() => {
    onSearchOpenChange(false)
    setSearchQuery('')
  }, [onSearchOpenChange])

  const handleSelectSearchConversation = useCallback((conversation: ConversationListItem) => {
    const project = findConversationProject(conversation, projects)
    if (project) {
      onSelectProject(project)
    } else if (selectedProject) {
      onSelectProject(null)
    }
    onSelectConversation(conversation.id)
    closeSearch()
  }, [closeSearch, onSelectConversation, onSelectProject, projects, selectedProject])

  const menuProject = projectMenuState
    ? projects.find((project) => project.id === projectMenuState.projectId)
    : undefined

  return (
    <>
      <aside
        ref={asideRef}
        className={`chat-sidebar-shell flex w-[240px] shrink-0 flex-col overflow-hidden${
          collapsed ? ' is-collapsed' : ''
        }`}
        aria-hidden={collapsed}
      >
        {/* 侧栏内顶栏行只在 macOS 存在：那两枚按钮要贴着系统交通灯排。
            Windows / Linux 已把它们常驻到全宽标题栏带（见 ChatTitlebar），此处渲染会重复。 */}
        {usesNativeTitlebar && (
          <div
            className={`chat-titlebar-row chat-sidebar-titlebar-row flex shrink-0 gap-2 ${chatTitlebarMacInsetClass} pr-3`}
            data-tauri-drag-region
          >
            <ChatTitlebarActions
              sidebarExpanded
              onToggleSidebar={onToggleCollapsed}
              onNewConversation={onNewConversation}
            />
            <div className="min-w-0 flex-1" data-tauri-drag-region />
          </div>
        )}

      <nav
        className={`shrink-0 space-y-0.5 px-3 pb-2 ${usesNativeTitlebar ? '' : 'pt-2'}`}
        data-tauri-drag-region="false"
      >
        <NavRow
          icon={<SquarePen size={17} strokeWidth={1.75} />}
          label="新建聊天"
          onClick={onNewConversation}
          iconMotion="group-hover:-rotate-6 group-hover:scale-110"
        />
        <NavRow
          icon={<Search size={17} strokeWidth={1.75} />}
          label="搜索"
          onClick={() => onSearchOpenChange(true)}
          active={searchOpen}
          iconMotion="group-hover:scale-110"
        />
        <ExtensionsNav
          activeItem={extensionsActive}
          onSelectItem={onOpenExtensionsItem}
        />
      </nav>

      <div className="mx-3 border-t border-neutral-200/90 dark:border-neutral-800" />

      <div className="flex min-h-0 flex-1 flex-col" data-tauri-drag-region="false">
        {loading ? (
          <div className="space-y-2 px-3 py-3" aria-label="加载中" aria-busy="true">
            {[0, 1, 2, 3, 4, 5].map((i) => (
              <div key={i} className="kv-skeleton h-7 rounded-lg" />
            ))}
          </div>
        ) : (
          <>
            <div className="flex items-center justify-between px-3 pb-1 pt-3">
              <div className="flex items-center gap-1.5 text-[13px] font-semibold">
                {([
                  ['conversations', '最近'],
                  ['sets', '集'],
                  ['projects', '项目'],
                ] as const).flatMap(([tab, label], i) => {
                  const button = (
                    <button
                      key={tab}
                      type="button"
                      onClick={() => setActiveTab(tab)}
                      className={`rounded-md px-1.5 py-0.5 transition-colors ${
                        activeTab === tab
                          ? 'text-neutral-900 dark:text-neutral-100'
                          : 'text-neutral-400 hover:text-neutral-600 dark:text-neutral-500 dark:hover:text-neutral-300'
                      }`}
                      aria-current={activeTab === tab}
                    >
                      {label}
                    </button>
                  )
                  return i === 0
                    ? [button]
                    : [
                        <span key={`sep-${tab}`} className="text-neutral-300 dark:text-neutral-700">
                          /
                        </span>,
                        button,
                      ]
                })}
              </div>
              <div className="flex shrink-0 items-center gap-1">
                {activeTab === 'conversations' && (
                  <>
                    <IconButton
                      ref={sectionMenuButtonRef}
                      size="sm"
                      onClick={openSectionMenu}
                      className={sectionMenuAnchor ? 'bg-black/[0.06] text-neutral-600 dark:bg-white/[0.1] dark:text-neutral-200' : ''}
                      label="对话列表操作"
                      aria-haspopup="menu"
                      aria-expanded={sectionMenuAnchor !== null}
                    >
                      <MoreHorizontal size={15} />
                    </IconButton>
                    <IconButton
                      size="sm"
                      onClick={onNewConversation}
                      label="新建聊天"
                    >
                      <SquarePen size={15} strokeWidth={1.75} />
                    </IconButton>
                  </>
                )}
                {activeTab === 'sets' && (
                  <>
                    <IconButton
                      size="sm"
                      onClick={() => {
                        setCollapsedSetIds((previous) => {
                          const next = new Set(previous)
                          if (allVisibleSetsCollapsed) {
                            sets.forEach((set) => next.delete(set.id))
                          } else {
                            sets.forEach((set) => next.add(set.id))
                          }
                          return next
                        })
                      }}
                      label={allVisibleSetsCollapsed ? '展开全部集' : '折叠全部集'}
                    >
                      <MoreHorizontal size={15} />
                    </IconButton>
                    <IconButton
                      size="sm"
                      onClick={openCreateSetDialog}
                      label="新建集"
                    >
                      <Plus size={15} strokeWidth={2} />
                    </IconButton>
                  </>
                )}
                {activeTab === 'projects' && (
                  <>
                    <IconButton
                      size="sm"
                      onClick={() => {
                        setCollapsedProjectIds((previous) => {
                          const next = new Set(previous)
                          if (allVisibleProjectsCollapsed) {
                            visibleProjects.forEach((project) => next.delete(project.id))
                          } else {
                            visibleProjects.forEach((project) => next.add(project.id))
                          }
                          return next
                        })
                      }}
                      label={allVisibleProjectsCollapsed ? '展开全部项目' : '折叠全部项目'}
                    >
                      <MoreHorizontal size={15} />
                    </IconButton>
                    <IconButton
                      size="sm"
                      onClick={openCreateProjectDialog}
                      label="新建项目"
                      title={`新建项目 (${modLabel}P)`}
                    >
                      <FolderPlus size={15} strokeWidth={1.75} />
                    </IconButton>
                  </>
                )}
              </div>
            </div>

            <div className="custom-scrollbar min-h-0 flex-1 overflow-y-auto" data-tauri-drag-region="false">
            {activeTab === 'projects' && (
            <section key="projects" className="chat-motion-tab-in group/projects px-3 pb-2 pt-1">
                <div className="mt-1.5 space-y-1">
                  {visibleProjects.map((project) => {
                    const active = selectedProject?.id === project.id
                    const projectConversations = projectConversationMap.get(project.id) ?? []
                    const collapsedProject = collapsedProjectIds.has(project.id)
                    const expanded = expandedProjectConversationIds.has(project.id)
                    const previewConversations = expanded
                      ? projectConversations
                      : projectConversations.slice(0, PROJECT_PREVIEW_LIMIT)
                    return (
                      <div key={project.id}>
                        <div
                          className={`group flex min-w-0 items-center rounded-lg ${
                            active
                              ? 'bg-black/[0.04] dark:bg-white/[0.08]'
                              : 'hover:bg-black/[0.035] dark:hover:bg-white/[0.06]'
                          }`}
                        >
                          <button
                            type="button"
                            onClick={() => {
                              setCollapsedProjectIds((previous) => {
                                const next = new Set(previous)
                                if (next.has(project.id)) next.delete(project.id)
                                else next.add(project.id)
                                return next
                              })
                            }}
                            className={`flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-left text-[13px] ${
                              active
                                ? 'font-semibold text-neutral-900 dark:text-neutral-100'
                                : 'font-medium text-neutral-600 dark:text-neutral-300'
                            }`}
                            title={collapsedProject ? `展开 ${project.name}` : `折叠 ${project.name}`}
                            aria-expanded={!collapsedProject}
                          >
                            <ChevronRight
                              size={13}
                              strokeWidth={2}
                              className={`shrink-0 text-neutral-400 transition-transform dark:text-neutral-500 ${
                                collapsedProject ? '' : 'rotate-90'
                              }`}
                            />
                            <Folder
                              size={15}
                              strokeWidth={1.75}
                              className="shrink-0 text-neutral-500 dark:text-neutral-400"
                            />
                            <span className="min-w-0 truncate">{project.name}</span>
                          </button>
                          <IconButton
                            size="sm"
                            onClick={(e) => {
                              e.stopPropagation()
                              openProjectMenu(project.id, e.currentTarget)
                            }}
                            className={`shrink-0 transition-opacity ${
                              projectMenuState?.projectId === project.id
                                ? 'opacity-100'
                                : 'opacity-0 group-hover:opacity-100'
                            }`}
                            label="项目操作"
                          >
                            <MoreHorizontal size={15} />
                          </IconButton>
                          <IconButton
                            size="sm"
                            onClick={(e) => {
                              e.stopPropagation()
                              setCollapsedProjectIds((previous) => {
                                const next = new Set(previous)
                                next.delete(project.id)
                                return next
                              })
                              onSelectProject(project)
                            }}
                            className="mr-1 shrink-0 opacity-0 transition-opacity group-hover:opacity-100"
                            label="新建聊天"
                          >
                            <SquarePen size={15} strokeWidth={1.75} />
                          </IconButton>
                        </div>

                      {!collapsedProject && previewConversations.length > 0 && (
                        <ConversationList
                          conversations={previewConversations}
                          currentConversationId={currentConversationId}
                          generatingConversationIds={generatingConversationIds}
                          projects={projects}
                          sets={sets}
                          lang={lang}
                          compact
                          indent
                          showAssistantName={false}
                          onSelectConversation={(id) => {
                            if (selectedProject?.id !== project.id) onSelectProject(project)
                            onSelectConversation(id)
                          }}
                          onRenameConversation={handleRenameConversation}
                          onExportConversation={handleExportConversation}
                          onDeleteConversation={handleDeleteConversation}
                          onMoveConversationToProject={handleMoveConversationToProject}
                          onMoveConversationToSet={handleMoveConversationToSet}
                        />
                      )}

                      {!collapsedProject && projectConversations.length > PROJECT_PREVIEW_LIMIT && (
                        <button
                          type="button"
                          onClick={() => {
                            setExpandedProjectConversationIds((previous) => {
                              const next = new Set(previous)
                              if (next.has(project.id)) next.delete(project.id)
                              else next.add(project.id)
                              return next
                            })
                          }}
                          className="ml-8 rounded-md px-2.5 py-0.5 text-left text-[13px] font-medium text-neutral-400 transition-colors hover:bg-black/[0.035] hover:text-neutral-600 dark:text-neutral-500 dark:hover:bg-white/[0.06] dark:hover:text-neutral-300"
                        >
                          {expanded ? '收起' : '展开显示'}
                        </button>
                      )}
                      </div>
                    )
                  })}
                </div>
            </section>
            )}

            {activeTab === 'sets' && (
            <section key="sets" className="chat-motion-tab-in group/sets px-3 pb-2 pt-1">
                <div className="mt-1.5 space-y-1">
                  {sets.length === 0 ? (
                    <button
                      type="button"
                      onClick={openCreateSetDialog}
                      className="flex w-full items-center gap-1.5 rounded-lg px-2 py-1 text-left text-[13px] text-neutral-400 transition-colors hover:bg-black/[0.035] hover:text-neutral-600 dark:text-neutral-500 dark:hover:bg-white/[0.06] dark:hover:text-neutral-300"
                    >
                      <Plus size={14} strokeWidth={2} className="shrink-0" />
                      新建一个集（系统提示词 + 默认助手）
                    </button>
                  ) : (
                    sets.map((set) => {
                      const active = selectedSet?.id === set.id
                      const setConversations = setConversationMap.get(set.id) ?? []
                      const collapsedSet = collapsedSetIds.has(set.id)
                      const expanded = expandedSetConversationIds.has(set.id)
                      const previewConversations = expanded
                        ? setConversations
                        : setConversations.slice(0, PROJECT_PREVIEW_LIMIT)
                      return (
                        <div key={set.id}>
                          <div
                            className={`group flex min-w-0 items-center rounded-lg ${
                              active
                                ? 'bg-black/[0.04] dark:bg-white/[0.08]'
                                : 'hover:bg-black/[0.035] dark:hover:bg-white/[0.06]'
                            }`}
                          >
                            <button
                              type="button"
                              onClick={() => {
                                setCollapsedSetIds((previous) => {
                                  const next = new Set(previous)
                                  if (next.has(set.id)) next.delete(set.id)
                                  else next.add(set.id)
                                  return next
                                })
                              }}
                              className={`flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-left text-[13px] ${
                                active
                                  ? 'font-semibold text-neutral-900 dark:text-neutral-100'
                                  : 'font-medium text-neutral-600 dark:text-neutral-300'
                              }`}
                              title={collapsedSet ? `展开 ${set.name}` : `折叠 ${set.name}`}
                              aria-expanded={!collapsedSet}
                            >
                              <ChevronRight
                                size={13}
                                strokeWidth={2}
                                className={`shrink-0 text-neutral-400 transition-transform dark:text-neutral-500 ${
                                  collapsedSet ? '' : 'rotate-90'
                                }`}
                              />
                              <Layers
                                size={15}
                                strokeWidth={1.75}
                                className={`shrink-0 ${set.color ? '' : 'text-neutral-500 dark:text-neutral-400'}`}
                                style={set.color ? { color: set.color } : undefined}
                              />
                              <span className="min-w-0 truncate">{set.name}</span>
                            </button>
                            <IconButton
                              size="sm"
                              onClick={(e) => {
                                e.stopPropagation()
                                openSetMenu(set.id, e.currentTarget)
                              }}
                              className={`shrink-0 transition-opacity ${
                                setMenuState?.setId === set.id ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'
                              }`}
                              label="集操作"
                            >
                              <MoreHorizontal size={15} />
                            </IconButton>
                            <IconButton
                              size="sm"
                              onClick={(e) => {
                                e.stopPropagation()
                                setCollapsedSetIds((previous) => {
                                  const next = new Set(previous)
                                  next.delete(set.id)
                                  return next
                                })
                                onSelectSet(set)
                              }}
                              className="mr-1 shrink-0 opacity-0 transition-opacity group-hover:opacity-100"
                              label="在此集新建聊天"
                            >
                              <SquarePen size={15} strokeWidth={1.75} />
                            </IconButton>
                          </div>

                          {!collapsedSet && previewConversations.length > 0 && (
                            <ConversationList
                              conversations={previewConversations}
                              currentConversationId={currentConversationId}
                              generatingConversationIds={generatingConversationIds}
                              projects={projects}
                              sets={sets}
                              lang={lang}
                              compact
                              indent
                              showAssistantName={false}
                              onSelectConversation={(id) => {
                                if (selectedSet?.id !== set.id) onSelectSet(set)
                                onSelectConversation(id)
                              }}
                              onRenameConversation={handleRenameConversation}
                              onExportConversation={handleExportConversation}
                              onDeleteConversation={handleDeleteConversation}
                              onMoveConversationToProject={handleMoveConversationToProject}
                              onMoveConversationToSet={handleMoveConversationToSet}
                            />
                          )}

                          {!collapsedSet && setConversations.length > PROJECT_PREVIEW_LIMIT && (
                            <button
                              type="button"
                              onClick={() => {
                                setExpandedSetConversationIds((previous) => {
                                  const next = new Set(previous)
                                  if (next.has(set.id)) next.delete(set.id)
                                  else next.add(set.id)
                                  return next
                                })
                              }}
                              className="ml-8 rounded-md px-2.5 py-0.5 text-left text-[13px] font-medium text-neutral-400 transition-colors hover:bg-black/[0.035] hover:text-neutral-600 dark:text-neutral-500 dark:hover:bg-white/[0.06] dark:hover:text-neutral-300"
                            >
                              {expanded ? '收起' : '展开显示'}
                            </button>
                          )}
                        </div>
                      )
                    })
                  )}
                </div>
            </section>
            )}

            {activeTab === 'conversations' && (
            <section key="conversations" className="chat-motion-tab-in group/conversations px-3 pb-5 pt-1">
              {sectionMenuAnchor && (
                <ChatSectionMenu
                  anchor={sectionMenuAnchor}
                  hasConversations={clearableConversationCount > 0}
                  onNewConversation={onNewConversation}
                  onOpenSearch={() => onSearchOpenChange(true)}
                  onClearAll={() => void handleClearAllConversations()}
                  onClose={() => setSectionMenuAnchor(null)}
                  triggerRef={sectionMenuButtonRef}
                />
              )}

              {recentConversations.length > 0 ? (
                <div className="mt-1.5">
                    <ConversationList
                      conversations={recentConversations}
                      currentConversationId={currentConversationId}
                      generatingConversationIds={generatingConversationIds}
                      projects={projects}
                      sets={sets}
                      lang={lang}
                      compact
                      showAssistantName={false}
                      showFolderLabel
                      onSelectConversation={(id) => {
                        if (selectedProject) onSelectProject(null)
                        if (selectedSet) onSelectSet(null)
                        onSelectConversation(id)
                      }}
                      onRenameConversation={handleRenameConversation}
                      onExportConversation={handleExportConversation}
                      onDeleteConversation={handleDeleteConversation}
                      onMoveConversationToProject={handleMoveConversationToProject}
                      onMoveConversationToSet={handleMoveConversationToSet}
                    />
                </div>
              ) : null}
            </section>
            )}
            </div>
          </>
        )}
      </div>

      <SidebarUserFooter
        profile={userProfile}
        lang={lang}
        settingsActive={settingsActive}
        onOpenSettings={onOpenSettings}
        onSelectLang={onSelectLang}
        onCheckUpdate={onCheckUpdate}
      />

      {projectMenuState && menuProject && (
        <ProjectContextMenu
          anchor={projectMenuState.anchor}
          hasRootFolder={Boolean((menuProject.root_path ?? menuProject.rootPath ?? '').trim())}
          onRename={() => {
            setDialogProject(menuProject)
            setProjectError('')
          }}
          onOpenFolder={() => void handleOpenProjectFolder(menuProject)}
          onDelete={() => void handleDeleteProject(menuProject)}
          onClose={() => setProjectMenuState(null)}
        />
      )}

      {dialogProject !== undefined && (
        <ProjectDialog
          project={dialogProject}
          saving={projectSaving}
          error={projectError}
          onSave={(name, rootPath) => void handleSaveProject(name, rootPath)}
          onClose={() => setDialogProject(undefined)}
        />
      )}

      {setMenuState && menuSet && (
        <SetContextMenu
          anchor={setMenuState.anchor}
          onRename={() => {
            setDialogSet(menuSet)
            setSetDialogError('')
          }}
          onDelete={() => void handleDeleteSet(menuSet)}
          onClose={() => setSetMenuState(null)}
        />
      )}

      {dialogSet !== undefined && (
        <SetDialog
          set={dialogSet}
          assistants={assistants}
          saving={setDialogSaving}
          error={setDialogError}
          onSave={(name, systemPrompt, defaultAssistantId, color) =>
            void handleSaveSet(name, systemPrompt, defaultAssistantId, color)
          }
          onClose={() => setDialogSet(undefined)}
        />
      )}
    </aside>

    {searchOpen && (
      <SearchDialog
        query={searchQuery}
        results={searchResults}
        currentConversationId={currentConversationId}
        projects={projects}
        sets={sets}
        onQueryChange={setSearchQuery}
        onSelectConversation={handleSelectSearchConversation}
        onClose={closeSearch}
      />
    )}
    </>
  )
})
