import { memo, useEffect, useRef, useState } from 'react'
import { Archive, Pin } from 'lucide-react'
import type { ChatProject, ChatSet, ConversationListItem } from './types'
import { i18n, type I18n, type Lang } from '../settings/i18n'
import { isProvisionalTitle } from './conversationTitle'
import { SwapTitle } from './SwapTitle'
import {
  ConversationContextMenu,
  type ConversationMenuAnchor,
} from './ConversationContextMenu'

/** 对话所属分组标签：优先「集 · 名」，否则项目名（按 project_id，退回 folder===项目名）。
 *  与 Sidebar 搜索弹层的显示逻辑一致。无归属时返回空串。 */
function conversationFolderLabel(
  conv: ConversationListItem,
  projects: ChatProject[],
  sets: ChatSet[],
  t: I18n,
): string {
  const setId = conv.set_id ?? conv.setId ?? null
  if (setId) {
    const setName = sets.find((s) => s.id === setId)?.name
    if (setName) return `${t.chatSetPrefix} · ${setName}`
  }
  const projectId = conv.project_id ?? conv.projectId ?? null
  const project = projectId
    ? projects.find((p) => p.id === projectId)
    : projects.find((p) => conv.folder === p.name)
  return project?.name ?? conv.folder ?? ''
}

interface ConversationListProps {
  conversations: ConversationListItem[]
  currentConversationId?: string
  generatingConversationIds?: ReadonlySet<string>
  titleGeneratingConversationIds?: ReadonlySet<string>
  projects: ChatProject[]
  sets: ChatSet[]
  lang: Lang
  compact?: boolean
  indent?: boolean
  showAssistantName?: boolean
  // 「最近」平铺列表用：在每条对话右侧显示其所属「集 / 项目」标签（与搜索弹层一致）。
  // 项目/集 tab 的嵌套列表不传（已在该分组下，标签冗余）。
  showFolderLabel?: boolean
  /**
   * 集/项目下的手动排序。**不传 = 不可拖**（「最近」就不传：那里的语义是时间线）。
   * `scopeId` 写进容器的 `data-reorder-scope`，让 useInsertionReorder 只在本分组内取样。
   */
  reorder?: {
    scopeId: string
    draggingId: string | null
    startDrag: (e: React.PointerEvent<HTMLElement>, id: string) => void
  }
  onSelectConversation: (id: string, conversation?: ConversationListItem) => void
  onRenameConversation: (id: string, title: string) => Promise<void>
  onRegenerateConversationTitle: (id: string) => Promise<void>
  onTogglePinConversation: (id: string, pinned: boolean) => Promise<void>
  /** 一键归档（侧栏默认不再显示归档对话） */
  onArchiveConversation: (id: string) => Promise<void>
  onExportConversation: (id: string, title: string) => Promise<void>
  onDeleteConversation: (id: string) => Promise<void>
  onMoveConversationToProject: (id: string, projectId: string | undefined) => Promise<void>
  onMoveConversationToSet: (id: string, setId: string | undefined) => Promise<void>
}

