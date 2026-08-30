export const AUTOMATION_SCHEMA_VERSION = 1

export type AutomationNodeType =
  | 'trigger.manual'
  | 'trigger.schedule'
  | 'trigger.hotkey'
  | 'action.agent'
  | 'action.notify'
  | 'action.http'
  | 'action.set'
  | 'action.clipboard'
  | 'action.file'
  | 'action.command'
  | 'logic.if'
  | 'logic.switch'
  | 'logic.delay'
  | 'agent.runtime'
  | 'agent.context'
  | 'agent.tool'
  | 'agent.skill'

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

export type AgentRuntimeKind = 'builtin' | 'chat' | 'external'

export type AgentSlot = 'runtime' | 'context' | 'tool' | 'skill'

export interface AgentData {
  prompt: string
  /** @deprecated 读时并入 skillIds；写新图请用 skillIds。 */
  skillId?: string | null
  runtimeKind?: AgentRuntimeKind
  externalAgentId?: string | null
  externalModel?: string | null
  providerId?: string | null
  model?: string | null
  /** 非空 = 额外挂载写入/执行类工具；空 = 只挂只读工具 + skill 加载器，不挂记忆。 */
  toolIds?: string[]
  skillIds?: string[]
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

export interface SwitchCase {
  id: string
  op: IfOp
  value: string
}

export interface SwitchData {
  cases: SwitchCase[]
}

export const MAX_SWITCH_CASES = 4

export interface SetField {
  key: string
  value: string
}

export interface SetData {
  fields: SetField[]
}

export interface DelayData {
  seconds: number
}

export type ClipboardOp = 'copy' | 'read'

export interface ClipboardData {
  op: ClipboardOp
  text: string
}

export type FileOp = 'read' | 'write'

export interface FileData {
  op: FileOp
  path: string
  content: string
}

export interface CommandData {
  command: string
  cwd?: string
  timeoutSeconds?: number
  continueOnFail?: boolean
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
  switch?: SwitchData
  set?: SetData
  delay?: DelayData
  clipboard?: ClipboardData
  file?: FileData
  command?: CommandData
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

export function isAttachmentType(type: string): boolean {
  return type.startsWith('agent.')
}

export function isIfType(type: string): boolean {
  return type === 'logic.if'
}

export function isSwitchType(type: string): boolean {
  return type === 'logic.switch'
}

export function branchHandles(type: string, data?: FlowNodeData): string[] | null {
  if (isIfType(type)) return ['true', 'false']
  if (isSwitchType(type)) {
    const ids = (data?.switch?.cases ?? [])
      .map((item) => item.id.trim())
      .filter(Boolean)
    return [...ids, 'default']
  }
  return null
}
