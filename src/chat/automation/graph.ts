import { AUTOMATION_SCHEMA_VERSION, isActionType, isTriggerType, type Automation, type AutomationNodeType, type FlowEdge, type FlowNode, type FlowNodeData } from './types'

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

export function nextNodePosition(nodes: FlowNode[]): { x: number; y: number } {
  if (nodes.length === 0) return { x: 80, y: 160 }
  const last = nodes[nodes.length - 1]
  return { x: last.position.x + 220, y: last.position.y }
}

export function canConnect(
  source: string,
  target: string,
  nodes: FlowNode[],
  edges: FlowEdge[],
): boolean {
  if (source === target) return false
  const from = nodes.find((node) => node.id === source)
  const to = nodes.find((node) => node.id === target)
  if (!from || !to) return false
  if (isTriggerType(to.type)) return false
  if (!isActionType(to.type)) return false
  if (edges.some((edge) => edge.target === target)) return false
  if (edges.some((edge) => edge.source === source)) return false
  return true
}

export function connectNodes(source: string, target: string): FlowEdge {
  return {
    id: `e-${source}-${target}`,
    source,
    target,
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
