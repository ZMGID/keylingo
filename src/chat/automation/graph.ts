import { AUTOMATION_SCHEMA_VERSION, isIfType, isStepType, isTriggerType, type Automation, type AutomationNodeType, type FlowEdge, type FlowNode, type FlowNodeData } from './types'

export const FLOW_NODE_WIDTH = 220
export const FLOW_NODE_GAP_X = 72
export const FLOW_NODE_GAP_Y = 96
export const FLOW_ORIGIN = { x: 72, y: 144 }

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
    x: from.position.x + FLOW_NODE_WIDTH + FLOW_NODE_GAP_X,
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
    if (!node) return null
    if (isIfType(node.type)) {
      const used = edges
        .filter((edge) => edge.source === id)
        .map((edge) => edge.sourceHandle || 'true')
      if (!used.includes('true')) return { nodeId: id, handle: 'true' }
      if (!used.includes('false')) return { nodeId: id, handle: 'false' }
      return null
    }
    if (edges.some((edge) => edge.source === id)) return null
    return { nodeId: id }
  }
  if (preferredId) {
    const hit = tryNode(preferredId)
    if (hit) return hit
  }
  for (let i = nodes.length - 1; i >= 0; i--) {
    const hit = tryNode(nodes[i].id)
    if (hit) return hit
  }
  return null
}

export function layoutFlow<
  N extends { id: string, type?: string, position: { x: number, y: number } },
>(
  nodes: N[],
  edges: { source: string, target: string, sourceHandle?: string | null }[],
): N[] {
  if (nodes.length === 0) return nodes
  const byId = new Map(nodes.map((node) => [node.id, node]))
  const outgoing = new Map<string, typeof edges>()
  for (const edge of edges) {
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
    const nextX = x + FLOW_NODE_WIDTH + FLOW_NODE_GAP_X
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
): boolean {
  if (source === target) return false
  const from = nodes.find((node) => node.id === source)
  const to = nodes.find((node) => node.id === target)
  if (!from || !to) return false
  if (isTriggerType(to.type)) return false
  if (!isStepType(to.type)) return false
  if (edges.some((edge) => edge.target === target)) return false
  const same = edges.some((edge) =>
    edge.source === source
    && edge.target === target
    && (edge.sourceHandle || '') === (sourceHandle || ''),
  )
  if (same) return false
  if (isIfType(from.type)) {
    const handle = sourceHandle === 'false' ? 'false' : 'true'
    if (edges.some((edge) => edge.source === source && (edge.sourceHandle || 'true') === handle)) {
      return false
    }
  }
  return true
}

export function connectNodes(
  source: string,
  target: string,
  sourceHandle?: string | null,
): FlowEdge {
  return {
    id: sourceHandle ? `e-${source}-${target}-${sourceHandle}` : `e-${source}-${target}`,
    source,
    target,
    sourceHandle: sourceHandle ?? undefined,
  }
}

export function triggerNode(automation: Automation): FlowNode | undefined {
  return automation.nodes.find((node) => isTriggerType(node.type))
}

export function topologicalOrder(automation: Automation): string[] {
  const incoming = new Map<string, number>()
  for (const node of automation.nodes) incoming.set(node.id, 0)
  for (const edge of automation.edges) {
    incoming.set(edge.target, (incoming.get(edge.target) ?? 0) + 1)
  }
  const queue = [...incoming.entries()].filter(([, count]) => count === 0).map(([id]) => id)
  const order: string[] = []
  const outgoing = new Map<string, string[]>()
  for (const edge of automation.edges) {
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
