import { memo, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Boxes } from 'lucide-react'
import { useT } from '../settings/i18n'
import { messageNavigatorProximityWidth, type MessageNavigatorNode } from './messageNavigator'

interface PreviewAnchor {
  node: MessageNavigatorNode
  top: number
  left: number
}

interface MessageNavigatorProps {
  nodes: MessageNavigatorNode[]
  activeNodeId: string | null
  visibleNodeIds: string[]
  onNavigate: (node: MessageNavigatorNode) => void
  onNavigateStep: (direction: -1 | 1) => void
}

function nodesEqual(a: MessageNavigatorNode[], b: MessageNavigatorNode[]): boolean {
  if (a === b) return true
  if (a.length !== b.length) return false
  return a.every((node, index) => {
    const other = b[index]
    return node.kind === other.kind
      && node.id === other.id
      && node.targetRenderIndex === other.targetRenderIndex
      && node.title === other.title
      && node.answerPreview === other.answerPreview
      && node.modelLabel === other.modelLabel
  })
}

function MessageNavigatorBase({
  nodes,
  activeNodeId,
  visibleNodeIds,
  onNavigate,
  onNavigateStep,
}: MessageNavigatorProps) {
  const t = useT()
  const viewportRef = useRef<HTMLDivElement>(null)
  const lastWheelAtRef = useRef(0)
  const proximityFrameRef = useRef<number | null>(null)
  const proximityPointerYRef = useRef(0)
  const [preview, setPreview] = useState<PreviewAnchor | null>(null)
  const visibleNodeIdSet = new Set(visibleNodeIds)

  useEffect(() => () => {
    if (proximityFrameRef.current != null) cancelAnimationFrame(proximityFrameRef.current)
  }, [])

  useEffect(() => {
    if (!activeNodeId) return
    const viewport = viewportRef.current
    const active = [...(viewport?.querySelectorAll<HTMLElement>('[data-message-navigator-id]') ?? [])]
      .find((element) => element.dataset.messageNavigatorId === activeNodeId)
    if (!viewport || !active) return
    const viewportRect = viewport.getBoundingClientRect()
    const activeRect = active.getBoundingClientRect()
    if (activeRect.top < viewportRect.top || activeRect.bottom > viewportRect.bottom) {
      active.scrollIntoView({ block: 'nearest' })
    }
  }, [activeNodeId])

  const showPreview = (node: MessageNavigatorNode, button: HTMLButtonElement) => {
    const rect = button.getBoundingClientRect()
    const center = rect.top + rect.height / 2
    // 导航条贴右侧滚动条，预览气泡向左弹出，避免超出右边界。
    const previewWidth = Math.min(320, window.innerWidth - 32)
    setPreview({
      node,
      top: Math.min(window.innerHeight - 96, Math.max(96, center)),
      left: Math.max(16, rect.left - 12 - previewWidth),
    })
  }

  const handleWheel = (event: React.WheelEvent) => {
    event.preventDefault()
    event.stopPropagation()
    if (Math.abs(event.deltaY) < 4) return
    const now = performance.now()
    if (now - lastWheelAtRef.current < 150) return
    lastWheelAtRef.current = now
    onNavigateStep(event.deltaY > 0 ? 1 : -1)
  }

  const applyPointerProximity = () => {
    proximityFrameRef.current = null
    const pointerY = proximityPointerYRef.current
    const buttons = viewportRef.current?.querySelectorAll<HTMLElement>('.chat-message-navigator-node')
    buttons?.forEach((button) => {
      const rect = button.getBoundingClientRect()
      const distance = Math.abs(pointerY - (rect.top + rect.height / 2))
      const width = messageNavigatorProximityWidth(distance)
      button.style.setProperty('--message-navigator-node-width', `${width.toFixed(2)}px`)
    })
  }

  const handlePointerMove = (event: React.MouseEvent) => {
    proximityPointerYRef.current = event.clientY
    if (proximityFrameRef.current != null) return
    proximityFrameRef.current = requestAnimationFrame(applyPointerProximity)
  }

  const clearPointerProximity = () => {
    if (proximityFrameRef.current != null) cancelAnimationFrame(proximityFrameRef.current)
    proximityFrameRef.current = null
    viewportRef.current?.querySelectorAll<HTMLElement>('.chat-message-navigator-node')
      .forEach((button) => button.style.removeProperty('--message-navigator-node-width'))
    setPreview(null)
  }

  return (
    <>
      <aside className="chat-message-navigator" aria-label={t.chatTurnNavigator}>
        <div
          ref={viewportRef}
          className="chat-message-navigator-viewport"
          onWheel={handleWheel}
          onMouseMove={handlePointerMove}
          onMouseLeave={clearPointerProximity}
        >
          <div className="chat-message-navigator-track">
            {nodes.map((node, index) => {
              const active = node.id === activeNodeId
              return (
                <button
                  key={node.id}
                  type="button"
                  data-message-navigator-id={node.id}
                  className={`chat-message-navigator-node ${active ? 'is-active' : ''} ${visibleNodeIdSet.has(node.id) ? 'is-visible' : ''} ${node.kind === 'compaction' ? 'is-compaction' : ''}`}
                  aria-current={active ? 'location' : undefined}
                  aria-label={
                    node.kind === 'compaction'
                      ? t.chatCompactionSummaryLabel
                      : t.chatTurnLabel
                          .replace('{n}', String(index + 1))
                          .replace('{title}', node.title)
                  }
                  onClick={() => onNavigate(node)}
                  onMouseEnter={(event) => showPreview(node, event.currentTarget)}
                  onFocus={(event) => showPreview(node, event.currentTarget)}
                  onBlur={() => setPreview(null)}
                >
                  {node.kind === 'compaction' ? (
                    <span className="chat-message-navigator-compaction-mark" aria-hidden="true">
                      <span />
                      <span />
                    </span>
                  ) : (
                    <span className="chat-message-navigator-tick" aria-hidden="true" />
                  )}
                </button>
              )
            })}
          </div>
        </div>
      </aside>
      {preview && createPortal(
        <div
          className="chat-message-navigator-preview"
          style={{ top: preview.top, left: preview.left }}
          role="tooltip"
        >
          <div className="chat-message-navigator-preview-title">
            {preview.node.kind === 'compaction' && <Boxes size={14} aria-hidden="true" />}
            <span>{preview.node.title || t.chatUntitledMessage}</span>
          </div>
          {preview.node.answerPreview && (
            <p className="chat-message-navigator-preview-answer">{preview.node.answerPreview}</p>
          )}
          {preview.node.modelLabel && (
            <div className="chat-message-navigator-preview-model">{preview.node.modelLabel}</div>
          )}
        </div>,
        document.body,
      )}
    </>
  )
}

export const MessageNavigator = memo(
  MessageNavigatorBase,
  (prev, next) => (
    prev.activeNodeId === next.activeNodeId
    && prev.visibleNodeIds.length === next.visibleNodeIds.length
    && prev.visibleNodeIds.every((id, index) => id === next.visibleNodeIds[index])
    && prev.onNavigate === next.onNavigate
    && prev.onNavigateStep === next.onNavigateStep
    && nodesEqual(prev.nodes, next.nodes)
  ),
)
