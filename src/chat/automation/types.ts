export const AUTOMATION_SCHEMA_VERSION = 1

export type AutomationNodeType =
  | 'trigger.manual'
  | 'trigger.schedule'
  | 'trigger.hotkey'
  | 'action.agent'
  | 'action.notify'
  | 'action.http'
  | 'logic.if'

export type ScheduleKind = 'daily' | 'weekdays' | 'interval'

export interface ScheduleData {
  kind: ScheduleKind
  hour: number
  minute: number
  intervalMinutes: number
}

export interface HotkeyData {
  accelerator: string
}

export interface AgentData {
  prompt: string
  skillId: string | null
}

export interface NotifyData {
  body: string
}

export interface HttpData {
  method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  url: string
  headers: string
  body: string
}

export type IfOp = 'contains' | 'equals' | 'notEmpty'

export interface IfData {
  op: IfOp
  value: string
}

export type NodeRunStatus = 'running' | 'success' | 'error' | 'skipped'

export interface FlowNodeData {
  label: string
  disabled?: boolean
  schedule?: ScheduleData
  hotkey?: HotkeyData
  agent?: AgentData
  notify?: NotifyData
  http?: HttpData
  if?: IfData
  [key: string]: unknown
}

export interface FlowNode {
  id: string
  type: AutomationNodeType
  position: { x: number; y: number }
  data: FlowNodeData
}

export interface FlowEdge {
  id: string
  source: string
  target: string
  sourceHandle?: string | null
  targetHandle?: string | null
}

export interface Automation {
  schemaVersion: number
  id: string
  name: string
  enabled: boolean
  nodes: FlowNode[]
  edges: FlowEdge[]
  viewport: { x: number; y: number; zoom: number }
  createdAt: string
  updatedAt: string
}

export interface AutomationMeta {
  id: string
  name: string
  enabled: boolean
  triggerType: string | null
  updatedAt: string
}

export type AutomationRunOrigin = 'manual' | 'schedule' | 'hotkey'

export interface AutomationRunEvent {
  kind: 'run_started' | 'node_started' | 'node_finished' | 'run_finished'
  automationId: string
  runId: string
  nodeId?: string | null
  status?: string | null
  output?: string | null
  error?: string | null
}

export interface AutomationRunSummary {
  id: string
  origin: AutomationRunOrigin | string
  status: string
  startedAt: string
  finishedAt?: string | null
  error?: string | null
}

export interface AutomationRunStarted {
  runId: string
}

export function isTriggerType(type: string): type is AutomationNodeType {
  return type.startsWith('trigger.')
}

export function isActionType(type: string): type is AutomationNodeType {
  return type.startsWith('action.')
}

export function isStepType(type: string): boolean {
  return type.startsWith('action.') || type.startsWith('logic.')
}

export function isIfType(type: string): boolean {
  return type === 'logic.if'
}
