import type { I18n } from '../../settings/i18n'
import type {
  AgentData,
  AgentRuntimeKind,
  AgentSlot,
  AutomationNodeType,
  FlowEdge,
  FlowNode,
  FlowNodeData,
} from './types'

export const AGENT_SLOTS: readonly AgentSlot[] = ['runtime', 'context', 'tool', 'skill']

export interface NormalizedAgent {
  prompt: string
  runtimeKind: AgentRuntimeKind
  externalAgentId: string | null
  externalModel: string | null
  providerId: string | null
  model: string | null
  toolIds: string[]
  skillIds: string[]
}

const RUNTIME_KINDS: AgentRuntimeKind[] = ['builtin', 'chat', 'external']

function text(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function idList(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  const seen = new Set<string>()
  const ids: string[] = []
  for (const item of value) {
    if (typeof item !== 'string') continue
    const id = item.trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    ids.push(id)
  }
  return ids
}

export function normalizeAgent(data?: AgentData | null): NormalizedAgent {
  const runtimeKind = RUNTIME_KINDS.includes(data?.runtimeKind as AgentRuntimeKind)
    ? data!.runtimeKind!
    : 'builtin'
  const legacySkill = data?.skillId?.trim() || null
  const skillIds = idList(data?.skillIds)
  if (legacySkill && !skillIds.includes(legacySkill)) skillIds.unshift(legacySkill)
  return {
    prompt: text(data?.prompt),
    runtimeKind,
    externalAgentId: data?.externalAgentId?.trim() || null,
    externalModel: data?.externalModel?.trim() || null,
    providerId: data?.providerId?.trim() || null,
    model: data?.model?.trim() || null,
    toolIds: idList(data?.toolIds),
    skillIds,
  }
}

export function toAgentData(agent: NormalizedAgent): AgentData {
  const external = agent.runtimeKind === 'external'
  return {
    prompt: agent.prompt,
    skillId: agent.skillIds[0] ?? null,
    runtimeKind: agent.runtimeKind,
    externalAgentId: external ? agent.externalAgentId : null,
    externalModel: external ? agent.externalModel : null,
    providerId: external ? null : agent.providerId,
    model: external ? null : agent.model,
    toolIds: agent.toolIds,
    skillIds: agent.skillIds,
  }
}

/** Model id that belongs to the current runtime. The other family is ignored so a leftover Kivio model cannot show up on a CLI node (or vice versa). */
export function agentSelectedModel(agent: NormalizedAgent): string {
  if (agent.runtimeKind === 'external') return agent.externalModel?.trim() || ''
  return agent.model?.trim() || ''
}

export function withRuntimeKind(
  agent: NormalizedAgent,
  runtimeKind: AgentRuntimeKind,
  extra: Partial<NormalizedAgent> = {},
): NormalizedAgent {
  if (runtimeKind === 'external') {
    return {
      ...agent,
      ...extra,
      runtimeKind,
      providerId: null,
      model: null,
    }
  }
  return {
    ...agent,
    ...extra,
    runtimeKind,
    externalAgentId: null,
    externalModel: null,
  }
}

export function isAgentSlotFilled(slot: AgentSlot, agent: NormalizedAgent): boolean {
  switch (slot) {
    case 'runtime':
      return agent.runtimeKind !== 'external' || Boolean(agent.externalAgentId)
    case 'context':
      return agent.prompt.trim().length > 0
    case 'tool':
      return agent.toolIds.length > 0
    case 'skill':
      return agent.skillIds.length > 0
  }
}

export function isAgentSlotRequired(slot: AgentSlot): boolean {
  return slot === 'runtime' || slot === 'context'
}

export function agentSlotLabel(slot: AgentSlot, t: I18n): string {
  switch (slot) {
    case 'runtime':
      return t.chatAutomationAgentSlotRuntime
    case 'context':
      return t.chatAutomationAgentSlotContext
    case 'tool':
      return t.chatAutomationAgentSlotTool
    case 'skill':
      return t.chatAutomationAgentSlotSkill
  }
}

export function agentRuntimeSummary(agent: NormalizedAgent, t: I18n): string {
  if (agent.runtimeKind === 'chat') return t.chatAutomationKivioChat
  if (agent.runtimeKind === 'external') {
    return agent.externalAgentId || t.chatAutomationExternalCli
  }
  return t.chatAutomationKivioAgent
}

export function nodeTypeForSlot(slot: AgentSlot): AutomationNodeType {
  switch (slot) {
    case 'runtime':
      return 'agent.runtime'
    case 'context':
      return 'agent.context'
    case 'tool':
      return 'agent.tool'
    case 'skill':
      return 'agent.skill'
  }
}

export function slotForNodeType(type: string): AgentSlot | null {
  switch (type) {
    case 'agent.runtime':
      return 'runtime'
    case 'agent.context':
      return 'context'
    case 'agent.tool':
      return 'tool'
    case 'agent.skill':
      return 'skill'
    default:
      return null
  }
}

export function isSlotEdge(edge: { targetHandle?: string | null }): boolean {
  return AGENT_SLOTS.includes(edge.targetHandle as AgentSlot)
}

export function slotAllowsMany(slot: AgentSlot): boolean {
  return slot === 'tool' || slot === 'skill'
}

export function resolveSlotConnection(fromType: string, targetHandle?: string | null): AgentSlot | null {
  const slot = slotForNodeType(fromType)
  if (!slot) return null
  if (targetHandle && AGENT_SLOTS.includes(targetHandle as AgentSlot) && targetHandle !== slot) {
    return null
  }
  return slot
}

export const FLOW_AGENT_WIDTH = 280

export function slotAttachPosition(
  agent: { x: number, y: number },
  slot: AgentSlot,
  index = 0,
): { x: number, y: number } {
  const col = Math.max(0, AGENT_SLOTS.indexOf(slot))
  return {
    x: agent.x + FLOW_AGENT_WIDTH / 2 + (col - 1.5) * 216 - 36,
    y: agent.y + 240 + index * 216,
  }
}

export function connectSlotEdge(source: string, target: string, slot: AgentSlot): FlowEdge {
  return {
    id: `e-${source}-${target}-${slot}`,
    source,
    target,
    sourceHandle: 'slot',
    targetHandle: slot,
  }
}

function mergeIds(into: string[], extra: string[]) {
  for (const id of extra) {
    if (!into.includes(id)) into.push(id)
  }
}

/** Merge inline AgentData with whatever is plugged into the four bottom slots. */
export function composeAgent(
  agentId: string,
  nodes: Array<{ id: string, type?: string, data: FlowNodeData }>,
  edges: Array<{ source: string, target: string, targetHandle?: string | null }>,
): NormalizedAgent {
  const agentNode = nodes.find((node) => node.id === agentId)
  const base = normalizeAgent(agentNode?.data.agent)
  let runtime = base
  let prompt = base.prompt
  let sawTools = false
  let sawSkills = false
  const toolIds: string[] = []
  const skillIds: string[] = []
  const byId = new Map(nodes.map((node) => [node.id, node]))
  for (const edge of edges) {
    if (edge.target !== agentId || !isSlotEdge(edge)) continue
    const src = byId.get(edge.source)
    if (!src) continue
    const part = normalizeAgent(src.data.disabled ? undefined : src.data.agent)
    switch (edge.targetHandle) {
      case 'runtime':
        runtime = part
        break
      case 'context':
        prompt = part.prompt
        break
      case 'tool':
        sawTools = true
        mergeIds(toolIds, part.toolIds)
        break
      case 'skill':
        sawSkills = true
        mergeIds(skillIds, part.skillIds)
        break
      default:
        break
    }
  }
  return withRuntimeKind({
    prompt,
    runtimeKind: runtime.runtimeKind,
    externalAgentId: runtime.externalAgentId,
    externalModel: runtime.externalModel,
    providerId: runtime.providerId,
    model: runtime.model,
    toolIds: sawTools ? toolIds : base.toolIds,
    skillIds: sawSkills ? skillIds : base.skillIds,
  }, runtime.runtimeKind)
}

function hasInlineAgentConfig(agent: NormalizedAgent): boolean {
  return Boolean(
    agent.prompt.trim()
    || agent.toolIds.length
    || agent.skillIds.length
    || agent.runtimeKind !== 'builtin'
    || agent.externalAgentId
    || agent.providerId
    || agent.model,
  )
}

/** One-time split: old Agent nodes stored everything inline. Plug those fields into slot nodes. */
export function explodeInlineAgents(
  nodes: FlowNode[],
  edges: FlowEdge[],
): { nodes: FlowNode[], edges: FlowEdge[], changed: boolean } {
  let nextNodes = nodes
  let nextEdges = edges
  let changed = false
  for (const node of nodes) {
    if (node.type !== 'action.agent') continue
    if (nextEdges.some((edge) => edge.target === node.id && isSlotEdge(edge))) continue
    const agent = normalizeAgent(node.data.agent)
    if (!hasInlineAgentConfig(agent)) continue
    changed = true
    const spawned: FlowNode[] = []
    const spawnedEdges: FlowEdge[] = []
    const spawn = (slot: AgentSlot, data: AgentData, label: string) => {
      const child: FlowNode = {
        id: crypto.randomUUID(),
        type: nodeTypeForSlot(slot),
        position: slotAttachPosition(node.position, slot),
        data: { label, agent: data },
      }
      spawned.push(child)
      spawnedEdges.push(connectSlotEdge(child.id, node.id, slot))
    }
    const runtimeLabel = agent.runtimeKind === 'chat'
      ? 'Kivio Chat'
      : agent.runtimeKind === 'external'
        ? (agent.externalAgentId || 'CLI')
        : 'Kivio Agent'
    spawn('runtime', toAgentData({
      prompt: '',
      runtimeKind: agent.runtimeKind,
      externalAgentId: agent.externalAgentId,
      externalModel: agent.externalModel,
      providerId: agent.providerId,
      model: agent.model,
      toolIds: [],
      skillIds: [],
    }), runtimeLabel)
    if (agent.prompt.trim()) {
      spawn('context', { prompt: agent.prompt }, 'Context')
    }
    if (agent.toolIds.length) {
      spawn('tool', { prompt: '', toolIds: agent.toolIds }, 'Tool')
    }
    if (agent.skillIds.length) {
      spawn('skill', { prompt: '', skillIds: agent.skillIds, skillId: agent.skillIds[0] }, 'Skill')
    }
    nextNodes = nextNodes.map((item) =>
      item.id === node.id
        ? { ...item, data: { ...item.data, agent: { prompt: '' } } }
        : item,
    )
    nextNodes = [...nextNodes, ...spawned]
    nextEdges = [...nextEdges, ...spawnedEdges]
  }
  return { nodes: nextNodes, edges: nextEdges, changed }
}
