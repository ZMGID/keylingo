import { useContext, useState } from 'react'
import { BaseEdge, EdgeLabelRenderer, getBezierPath, type EdgeProps } from '@xyflow/react'
import { Plus, X } from 'lucide-react'
import { IconButton } from '../../../components/Button'
import { useT } from '../../../settings/i18n'
import { CanvasChromeContext } from './chrome'

/** n8n 式连线：hover / 选中时在中点浮出「插入节点」和「删除连线」。槽边只允许删。 */
export function ActionEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  selected,
  markerEnd,
  style,
  data,
}: EdgeProps) {
  const t = useT()
  const chrome = useContext(CanvasChromeContext)
  const [hovered, setHovered] = useState(false)
  const [path, labelX, labelY] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
  })
  const slot = Boolean(data && (data as { slot?: boolean }).slot)
  const visible = hovered || selected
  return (
    <g onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)}>
      <BaseEdge id={id} path={path} markerEnd={slot ? undefined : markerEnd} style={style} />
      <EdgeLabelRenderer>
        <div
          className={`kv-automation-edge-actions nodrag nopan${visible ? ' is-visible' : ''}`}
          style={{ transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)` }}
          onMouseEnter={() => setHovered(true)}
          onMouseLeave={() => setHovered(false)}
        >
          {slot ? null : (
            <IconButton
              size="xs"
              shape="circle"
              label={t.chatAutomationInsertNode}
              onClick={(event) => {
                event.stopPropagation()
                chrome.onInsertOnEdge(id)
              }}
            >
              <Plus size={11} strokeWidth={2.25} />
            </IconButton>
          )}
          <IconButton
            size="xs"
            shape="circle"
            variant="danger"
            label={t.chatAutomationDeleteEdge}
            onClick={(event) => {
              event.stopPropagation()
              chrome.onDeleteEdge(id)
            }}
          >
            <X size={11} strokeWidth={2.25} />
          </IconButton>
        </div>
      </EdgeLabelRenderer>
    </g>
  )
}
