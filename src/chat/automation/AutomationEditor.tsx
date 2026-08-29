import { useCallback, useMemo, useRef, useState } from 'react'
import {
  Background,
  BackgroundVariant,
  Controls,
  ReactFlow,
  ReactFlowProvider,
  addEdge,
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type OnConnect,
  type Viewport,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import './automation.css'
import { ArrowLeft, Plus } from 'lucide-react'
import { Button } from '../../components/Button'
import { Toggle } from '../../settings/components'
import { useT } from '../../settings/i18n'
import { AddNodePicker } from './AddNodePicker'
import { NodeInspector } from './NodeInspector'
import { canConnect, connectNodes, createFlowNode, nextNodePosition, triggerNode } from './graph'
import { type NodeCatalogEntry } from './nodeCatalog'
import { FlowNode, type AutomationRfNode } from './nodes/FlowNode'
import { isTriggerType, type Automation, type AutomationNodeType, type FlowNode as FlowNodeModel } from './types'

const nodeTypes = {
  'trigger.manual': FlowNode,
  'trigger.schedule': FlowNode,
  'trigger.hotkey': FlowNode,
  'action.agent': FlowNode,
  'action.notify': FlowNode,
}

function toRfNodes(nodes: FlowNodeModel[]): AutomationRfNode[] {
  return nodes.map((node) => ({
    id: node.id,
    type: node.type,
    position: node.position,
    data: node.data,
  }))
}

function toRfEdges(automation: Automation): Edge[] {
  return automation.edges.map((edge) => ({
    id: edge.id,
    source: edge.source,
    target: edge.target,
    sourceHandle: edge.sourceHandle ?? undefined,
    targetHandle: edge.targetHandle ?? undefined,
  }))
}

function persist(
  base: Automation,
  nodes: AutomationRfNode[],
  edges: Edge[],
  viewport: Viewport,
): Automation {
  return {
    ...base,
    nodes: nodes.map((node) => ({
      id: node.id,
      type: (node.type ?? 'action.notify') as AutomationNodeType,
      position: node.position,
      data: node.data,
    })),
    edges: edges.map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      sourceHandle: edge.sourceHandle,
      targetHandle: edge.targetHandle,
    })),
    viewport,
  }
}

