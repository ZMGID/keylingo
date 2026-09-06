import { AUTOMATION_SCHEMA_VERSION, branchHandles, isAttachmentType, isStepType, isTriggerType, type Automation, type AutomationNodeType, type FlowEdge, type FlowNode, type FlowNodeData } from './types'
import { AGENT_SLOTS, FLOW_AGENT_WIDTH, connectSlotEdge, isSlotEdge, resolveSlotConnection, slotAllowsMany } from './agentModel'

/* n8n 式节点：卡片本体 104×80，文字标签悬挂在卡片下方（不占节点边界）。
   GAP_Y 要给下方标签留出高度，否则 IF 分叉的两支会叠字。 */
export const FLOW_NODE_WIDTH = 104
export { FLOW_AGENT_WIDTH }
export const FLOW_NODE_GAP_X = 112
export const FLOW_NODE_GAP_Y = 216
export const FLOW_ORIGIN = { x: 72, y: 144 }
export const FLOW_NODE_MIN_GAP = 24

type PositionedNode = { id: string, type?: string, position: { x: number, y: number } }

/** Include the caption, hover toolbar, output + and Agent slot labels, not just the card. */
export function nodeVisibleBounds(node: PositionedNode) {
  const width = nodeCardWidth(node.type)
  return {
    left: node.position.x + Math.min(0, (width - 180) / 2),
    right: node.position.x + Math.max(width + 34, (width + 180) / 2),
    top: node.position.y - 36,
    bottom: node.position.y + (node.type === 'action.agent' ? 172 : 128),
  }
}

function boundsTooClose(a: ReturnType<typeof nodeVisibleBounds>, b: ReturnType<typeof nodeVisibleBounds>) {
  return a.left < b.right + FLOW_NODE_MIN_GAP && a.right + FLOW_NODE_MIN_GAP > b.left
    && a.top < b.bottom + FLOW_NODE_MIN_GAP && a.bottom + FLOW_NODE_MIN_GAP > b.top
}

/** Keep untouched nodes fixed; move conflicting additions/drags to the nearest free grid position. */
export function ensureNodeSpacing<N extends PositionedNode>(nodes: N[], movingIds?: ReadonlySet<string>): N[] {
  const ordered = movingIds
    ? [...nodes.filter((node) => !movingIds.has(node.id)), ...nodes.filter((node) => movingIds.has(node.id))]
    : nodes
  const placed: ReturnType<typeof nodeVisibleBounds>[] = []
  const adjusted = new Map<string, N>()
  for (const node of ordered) {
    const bounds = nodeVisibleBounds(node)
    if (!placed.some((other) => boundsTooClose(bounds, other))) {
      placed.push(bounds)
      continue
    }
    const { x, y } = node.position
    const candidates = placed.flatMap((other) => [
      { x: Math.ceil((other.right + FLOW_NODE_MIN_GAP - (bounds.left - x)) / 24) * 24, y },
      { x, y: Math.ceil((other.bottom + FLOW_NODE_MIN_GAP - (bounds.top - y)) / 24) * 24 },
      { x: Math.floor((other.left - FLOW_NODE_MIN_GAP - (bounds.right - x)) / 24) * 24, y },
      { x, y: Math.floor((other.top - FLOW_NODE_MIN_GAP - (bounds.bottom - y)) / 24) * 24 },
    ])
    candidates.sort((a, b) => (a.x - x) ** 2 + (a.y - y) ** 2 - ((b.x - x) ** 2 + (b.y - y) ** 2))
    // At least the candidate outside the outermost obstacle is always free.
    const position = candidates.find((candidate) =>
      !placed.some((other) => boundsTooClose(nodeVisibleBounds({ ...node, position: candidate }), other)),
    )!
    const next = { ...node, position }
    adjusted.set(node.id, next)
    placed.push(nodeVisibleBounds(next))
  }
  return adjusted.size ? nodes.map((node) => adjusted.get(node.id) ?? node) : nodes
}

function branchYOffset(handles: string[], handle: string): number {
  const idx = handles.indexOf(handle)
  if (idx < 0) return 0
  if (handles.length === 2 && handles[0] === 'true') {
    return idx === 0 ? -FLOW_NODE_GAP_Y : FLOW_NODE_GAP_Y
  }
  return (idx - (handles.length - 1) / 2) * FLOW_NODE_GAP_Y
}

export function nodeCardWidth(type?: string): number {
  if (type === 'action.agent') return FLOW_AGENT_WIDTH
  if (type && isAttachmentType(type)) return 72
  return FLOW_NODE_WIDTH
}

export function createBlankAutomation(): Automation {
  const now = new Date().toISOString()
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    id: crypto.randomUUID(),
    name: '',
    enabled: false,
    nodes: [],
    edges: [],
    viewport: { x: 0, y: 0, zoom: 1 },
    createdAt: now,
    updatedAt: now,
  }
}