export const ConversationList = memo(function ConversationList({
  conversations,
  currentConversationId,
  generatingConversationIds = new Set(),
  titleGeneratingConversationIds = new Set(),
  projects,
  sets,
  lang,
  compact = false,
  indent = false,
  showAssistantName = true,
  showFolderLabel = false,
  reorder,
  onSelectConversation,
  onRenameConversation,
  onRegenerateConversationTitle,
  onTogglePinConversation,
  onArchiveConversation,
  onExportConversation,
  onDeleteConversation,
  onMoveConversationToProject,
  onMoveConversationToSet,
}: ConversationListProps) {
  const [menuState, setMenuState] = useState<{
    conversationId: string
    anchor: ConversationMenuAnchor
  } | null>(null)
  const t = i18n[lang]
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renameDraft, setRenameDraft] = useState('')
  const renameInputRef = useRef<HTMLInputElement>(null)

  const menuConversation = menuState
    ? conversations.find((c) => c.id === menuState.conversationId)
    : undefined

  useEffect(() => {
    if (renamingId) {
      renameInputRef.current?.focus()
      renameInputRef.current?.select()
    }
  }, [renamingId])

  /** 右键在光标处弹出菜单（不再用行尾 ⋯ / 图钉按钮）。 */
  const openMenuAtPointer = (conversationId: string, clientX: number, clientY: number) => {
    setMenuState({
      conversationId,
      anchor: { left: clientX, top: clientY },
    })
  }

  const startRename = (conv: ConversationListItem) => {
    setRenamingId(conv.id)
    setRenameDraft(conv.title)
    setMenuState(null)
  }

  const commitRename = async (conversationId: string) => {
    const nextTitle = renameDraft.trim()
    setRenamingId(null)
    if (!nextTitle) return
    const conv = conversations.find((c) => c.id === conversationId)
    if (!conv || conv.title === nextTitle) return
    await onRenameConversation(conversationId, nextTitle)
  }

  if (conversations.length === 0) {
    return null
  }

  return (
    <>
      <div
        className={compact ? 'space-y-0.5 py-0.5' : 'space-y-0.5 py-1'}
        data-reorder-scope={reorder?.scopeId}
      >
        {conversations.map((conv) => {
          const active = currentConversationId === conv.id
          const isGenerating = generatingConversationIds.has(conv.id)
          const isTitleGenerating = titleGeneratingConversationIds.has(conv.id)
          const isRenaming = renamingId === conv.id
          const folderLabel = showFolderLabel ? conversationFolderLabel(conv, projects, sets, t) : ''
          // 分支对话：把「（分支）」后缀从可截断的标题里拆出，做成不缩的固定标签，
          // 避免侧栏窄宽时被省略号吃掉（forked_from 字段判定，不依赖标题文字）。
          const isFork = Boolean(conv.forked_from ?? conv.forkedFrom)
          // 后端写进标题的后缀恒为中文（存量数据也是），所以剥离用常量、显示用 t。
          const FORK_SUFFIX = '（分支）'
          const displayTitle =
            isFork && conv.title.endsWith(FORK_SUFFIX)
              ? conv.title.slice(0, -FORK_SUFFIX.length)
              : conv.title
          const isDragging = reorder?.draggingId === conv.id

          if (isRenaming) {
            return (
              <div
                key={conv.id}
                data-reorder-id={conv.id}
                className={`kv-conv-row group relative flex min-w-0 items-center rounded-lg ${
                  isDragging ? 'is-dragging ' : ''
                }${
                  active
                    ? 'bg-black/[0.07] dark:bg-white/[0.11]'
                    : 'hover:bg-black/[0.04] dark:hover:bg-white/[0.06]'
                }`}
              >
                <input
                  ref={renameInputRef}
                  type="text"
                  value={renameDraft}
                  onChange={(e) => setRenameDraft(e.target.value)}
                  onBlur={() => void commitRename(conv.id)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault()
                      void commitRename(conv.id)
                    }
                    if (e.key === 'Escape') {
                      setRenamingId(null)
                    }
                  }}
                  className={`min-w-0 flex-1 border-0 bg-transparent text-left outline-none focus:ring-0 ${
                    compact
                      ? `${indent ? 'pl-8' : 'pl-2.5'} pr-2 py-1 text-[13px] leading-5`
                      : 'px-3 py-2 text-[13px]'
                  } ${
                    active
                      ? 'font-semibold text-neutral-900 dark:text-neutral-100'
                      : compact
                        ? 'font-medium text-neutral-700 dark:text-neutral-300'
                        : 'text-neutral-700 dark:text-neutral-300'
                  }`}
                />
              </div>
            )
          }

          return (
            <div
              key={conv.id}
              data-reorder-id={conv.id}
              onPointerDown={reorder ? (e) => reorder.startDrag(e, conv.id) : undefined}
              onContextMenu={(e) => {
                e.preventDefault()
                e.stopPropagation()
                openMenuAtPointer(conv.id, e.clientX, e.clientY)
              }}
              className={`kv-conv-row group relative flex min-w-0 items-center rounded-lg ${
                isDragging ? 'is-dragging ' : ''
              }${
                active
                  ? 'bg-black/[0.07] dark:bg-white/[0.11]'
                  : 'hover:bg-black/[0.04] dark:hover:bg-white/[0.06]'
              }`}
            >
              <button
                type="button"
                onClick={() => onSelectConversation(conv.id, conv)}
                onDoubleClick={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  startRename(conv)
                }}
                className={`min-w-0 flex-1 text-left transition-colors ${
                  compact
                    ? `${indent ? 'pl-8' : 'pl-2.5'} pr-2 py-1 text-[13px] leading-5`
                    : 'px-3 py-2 text-[13px]'
                } ${
                  active
                    ? 'font-semibold text-neutral-900 dark:text-neutral-100'
                    : compact
                      ? 'font-medium text-neutral-700 dark:text-neutral-300'
                      : 'text-neutral-700 dark:text-neutral-300'
                }`}
                title={
                  isGenerating || isTitleGenerating
                    ? t.chatTitleGenerating.replace('{title}', conv.title)
                    : conv.title
                }
              >
                <span className="flex min-w-0 items-center gap-1.5">
                  {/* 模型标题替换乐观截断标题时打字机逐字打出；生成中的临时标题置灰 */}
                  <SwapTitle
                    text={displayTitle}
                    className={`block min-w-0 flex-1 truncate${
                      isTitleGenerating
                      || (isGenerating && isProvisionalTitle(displayTitle, conv.preview))
                        ? ' kv-title-provisional'
                        : ''
                    }`}
                  />
                  {isFork && (
                    <span
                      className="shrink-0 text-[11px] font-normal text-neutral-400 dark:text-neutral-500"
                      title={t.chatForkedFromOther}
                    >
                      {t.chatForkSuffix}
                    </span>
                  )}
                  {folderLabel && (
                    <span
                      className="max-w-[96px] shrink-0 truncate text-[11px] font-normal text-neutral-400 dark:text-neutral-500"
                      title={folderLabel}
                    >
                      {folderLabel}
                    </span>
                  )}
                </span>
                {showAssistantName && (conv.assistant_name ?? conv.assistantName) && (
                  <span className="mt-0.5 block truncate text-[11px] font-normal text-neutral-400 dark:text-neutral-500">
                    {(conv.assistant_name ?? conv.assistantName)}
                  </span>
                )}
              </button>
              {/* 行尾：生成中慢波；悬停/已置顶时换成 PIN + 归档。
                  慢波保持原始 chat-gen-wave（14×11 + absolute right-1），与按钮同槽 visibility 互斥。 */}

              <div
                className="kv-conv-trailing relative mr-1 flex h-[22px] w-[44px] shrink-0 items-center justify-end"
                data-busy={isGenerating && !conv.pinned ? '' : undefined}
              >
                {isGenerating && !conv.pinned && (
                  <span
                    className="kv-conv-wave chat-gen-wave pointer-events-none absolute right-1"
                    aria-label={t.chatGenerating}
                    role="status"
                  >
                    <span /><span /><span /><span />
                  </span>
                )}
                <div className="kv-conv-actions flex items-center">
                  <button
                    type="button"
                    data-no-drag
                    onClick={(e) => {
                      e.stopPropagation()
                      void onTogglePinConversation(conv.id, !conv.pinned)
                    }}
                    className={`shrink-0 rounded-md p-0.5 transition-opacity hover:bg-black/[0.06] dark:hover:bg-white/[0.1] ${
                      conv.pinned
                        ? 'text-neutral-700 opacity-100 dark:text-neutral-200'
                        : isGenerating
                          ? 'text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-200'
                          : 'text-neutral-400 opacity-0 group-hover:opacity-100 hover:text-neutral-600 dark:hover:text-neutral-200'
                    }`}
                    aria-label={conv.pinned ? t.chatUnpin : t.chatPin}
                    title={conv.pinned ? t.chatUnpin : t.chatPin}
                  >
                    <Pin size={14} strokeWidth={conv.pinned ? 2.5 : 1.75} />
                  </button>
                  <button
                    type="button"
                    data-no-drag
                    onClick={(e) => {
                      e.stopPropagation()
                      void onArchiveConversation(conv.id)
                    }}
                    className={`shrink-0 rounded-md p-0.5 text-neutral-400 transition-opacity hover:bg-black/[0.06] hover:text-neutral-600 dark:hover:bg-white/[0.1] dark:hover:text-neutral-200 ${
                      isGenerating && !conv.pinned
                        ? ''
                        : 'opacity-0 group-hover:opacity-100'
                    }`}
                    aria-label={t.chatLibArchive}
                    title={t.chatLibArchive}
                  >
                    <Archive size={14} strokeWidth={1.75} />
                  </button>
                </div>
              </div>

            </div>
          )
        })}
      </div>

      {menuState && menuConversation && (
        <ConversationContextMenu
          anchor={menuState.anchor}
          conversationFolder={menuConversation.folder}
          conversationProjectId={menuConversation.project_id ?? menuConversation.projectId ?? null}
          conversationSetId={menuConversation.set_id ?? menuConversation.setId ?? null}
          pinned={Boolean(menuConversation.pinned)}
          projects={projects}
          sets={sets}
          lang={lang}
          onRename={() => startRename(menuConversation)}
          canRegenerateTitle={(menuConversation.message_count ?? 0) > 0}
          regeneratingTitle={titleGeneratingConversationIds.has(menuConversation.id)}
          onRegenerateTitle={() => void onRegenerateConversationTitle(menuConversation.id)}
          onTogglePin={() =>
            void onTogglePinConversation(menuConversation.id, !menuConversation.pinned)
          }
          onExport={() => void onExportConversation(menuConversation.id, menuConversation.title)}
          onMoveToProject={(projectId) => void onMoveConversationToProject(menuConversation.id, projectId)}
          onMoveToSet={(setId) => void onMoveConversationToSet(menuConversation.id, setId)}
          onDelete={() => void onDeleteConversation(menuConversation.id)}
          onClose={() => setMenuState(null)}
        />
      )}
    </>
  )
})
