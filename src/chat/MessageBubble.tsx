import { memo, useEffect, useRef, useState } from 'react'
import {
  AlertCircle,
  Bot,
  Brain,
  Check,
  ChevronDown,
  Copy,
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
  Pencil,
  Play,
  Plug,
  ScrollText,
  Search,
  SquareTerminal,
  Trash2,
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
import { knowledgeSearchHits, type KbHitView } from './knowledgeBaseHits'
import { compareTimelineSegments, groupTimelineSegments, isStandaloneToolCard, segmentToolCallId, summarizeToolGroup, toolRecordId } from './segments'
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

function TimelineToolSegment({
  segment,
  toolCalls,
  artifacts,
  conversationId,
}: {
  segment: ChatMessageSegment
  toolCalls: ToolCallRecord[]
  artifacts: ChatToolArtifact[]
  conversationId?: string | null
}) {
  const toolCallId = segmentToolCallId(segment)
  const toolCall = toolCalls.find((record) => toolRecordId(record) === toolCallId)
  if (!toolCall) {
    return <MissingToolSegment toolCallId={toolCallId} />
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
  citations?: Map<number, KbHitView>
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
  toolCalls,
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
  toolCalls: ToolCallRecord[]
  artifacts: ChatToolArtifact[]
  citations?: Map<number, KbHitView>
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
        toolCalls={toolCalls}
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
  artifacts: ChatToolArtifact[]
  citations?: Map<number, KbHitView>
  conversationId?: string | null
  isLastGroup: boolean
  messageStreaming: boolean
  reasoningStreaming: boolean
  reasoningDurationMs?: number | null
  reasoningDurationMsBySegmentId?: Record<string, number>
  reasoningSegmentCount: number
}) {
  const generating = messageStreaming && isLastGroup
  const summary = summarizeToolGroup(segments, toolCalls)
  const SummaryIcon = GROUP_ICON_BY_CATEGORY[summary.icon]
  const [open, setOpen] = useState(generating)
  const userToggledRef = useRef(false)

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
        aria-expanded={open}
        data-tauri-drag-region="false"
        className="mb-1 flex w-full items-center gap-1.5 text-left text-[15px] leading-relaxed font-medium text-neutral-400 transition-colors hover:text-neutral-600 dark:text-neutral-500 dark:hover:text-neutral-300"
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
          {summary.categories.length > 1 && (
            <span className="flex shrink-0 items-center gap-1" aria-hidden="true">
              {summary.categories.map((category) => {
                const CategoryIcon = GROUP_ICON_BY_CATEGORY[category]
                return <CategoryIcon key={category} size={14} />
              })}
            </span>
          )}
        </div>
        <ChevronDown
          size={16}
          strokeWidth={2}
          className={`shrink-0 transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] ${open ? 'rotate-180' : ''}`}
        />
      </button>
      {open && (
        <div className="chat-motion-reveal is-open" aria-hidden={false}>
          <div className="space-y-1.5">
            {segments.map((segment, index) => (
              <div key={segment.id} className="chat-motion-fade">
                <TimelineSegmentNode
                  segment={segment}
                  index={index}
                  segmentCount={segments.length}
                  toolCalls={toolCalls}
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

/** 汇总本条消息所有 knowledge_search 命中，按 n 建索引，供答案里的 `[n]` 角标查源。
 *  多次检索 n 会重叠 —— 后写覆盖（罕见，且只影响弹窗预览内容）。 */
function buildCitationMap(toolCalls: ToolCallRecord[]): Map<number, KbHitView> {
  const map = new Map<number, KbHitView>()
  for (const toolCall of toolCalls) {
    for (const hit of knowledgeSearchHits(toolCall) ?? []) {
      map.set(hit.n, hit)
    }
  }
  return map
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
  const ordered = orderedSegments(segments)
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
    const toolCall = toolCalls.find((tc) => toolRecordId(tc) === id)
    return toolCall ? isStandaloneToolCard(toolCall) : false
  })
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
            <div key={item.segment.id} className="chat-motion-fade">
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
          const toolCall = toolCalls.find((tc) => toolRecordId(tc) === id)
          if (!toolCall) return null
          return (
            <div key={item.segment.id} className="chat-motion-fade">
              {isArtifactPresentationToolCall(toolCall) ? (
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
          <div key={groupKey} className="chat-motion-fade">
            <TimelineGroupBlock
              segments={item.segments}
              toolCalls={toolCalls}
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
        <div key={toolRecordId(toolCall) || `orphan-tool-${index}`} className="chat-motion-fade">
          {isArtifactPresentationToolCall(toolCall) ? (
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
  onDeleteMessage,
  onSaveMessageToNote,
  agentPlanOverride = null,
  onExecuteAgentPlan,
}: MessageBubbleProps) {
  const isUser = message.role === 'user'
  // 历史消息会被虚拟列表反复卸载/挂载；只让真正的流式预览播放进入动画，
  // 否则滚动时每个重新进入 DOM 的旧气泡都会淡入并上移，看起来像刷新且阻滞滚动。
  const playEntranceAnimation = messageStreaming
  const canMutate = Boolean(onUpdateMessage && onDeleteMessage && onRegenerateMessage)
  const attachments = message.attachments ?? []
  const toolCalls = message.tool_calls ?? message.toolCalls ?? []
  const [isEditing, setIsEditing] = useState(false)
  const timelineSegments = orderedSegments(message.segments)
  const hasTimelineSegments = !isEditing && timelineSegments.length > 0
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
  const generatedFileArtifacts = [...legacyMessageArtifacts, ...legacyToolCalls.flatMap((toolCall) => toolCall.artifacts ?? [])]
    .filter((artifact) => !isImageArtifact(artifact))
  // 后端 recovery.rs 产出的降级描述；旧会话无此字段 → undefined → 不渲染卡片。
  const degraded = message.degraded ?? null
  // content 仍保留同一段文本（旧前端 / 外部 CLI 只读 content），但卡片已经完整表达了
  // 同样的信息 —— 这里不再把它当正文渲染，避免一模一样的内容出现两遍。
  const hasAnswerContent =
    !isDirectImageGenerationPending &&
    message.content.trim().length > 0 &&
    !(degraded && message.content.trim() === degraded.text.trim())
  const hasGeneratedImages = galleryImageArtifacts.length > 0
  const hasGeneratedFiles = generatedFileArtifacts.length > 0
  const [draft, setDraft] = useState(message.content)
  const [saving, setSaving] = useState(false)
  const [copied, setCopied] = useState(false)
  const [toolsExpanded, setToolsExpanded] = useState(false)
  // 工具调用超过 4 个时默认折叠（与思考过程一致）
  const toolsCollapsible = toolCalls.length > 4
  const agentPlan = message.agent_plan ?? message.agentPlan ?? agentPlanOverride
  const isAgentPlanMessage = isExecutableAgentPlanText(agentPlan?.plan)

  useEffect(() => {
    setDraft(message.content)
    setIsEditing(false)
  }, [message.id, message.content])

  const handleCopy = async () => {
    const ok = await copyToClipboard(message.content)
    if (!ok) return
    setCopied(true)
    window.setTimeout(() => setCopied(false), 2000)
  }

  if (isUser) {
    const hasText = message.content.trim().length > 0
    const canEditUser = Boolean(onRegenerateMessage)
    const handleEditAndRegenerate = () => {
      const trimmed = draft.trim()
      if (!trimmed || !onRegenerateMessage) return
      // regenerate 的 Promise 要等整个生成结束才 resolve（agent run 可能数分钟），编辑框不陪跑：
      // 乐观关闭编辑态（handleRegenerateMessage 的乐观截断会同步重渲本气泡）。失败时它内部
      // 会 reload 会话恢复原文，并在线程里展示错误 + 重试。
      // 内容没变 → 纯重新生成；变了 → 编辑并重新生成（后端原子替换+截断）。
      setIsEditing(false)
      void onRegenerateMessage(message.id, trimmed === message.content ? undefined : trimmed)
    }
    // R8（多模型一问多答）：本问发给 ≥2 个模型时，在 user 气泡顶部渲染模型标签行（如 @deepseek @qwen）。
    // 单模型不显示这行（sentModels 缺省或 <2）。
    const replyModelTags = (sentModels ?? []).filter((m) => (m.model ?? '').trim().length > 0)
    const showModelTags = replyModelTags.length >= 2
    return (
      <div className={`group flex justify-end py-2 ${playEntranceAnimation ? 'chat-motion-bubble-in' : ''}`}>
        <div className={`flex min-w-0 flex-col items-end gap-1 ${isEditing ? 'w-full max-w-full' : 'max-w-[85%]'}`}>
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
          {isEditing ? (
            <div className="w-full space-y-2">
              <textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                rows={4}
                autoFocus
                autoCapitalize="off"
                autoCorrect="off"
                spellCheck={false}
                className="w-full resize-y rounded-[20px] border border-neutral-200/90 bg-neutral-50 px-4 py-2.5 text-[15px] leading-relaxed text-neutral-900 outline-none focus:border-neutral-400 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:focus:border-neutral-500"
              />
              <div className="flex items-center justify-end gap-2">
                <Button
                  variant="ghost"
                  onClick={() => {
                    setDraft(message.content)
                    setIsEditing(false)
                  }}
                >
                  取消
                </Button>
                <Button
                  variant="primary"
                  // 编辑中若本会话起了新 run（如输入栏又发了一条），回调被 MessageList 收走
                  // （canEditUser 变 false）→ 禁用保存避免静默 no-op；取消仍可退出。
                  disabled={!draft.trim() || !canEditUser}
                  onClick={handleEditAndRegenerate}
                  title="替换这条提问，并丢弃其后的回复重新生成"
                >
                  保存并重新生成
                </Button>
              </div>
            </div>
          ) : (
            hasText && (
              <div className="chat-user-bubble rounded-[20px] px-4 py-2.5 text-neutral-900 dark:text-neutral-100">
                <div className="whitespace-pre-wrap [overflow-wrap:anywhere] text-[15px] leading-relaxed">
                  {message.content}
                </div>
              </div>
            )
          )}
          {hasText && !isEditing && (
            <div className="flex items-center gap-0.5 pr-0.5 opacity-0 transition-opacity duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-out)] focus-within:opacity-100 group-hover:opacity-100">
              <IconButton
                size="sm"
                onClick={() => void handleCopy()}
                label={copied ? '已复制' : '复制'}
              >
                {copied ? <Check size={14} strokeWidth={2} className="chat-motion-pop" /> : <Copy size={14} strokeWidth={2} />}
              </IconButton>
              {canEditUser && (
                <IconButton
                  size="sm"
                  onClick={() => setIsEditing(true)}
                  label="编辑并重新生成"
                >
                  <Pencil size={14} strokeWidth={2} />
                </IconButton>
              )}
              {onForkMessage && (
                <IconButton
                  size="sm"
                  onClick={() => void onForkMessage(message.id)}
                  label="建分支"
                  title="从这里建分支（复制到新对话）"
                >
                  <GitBranch size={14} strokeWidth={2} />
                </IconButton>
              )}
              {onDeleteMessage && (
                <IconButton
                  size="sm"
                  onClick={() => void onDeleteMessage(message.id)}
                  label="删除"
                >
                  <Trash2 size={14} strokeWidth={2} />
                </IconButton>
              )}
            </div>
          )}
        </div>
      </div>
    )
  }

  const handleSaveEdit = async () => {
    const trimmed = draft.trim()
    if (!trimmed || !onUpdateMessage) return
    setSaving(true)
    try {
      await onUpdateMessage(message.id, trimmed)
      setIsEditing(false)
    } finally {
      setSaving(false)
    }
  }

  // 折叠时仅隐藏较早的，始终保留最新 4 个可见
  const RECENT_TOOL_COUNT = 4
  const olderToolCalls = toolsCollapsible ? toolCalls.slice(0, toolCalls.length - RECENT_TOOL_COUNT) : []
  const recentToolCalls = toolsCollapsible ? toolCalls.slice(toolCalls.length - RECENT_TOOL_COUNT) : toolCalls
  const renderToolCall = (toolCall: ToolCallRecord, index: number) => {
    const key = toolCall.id || toolCall.call_id || toolCall.callId || index
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
    <div className={`flex justify-start py-3 ${playEntranceAnimation ? 'chat-motion-bubble-in' : ''}`}>
      <div className="w-full min-w-0">
        {toolCalls.length > 0 && !isEditing && !hasTimelineSegments && (
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
                  {!toolsExpanded ? ` · 显示最新 ${RECENT_TOOL_COUNT} 个` : ''}
                </span>
                <ChevronDown
                  size={12}
                  strokeWidth={2}
                  className={`ml-auto shrink-0 transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] ${toolsExpanded ? 'rotate-180' : ''}`}
                />
              </button>
            ) : (
              <div className="mb-1 text-[11px] font-medium text-neutral-400 dark:text-neutral-500">
                工具调用
              </div>
            )}
            {toolsCollapsible && toolsExpanded && (
              <div className="chat-motion-reveal is-open">
                <div>{olderToolCalls.map((toolCall, index) => renderToolCall(toolCall, index))}</div>
              </div>
            )}
            {recentToolCalls.map((toolCall, index) =>
              renderToolCall(toolCall, olderToolCalls.length + index),
            )}
          </section>
        )}

        {message.reasoning && !isEditing && !hasTimelineSegments && (
          <ReasoningBlock
            reasoning={message.reasoning}
            streaming={reasoningStreaming}
            durationMs={reasoningDurationMs}
          />
        )}

        {isEditing ? (
          <div className="space-y-2">
            <textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              rows={6}
              disabled={saving}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              className="w-full resize-y rounded-xl border border-neutral-200/90 bg-white px-3 py-2.5 text-[15px] leading-relaxed text-neutral-900 outline-none focus:border-neutral-400 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:focus:border-neutral-500"
            />
            <div className="flex items-center gap-2">
              <Button
                variant="primary"
                disabled={saving || !draft.trim()}
                onClick={() => void handleSaveEdit()}
              >
                {saving ? '保存中…' : '保存'}
              </Button>
              <Button
                variant="ghost"
                disabled={saving}
                onClick={() => {
                  setDraft(message.content)
                  setIsEditing(false)
                }}
              >
                取消
              </Button>
            </div>
          </div>
        ) : isDirectImageGenerationPending ? (
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
        {!isEditing && degraded && <DegradedAnswerCard degraded={degraded} />}

        {!isEditing && isAgentPlanMessage && !isDirectImageGenerationPending && (
          <AgentPlanAction
            messageId={message.id}
            planState={agentPlan}
            disabled={messageStreaming}
            onExecute={onExecuteAgentPlan}
          />
        )}

        {!isEditing && message.content.trim().length > 0 && !isDirectImageGenerationPending && (
          <AssistantMessageMeta
            content={message.content}
            reasoning={message.reasoning}
            timestamp={message.timestamp}
            tokensPerSec={tokensPerSec}
            runEntry={message.run_entry ?? message.runEntry}
            streamOutcome={message.stream_outcome ?? message.streamOutcome}
            usage={message.usage}
            onEdit={canMutate ? () => setIsEditing(true) : undefined}
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
            onDelete={
              canMutate
                ? () => {
                    void onDeleteMessage!(message.id)
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
  )
}

// memo：流式生成时历史消息 props 不变 → 跳过重渲染，避免每个 token 重新解析 Markdown
export const MessageBubble = memo(MessageBubbleComponent)
