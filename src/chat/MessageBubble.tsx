import { memo, useEffect, useMemo, useRef, useState } from 'react'
import {
  AlertCircle,
  Bot,
  Brain,
  Check,
  Copy,
  CornerDownRight,
  FileCode2,
  FilePen,
  FileSearch,
  FileText,
  FolderInput,
  FolderOpen,
  Globe,
  GitBranch,
  ImagePlus,
  ListChecks,
  Play,
  Plug,
  RotateCcw,
  ScrollText,
  Search,
  SquareTerminal,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { Button, IconButton } from '../components/Button'
import { copyToClipboard } from '../utils/clipboard'
import { AssistantMessageMeta } from './AssistantMessageMeta'
import { ChatAttachments } from './ChatAttachments'
import { ChatDotGridBackground } from './ChatDotGridBackground'
import { ChatMarkdown } from './ChatMarkdown'
import { DegradedAnswerCard } from './DegradedAnswerCard'
import { GeneratedFileArtifacts } from './GeneratedFileArtifacts'
import { MarkdownStreamingContext } from './markdownStreaming'
import { artifactId, artifactPresentationFromToolCall, isArtifactPresentationToolCall } from './artifactPresentation'
import { isExecutableAgentPlanText } from './agentPlan'
import { artifactDataUrl, isImageArtifact } from './artifacts'
import { loadArtifactDataUrl } from './attachmentPreview'
import { openChatImageViewer } from './imageViewer'
import { ChatInlineImage } from './ChatInlineImage'
import { ReasoningBlock } from './ReasoningBlock'
import { ModelIcon } from './ModelIcon'
import { ToolCallBlock } from './ToolCallBlock'
import { ToolCallErrorBoundary } from './ToolCallErrorBoundary'
import type { AgentPlanState, ChatMessage, ChatMessageSegment, ChatToolArtifact, ToolCallRecord } from './types'
import { buildCitationMap, type CitationView } from './citations'
import {
  compareTimelineSegments,
  groupTimelineSegments,
  isStandaloneToolCard,
  isUserFollowUpToolCall,
  isUserSteerToolCall,
  segmentToolCallId,
  summarizeToolGroup,
  toolRecordId,
  userFollowUpText,
  userSteerText,
} from './segments'
import type { TimelineGroupItem, ToolGroupIcon } from './segments'

const DIRECT_IMAGE_GENERATION_PENDING = '[[KIVIO_DIRECT_IMAGE_GENERATION_PENDING]]'

// 模块级稳定引用：内联箭头每次渲染新建会打穿 ChatMarkdown 的 memo（导致公式重渲）。
const handleChatImageClick = (src: string, alt: string, name?: string) =>
  openChatImageViewer({ src, alt, name })

interface MessageBubbleProps {
  message: ChatMessage
  conversationId?: string | null
  tokensPerSec?: number
  reasoningDurationMs?: number | null
  reasoningDurationMsBySegmentId?: Record<string, number>
  /** 思维链正在流式写入 */
  reasoningStreaming?: boolean
  /** 这条消息整体是否在流式生成中（仅 streaming-assistant bubble 为 true） */
  messageStreaming?: boolean
  /** R8（多模型一问多答）：本条 user 消息这一问发给了哪些模型；多模型时渲染在气泡顶部。 */
  sentModels?: { providerId: string | null; model: string | null }[]
  onUpdateMessage?: (messageId: string, content: string) => Promise<void>
  onRegenerateMessage?: (messageId: string, newContent?: string) => Promise<void>
  onForkMessage?: (messageId: string) => Promise<void>
  /** 一键 rewind：截掉这条提问及其之后的消息，原文回输入框（仅 user 气泡）。 */
  onRewindMessage?: (messageId: string) => Promise<void>
  onDeleteMessage?: (messageId: string) => Promise<void>
  onSaveMessageToNote?: (messageId: string) => Promise<boolean>
  agentPlanOverride?: AgentPlanState | null
  onExecuteAgentPlan?: (messageId: string) => Promise<void> | void
}

function markdownImageSources(content: string): Set<string> {
  const sources = new Set<string>()
  for (const match of content.matchAll(/!\[[^\]]*]\(([^)\s]+)(?:\s+"[^"]*")?\)/g)) {
    sources.add(match[1].trim().toLowerCase())
  }
  return sources
}

function artifactDisplayKey(name: string): string {
  try {
    return decodeURIComponent(name).trim().replace(/^\.?\//, '').replace(/\\/g, '/').toLowerCase()
  } catch {
    return name.trim().replace(/^\.?\//, '').replace(/\\/g, '/').toLowerCase()
  }
}

function artifactIsReferenced(content: string, artifact: ChatToolArtifact): boolean {
  const sources = markdownImageSources(content)
  if (sources.size === 0) return false
  const dataUrl = artifactDataUrl(artifact)
  if (dataUrl && content.includes(dataUrl)) return true
  const name = artifactDisplayKey(artifact.name)
  const basename = name.split('/').filter(Boolean).pop() ?? name
  for (const source of sources) {
    const normalizedSource = artifactDisplayKey(source)
    if (normalizedSource === name || normalizedSource === basename) {
      return true
    }
  }
  return false
}

/** MCP 默认名 mcp-image-*.png 是技术附件 id，不适合当用户可读标题。 */
function isTechnicalMcpImageName(name: string | undefined | null): boolean {
  if (!name) return false
  return /^mcp-image-[\w.-]+\.(png|jpe?g|gif|webp)$/i.test(name.trim())
}

function artifactCaption(artifact: ChatToolArtifact, index: number, total: number): string | null {
  const name = (artifact.name ?? '').trim()
  if (!name) return null
  if (isTechnicalMcpImageName(name)) {
    // 多张时显示「截图 1/3」，单张只写「截图」
    return total > 1 ? `截图 ${index + 1}/${total}` : '截图'
  }
  return name
}

function ArtifactImage({
  artifact,
  conversationId,
  caption,
}: {
  artifact: ChatToolArtifact
  conversationId?: string | null
  caption?: string | null
}) {
  const inline = artifactDataUrl(artifact)
  const path = (artifact.path ?? '').trim()
  // 有 path 时 data_url 通常是 256px 缩略图（落盘外置后）；聊天区应显示整图，缩略图仅作秒显占位。
  const [src, setSrc] = useState<string>(inline)

  useEffect(() => {
    let cancelled = false
    if (path && conversationId) {
      if (inline) setSrc(inline)
      void loadArtifactDataUrl(artifact, conversationId).then((loaded) => {
        if (!cancelled && loaded) setSrc(loaded)
      })
      return () => {
        cancelled = true
      }
    }
    if (inline) {
      setSrc(inline)
      return
    }
    return () => {
      cancelled = true
    }
  }, [inline, path, artifact, conversationId])

  if (!src) return null
  const name = artifact.name || 'Generated image'
  const label = caption === undefined ? name : caption
  const openViewer = () =>
    openChatImageViewer({
      src,
      alt: label || name,
      name: artifact.name,
      path: artifact.path,
      conversationId,
    })
  return (
    <figure className="m-0">
      <ChatInlineImage
        src={src}
        alt={label || name}
        name={artifact.name}
        onOpenViewer={openViewer}
      />
      {label ? (
        <figcaption className="mt-1 text-[11px] text-neutral-400 dark:text-neutral-500">
          {label}
        </figcaption>
      ) : null}
    </figure>
  )
}

/**
 * 答案下方图片画廊：
 * - 消息级 artifacts 优先（明确交付物）
 * - 工具产生的截图只保留「最后一轮」有图的 tool calls，避免 3 轮×3 页堆出 9 张同名小图
 */
function selectGalleryImageArtifacts(
  messageArtifacts: ChatToolArtifact[],
  toolCalls: ToolCallRecord[],
  contentForRefs: string,
): ChatToolArtifact[] {
  const notReferenced = (a: ChatToolArtifact) =>
    isImageArtifact(a) && !artifactIsReferenced(contentForRefs, a)

  const fromMessage = messageArtifacts.filter(notReferenced)
  if (fromMessage.length > 0) return fromMessage

  // 找最后一个产生图片的 round，只展示该轮（通常是最终 screenshot 验收）
  let lastImageRound: number | null = null
  for (const tc of toolCalls) {
    const hasImg = (tc.artifacts ?? []).some(isImageArtifact)
    if (!hasImg) continue
    const r = tc.round ?? 0
    if (lastImageRound == null || r >= lastImageRound) lastImageRound = r
  }
  if (lastImageRound == null) return []

  const fromLastRound: ChatToolArtifact[] = []
  for (const tc of toolCalls) {
    const r = tc.round ?? 0
    if (r !== lastImageRound) continue
    for (const a of tc.artifacts ?? []) {
      if (notReferenced(a)) fromLastRound.push(a)
    }
  }
  return fromLastRound
}

function GeneratedImageArtifacts({
  artifacts,
  conversationId,
}: {
  artifacts: ChatToolArtifact[]
  conversationId?: string | null
}) {
  const imageArtifacts = artifacts.filter(isImageArtifact)
  if (imageArtifacts.length === 0) return null
  const total = imageArtifacts.length

  return (
    <div className="mt-3 space-y-3">
      {imageArtifacts.map((artifact, index) => (
        <ArtifactImage
          key={`${artifact.path || artifact.name || 'img'}-${index}`}
          artifact={artifact}
          conversationId={conversationId}
          caption={artifactCaption(artifact, index, total)}
        />
      ))}
    </div>
  )
}

function ArtifactPresentationBlock({
  toolCall,
  artifacts,
  conversationId,
}: {
  toolCall: ToolCallRecord
  artifacts: ChatToolArtifact[]
  conversationId?: string | null
}) {
  const presentation = artifactPresentationFromToolCall(toolCall)
  if (!presentation) {
    return (
      <ToolCallErrorBoundary>
        <ToolCallBlock toolCall={toolCall} />
      </ToolCallErrorBoundary>
    )
  }
  const artifactById = new Map(
    artifacts
      .map((artifact) => [artifactId(artifact), artifact] as const)
      .filter(([id]) => Boolean(id)),
  )
  const selected = presentation.artifactIds
    .map((id) => artifactById.get(id))
    .filter((artifact): artifact is ChatToolArtifact => Boolean(artifact))
  const missingCount = presentation.artifactIds.length - selected.length
  if (presentation.artifactIds.length === 0) {
    return (
      <ToolCallErrorBoundary>
        <ToolCallBlock toolCall={toolCall} />
      </ToolCallErrorBoundary>
    )
  }

  return (
    <section aria-label="展示文件" className="not-prose my-2">
      {presentation.caption ? (
        <div className="mb-2 text-[13px] leading-5 text-neutral-600 dark:text-neutral-300">
          {presentation.caption}
        </div>
      ) : null}
      <GeneratedImageArtifacts artifacts={selected} conversationId={conversationId} />
      <GeneratedFileArtifacts artifacts={selected} />
      {missingCount > 0 ? (
        <div className="mt-2 inline-flex items-center gap-1.5 text-[11.5px] text-neutral-400 dark:text-neutral-500">
          <AlertCircle size={12} strokeWidth={1.9} />
          <span>{missingCount} 个文件不可用</span>
        </div>
      ) : null}
    </section>
  )
}

function ImageGenerationPending() {
  return (
    <section aria-label="图片生成中" className="image-generation-pending">
      <div className="mb-3">
        <div className="flex items-center gap-2 text-[14px] font-medium leading-5 text-neutral-700 dark:text-neutral-300">
          <span className="image-generation-pending-indicator" aria-hidden="true" />
          <span>正在生成图片</span>
        </div>
        <div className="mt-1 pl-4 text-[12px] leading-5 text-neutral-400 dark:text-neutral-500">
          正在细化画面细节，请稍候。
        </div>
      </div>
      <div className="image-generation-pending-frame" aria-hidden="true">
        <ChatDotGridBackground />
      </div>
    </section>
  )
}

function AgentPlanAction({
  messageId,
  planState,
  disabled,
  onExecute,
}: {
  messageId: string
  planState?: AgentPlanState | null
  disabled?: boolean
  onExecute?: (messageId: string) => Promise<void> | void
}) {
  const plan = planState?.plan?.trim() ?? ''
  if (!isExecutableAgentPlanText(plan)) return null

  const approved = (planState?.status ?? 'draft') === 'approved'
  return (
    <div className="not-prose mt-3 flex max-w-full items-center gap-2 border-l-2 border-emerald-400/70 pl-3 text-[12px] leading-5 text-neutral-500 dark:border-emerald-500/60 dark:text-neutral-400">
      <ListChecks size={14} strokeWidth={2} className="shrink-0 text-emerald-600 dark:text-emerald-400" />
      <span className="min-w-0 flex-1 truncate">{approved ? '已按这条计划执行' : '计划草案'}</span>
      {!approved && onExecute && (
        <Button
          variant="primary"
          size="sm"
          onClick={() => void onExecute(messageId)}
          disabled={disabled}
          title="执行这条计划"
          aria-label="执行这条计划"
        >
          <Play size={12} strokeWidth={2.2} fill="currentColor" />
          执行这条计划
        </Button>
      )}
    </div>
  )
}

function orderedSegments(segments?: ChatMessageSegment[]): ChatMessageSegment[] {
  return [...(segments ?? [])].sort(compareTimelineSegments)
}

function segmentText(segment: ChatMessageSegment): string {
  return segment.text ?? ''
}

function MissingToolSegment({ toolCallId }: { toolCallId: string }) {
  return (
    <div className="not-prose mb-2 inline-flex max-w-full items-center gap-1.5 rounded-md py-0.5 text-[11.5px] leading-5 text-neutral-400 dark:text-neutral-500">
      <AlertCircle size={12} strokeWidth={1.9} className="shrink-0" />
      <span className="truncate">工具记录缺失{toolCallId ? ` · ${toolCallId}` : ''}</span>
    </div>
  )
}

/**
 * 用户在生成中插进来的那句话（「立刻引导」）。它不是一次工具调用，所以不套工具卡的外壳 ——
 * 渲染成一条右对齐的小气泡，读起来就是「我在这里插了一句」，与时间线上下文的因果关系对得上。
 */
function UserSteerSegment({ toolCall }: { toolCall: ToolCallRecord }) {
  const text = isUserFollowUpToolCall(toolCall) ? userFollowUpText(toolCall) : userSteerText(toolCall)
  if (!text.trim()) return null
  return (
    <div className="not-prose flex justify-end">
      <div className="flex max-w-[85%] items-start gap-1.5 rounded-md bg-neutral-100 px-2.5 py-1.5 text-[12.5px] leading-5 text-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
        <CornerDownRight
          size={13}
          strokeWidth={1.9}
          className="mt-0.5 shrink-0 text-neutral-400 dark:text-neutral-500"
        />
        <span className="min-w-0 whitespace-pre-wrap break-words">{text}</span>
      </div>
    </div>
  )
}

function isUserInjectedToolCall(toolCall: ToolCallRecord): boolean {
  return isUserSteerToolCall(toolCall) || isUserFollowUpToolCall(toolCall)
}

function TimelineToolSegment({
  segment,
  toolCallById,
  artifacts,
  conversationId,
}: {
  segment: ChatMessageSegment
  toolCallById: ReadonlyMap<string, ToolCallRecord>
  artifacts: ChatToolArtifact[]
  conversationId?: string | null
}) {
  const toolCallId = segmentToolCallId(segment)
  const toolCall = toolCallById.get(toolCallId)
  if (!toolCall) {
    return <MissingToolSegment toolCallId={toolCallId} />
  }
  if (isUserInjectedToolCall(toolCall)) {
    return <UserSteerSegment toolCall={toolCall} />
  }
  if (isArtifactPresentationToolCall(toolCall)) {
    return (
      <ArtifactPresentationBlock
        toolCall={toolCall}
        artifacts={artifacts}
        conversationId={conversationId}
      />
    )
  }
  return (
    <ToolCallErrorBoundary>
      <ToolCallBlock toolCall={toolCall} />
    </ToolCallErrorBoundary>
  )
}

function TimelineTextSegment({
  segment,
  artifacts,
  citations,
  conversationId,
}: {
  segment: ChatMessageSegment
  artifacts: ChatToolArtifact[]
  citations?: Map<number, CitationView>
  conversationId?: string | null
}) {
  const text = segmentText(segment).trim()
  if (!text) return null
  const isProcessText = segment.phase === 'tool_loop' || segment.phase === 'auxiliary'
  return (
    <div className={isProcessText ? 'text-neutral-600 dark:text-neutral-300' : undefined}>
      <ChatMarkdown
        content={text}
        artifacts={artifacts}
        conversationId={conversationId}
        citations={citations}
        onImageClick={handleChatImageClick}
      />
    </div>
  )
}

function TimelineSegmentNode({
  segment,
  index,
  segmentCount,
  toolCallById,
  artifacts,
  citations,
  conversationId,
  reasoningStreaming,
  reasoningDurationMs,
  reasoningDurationMsBySegmentId,
  reasoningSegmentCount,
}: {
  segment: ChatMessageSegment
  index: number
  segmentCount: number
  toolCallById: ReadonlyMap<string, ToolCallRecord>
  artifacts: ChatToolArtifact[]
  citations?: Map<number, CitationView>
  conversationId?: string | null
  reasoningStreaming: boolean
  reasoningDurationMs?: number | null
  reasoningDurationMsBySegmentId?: Record<string, number>
  reasoningSegmentCount: number
}) {
  if (segment.kind === 'tool') {
    return (
      <TimelineToolSegment
        segment={segment}
        toolCallById={toolCallById}
        artifacts={artifacts}
        conversationId={conversationId}
      />
    )
  }
  if (segment.kind === 'reasoning') {
    const reasoning = segmentText(segment)
    if (!reasoning.trim()) return null
    return (
      <ReasoningBlock
        reasoning={reasoning}
        streaming={reasoningStreaming && index === segmentCount - 1}
        durationMs={
          reasoningDurationMsBySegmentId?.[segment.id]
            ?? (reasoningSegmentCount === 1 ? reasoningDurationMs : null)
        }
      />
    )
  }
  if (!segmentText(segment).trim()) return null
  return (
    <TimelineTextSegment
      segment={segment}
      artifacts={artifacts}
      citations={citations}
      conversationId={conversationId}
    />
  )
}

function TimelineStepsIcon({ size = 16, className }: { size?: number; className?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
    >
      <circle cx="3.5" cy="4" r="1.6" />
      <circle cx="3.5" cy="12" r="1.6" />
      <path d="M3.5 5.6v4.8" />
      <path d="M8 4h5" />
      <path d="M8 12h3.5" />
    </svg>
  )
}

/** macOS 经典放射状短线 spinner：8 根短线绕中心放射、透明度阶梯递增，整体步进旋转。 */
function TimelineSpinner({ size = 16, className }: { size?: number; className?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      className={className}
      aria-hidden="true"
    >
      <g className="kv-tick-spinner" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round">
        {Array.from({ length: 8 }).map((_, i) => (
          <line
            key={i}
            x1="12"
            y1="3.5"
            x2="12"
            y2="7"
            transform={`rotate(${i * 45} 12 12)`}
            opacity={(i + 1) / 8}
          />
        ))}
      </g>
    </svg>
  )
}

/**
 * 分组头折叠态图标：按摘要代表类别选 lucide 图标，与 ToolCallBlock 的单工具图标观感一致。
 * `other`（通用/混合兜底）保留自绘 TimelineStepsIcon。
 */
const GROUP_ICON_BY_CATEGORY: Record<
  ToolGroupIcon,
  LucideIcon | typeof TimelineStepsIcon
> = {
  read: FileText,
  codeSearch: Search,
  globFiles: FileSearch,
  fileWrite: FilePen,
  runCommand: SquareTerminal,
  webFetch: Globe,
  webSearch: Search,
  runPython: FileCode2,
  listDir: FolderOpen,
  fileOps: FolderInput,
  todo: ListChecks,
  memory: Brain,
  subAgent: Bot,
  skill: ScrollText,
  image: ImagePlus,
  notion: Plug,
  mcp: Plug,
  reasoning: Brain,
  other: TimelineStepsIcon,
}

/**
 * 一组「连续的 thinking + tool 段」= 单一可折叠单元。
 * - 「生成中」= 这条消息还在流式生成、且这是末组（messageStreaming && isLastGroup）：
 *   始终保持展开，不受工具间隙/reasoning 是否在流影响，避免抖动。
 * - 后面出现正文/别的块（非末组）或消息流式结束（含历史消息）→ 折叠成一行摘要。
 * - 用户手动点过开关后以用户操作为准（userToggledRef，参考 ReasoningBlock）。
 * - 历史折叠态只保留摘要 header，不挂载组内 ReasoningBlock / ToolCallBlock；
 *   展开后再原样平铺，避免重历史消息默认挂满工具/Markdown/Diff 子树。
 */
function TimelineGroupBlock({
  segments,
  toolCalls,
  toolCallById,
  artifacts,
  citations,
  conversationId,
  isLastGroup,
  messageStreaming,
  reasoningStreaming,
  reasoningDurationMs,
  reasoningDurationMsBySegmentId,
  reasoningSegmentCount,
}: {
  segments: ChatMessageSegment[]
  toolCalls: ToolCallRecord[]
  toolCallById: ReadonlyMap<string, ToolCallRecord>
  artifacts: ChatToolArtifact[]
  citations?: Map<number, CitationView>
  conversationId?: string | null
  isLastGroup: boolean
  messageStreaming: boolean
  reasoningStreaming: boolean
  reasoningDurationMs?: number | null
  reasoningDurationMsBySegmentId?: Record<string, number>
  reasoningSegmentCount: number
}) {
  const generating = messageStreaming && isLastGroup
  const summary = useMemo(
    () => summarizeToolGroup(segments, toolCalls, toolCallById),
    [segments, toolCalls, toolCallById],
  )
  const SummaryIcon = GROUP_ICON_BY_CATEGORY[summary.icon]
  const [open, setOpen] = useState(generating)
  const userToggledRef = useRef(false)

  // 自动折叠不能等 effect：生成结束后的第一次 render 仍可能带着旧的 open=true，
  // 先把整棵 ToolCallBlock/Markdown 子树创建出来，effect 下一拍才卸载。用当前生成态
  // 直接参与渲染，保证结束这一帧就不创建详情树；用户手动操作后再由 open 接管。
  const renderDetails = userToggledRef.current ? open : generating

  // 生成中默认展开、完成自动折叠；用户手动操作后不再覆盖。
  useEffect(() => {
    if (userToggledRef.current) return
    setOpen(generating)
  }, [generating])

  const handleToggle = () => {
    userToggledRef.current = true
    setOpen((value) => !value)
  }

  return (
    <section aria-label="过程分组" className="not-prose">
      <button
        type="button"
        onClick={handleToggle}
        aria-expanded={renderDetails}
        data-tauri-drag-region="false"
        className="mb-1 flex w-full items-center gap-1.5 text-left text-[12px] leading-relaxed font-medium text-neutral-400 transition-colors hover:text-neutral-600 dark:text-neutral-500 dark:hover:text-neutral-300"
      >
        {generating ? (
          <TimelineSpinner size={16} className="shrink-0 text-neutral-400 dark:text-neutral-500" />
        ) : (
          <SummaryIcon size={16} className="shrink-0" />
        )}
        <div className="flex min-w-0 items-center gap-1.5">
          <span
            className={`min-w-0 truncate ${
              generating ? 'chat-motion-tool-shimmer' : ''
            }`}
          >
            {summary.text}
          </span>
          {summary.diffStats && (
            <span className="shrink-0 font-mono text-[11px] tabular-nums">
              <span className="text-emerald-600 dark:text-emerald-400">+{summary.diffStats.additions}</span>
              <span className="ml-1 text-red-500/80 dark:text-red-400/80">-{summary.diffStats.removals}</span>
            </span>
          )}
          {summary.categories.length > 1 && (
            <span className="flex shrink-0 items-center gap-1" aria-hidden="true">
              {summary.categories.map((category) => {
                const CategoryIcon = GROUP_ICON_BY_CATEGORY[category]
                return <CategoryIcon key={category} size={14} />
              })}
            </span>
          )}
        </div>
      </button>
      {renderDetails && (
        <div className="chat-motion-reveal is-open" aria-hidden={false}>
          <div className="space-y-1.5">
            {/* 段级淡入只在流式中播：历史消息被虚拟列表反复卸载/重挂载，无条件的
                `both` fill 动画会让回翻时每个重进 DOM 的段落整批重播淡入——外层气泡
                入场早已为此 gate（playEntranceAnimation），内层段落同理。 */}
            {segments.map((segment, index) => (
              <div key={segment.id} className={messageStreaming ? 'chat-motion-fade' : undefined}>
                <TimelineSegmentNode
                  segment={segment}
                  index={index}
                  segmentCount={segments.length}
                  toolCallById={toolCallById}
                  artifacts={artifacts}
                  citations={citations}
                  conversationId={conversationId}
                  reasoningStreaming={reasoningStreaming && isLastGroup}
                  reasoningDurationMs={reasoningDurationMs}
                  reasoningDurationMsBySegmentId={reasoningDurationMsBySegmentId}
                  reasoningSegmentCount={reasoningSegmentCount}
                />
              </div>
            ))}
          </div>
        </div>
      )}
    </section>
  )
}

function TimelineSegments({
  segments,
  toolCalls,
  artifacts,
  conversationId,
  messageStreaming,
  reasoningStreaming,
  reasoningDurationMs,
  reasoningDurationMsBySegmentId,
}: {
  segments: ChatMessageSegment[]
  toolCalls: ToolCallRecord[]
  artifacts: ChatToolArtifact[]
  conversationId?: string | null
  messageStreaming: boolean
  reasoningStreaming: boolean
  reasoningDurationMs?: number | null
  reasoningDurationMsBySegmentId?: Record<string, number>
}) {
  const prepared = useMemo(() => {
    const ordered = orderedSegments(segments)
    const toolCallById = new Map<string, ToolCallRecord>()
    for (const toolCall of toolCalls) {
      const id = toolRecordId(toolCall)
      if (id) toolCallById.set(id, toolCall)
    }

    const citations = buildCitationMap(toolCalls)
    const reasoningSegmentCount = ordered.filter((segment) => segment.kind === 'reasoning').length
    const referencedToolIds = new Set(
      ordered
        .filter((segment) => segment.kind === 'tool')
        .map((segment) => segmentToolCallId(segment))
        .filter(Boolean),
    )
    const orphanTools = toolCalls
      .filter((toolCall) => {
        const id = toolRecordId(toolCall)
        return id && !referencedToolIds.has(id)
      })
      .sort((left, right) => {
        const leftStarted = left.startedAt ?? left.started_at ?? 0
        const rightStarted = right.startedAt ?? right.started_at ?? 0
        return leftStarted - rightStarted
      })

    const groupItems = groupTimelineSegments(ordered, (segment) => {
      const id = segmentToolCallId(segment)
      if (!id) return false
      const toolCall = toolCallById.get(id)
      return toolCall ? isStandaloneToolCard(toolCall) : false
    })

    return { toolCallById, citations, reasoningSegmentCount, orphanTools, groupItems }
  }, [segments, toolCalls])

  const { toolCallById, citations, reasoningSegmentCount, orphanTools, groupItems } = prepared
  const lastGroupIndex = groupItems.reduce(
    (last, item, index) => (item.type === 'group' ? index : last),
    -1,
  )

  return (
    <section aria-label="回答时间线" className="space-y-1.5">
      {groupItems.map((item: TimelineGroupItem, index) => {
        if (item.type === 'text') {
          if (!segmentText(item.segment).trim()) return null
          // 每个时间线分段单独淡入：流式中新分段顺次出现而非"啪"地弹出。
          return (
            <div key={item.segment.id} className={messageStreaming ? 'chat-motion-fade' : undefined}>
              <TimelineTextSegment
                segment={item.segment}
                artifacts={artifacts}
                citations={citations}
                conversationId={conversationId}
              />
            </div>
          )
        }
        if (item.type === 'standaloneTool') {
          // advisor / subagent：专属卡片常驻渲染，不折叠进「调用 N 次工具」组。
          const id = segmentToolCallId(item.segment)
          const toolCall = toolCallById.get(id)
          if (!toolCall) return null
          return (
            <div key={item.segment.id} className={messageStreaming ? 'chat-motion-fade' : undefined}>
              {isUserInjectedToolCall(toolCall) ? (
                <UserSteerSegment toolCall={toolCall} />
              ) : isArtifactPresentationToolCall(toolCall) ? (
                <ArtifactPresentationBlock
                  toolCall={toolCall}
                  artifacts={artifacts}
                  conversationId={conversationId}
                />
              ) : (
                <ToolCallErrorBoundary>
                  <ToolCallBlock toolCall={toolCall} />
                </ToolCallErrorBoundary>
              )}
            </div>
          )
        }
        const groupKey = item.segments[0]?.id ?? `group-${index}`
        return (
          <div key={groupKey} className={messageStreaming ? 'chat-motion-fade' : undefined}>
            <TimelineGroupBlock
              segments={item.segments}
              toolCalls={toolCalls}
              toolCallById={toolCallById}
              artifacts={artifacts}
              citations={citations}
              conversationId={conversationId}
              isLastGroup={index === lastGroupIndex}
              messageStreaming={messageStreaming}
              reasoningStreaming={reasoningStreaming}
              reasoningDurationMs={reasoningDurationMs}
              reasoningDurationMsBySegmentId={reasoningDurationMsBySegmentId}
              reasoningSegmentCount={reasoningSegmentCount}
            />
          </div>
        )
      })}
      {orphanTools.map((toolCall, index) => (
        <div key={toolRecordId(toolCall) || `orphan-tool-${index}`} className={messageStreaming ? 'chat-motion-fade' : undefined}>
          {isUserInjectedToolCall(toolCall) ? (
            <UserSteerSegment toolCall={toolCall} />
          ) : isArtifactPresentationToolCall(toolCall) ? (
            <ArtifactPresentationBlock
              toolCall={toolCall}
              artifacts={artifacts}
              conversationId={conversationId}
            />
          ) : (
            <ToolCallErrorBoundary>
              <ToolCallBlock toolCall={toolCall} />
            </ToolCallErrorBoundary>
          )}
        </div>
      ))}
    </section>
  )
}

function MessageBubbleComponent({
  message,
  conversationId,
  tokensPerSec,
  reasoningDurationMs,
  reasoningDurationMsBySegmentId,
  reasoningStreaming = false,
  messageStreaming = false,
  sentModels,
  onUpdateMessage,
  onRegenerateMessage,
  onForkMessage,
  onRewindMessage,
  onDeleteMessage,
  onSaveMessageToNote,
  agentPlanOverride = null,
  onExecuteAgentPlan,
}: MessageBubbleProps) {
  const isUser = message.role === 'user'
  // 历史消息会被虚拟列表反复卸载/挂载；只让真正的流式预览播放进入动画，
  // 否则滚动时每个重新进入 DOM 的旧气泡都会淡入并上移，看起来像刷新且阻滞滚动。
  const playEntranceAnimation = messageStreaming
  // 「这条是否已落盘并允许历史操作」：门控重新生成。`onUpdateMessage` / `onDeleteMessage`
  // 在这里作为完整可变能力信号；MessageGroup 的在飞列不传它们，从而一次关掉这些入口。
  // 编辑与删除入口已按需求移除，但底层能力仍保留。
  const canMutate = Boolean(onUpdateMessage && onDeleteMessage && onRegenerateMessage)
  const prepared = useMemo(() => {
    const attachments = message.attachments ?? []
    const toolCalls = message.tool_calls ?? message.toolCalls ?? []
    // 后端 recovery.rs 产出的降级描述；旧会话无此字段 → null → 不渲染卡片。
    const degraded = message.degraded ?? null
    // 降级文案同时走三条路：content、时间线 text 分段、以及这张卡片。卡片已完整表达，
    // 另外两条都要按文本相等剔掉，否则同一段话在气泡里出现两遍（正是用户看到的样子）。
    const degradedText = degraded?.text.trim() ?? ''
    const timelineSegments = orderedSegments(message.segments).filter(
      (segment) =>
        !degradedText || segment.kind !== 'text' || segmentText(segment).trim() !== degradedText,
    )
    const hasTimelineSegments = timelineSegments.length > 0
    const messageArtifacts = message.artifacts ?? []
    const toolArtifacts = toolCalls.flatMap((toolCall) => toolCall.artifacts ?? [])
    // Markdown 和显式展示引用仍使用全量 artifacts；回答末尾自动区域只兼容旧的无 ID artifact。
    const renderArtifacts = [...messageArtifacts, ...toolArtifacts]
    const legacyMessageArtifacts = messageArtifacts.filter((artifact) => !artifactId(artifact))
    const legacyToolCalls = toolCalls.map((toolCall) => ({
      ...toolCall,
      artifacts: (toolCall.artifacts ?? []).filter((artifact) => !artifactId(artifact)),
    }))
    const isDirectImageGenerationPending =
      !isUser && message.content.trim() === DIRECT_IMAGE_GENERATION_PENDING
    const artifactReferenceContent = [
      message.content,
      ...timelineSegments.map((segment) => segmentText(segment)),
    ].join('\n\n')
    // 答案下方画廊：只挂「未引用 + 最后一轮截图」，避免 3 轮验收堆 9 张同名图
    const galleryImageArtifacts = selectGalleryImageArtifacts(
      legacyMessageArtifacts,
      legacyToolCalls,
      artifactReferenceContent,
    )
    const generatedFileArtifacts = [
      ...legacyMessageArtifacts,
      ...legacyToolCalls.flatMap((toolCall) => toolCall.artifacts ?? []),
    ].filter((artifact) => !isImageArtifact(artifact))
    const hasAnswerContent =
      !isDirectImageGenerationPending &&
      message.content.trim().length > 0 &&
      message.content.trim() !== degradedText
    const hasGeneratedImages = galleryImageArtifacts.length > 0
    const hasGeneratedFiles = generatedFileArtifacts.length > 0

    return {
      attachments,
      toolCalls,
      degraded,
      degradedText,
      timelineSegments,
      hasTimelineSegments,
      renderArtifacts,
      galleryImageArtifacts,
      generatedFileArtifacts,
      isDirectImageGenerationPending,
      hasAnswerContent,
      hasGeneratedImages,
      hasGeneratedFiles,
    }
  }, [isUser, message])
  const {
    attachments,
    toolCalls,
    degraded,
    timelineSegments,
    hasTimelineSegments,
    renderArtifacts,
    galleryImageArtifacts,
    generatedFileArtifacts,
    isDirectImageGenerationPending,
    hasAnswerContent,
    hasGeneratedImages,
    hasGeneratedFiles,
  } = prepared
  // 后端 recovery.rs 产出的降级描述；旧会话无此字段 → undefined → 不渲染卡片。
  // content 仍保留同一段文本（旧前端 / 外部 CLI 只读 content），但卡片已经完整表达了
  // 同样的信息 —— 这里不再把它当正文渲染，避免一模一样的内容出现两遍。
  const [copied, setCopied] = useState(false)
  const [toolsExpanded, setToolsExpanded] = useState(false)
  // 消息级悬停：鼠标在这条消息上 → 底部操作/元信息条显示，移走 → 隐藏。
  // 显示走 onPointerEnter（送达可靠）；**隐藏不依赖 pointerleave**——悬停期间挂一个
  // document 级 pointermove，指针落在消息外即收起。移动事件是持续流，漏一帧还有
  // 下一帧，不存在「边界事件漏发一次就永久卡住」。监听只在悬停的那一条上活跃
  // （全局同时至多一个），handler 是一次 contains 判断。
  //
  // **不走 React state，走 DOM 属性 + CSS**（`[data-msg-hovered] .msg-hover-reveal`）：
  // 滚动时内容从静止的光标下滑过，WebKit 会随滚动连环补发 enter/leave，每滑过一条
  // 消息就是两次 setState = 两次整棵 MessageBubble 重渲（体内的 map/filter 每次产新
  // 数组，memo 的 ToolCallBlock 等照样全部重渲）——这是滚动不顺滑的主因之一。
  // 属性切换不进 React，重渲为零；React 重渲也不会碰这个非受控属性。
  //
  // ⚠️ 显隐的最终修复不在这里而在渲染层：操作行必须带 `[will-change:opacity]`
  // （见 AssistantMessageMeta / 下方用户操作行）。WKWebView 对非合成层的 opacity
  // 变化存在重绘失效——探针实测状态/类名/computed opacity 全部正确置 0，屏幕上
  // 旧画面滞留不消；提升为合成层后 opacity 由合成器每帧应用，不走重绘路径。
  const hoverRootRef = useRef<HTMLDivElement>(null)
  const hoverMoveCleanupRef = useRef<(() => void) | null>(null)
  const setBubbleHovered = (on: boolean) => {
    // 先清后挂（幂等）：重复 enter、或悬停期间根元素被 React 重挂，都不会漏掉旧监听。
    hoverMoveCleanupRef.current?.()
    hoverMoveCleanupRef.current = null
    const root = hoverRootRef.current
    if (!root) return
    root.toggleAttribute('data-msg-hovered', on)
    if (!on) return
    const onMove = (event: PointerEvent) => {
      const current = hoverRootRef.current
      if (!current || !(event.target instanceof Node) || !current.contains(event.target)) {
        setBubbleHovered(false)
      }
    }
    document.addEventListener('pointermove', onMove, { passive: true })
    hoverMoveCleanupRef.current = () => document.removeEventListener('pointermove', onMove)
  }
  // 悬停中整行被 virtualizer 卸载时，document 监听不能漏。
  useEffect(() => () => {
    hoverMoveCleanupRef.current?.()
    hoverMoveCleanupRef.current = null
  }, [])
  const hoverProps = {
    ref: hoverRootRef,
    onPointerEnter: () => setBubbleHovered(true),
    // 快路径：leave 真来了立刻收；没来由上面的 pointermove 兜底。
    onPointerLeave: () => setBubbleHovered(false),
  }
  // 工具调用超过 4 个时默认折叠（与思考过程一致）
  const toolsCollapsible = toolCalls.length > 4
  const agentPlan = message.agent_plan ?? message.agentPlan ?? agentPlanOverride
  const isAgentPlanMessage = isExecutableAgentPlanText(agentPlan?.plan)

  const handleCopy = async () => {
    const ok = await copyToClipboard(message.content)
    if (!ok) return
    setCopied(true)
    window.setTimeout(() => setCopied(false), 2000)
  }

  if (isUser) {
    const hasText = message.content.trim().length > 0
    // R8（多模型一问多答）：本问发给 ≥2 个模型时，在 user 气泡顶部渲染模型标签行（如 @deepseek @qwen）。
    // 单模型不显示这行（sentModels 缺省或 <2）。
    const replyModelTags = (sentModels ?? []).filter((m) => (m.model ?? '').trim().length > 0)
    const showModelTags = replyModelTags.length >= 2
    return (
      <div
        {...hoverProps}
        className={`flex justify-end py-2 ${playEntranceAnimation ? 'chat-motion-bubble-in' : ''}`}
      >
        <div className="flex min-w-0 max-w-[85%] flex-col items-end gap-1">
          {showModelTags && (
            <div className="flex flex-wrap items-center justify-end gap-1.5 pr-0.5">
              {replyModelTags.map((tag, index) => (
                <span
                  key={`${tag.model}-${index}`}
                  className="chat-user-bubble inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium text-neutral-500 dark:text-neutral-400"
                  title={tag.providerId ? `${tag.model} | ${tag.providerId}` : (tag.model ?? '')}
                >
                  {tag.model && <ModelIcon model={tag.model} size={12} />}
                  <span className="max-w-[140px] truncate">@{tag.model}</span>
                </span>
              ))}
            </div>
          )}
          {attachments.length > 0 && (
            <ChatAttachments
              attachments={attachments}
              conversationId={conversationId}
              variant="user"
            />
          )}
          {hasText && (
            <div className="chat-user-bubble rounded-[20px] px-4 py-2.5 text-neutral-900 dark:text-neutral-100">
              <div className="whitespace-pre-wrap [overflow-wrap:anywhere] text-[15px] leading-relaxed">
                {message.content}
              </div>
            </div>
          )}
          {hasText && (
            <div
              className="msg-hover-reveal flex items-center gap-0.5 pr-0.5 opacity-0 transition-opacity duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-out)] [will-change:opacity] focus-within:opacity-100"
            >
              <IconButton
                size="xs"
                onClick={() => void handleCopy()}
                label={copied ? '已复制' : '复制'}
              >
                {copied ? <Check size={13} strokeWidth={2} className="chat-motion-pop" /> : <Copy size={13} strokeWidth={2} />}
              </IconButton>
              {onRewindMessage && (
                <IconButton
                  size="xs"
                  onClick={() => void onRewindMessage(message.id)}
                  label="回到这里"
                  title="回到这里：删掉这条提问及其之后的消息，原文放回输入框"
                >
                  <RotateCcw size={13} strokeWidth={2} />
                </IconButton>
              )}
              {onForkMessage && (
                <IconButton
                  size="xs"
                  onClick={() => void onForkMessage(message.id)}
                  label="建分支"
                  title="从这里建分支（复制到新对话）"
                >
                  <GitBranch size={13} strokeWidth={2} />
                </IconButton>
              )}
            </div>
          )}
        </div>
      </div>
    )
  }

  const renderToolCall = (toolCall: ToolCallRecord, index: number) => {
    const key = toolCall.id || toolCall.call_id || toolCall.callId || index
    // 无时间线段的旧路径：插话卡照样不能退化成一张写着 user_steer 的工具卡。
    if (isUserInjectedToolCall(toolCall)) {
      return <UserSteerSegment key={key} toolCall={toolCall} />
    }
    if (isArtifactPresentationToolCall(toolCall)) {
      return (
        <ArtifactPresentationBlock
          key={key}
          toolCall={toolCall}
          artifacts={renderArtifacts}
          conversationId={conversationId}
        />
      )
    }
    return (
      <ToolCallErrorBoundary key={key}>
        <ToolCallBlock toolCall={toolCall} />
      </ToolCallErrorBoundary>
    )
  }

  return (
    <MarkdownStreamingContext.Provider value={messageStreaming}>
    <div
      {...hoverProps}
      className={`flex justify-start py-3 ${playEntranceAnimation ? 'chat-motion-bubble-in' : ''}`}
    >
      <div className="w-full min-w-0">
        {toolCalls.length > 0 && !hasTimelineSegments && (
          <section
            aria-label="工具调用"
            className={message.content.trim().length > 0 || message.reasoning ? 'mb-3' : ''}
          >
            {toolsCollapsible ? (
              <button
                type="button"
                onClick={() => setToolsExpanded((value) => !value)}
                className="mb-1 flex w-full items-center gap-1 text-left text-[11px] font-medium text-neutral-400 transition-colors hover:text-neutral-600 dark:text-neutral-500 dark:hover:text-neutral-300"
                aria-expanded={toolsExpanded}
                data-tauri-drag-region="false"
              >
                <span>
                  工具调用 · {toolCalls.length} 个
                </span>
              </button>
            ) : (
              <div className="mb-1 text-[11px] font-medium text-neutral-400 dark:text-neutral-500">
                工具调用
              </div>
            )}
            {toolsCollapsible && toolsExpanded && (
              <div className="chat-motion-reveal is-open">
                <div>{toolCalls.map((toolCall, index) => renderToolCall(toolCall, index))}</div>
              </div>
            )}
            {!toolsCollapsible && toolCalls.map((toolCall, index) => renderToolCall(toolCall, index))}
          </section>
        )}

        {message.reasoning && !hasTimelineSegments && (
          <ReasoningBlock
            reasoning={message.reasoning}
            streaming={reasoningStreaming}
            durationMs={reasoningDurationMs}
          />
        )}

        {isDirectImageGenerationPending ? (
          <ImageGenerationPending />
        ) : hasTimelineSegments ? (
          <>
            <TimelineSegments
              segments={timelineSegments}
              toolCalls={toolCalls}
              artifacts={renderArtifacts}
              conversationId={conversationId}
              messageStreaming={messageStreaming}
              reasoningStreaming={reasoningStreaming}
              reasoningDurationMs={reasoningDurationMs}
              reasoningDurationMsBySegmentId={reasoningDurationMsBySegmentId}
            />
            {hasGeneratedImages && (
              <GeneratedImageArtifacts
                artifacts={galleryImageArtifacts}
                conversationId={conversationId}
              />
            )}
            {hasGeneratedFiles && <GeneratedFileArtifacts artifacts={generatedFileArtifacts} />}
          </>
        ) : (
          (hasAnswerContent || hasGeneratedImages || hasGeneratedFiles) && (
            <section aria-label="回答">
              {(toolCalls.length > 0 || message.reasoning) && (
                <div className="mb-1 text-[11px] font-medium text-neutral-400 dark:text-neutral-500">
                  回答
                </div>
              )}
              {hasAnswerContent && (
                <ChatMarkdown
                  content={message.content}
                  artifacts={renderArtifacts}
                  conversationId={conversationId}
                  onImageClick={handleChatImageClick}
                />
              )}
              {hasGeneratedImages && (
                <GeneratedImageArtifacts
                  artifacts={galleryImageArtifacts}
                  conversationId={conversationId}
                />
              )}
              {hasGeneratedFiles && <GeneratedFileArtifacts artifacts={generatedFileArtifacts} />}
            </section>
          )
        )}

        {/* 降级兜底渲染成独立卡片：故障不混进正文，也不会被复制/回灌给模型。 */}
        {degraded && <DegradedAnswerCard degraded={degraded} />}

        {isAgentPlanMessage && !isDirectImageGenerationPending && (
          <AgentPlanAction
            messageId={message.id}
            planState={agentPlan}
            disabled={messageStreaming}
            onExecute={onExecuteAgentPlan}
          />
        )}

        {message.content.trim().length > 0 && !isDirectImageGenerationPending && (
          <AssistantMessageMeta
            content={message.content}
            reasoning={message.reasoning}
            timestamp={message.timestamp}
            tokensPerSec={tokensPerSec}
            runEntry={message.run_entry ?? message.runEntry}
            streamOutcome={message.stream_outcome ?? message.streamOutcome}
            usage={message.usage}
            onRegenerate={
              canMutate
                ? () => {
                    void onRegenerateMessage!(message.id)
                  }
                : undefined
            }
            onFork={
              onForkMessage
                ? () => {
                    void onForkMessage(message.id)
                  }
                : undefined
            }
            onSaveToNote={
              onSaveMessageToNote
                ? async () => onSaveMessageToNote(message.id)
                : undefined
            }
          />
        )}

        {attachments.length > 0 && (
          <ChatAttachments
            attachments={attachments}
            conversationId={conversationId}
            variant="assistant"
          />
        )}
      </div>
    </div>
    </MarkdownStreamingContext.Provider>
  )
}

// memo：流式生成时历史消息 props 不变 → 跳过重渲染，避免每个 token 重新解析 Markdown
export const MessageBubble = memo(MessageBubbleComponent)