export function createFlowNode(
  type: AutomationNodeType,
  data: FlowNodeData,
  position: { x: number; y: number },
): FlowNode {
  return {
    id: crypto.randomUUID(),
    type,
    position,
    data,
  }
}

export function nextNodePosition(
  nodes: FlowNode[],
  fromId?: string,
  sourceHandle?: string | null,
): { x: number; y: number } {
  if (nodes.length === 0) return { ...FLOW_ORIGIN }
  const from = (fromId && nodes.find((node) => node.id === fromId))
    || nodes.reduce((best, node) => (node.position.x >= best.position.x ? node : best))
  const handles = branchHandles(from.type, from.data)
  const yOff = handles && sourceHandle ? branchYOffset(handles, sourceHandle) : 0
  return {
    x: from.position.x + nodeCardWidth(from.type) + FLOW_NODE_GAP_X,
    y: from.position.y + yOff,
  }
}

export function nextTriggerPosition(nodes: FlowNode[]): { x: number; y: number } {
  const triggers = nodes.filter((node) => isTriggerType(node.type))
  if (triggers.length === 0) return { ...FLOW_ORIGIN }
  const lowest = triggers.reduce((best, node) => (
    node.position.y >= best.position.y ? node : best
  ))
  return { x: FLOW_ORIGIN.x, y: lowest.position.y + FLOW_NODE_GAP_Y }
}

export function pickAppendSource(
  nodes: FlowNode[],
  edges: FlowEdge[],
  preferredId?: string | null,
): { nodeId: string, handle?: string } | null {
  const tryNode = (id: string): { nodeId: string, handle?: string } | null => {
    const node = nodes.find((item) => item.id === id)
    if (!node || isAttachmentType(node.type)) return null
    const handles = branchHandles(node.type, node.data)
    if (handles) {
      const used = edges
        .filter((edge) => edge.source === id && !isSlotEdge(edge))
        .map((edge) => edge.sourceHandle || handles[0])
      const free = handles.find((handle) => !used.includes(handle))
      return free ? { nodeId: id, handle: free } : null
    }
    if (edges.some((edge) => edge.source === id && !isSlotEdge(edge))) return null
    return { nodeId: id }
  }
  if (preferredId) {
    const hit = tryNode(preferredId)
    if (hit) return hit
  }
  for (let i = nodes.length - 1; i >= 0; i--) {
    if (isAttachmentType(nodes[i].type)) continue
    const hit = tryNode(nodes[i].id)
    if (hit) return hit
  }
  return null
}

export function layoutFlow<
  N extends { id: string, type?: string, position: { x: number, y: number }, data?: FlowNodeData },
>(
  nodes: N[],
  edges: { source: string, target: string, sourceHandle?: string | null, targetHandle?: string | null }[],
): N[] {
  if (nodes.length === 0) return nodes
  const byId = new Map(nodes.map((node) => [node.id, node]))
  const outgoing = new Map<string, typeof edges>()
  for (const edge of edges) {
    if (isSlotEdge(edge)) continue
    const list = outgoing.get(edge.source) ?? []
    list.push(edge)
    outgoing.set(edge.source, list)
  }
  const placed = new Map<string, { x: number, y: number }>()
  const start = nodes.find((node) => isTriggerType(node.type ?? '')) ?? nodes[0]
  const walk = (id: string, x: number, y: number) => {
    if (placed.has(id)) return
    placed.set(id, { x, y })
    const node = byId.get(id)
    if (!node) return
    const outs = outgoing.get(id) ?? []
    const nextX = x + nodeCardWidth(node.type) + FLOW_NODE_GAP_X
    const handles = branchHandles(node.type ?? '', node.data)
    if (handles) {
      handles.forEach((handle) => {
        const edge = outs.find((item) => (item.sourceHandle || handles[0]) === handle)
        if (!edge) return
        walk(edge.target, nextX, y + branchYOffset(handles, handle))
      })
      return
    }
    for (const edge of outs) walk(edge.target, nextX, y)
  }
  walk(start.id, FLOW_ORIGIN.x, FLOW_ORIGIN.y)
  let extraY = FLOW_ORIGIN.y + FLOW_NODE_GAP_Y * 2
  for (const node of nodes) {
    if (placed.has(node.id)) continue
    placed.set(node.id, { x: FLOW_ORIGIN.x, y: extraY })
    extraY += FLOW_NODE_GAP_Y
  }
  return ensureNodeSpacing(nodes.map((node) => ({ ...node, position: placed.get(node.id)! })))
}

