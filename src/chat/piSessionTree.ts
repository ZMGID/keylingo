import type { Conversation } from './types'

export type PiSessionEntry = {
  type?: string
  id?: string
  parentId?: string | null
  timestamp?: string
  message?: {
    role?: string
    content?: unknown
    toolName?: string
  }
  summary?: string
  provider?: string
  modelId?: string
  thinkingLevel?: string
  customType?: string
  targetId?: string
  label?: string
  [key: string]: unknown
}

export type PiSessionTreeNode = {
  entry: PiSessionEntry
  children: PiSessionTreeNode[]
  label?: string
  labelTimestamp?: string
}

export type PiSessionTreeSnapshot = {
  tree: PiSessionTreeNode[]
  leafId: string | null
  sessionId: string
  sessionFile: string | null
}

export type PiForkMessage = {
  entryId: string
  text: string
}

export type PiSessionMutationResult = {
  cancelled: boolean
  text: string | null
  sessionId: string
  sessionFile: string | null
  previousSessionId: string
  previousSessionFile: string | null
  conversationId: string | null
  conversation: Conversation | null
}

export type PiSessionSwitchResult = {
  conversationId: string
}
export type FlatPiSessionNode = {
  node: PiSessionTreeNode
  depth: number
  hasChildren: boolean
}

function textFromContent(content: unknown): string {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .map((part) => {
      if (!part || typeof part !== 'object') return ''
      const record = part as Record<string, unknown>
      if (record.type === 'text' && typeof record.text === 'string') return record.text
      if (record.type === 'thinking' && typeof record.thinking === 'string') return record.thinking
      if (record.type === 'toolCall' && typeof record.name === 'string') return record.name
      return ''
    })
    .filter(Boolean)
    .join(' ')
}

export function piSessionEntryRole(entry: PiSessionEntry): string {
  if (entry.type === 'message' && typeof entry.message?.role === 'string') return entry.message.role
  return entry.type || 'entry'
}

export function piSessionEntryText(entry: PiSessionEntry): string {
  if (entry.type === 'message') {
    const content = textFromContent(entry.message?.content).trim()
    if (content) return content
    if (typeof entry.message?.toolName === 'string') return entry.message.toolName
  }
  if (typeof entry.summary === 'string' && entry.summary.trim()) return entry.summary.trim()
  if (entry.type === 'model_change') {
    return [entry.provider, entry.modelId].filter((value): value is string => typeof value === 'string').join('/')
  }
  if (entry.type === 'thinking_level_change' && typeof entry.thinkingLevel === 'string') {
    return entry.thinkingLevel
  }
  if (entry.type === 'custom' || entry.type === 'custom_message') {
    return typeof entry.customType === 'string' ? entry.customType : entry.type
  }
  if (entry.type === 'label' && typeof entry.label === 'string') return entry.label
  return entry.type || entry.id || 'entry'
}

export function mapPiForkEntriesToMessages(
  userMessages: Array<{ id: string; content: string }>,
  forkMessages: readonly PiForkMessage[],
): Map<string, string> {
  const next = new Map<string, string>()
  for (const [messageIndex, message] of userMessages.entries()) {
    const text = message.content.trim()
    if (!text) continue
    const occurrence = userMessages
      .slice(0, messageIndex)
      .filter((entry) => entry.content.trim() === text).length
    const fork = forkMessages.filter((entry) => entry.text.trim() === text)[occurrence]
    if (fork) next.set(message.id, fork.entryId)
  }
  return next
}

export function flattenPiSessionTree(
  tree: PiSessionTreeNode[],
  expanded: ReadonlySet<string>,
): FlatPiSessionNode[] {
  const result: FlatPiSessionNode[] = []
  const visit = (nodes: PiSessionTreeNode[], depth: number) => {
    for (const node of nodes) {
      const id = typeof node.entry.id === 'string' ? node.entry.id : ''
      const children = Array.isArray(node.children) ? node.children : []
      result.push({ node, depth, hasChildren: children.length > 0 })
      if (children.length > 0 && id && expanded.has(id)) visit(children, depth + 1)
    }
  }
  visit(Array.isArray(tree) ? tree : [], 0)
  return result
}

export function piSessionLeafPath(tree: PiSessionTreeNode[], leafId: string | null): string[] {
  if (!leafId) return []
  const visit = (nodes: PiSessionTreeNode[], path: string[]): string[] | null => {
    for (const node of nodes) {
      const id = typeof node.entry.id === 'string' ? node.entry.id : ''
      const next = id ? [...path, id] : path
      if (id === leafId) return next
      const found = visit(Array.isArray(node.children) ? node.children : [], next)
      if (found) return found
    }
    return null
  }
  return visit(Array.isArray(tree) ? tree : [], []) ?? []
}

export function isPiForkableEntry(entry: PiSessionEntry, forkableIds: ReadonlySet<string>): boolean {
  return entry.type === 'message'
    && entry.message?.role === 'user'
    && typeof entry.id === 'string'
    && forkableIds.has(entry.id)
}
