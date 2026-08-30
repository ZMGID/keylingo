import { createContext, useContext } from 'react'
import { Handle, Position, type Node, type NodeProps } from '@xyflow/react'
import { Plus } from 'lucide-react'
import { useT } from '../../../settings/i18n'
import { catalogEntry } from '../nodeCatalog'
import { isIfType, isTriggerType, type AutomationNodeType, type FlowNodeData, type NodeRunStatus } from '../types'

export type AutomationRfNode = Node<FlowNodeData, AutomationNodeType>

export const NodeRunStatusContext = createContext<Record<string, NodeRunStatus>>({})

export interface NodeChrome {
  onAddNext: (nodeId: string, sourceHandle?: string) => void
  occupied: ReadonlyMap<string, ReadonlySet<string>>
}

export const NodeChromeContext = createContext<NodeChrome>({
  onAddNext: () => {},
  occupied: new Map(),
})

function AddNextButton({
  onClick,
  branch,
}: {
  onClick: () => void
  branch?: 'true' | 'false'
}) {
  const t = useT()
  return (
    <button
      type="button"
      className={`kv-automation-node-plus nodrag nopan${branch ? ` ${branch}` : ''}`}
      aria-label={t.chatAutomationAddStep}
      onClick={(event) => {
        event.stopPropagation()
        event.preventDefault()
        onClick()
      }}
      onMouseDown={(event) => event.stopPropagation()}
    >
      <Plus size={12} strokeWidth={2.25} />
    </button>
  )
}

export function FlowNode({ id, data, selected, type }: NodeProps<AutomationRfNode>) {
  const t = useT()
  const trigger = isTriggerType(type ?? '')
  const branching = isIfType(type ?? '')
  const entry = catalogEntry(type ?? '')
  const Icon = entry?.icon
  const status = useContext(NodeRunStatusContext)[id]
  const chrome = useContext(NodeChromeContext)
  const taken = chrome.occupied.get(id)
  const kind = trigger
    ? t.chatAutomationKindTrigger
    : branching
      ? t.chatAutomationKindLogic
      : t.chatAutomationKindAction
  return (
    <div
      className={[
        'kv-automation-node',
        trigger ? 'is-trigger' : branching ? 'is-logic' : 'is-action',
        selected ? 'is-selected' : '',
        status ? `is-${status}` : '',
        data.disabled ? 'is-disabled' : '',
      ].filter(Boolean).join(' ')}
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
        <span className="kv-automation-node-kind">{kind}</span>
        <span className="kv-automation-node-title">{data.label}</span>
      </span>
      {branching ? (
        <>
          <Handle
            type="source"
            id="true"
            position={Position.Right}
            className="kv-automation-handle kv-automation-handle--true"
            style={{ top: '32%' }}
          />
          <Handle
            type="source"
            id="false"
            position={Position.Right}
            className="kv-automation-handle kv-automation-handle--false"
            style={{ top: '68%' }}
          />
          <span className="kv-automation-branch true">{t.chatAutomationIfTrue}</span>
          <span className="kv-automation-branch false">{t.chatAutomationIfFalse}</span>
          {!taken?.has('true') ? (
            <AddNextButton branch="true" onClick={() => chrome.onAddNext(id, 'true')} />
          ) : null}
          {!taken?.has('false') ? (
            <AddNextButton branch="false" onClick={() => chrome.onAddNext(id, 'false')} />
          ) : null}
        </>
      ) : (
        <>
          <Handle type="source" position={Position.Right} className="kv-automation-handle" />
          <AddNextButton onClick={() => chrome.onAddNext(id)} />
        </>
      )}
    </div>
  )
}
