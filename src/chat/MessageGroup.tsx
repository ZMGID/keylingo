import { memo, useMemo, useState, type ReactNode } from 'react'
import { Check, Columns2, Square } from 'lucide-react'
import type { ChatMessage } from './types'
import { MessageBubble } from './MessageBubble'
import { ModelIcon } from './ModelIcon'
import { getActiveGroup, useGroupVersion, type GroupColumnSnapshot } from './groupStreamingStore'
import { useMultiAnswerViewMode } from './multiAnswerViewMode'

// 多模型一问多答（任务 06-30 / 步骤 6 + 8）：把同一 group_id 的 N 条 assistant 答案展示出来。
// 两种来源互斥：
//  - 流式中（sendMessage 未返回）：列来自 groupStreamingStore 的实时列（live=true）。
//  - 落库后：列来自持久化的 assistant 消息（live=false），各带 group_id / provider_id / model。
// virtua 把整组当「一行」item 虚拟化（见 MessageList），不破坏滚动/钉底。
//
// 两种展示模式（全局偏好 useMultiAnswerViewMode，默认 'tabs'）：
//  - 'tabs'（切换）：一次只整宽显示**当前选中条**（默认第一条），组末尾 footer 切换显示哪条。
//  - 'columns'（并排）：N 列横向并排（原有实现，视觉/性能完全不变）。
// 组末尾 footer：视图切换控件 + 一排模型 chip（点 chip = 切显示条 +「续聊选中条」一举两用）。
//
// 性能降级（步骤 8 / R10）：N 列同时全量渲染 reasoning + markdown 是内存/CPU 大头。
// 「聚焦列」（hover 的列 / tabs 模式当前显示列）展开 reasoning 流式；其余「非聚焦列」把
// reasoningStreaming 置 false → ReasoningBlock 折叠并把正文从 DOM 卸载（hideBody）。
// 复用既有 KaTeX Shadow DOM / rAF 合帧（touchGroup）/ virtua 屏外卸载，不重复造轮子。

interface MessageGroupProps {
  conversationId?: string | null
  groupId: string
  // 落库后的本组 assistant 消息（顺序即列序）；流式中为空。
  messages: ChatMessage[]
  // 当前组的选中条 message id（D5）；空时默认第一列。
  selectedMessageId?: string | null
  onSelectColumn?: (groupId: string, messageId: string) => void
  onUpdateMessage?: (messageId: string, content: string) => Promise<void>
  onRegenerateMessage?: (messageId: string) => Promise<void>
  onForkMessage?: (messageId: string) => Promise<void>
  onDeleteMessage?: (messageId: string) => Promise<void>
  onSaveMessageToNote?: (messageId: string) => Promise<boolean>
}

interface GroupColumn {
  message: ChatMessage
  streaming: boolean
}

function columnModelLabel(provider: string | null | undefined, model: string | null | undefined): string {
  const m = (model ?? '').trim()
  const p = (provider ?? '').trim()
  if (m && p) return `${m} | ${p}`
  return m || p || '模型'
}

function streamColumnToMessage(column: GroupColumnSnapshot): ChatMessage {
  return {
    id: column.messageId,
    role: 'assistant',
    content: column.content,
    reasoning: column.reasoning || undefined,
    artifacts: [],
    tool_calls: column.toolCalls,
    segments: column.segments,
    provider_id: column.providerId,
    model: column.model,
    timestamp: Math.floor(Date.now() / 1000),
  }
}

// 列内滚动框：用原生滚动 + CSS `overscroll-contain`（滚到列边界不串联到外层列表）。
// 不要用 JS wheel 监听手动改 scrollTop——那会绕过浏览器合成器的平滑/惯性滚动，导致掉帧。
function ColumnScrollBody({ children }: { children: ReactNode }) {
  return (
    <div className="chat-message-group-col-body custom-scrollbar min-h-0 flex-1 overflow-y-auto overscroll-contain">
      {children}
    </div>
  )
}