export function canConnect(
  source: string,
  target: string,
  nodes: FlowNode[],
  edges: FlowEdge[],
  sourceHandle?: string | null,
  targetHandle?: string | null,
): boolean {
  if (source === target) return false
  const from = nodes.find((node) => node.id === source)
  const to = nodes.find((node) => node.id === target)
  if (!from || !to) return false

  const slot = resolveSlotConnection(from.type, targetHandle)
  if (slot || AGENT_SLOTS.includes(targetHandle as typeof AGENT_SLOTS[number])) {
    if (to.type !== 'action.agent' || !slot) return false
    if (edges.some((edge) =>
      edge.source === source && edge.target === target && edge.targetHandle === slot
    )) return false
    if (!slotAllowsMany(slot)
      && edges.some((edge) => edge.target === target && edge.targetHandle === slot)) {
      return false
    }
    return true
  }

  if (isAttachmentType(from.type) || isAttachmentType(to.type)) return false
  if (isTriggerType(to.type)) return false
  if (!isStepType(to.type)) return false
  const incoming = edges.filter((edge) => edge.target === target && !isSlotEdge(edge))
  if (incoming.length > 0) {
    const joinTriggers = isTriggerType(from.type) && incoming.every((edge) => {
      const src = nodes.find((node) => node.id === edge.source)
      return src ? isTriggerType(src.type) : false
    })
    if (!joinTriggers) return false
  }
  const same = edges.some((edge) =>
    edge.source === source
    && edge.target === target
    && (edge.sourceHandle || '') === (sourceHandle || '')
    && !isSlotEdge(edge),
  )
  if (same) return false
  const handles = branchHandles(from.type, from.data)
  if (handles) {
    const handle = sourceHandle && handles.includes(sourceHandle) ? sourceHandle : handles[0]
    if (edges.some((edge) =>
      edge.source === source && !isSlotEdge(edge) && (edge.sourceHandle || handles[0]) === handle
    )) {
      return false
    }
  }
  if (wouldCreateCycle(source, target, edges)) return false
  return true
}

/** True if adding source→target on the main flow would close a cycle. */
export function wouldCreateCycle(source: string, target: string, edges: FlowEdge[]): boolean {
  const outgoing = new Map<string, string[]>()
  for (const edge of edges) {
    if (isSlotEdge(edge)) continue
    const list = outgoing.get(edge.source) ?? []
    list.push(edge.target)
    outgoing.set(edge.source, list)
  }
  const stack = [target]
  const seen = new Set<string>()
  while (stack.length > 0) {
    const id = stack.pop()!
    if (id === source) return true
    if (seen.has(id)) continue
    seen.add(id)
    for (const next of outgoing.get(id) ?? []) stack.push(next)
  }
  return false
}

export function pruneDanglingBranchEdges<
  N extends { id: string, type?: string, data?: FlowNodeData },
  E extends { source: string, sourceHandle?: string | null, targetHandle?: string | null },
>(nodes: N[], edges: E[]): E[] {
  const byId = new Map(nodes.map((node) => [node.id, node]))
  return edges.filter((edge) => {
    if (isSlotEdge(edge)) return true
    const from = byId.get(edge.source)
    if (!from) return true
    const handles = branchHandles(from.type ?? '', from.data)
    if (!handles) return true
    const handle = edge.sourceHandle || handles[0]
    return handles.includes(handle)
  })
}

export function connectNodes(
  source: string,
  target: string,
  sourceHandle?: string | null,
  targetHandle?: string | null,
): FlowEdge {
  const suffix = [sourceHandle, targetHandle].filter(Boolean).join('-')
  return {
    id: suffix ? `e-${source}-${target}-${suffix}` : `e-${source}-${target}`,
    source,
    target,
    sourceHandle: sourceHandle ?? undefined,
    targetHandle: targetHandle ?? undefined,
  }
}

/** Satellite → Agent always lands on the matching slot, even if the drop missed the diamond. */
export function flowEdgeFromConnection(
  fromType: string,
  source: string,
  target: string,
  sourceHandle?: string | null,
  targetHandle?: string | null,
): FlowEdge {
  const slot = resolveSlotConnection(fromType, targetHandle)
  if (slot) return connectSlotEdge(source, target, slot)
  return connectNodes(source, target, sourceHandle, targetHandle)
}

export function triggerNode(automation: Automation): FlowNode | undefined {
  return automation.nodes.find((node) => isTriggerType(node.type))
}

export function topologicalOrder(automation: Automation): string[] {
  const incoming = new Map<string, number>()
  for (const node of automation.nodes) {
    if (isAttachmentType(node.type)) continue
    incoming.set(node.id, 0)
  }
  for (const edge of automation.edges) {
    if (isSlotEdge(edge)) continue
    incoming.set(edge.target, (incoming.get(edge.target) ?? 0) + 1)
  }
  const queue = [...incoming.entries()].filter(([, count]) => count === 0).map(([id]) => id)
  const order: string[] = []
  const outgoing = new Map<string, string[]>()
  for (const edge of automation.edges) {
    if (isSlotEdge(edge)) continue
    const list = outgoing.get(edge.source) ?? []
    list.push(edge.target)
    outgoing.set(edge.source, list)
  }
  while (queue.length > 0) {
    const id = queue.shift()!
    order.push(id)
    for (const next of outgoing.get(id) ?? []) {
      const count = (incoming.get(next) ?? 1) - 1
      incoming.set(next, count)
      if (count === 0) queue.push(next)
    }
  }
  return order
}