function EditorInner({
  automation,
  onChange,
  onBack,
}: {
  automation: Automation
  onChange: (next: Automation) => void
  onBack: () => void
}) {
  const t = useT()
  const [nodes, setNodes, onNodesChange] = useNodesState<AutomationRfNode>(toRfNodes(automation.nodes))
  const [edges, setEdges, onEdgesChange] = useEdgesState(toRfEdges(automation))
  const [selectedId, setSelectedId] = useState<string | null>(
    automation.nodes[0]?.id ?? null,
  )
  const [picker, setPicker] = useState<'trigger' | 'action' | null>(
    automation.nodes.length === 0 ? 'trigger' : null,
  )
  const viewportRef = useRef<Viewport>(automation.viewport)

  const selected = useMemo((): FlowNodeModel | null => {
    const rf = nodes.find((node) => node.id === selectedId)
    if (!rf?.type) return null
    return {
      id: rf.id,
      type: rf.type as AutomationNodeType,
      position: rf.position,
      data: rf.data,
    }
  }, [nodes, selectedId])
  const hasTrigger = nodes.some((node) => isTriggerType(node.type ?? ''))

  const commit = useCallback((nextNodes: AutomationRfNode[], nextEdges: Edge[]) => {
    onChange(persist(automation, nextNodes, nextEdges, viewportRef.current))
  }, [automation, onChange])

  const onConnect: OnConnect = useCallback((connection: Connection) => {
    if (!connection.source || !connection.target) return
    const model = persist(automation, nodes, edges, viewportRef.current)
    if (!canConnect(connection.source, connection.target, model.nodes, model.edges)) return
    const next = addEdge({ ...connection, id: connectNodes(connection.source, connection.target).id }, edges)
    setEdges(next)
    commit(nodes, next)
  }, [automation, commit, edges, nodes, setEdges])

  const isValidConnection = useCallback((connection: Edge | Connection) => {
    if (!connection.source || !connection.target) return false
    const model = persist(automation, nodes, edges, viewportRef.current)
    return canConnect(connection.source, connection.target, model.nodes, model.edges)
  }, [automation, edges, nodes])

  const addFromCatalog = useCallback((entry: NodeCatalogEntry) => {
    const model = persist(automation, nodes, edges, viewportRef.current)
    if (entry.kind === 'trigger' && triggerNode(model)) {
      setPicker(null)
      return
    }
    const node = createFlowNode(entry.type, entry.defaultData(t), nextNodePosition(model.nodes))
    const nextNodes: AutomationRfNode[] = [...nodes, {
      id: node.id,
      type: node.type,
      position: node.position,
      data: node.data,
    }]
    let nextEdges = edges
    const previous = model.nodes[model.nodes.length - 1]
    if (previous && canConnect(previous.id, node.id, [...model.nodes, node], model.edges)) {
      const edge = connectNodes(previous.id, node.id)
      nextEdges = [...edges, { id: edge.id, source: edge.source, target: edge.target }]
    }
    setNodes(nextNodes)
    setEdges(nextEdges)
    setSelectedId(node.id)
    setPicker(null)
    commit(nextNodes, nextEdges)
  }, [automation, commit, edges, nodes, setEdges, setNodes, t])

  const patchSelected = useCallback((next: FlowNodeModel) => {
    const nextNodes = nodes.map((node) =>
      node.id === next.id ? { ...node, data: next.data, type: next.type } : node,
    )
    setNodes(nextNodes)
    commit(nextNodes, edges)
  }, [commit, edges, nodes, setNodes])

  const defaultViewport = useMemo(
    () => ({ x: automation.viewport.x, y: automation.viewport.y, zoom: automation.viewport.zoom || 1 }),
    [automation.viewport.x, automation.viewport.y, automation.viewport.zoom],
  )

  return (
    <div className="kv-automation-editor">
      <header className="kv-automation-editor-bar">
        <Button variant="ghost" size="sm" onClick={onBack}>
          <ArrowLeft size={14} />
          {t.chatAutomationBack}
        </Button>
        <input
          className="kv-input kv-automation-name"
          value={automation.name}
          placeholder={t.chatAutomationUntitled}
          onChange={(event) =>
            onChange(persist({ ...automation, name: event.target.value }, nodes, edges, viewportRef.current))
          }
        />
        <div className="kv-automation-enable">
          <Toggle
            checked={automation.enabled}
            onChange={(enabled) =>
              onChange(persist({ ...automation, enabled }, nodes, edges, viewportRef.current))
            }
            ariaLabel={t.chatAutomationEnabled}
          />
          <span>{automation.enabled ? t.chatAutomationEnabled : t.chatAutomationDisabled}</span>
        </div>
        <div className="kv-automation-add-wrap">
          <Button
            size="sm"
            onClick={() => setPicker((current) => {
              const next = hasTrigger ? 'action' : 'trigger'
              return current === next ? null : next
            })}
          >
            <Plus size={14} />
            {hasTrigger ? t.chatAutomationAddStep : t.chatAutomationAddTrigger}
          </Button>
          {picker ? (
            <AddNodePicker
              kind={picker}
              onPick={addFromCatalog}
              onCancel={() => setPicker(null)}
            />
          ) : null}
        </div>
      </header>
      <div className="kv-automation-editor-body">
        <div className="kv-automation-canvas">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={onConnect}
            isValidConnection={isValidConnection}
            onNodeClick={(_, node) => setSelectedId(node.id)}
            onPaneClick={() => {
              setSelectedId(null)
              setPicker(null)
            }}
            onNodeDragStop={() => commit(nodes, edges)}
            onNodesDelete={(deleted) => {
              const ids = new Set(deleted.map((node) => node.id))
              commit(
                nodes.filter((node) => !ids.has(node.id)),
                edges.filter((edge) => !ids.has(edge.source) && !ids.has(edge.target)),
              )
            }}
            onEdgesDelete={(deleted) => {
              const ids = new Set(deleted.map((edge) => edge.id))
              commit(nodes, edges.filter((edge) => !ids.has(edge.id)))
            }}
            defaultViewport={defaultViewport}
            onMoveEnd={(_, viewport) => {
              viewportRef.current = viewport
              onChange(persist(automation, nodes, edges, viewport))
            }}
            fitView={automation.nodes.length > 0}
            minZoom={0.4}
            maxZoom={1.6}
            deleteKeyCode={['Backspace', 'Delete']}
            defaultEdgeOptions={{ type: 'smoothstep' }}
            proOptions={{ hideAttribution: true }}
            colorMode={document.documentElement.classList.contains('dark') ? 'dark' : 'light'}
          >
            <Background variant={BackgroundVariant.Dots} gap={20} size={1.2} />
            <Controls showInteractive={false} />
          </ReactFlow>
          {nodes.length === 0 && picker !== 'trigger' ? (
            <div className="kv-automation-empty">
              <p>{t.chatAutomationEmptyCanvas}</p>
              <Button size="sm" onClick={() => setPicker('trigger')}>
                <Plus size={14} />
                {t.chatAutomationAddTrigger}
              </Button>
            </div>
          ) : null}
        </div>
        {selected ? (
          <NodeInspector
            node={selected}
            onChange={(next) => {
              setSelectedId(next.id)
              patchSelected(next)
            }}
          />
        ) : (
          <aside className="kv-automation-inspector">
            <p className="kv-automation-inspector-copy">{t.chatAutomationInspectorEmpty}</p>
          </aside>
        )}
      </div>
    </div>
  )
}

export function AutomationEditor(props: {
  automation: Automation
  onChange: (next: Automation) => void
  onBack: () => void
}) {
  return (
    <ReactFlowProvider>
      <EditorInner {...props} />
    </ReactFlowProvider>
  )
}