// 单列容器：并排模式（columns）和切换模式（tabs）共用同一个 MessageBubble 渲染，
// 仅外层布局/边框/选中态不同（通过 props 控制）。
function GroupColumnView({
  column,
  conversationId,
  live,
  isSelected,
  isFocused,
  showColumnChrome,
  groupId,
  onMouseEnter,
  onSelectColumn,
  onUpdateMessage,
  onRegenerateMessage,
  onForkMessage,
  onDeleteMessage,
  onSaveMessageToNote,
}: {
  column: GroupColumn
  conversationId?: string | null
  live: boolean
  isSelected: boolean
  isFocused: boolean
  // columns 模式渲染列头（model 标签 + 「用这条继续」按钮）；tabs 模式列头交给 footer chip。
  showColumnChrome: boolean
  groupId: string
  onMouseEnter?: () => void
  onSelectColumn?: (groupId: string, messageId: string) => void
  onUpdateMessage?: (messageId: string, content: string) => Promise<void>
  onRegenerateMessage?: (messageId: string) => Promise<void>
  onForkMessage?: (messageId: string) => Promise<void>
  onDeleteMessage?: (messageId: string) => Promise<void>
  onSaveMessageToNote?: (messageId: string) => Promise<boolean>
}) {
  const { message, streaming } = column
  const wrapperClass = showColumnChrome
    ? `chat-message-group-col flex max-h-[min(560px,70vh)] min-w-[280px] max-w-[420px] flex-1 flex-col rounded-2xl border px-3 py-2 ${
        isSelected
          ? 'border-emerald-400/70 bg-emerald-50/40 dark:border-emerald-500/50 dark:bg-emerald-950/20'
          : 'border-neutral-200/70 bg-neutral-50/40 dark:border-neutral-700/60 dark:bg-neutral-900/30'
      }`
    : 'chat-message-group-tab flex w-full flex-col'
  return (
    <div
      onMouseEnter={onMouseEnter}
      className={wrapperClass}
      data-chat-message-group-focused={isFocused ? 'true' : undefined}
    >
      {showColumnChrome && (
        <div className="mb-1 flex items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-1.5 text-[11px] font-medium text-neutral-500 dark:text-neutral-400">
            {message.model && <ModelIcon model={message.model} size={14} />}
            <span className="min-w-0 truncate" title={columnModelLabel(message.provider_id, message.model)}>
              {columnModelLabel(message.provider_id, message.model)}
            </span>
          </div>
          {!live && onSelectColumn && (
            <button
              type="button"
              onClick={() => onSelectColumn(groupId, message.id)}
              aria-pressed={isSelected}
              title={isSelected ? '已选为续聊上下文' : '用这条继续'}
              className={`inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium transition-colors ${
                isSelected
                  ? 'bg-emerald-500/90 text-white'
                  : 'border border-neutral-200 text-neutral-500 hover:bg-neutral-100 dark:border-neutral-700 dark:text-neutral-400 dark:hover:bg-neutral-800'
              }`}
            >
              <Check size={11} strokeWidth={2.5} />
              {isSelected ? '已选' : '用这条继续'}
            </button>
          )}
        </div>
      )}
      {/* 列内滚动框 + 滚动隔离：头部固定，正文超列高时列内竖向滚动；光标在哪列就滚哪列
          （ColumnScrollBody 非 passive wheel 监听），到边界再交给外层列表。
          tabs 模式整宽显示，不限列高，交给外层列表滚动（不套 ColumnScrollBody）。 */}
      {showColumnChrome ? (
        <ColumnScrollBody>
          <MessageBubble
            message={message}
            conversationId={conversationId}
            messageStreaming={streaming}
            // 性能降级（R10）：非聚焦列把 reasoningStreaming 置 false，让 ReasoningBlock 折叠
            // 并把思维链正文从 DOM 卸载（hideBody）；聚焦列正常展示流式思考。
            reasoningStreaming={streaming && isFocused}
            onUpdateMessage={!live ? onUpdateMessage : undefined}
            onRegenerateMessage={!live ? onRegenerateMessage : undefined}
            onForkMessage={!live ? onForkMessage : undefined}
            onDeleteMessage={!live ? onDeleteMessage : undefined}
            onSaveMessageToNote={!live ? onSaveMessageToNote : undefined}
          />
        </ColumnScrollBody>
      ) : (
        <MessageBubble
          message={message}
          conversationId={conversationId}
          messageStreaming={streaming}
          // tabs 模式：当前显示列即聚焦列 → 正常展示流式思考。
          reasoningStreaming={streaming && isFocused}
          onUpdateMessage={!live ? onUpdateMessage : undefined}
          onRegenerateMessage={!live ? onRegenerateMessage : undefined}
          onForkMessage={!live ? onForkMessage : undefined}
          onDeleteMessage={!live ? onDeleteMessage : undefined}
          onSaveMessageToNote={!live ? onSaveMessageToNote : undefined}
        />
      )}
    </div>
  )
}

