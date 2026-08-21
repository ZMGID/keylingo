import type { CompactionBoundaryView } from './compactionBoundary'
import type { ContextClearBoundaryView } from './contextClearBoundary'
import { compactionRecordTokens } from './compactionBoundary'
import type { MessageListItem } from './messageGroups'
import type { ChatMessage } from './types'

export type MessageNavigatorNode =
  | {
      kind: 'turn'
      id: string
      targetRenderIndex: number
      userMessageId: string
      title: string
      answerPreview: string
      modelLabel: string
    }
  | {
      kind: 'compaction'
      id: string
      targetRenderIndex: number
      title: string
      answerPreview: string
      modelLabel: ''
    }
  | {
      kind: 'clear'
      id: string
      targetRenderIndex: number
      title: string
      answerPreview: string
      modelLabel: ''
    }

const PREVIEW_CHAR_LIMIT = 280

export function normalizeMessageNavigatorPreview(content: string): string {
  const normalized = content.replace(/\s+/g, ' ').trim()
  if (normalized.length <= PREVIEW_CHAR_LIMIT) return normalized
  return `${normalized.slice(0, PREVIEW_CHAR_LIMIT).trimEnd()}…`
}

function messageModel(message: ChatMessage): string {
  return message.model?.trim() ?? ''
}

function assistantMessages(item: MessageListItem): ChatMessage[] {
  if (item.type === 'group') return item.messages
  return item.message.role === 'assistant' ? [item.message] : []
}

function modelLabel(messages: ChatMessage[]): string {
  const models = [...new Set(messages.map(messageModel).filter(Boolean))]
  if (models.length > 1) return `${models.length} 个模型`
  return models[0] ?? ''
}

interface TurnDraft {
  user: ChatMessage
  targetRenderIndex: number
  assistants: ChatMessage[]
}

function finishTurn(draft: TurnDraft | null): MessageNavigatorNode | null {
  if (!draft) return null
  const answer = draft.assistants.find((message) => message.content.trim())
  return {
    kind: 'turn',
    id: `turn-${draft.user.id}`,
    targetRenderIndex: draft.targetRenderIndex,
    userMessageId: draft.user.id,
    title: normalizeMessageNavigatorPreview(draft.user.content),
    answerPreview: normalizeMessageNavigatorPreview(answer?.content ?? ''),
    modelLabel: modelLabel(draft.assistants),
  }
}

export interface BuildMessageNavigatorNodesOptions {
  folded: MessageListItem[]
  boundaries: CompactionBoundaryView[]
  clearBoundaries?: ContextClearBoundaryView[]
  renderIndexByKey: ReadonlyMap<string, number>
}

export function buildMessageNavigatorNodes({
  folded,
  boundaries,
  clearBoundaries = [],
  renderIndexByKey,
}: BuildMessageNavigatorNodesOptions): MessageNavigatorNode[] {
  const nodes: MessageNavigatorNode[] = []
  let turn: TurnDraft | null = null

  for (const item of folded) {
    if (item.type === 'message' && item.message.role === 'user') {
      const completed = finishTurn(turn)
      if (completed) nodes.push(completed)
      const targetRenderIndex = renderIndexByKey.get(item.message.id)
      turn = targetRenderIndex == null
        ? null
        : { user: item.message, targetRenderIndex, assistants: [] }
      continue
    }
    if (turn) turn.assistants.push(...assistantMessages(item))
  }

  const completed = finishTurn(turn)
  if (completed) nodes.push(completed)

  for (const boundary of boundaries) {
    const targetRenderIndex = renderIndexByKey.get(`compaction-summary-${boundary.record.id}`)
    if (targetRenderIndex == null) continue
    nodes.push({
      kind: 'compaction',
      id: `compaction-${boundary.record.id}`,
      targetRenderIndex,
      title: '已压缩此前上下文',
      answerPreview: normalizeMessageNavigatorPreview(compactionRecordTokens(boundary.record).summary),
      modelLabel: '',
    })
  }

  for (const boundary of clearBoundaries) {
    const targetRenderIndex = renderIndexByKey.get(`context-clear-divider-${boundary.record.id}`)
    if (targetRenderIndex == null) continue
    nodes.push({
      kind: 'clear',
      id: `clear-${boundary.record.id}`,
      targetRenderIndex,
      title: '清空上下文',
      answerPreview: '',
      modelLabel: '',
    })
  }

  return nodes.sort((a, b) => a.targetRenderIndex - b.targetRenderIndex)
}

export function activeMessageNavigatorNodeId(
  nodes: readonly MessageNavigatorNode[],
  renderIndex: number,
): string | null {
  let active: MessageNavigatorNode | null = null
  for (const node of nodes) {
    if (node.targetRenderIndex > renderIndex) break
    active = node
  }
  return active?.id ?? nodes[0]?.id ?? null
}

export function visibleMessageNavigatorNodeIds(
  nodes: readonly MessageNavigatorNode[],
  firstRenderIndex: number,
  lastRenderIndex: number,
): string[] {
  if (nodes.length === 0 || firstRenderIndex > lastRenderIndex) return []
  const visible: string[] = []
  for (let index = 0; index < nodes.length; index++) {
    const node = nodes[index]
    const nextStart = nodes[index + 1]?.targetRenderIndex ?? Number.POSITIVE_INFINITY
    const nodeEnd = nextStart - 1
    if (node.targetRenderIndex <= lastRenderIndex && nodeEnd >= firstRenderIndex) {
      visible.push(node.id)
    }
  }
  return visible
}

export function messageNavigatorProximityWidth(distance: number): number {
  const influence = Math.max(0, 1 - Math.abs(distance) / 52)
  return 8 + 14 * influence * influence
}
