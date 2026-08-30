import { useContext } from 'react'
import { Handle, Position, type Node, type NodeProps } from '@xyflow/react'
import { Check, Loader2, Minus, Play, Plus, Power, Trash2, X } from 'lucide-react'
import { useT } from '../../../settings/i18n'
import { catalogEntry, nodeSummary } from '../nodeCatalog'
import {
  AGENT_SLOTS,
  agentSlotLabel,
  isAgentSlotRequired,
  slotAllowsMany,
} from '../agentModel'
import { isAttachmentType, isIfType, isTriggerType, type AutomationNodeType, type FlowNodeData, type NodeRunStatus } from '../types'
import { CanvasChromeContext, NodeRunStatusContext } from './chrome'

export type AutomationRfNode = Node<FlowNodeData, AutomationNodeType>

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

function ToolbarButton({
  label,
  danger,
  disabled,
  onClick,
  children,
}: {
  label: string
  danger?: boolean
  disabled?: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      className={`kv-automation-node-tool${danger ? ' is-danger' : ''}`}
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={(event) => {
        event.stopPropagation()
        event.preventDefault()
        onClick()
      }}
      onMouseDown={(event) => event.stopPropagation()}
    >
      {children}
    </button>
  )
}

function AgentSlots({ nodeId }: { nodeId: string }) {
  const t = useT()
  const chrome = useContext(CanvasChromeContext)
  const filled = chrome.occupiedSlots.get(nodeId)
  return (
    <>
      {AGENT_SLOTS.map((slot, index) => {
        const connected = filled?.has(slot) ?? false
        const required = isAgentSlotRequired(slot)
        return (
          <Handle
            key={slot}
            type="target"
            id={slot}
            position={Position.Bottom}
            className={[
              'kv-automation-slot-handle',
              connected ? 'is-filled' : '',
              required && !connected ? 'is-missing' : '',
            ].filter(Boolean).join(' ')}
            style={{ left: `${12.5 + index * 25}%` }}
          />
        )
      })}
      <div className="kv-automation-slots nodrag nopan">
        {AGENT_SLOTS.map((slot) => {
          const connected = filled?.has(slot) ?? false
          const required = isAgentSlotRequired(slot)
          const showPlus = !connected || slotAllowsMany(slot)
          return (
            <div key={slot} className="kv-automation-slot">
              <span className="kv-automation-slot-label">
                {agentSlotLabel(slot, t)}
                {required ? <span className="kv-automation-slot-req">*</span> : null}
              </span>
              {showPlus ? (
                <button
                  type="button"
                  className="kv-automation-slot-add"
                  aria-label={agentSlotLabel(slot, t)}
                  onClick={(event) => {
                    event.stopPropagation()
                    chrome.onAddAgentSlot(nodeId, slot)
                  }}
                  onMouseDown={(event) => event.stopPropagation()}
                >
                  <Plus size={10} strokeWidth={2.5} />
                </button>
              ) : null}
            </div>
          )
        })}
      </div>
    </>
  )
}

function StatusBadge({ status }: { status: NodeRunStatus }) {
  if (status === 'running') {
    return (
      <span className="kv-automation-node-status is-running" aria-hidden>
        <Loader2 size={10} strokeWidth={2.5} className="kv-automation-spin" />
      </span>
    )
  }
  if (status === 'success') {
    return (
      <span className="kv-automation-node-status is-success" aria-hidden>
        <Check size={10} strokeWidth={3} />
      </span>
    )
  }
  if (status === 'error') {
    return (
      <span className="kv-automation-node-status is-error" aria-hidden>
        <X size={10} strokeWidth={3} />
      </span>
    )
  }
  return (
    <span className="kv-automation-node-status is-skipped" aria-hidden>
      <Minus size={10} strokeWidth={3} />
    </span>
  )
}

export function FlowNode({ id, data, selected, type }: NodeProps<AutomationRfNode>) {
  const t = useT()
  const trigger = isTriggerType(type ?? '')
  const branching = isIfType(type ?? '')
  const attach = isAttachmentType(type ?? '')
  const agent = type === 'action.agent'
  const entry = catalogEntry(type ?? '')
  const Icon = entry?.icon
  const status = useContext(NodeRunStatusContext)[id]
  const chrome = useContext(CanvasChromeContext)
  const taken = chrome.occupied.get(id)
  const summary = nodeSummary(type ?? '', data, t)
  return (
    <div
      className={[
        'kv-automation-node',
        trigger ? 'is-trigger' : branching ? 'is-logic' : agent ? 'is-agent' : attach ? 'is-attach' : 'is-action',
        selected ? 'is-selected' : '',
        status ? `is-${status}` : '',
        data.disabled ? 'is-disabled' : '',
      ].filter(Boolean).join(' ')}
    >
      <div className="kv-automation-node-toolbar nodrag nopan">
        {!trigger && !attach ? (
          <ToolbarButton
            label={t.chatAutomationExecuteStep}
            disabled={chrome.running}
            onClick={() => chrome.onRunTo(id)}
          >
            <Play size={11} strokeWidth={2.25} />
          </ToolbarButton>
        ) : null}
        {!trigger ? (
          <ToolbarButton
            label={data.disabled ? t.chatAutomationNodeEnable : t.chatAutomationNodeDisable}
            onClick={() => chrome.onToggleDisabled(id)}
          >
            <Power size={11} strokeWidth={2.25} />
          </ToolbarButton>
        ) : null}
        <ToolbarButton
          label={t.chatAutomationDeleteNode}
          danger
          onClick={() => chrome.onDeleteNode(id)}
        >
          <Trash2 size={11} strokeWidth={2} />
        </ToolbarButton>
      </div>
      <div className="kv-automation-node-card">
        {attach ? (
          <Handle
            type="source"
            id="slot"
            position={Position.Top}
            className="kv-automation-slot-handle is-source"
          />
        ) : null}
        {!trigger && !attach ? (
          <Handle type="target" position={Position.Left} className="kv-automation-handle kv-automation-handle--in" />
        ) : null}
        {Icon ? (
          <span className="kv-automation-node-icon" aria-hidden>
            <Icon size={attach ? 20 : 22} strokeWidth={1.75} />
          </span>
        ) : null}
        {status ? <StatusBadge status={status} /> : null}
        {branching ? (
          <>
            <Handle
              type="source"
              id="true"
              position={Position.Right}
              className="kv-automation-handle kv-automation-handle--true"
              style={{ top: '30%' }}
            />
            <Handle
              type="source"
              id="false"
              position={Position.Right}
              className="kv-automation-handle kv-automation-handle--false"
              style={{ top: '70%' }}
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
        ) : attach ? null : (
          <>
            <Handle type="source" position={Position.Right} className="kv-automation-handle" />
            {!taken?.size ? (
              <AddNextButton onClick={() => chrome.onAddNext(id)} />
            ) : null}
          </>
        )}
      </div>
      {agent ? <AgentSlots nodeId={id} /> : null}
      <div className="kv-automation-node-caption">
        <span className="kv-automation-node-title">{data.label}</span>
        {summary ? <span className="kv-automation-node-sub">{summary}</span> : null}
      </div>
    </div>
  )
}