// 组末尾切换栏（参考 Cherry）：视图切换控件 + 一排模型 chip。
function GroupFooter({
  columns,
  viewMode,
  onChangeViewMode,
  activeMessageId,
  markContext,
  onSelectChip,
}: {
  columns: GroupColumn[]
  viewMode: 'tabs' | 'columns'
  onChangeViewMode: (mode: 'tabs' | 'columns') => void
  // 当前高亮的那条（tabs：正显示的；columns：续聊选中条）。
  activeMessageId: string | null
  // 高亮 chip 是否代表「已选为下一轮上下文」（落库后为 true；流式中上下文未定为 false）。
  markContext: boolean
  onSelectChip: (messageId: string) => void
}) {
  return (
    <div className="chat-message-group-footer mt-2 flex flex-wrap items-center gap-2 border-t border-neutral-200/60 pt-2.5 dark:border-neutral-700/50">
      {/* 视图切换：iOS 风分段控件，激活项白底浮起，克制不抢眼 */}
      <div className="inline-flex shrink-0 items-center rounded-lg bg-neutral-100 p-0.5 dark:bg-neutral-800/60">
        {([
          ['tabs', Square, '切换', '切换显示（一次一条）'],
          ['columns', Columns2, '并排', '并排显示（多列）'],
        ] as const).map(([mode, Icon, label, hint]) => (
          <button
            key={mode}
            type="button"
            onClick={() => onChangeViewMode(mode)}
            aria-pressed={viewMode === mode}
            title={hint}
            className={`inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium transition-colors ${
              viewMode === mode
                ? 'bg-white text-neutral-800 shadow-sm dark:bg-neutral-700 dark:text-neutral-100'
                : 'text-neutral-400 hover:text-neutral-600 dark:text-neutral-500 dark:hover:text-neutral-300'
            }`}
          >
            <Icon size={12} strokeWidth={2} />
            {label}
          </button>
        ))}
      </div>
      {/* 模型 chip：图标 + 模型名（短，无边框），当前显示/选中的淡绿高亮 */}
      <div className="flex min-w-0 flex-wrap items-center gap-0.5">
        {columns.map(({ message }) => {
          const isActive = message.id === activeMessageId
          const shortLabel = (message.model ?? '').trim() || columnModelLabel(message.provider_id, message.model)
          return (
            <button
              key={message.id}
              type="button"
              onClick={() => onSelectChip(message.id)}
              aria-pressed={isActive}
              title={columnModelLabel(message.provider_id, message.model)}
              className={`inline-flex max-w-[160px] shrink-0 items-center gap-1.5 rounded-full px-2.5 py-1 text-[11px] font-medium transition-colors ${
                isActive
                  ? 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
                  : 'text-neutral-500 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800'
              }`}
            >
              {message.model && <ModelIcon model={message.model} size={13} />}
              <span className="min-w-0 truncate">{shortLabel}</span>
              {isActive && markContext && (
                <Check size={12} strokeWidth={2.5} className="shrink-0 text-emerald-600 dark:text-emerald-400" />
              )}
            </button>
          )
        })}
      </div>
    </div>
  )
}

