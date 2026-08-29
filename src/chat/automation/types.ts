export const AUTOMATION_SCHEMA_VERSION = 1

export type AutomationNodeType =
  | 'trigger.manual'
  | 'trigger.schedule'
  | 'trigger.hotkey'
  | 'action.agent'
  | 'action.notify'

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

export interface FlowNodeData {
  label: string
  schedule?: ScheduleData
  hotkey?: HotkeyData
  agent?: AgentData
  notify?: NotifyData
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

export function isTriggerType(type: string): type is AutomationNodeType {
  return type.startsWith('trigger.')
}

export function isActionType(type: string): type is AutomationNodeType {
  return type.startsWith('action.')
}
