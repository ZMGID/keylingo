import { AUTOMATION_SCHEMA_VERSION, isAttachmentType, isIfType, isStepType, isTriggerType, type Automation, type AutomationNodeType, type FlowEdge, type FlowNode, type FlowNodeData } from './types'
import { AGENT_SLOTS, FLOW_AGENT_WIDTH, isSlotEdge, resolveSlotConnection, slotAllowsMany } from './agentModel'

/* n8n 式节点：卡片本体 104×80，文字标签悬挂在卡片下方（不占节点边界）。
   GAP_Y 要给下方标签留出高度，否则 IF 分叉的两支会叠字。 */
export const FLOW_NODE_WIDTH = 104
export { FLOW_AGENT_WIDTH }
export const FLOW_NODE_GAP_X = 88
export const FLOW_NODE_GAP_Y = 136
export const FLOW_ORIGIN = { x: 72, y: 144 }

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
  const yOff = sourceHandle === 'false' ? FLOW_NODE_GAP_Y : sourceHandle === 'true' ? -FLOW_NODE_GAP_Y : 0
  return {
    x: from.position.x + nodeCardWidth(from.type) + FLOW_NODE_GAP_X,
    y: from.position.y + yOff,
  }
}

export function pickAppendSource(
  nodes: FlowNode[],
  edges: FlowEdge[],
  preferredId?: string | null,
): { nodeId: string, handle?: string } | null {
  const tryNode = (id: string): { nodeId: string, handle?: string } | null => {
    const node = nodes.find((item) => item.id === id)
    if (!node || isAttachmentType(node.type)) return null
    if (isIfType(node.type)) {
      const used = edges
        .filter((edge) => edge.source === id && !isSlotEdge(edge))
        .map((edge) => edge.sourceHandle || 'true')
      if (!used.includes('true')) return { nodeId: id, handle: 'true' }
      if (!used.includes('false')) return { nodeId: id, handle: 'false' }
      return null
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
  N extends { id: string, type?: string, position: { x: number, y: number } },
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
    if (isIfType(node.type ?? '')) {
      const yes = outs.find((edge) => (edge.sourceHandle || 'true') === 'true')
      const no = outs.find((edge) => edge.sourceHandle === 'false')
      if (yes) walk(yes.target, nextX, y - FLOW_NODE_GAP_Y)
      if (no) walk(no.target, nextX, y + FLOW_NODE_GAP_Y)
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
  return nodes.map((node) => ({ ...node, position: placed.get(node.id)! }))
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
  if (edges.some((edge) => edge.target === target && !isSlotEdge(edge))) return false
  const same = edges.some((edge) =>
    edge.source === source
    && edge.target === target
    && (edge.sourceHandle || '') === (sourceHandle || '')
    && !isSlotEdge(edge),
  )
  if (same) return false
  if (isIfType(from.type)) {
    const handle = sourceHandle === 'false' ? 'false' : 'true'
    if (edges.some((edge) =>
      edge.source === source && !isSlotEdge(edge) && (edge.sourceHandle || 'true') === handle
    )) {
      return false
    }
  }
  return true
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
