import { Handle, Position, type Node, type NodeProps } from '@xyflow/react'
import { useT } from '../../../settings/i18n'
import { catalogEntry } from '../nodeCatalog'
import { isTriggerType, type AutomationNodeType, type FlowNodeData } from '../types'

export type AutomationRfNode = Node<FlowNodeData, AutomationNodeType>

export function FlowNode({ data, selected, type }: NodeProps<AutomationRfNode>) {
  const t = useT()
  const trigger = isTriggerType(type ?? '')
  const entry = catalogEntry(type ?? '')
  const Icon = entry?.icon
  return (
    <div
      className={`kv-automation-node ${trigger ? 'is-trigger' : 'is-action'}${selected ? ' is-selected' : ''}`}
    >
      {!trigger ? (
        <Handle type="target" position={Position.Left} className="kv-automation-handle" />
      ) : null}
      {Icon ? (
        <span className="kv-automation-node-icon" aria-hidden>
          <Icon size={16} strokeWidth={1.75} />
        </span>
      ) : null}
      <span className="kv-automation-node-copy">
        <span className="kv-automation-node-kind">
          {trigger ? t.chatAutomationKindTrigger : t.chatAutomationKindAction}
        </span>
        <span className="kv-automation-node-title">{data.label}</span>
      </span>
      <Handle type="source" position={Position.Right} className="kv-automation-handle" />
    </div>
  )
}
