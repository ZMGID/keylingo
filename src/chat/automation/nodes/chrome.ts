import { createContext } from 'react'
import type { AgentSlot } from '../types'
import type { NodeRunStatus } from '../types'

export const NodeRunStatusContext = createContext<Record<string, NodeRunStatus>>({})

/** 画布级回调：节点/连线的 hover 操作都经这里回到编辑器。 */
export interface CanvasChrome {
  onAddNext: (nodeId: string, sourceHandle?: string) => void
  onRunTo: (nodeId: string) => void
  onToggleDisabled: (nodeId: string) => void
  onDeleteNode: (nodeId: string) => void
  onInsertOnEdge: (edgeId: string) => void
  onDeleteEdge: (edgeId: string) => void
  onAddAgentSlot: (nodeId: string, slot: AgentSlot) => void
  running: boolean
  occupied: ReadonlyMap<string, ReadonlySet<string>>
  occupiedSlots: ReadonlyMap<string, ReadonlySet<AgentSlot>>
}

export const CanvasChromeContext = createContext<CanvasChrome>({
  onAddNext: () => {},
  onRunTo: () => {},
  onToggleDisabled: () => {},
  onDeleteNode: () => {},
  onInsertOnEdge: () => {},
  onDeleteEdge: () => {},
  onAddAgentSlot: () => {},
  running: false,
  occupied: new Map(),
  occupiedSlots: new Map(),
})