function MessageGroupBase({
  conversationId,
  groupId,
  messages,
  selectedMessageId,
  onSelectColumn,
  onUpdateMessage,
  onRegenerateMessage,
  onForkMessage,
  onDeleteMessage,
  onSaveMessageToNote,
}: MessageGroupProps) {
  // 订阅 group store 版本号：流式列内容更新时驱动重渲。
  // 版本号还必须进下面 columns 的 memo deps —— store 是原地 mutate 列对象，
  // liveGroup 引用永不变，只靠 [live, liveGroup, messages] 会让 memo 冻结在首帧。
  const groupsVersion = useGroupVersion(conversationId)
  const liveGroup = conversationId ? getActiveGroup(conversationId) : undefined
  const live = Boolean(liveGroup && liveGroup.groupId === groupId)

  // 全局展示模式偏好（默认 tabs）。
  const [viewMode, setViewMode] = useMultiAnswerViewMode()

  // 聚焦列索引（性能降级 R10，仅 columns 模式用）：hover 哪一列就聚焦哪一列；默认聚焦第一列。
  const [focusedIndex, setFocusedIndex] = useState(0)
  // tabs 模式当前显示的列 message id（未显式选 → 跟随选中条 / 第一条）。
  const [tabMessageId, setTabMessageId] = useState<string | null>(null)

  const columns = useMemo<GroupColumn[]>(() => {
    if (live && liveGroup) {
      return liveGroup.columns.map((col) => ({
        message: streamColumnToMessage(col),
        streaming: col.streaming,
      }))
    }
    return messages.map((message) => ({ message, streaming: false }))
    // groupsVersion：流式列被原地 mutate，靠版本号让 memo 重算（见上方注释）。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [live, liveGroup, messages, groupsVersion])

  if (columns.length === 0) return null

  // 选中列：有显式记录用它；否则默认第一列（D5）。流式态不显示选中标记（还没落库）。
  const effectiveSelectedId = selectedMessageId || (columns[0]?.message.id ?? null)

  // tabs 模式当前显示哪条：用户在本组点过 chip → tabMessageId（若仍存在于列里）；
  // 否则跟随续聊选中条 / 第一条。流式列认领后 message id 会从占位变真实，故做存在性校验。
  const tabActiveId =
    (tabMessageId && columns.some((c) => c.message.id === tabMessageId) ? tabMessageId : null) ??
    effectiveSelectedId
  const tabColumn = columns.find((c) => c.message.id === tabActiveId) ?? columns[0]

  // footer chip 点击：tabs 模式切显示条；并落到续聊选中条（onSelectColumn，落库态才有意义）。
  const handleChipClick = (messageId: string) => {
    setTabMessageId(messageId)
    if (!live && onSelectColumn) onSelectColumn(groupId, messageId)
  }

  // footer 高亮的那条：tabs 看正显示的；columns 看续聊选中条（流式态无选中 → 不高亮）。
  const footerActiveId = viewMode === 'tabs' ? tabColumn.message.id : (live ? null : effectiveSelectedId)

  return (
    <div className="chat-message-group-wrap flex w-full flex-col py-2">
      {viewMode === 'columns' ? (
        <div className="chat-message-group custom-scrollbar flex w-full gap-3 overflow-x-auto pb-1">
          {columns.map((column, index) => (
            <GroupColumnView
              key={column.message.id}
              column={column}
              conversationId={conversationId}
              live={live}
              isSelected={!live && column.message.id === effectiveSelectedId}
              isFocused={index === focusedIndex}
              showColumnChrome
              groupId={groupId}
              onMouseEnter={() => setFocusedIndex(index)}
              onSelectColumn={onSelectColumn}
              onUpdateMessage={onUpdateMessage}
              onRegenerateMessage={onRegenerateMessage}
              onForkMessage={onForkMessage}
              onDeleteMessage={onDeleteMessage}
              onSaveMessageToNote={onSaveMessageToNote}
            />
          ))}
        </div>
      ) : (
        <GroupColumnView
          key={tabColumn.message.id}
          column={tabColumn}
          conversationId={conversationId}
          live={live}
          isSelected={false}
          // tabs 当前显示列即聚焦列（正常展示流式思考）。
          isFocused
          showColumnChrome={false}
          groupId={groupId}
          onSelectColumn={onSelectColumn}
          onUpdateMessage={onUpdateMessage}
          onRegenerateMessage={onRegenerateMessage}
          onForkMessage={onForkMessage}
          onDeleteMessage={onDeleteMessage}
          onSaveMessageToNote={onSaveMessageToNote}
        />
      )}
      <GroupFooter
        columns={columns}
        viewMode={viewMode}
        onChangeViewMode={setViewMode}
        activeMessageId={footerActiveId}
        markContext={!live}
        onSelectChip={handleChipClick}
      />
    </div>
  )
}

export const MessageGroup = memo(MessageGroupBase)
