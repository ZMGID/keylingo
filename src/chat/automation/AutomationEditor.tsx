import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Background,
  BackgroundVariant,
  ConnectionLineType,
  Controls,
  MarkerType,
  ReactFlow,
  ReactFlowProvider,
  addEdge,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Connection,
  type Edge,
  type OnConnect,
  type OnConnectEnd,
  type Viewport,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import './automation.css'
import { save as saveDialog } from '@tauri-apps/plugin-dialog'
import { ArrowLeft, Download, Play, Plus, Square } from 'lucide-react'
import { api, isTauriRuntime } from '../../api/tauri'
import { Button } from '../../components/Button'
import { Toggle } from '../../settings/components'
import { useT } from '../../settings/i18n'
import { AddNodePicker } from './AddNodePicker'
import { automationApi } from './api'
import { NodeInspector } from './NodeInspector'
import {
  canConnect,
  connectNodes,
  createFlowNode,
  nextNodePosition,
  pickAppendSource,
  triggerNode,
} from './graph'
import { SLOT_CATALOG, type NodeCatalogEntry } from './nodeCatalog'
import {
  AGENT_SLOTS,
  connectSlotEdge,
  explodeInlineAgents,
  isSlotEdge,
  resolveSlotConnection,
  slotAttachPosition,
} from './agentModel'
import { ActionEdge } from './nodes/ActionEdge'
import {
  CanvasChromeContext,
  NodeRunStatusContext,
  type CanvasChrome,
} from './nodes/chrome'
import { FlowNode, type AutomationRfNode } from './nodes/FlowNode'
import {
  isAttachmentType,
  isIfType,
  isTriggerType,
  type AgentSlot,
  type Automation,
  type AutomationNodeType,
  type AutomationRunEvent,
  type AutomationRunSummary,
  type FlowNode as FlowNodeModel,
  type NodeRunStatus,
} from './types'

const nodeTypes = {
  'trigger.manual': FlowNode,
  'trigger.schedule': FlowNode,
  'trigger.hotkey': FlowNode,
  'action.agent': FlowNode,
  'action.notify': FlowNode,
  'action.http': FlowNode,
  'logic.if': FlowNode,
  'agent.runtime': FlowNode,
  'agent.context': FlowNode,
  'agent.tool': FlowNode,
  'agent.skill': FlowNode,
}

const edgeTypes = { action: ActionEdge }

const EDGE_MARKER = { type: MarkerType.ArrowClosed, width: 16, height: 16 }

/** 所有边都经这里补齐渲染属性（自定义边类型 + 箭头）；persist 时会被剥掉，不落盘。 */
function withEdgeChrome(edge: {
  id: string
  source: string
  target: string
  sourceHandle?: string | null
  targetHandle?: string | null
}): Edge {
  const slot = isSlotEdge(edge)
  return {
    id: edge.id,
    type: 'action',
    source: edge.source,
    target: edge.target,
    sourceHandle: edge.sourceHandle ?? undefined,
    targetHandle: edge.targetHandle ?? undefined,
    className: slot ? 'kv-automation-edge-slot' : undefined,
    data: slot ? { slot: true } : undefined,
    markerEnd: slot ? undefined : EDGE_MARKER,
    style: slot
      ? { strokeDasharray: '5 4', strokeWidth: 1.5 }
      : { strokeWidth: 1.75 },
  }
}

/** 加节点的来源意图：节点右侧 + / 拖线到空白（after），或连线中点的插入（edge）。 */
type AddIntent =
  | { kind: 'after', nodeId: string, handle?: string, at?: { x: number, y: number } }
  | { kind: 'edge', edgeId: string }

function toRfNodes(nodes: FlowNodeModel[]): AutomationRfNode[] {
  return nodes.map((node) => ({
    id: node.id,
    type: node.type,
    position: node.position,
    data: node.data,
  }))
}

function toRfEdges(automation: Automation): Edge[] {
  return automation.edges.map(withEdgeChrome)
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
  onFlushSave,
}: {
  automation: Automation
  onChange: (next: Automation) => void
  onBack: () => void
  onFlushSave: () => Promise<void>
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
  const [running, setRunning] = useState(false)
  const [runError, setRunError] = useState('')
  const [nodeStatus, setNodeStatus] = useState<Record<string, NodeRunStatus>>({})
  const [nodeOutput, setNodeOutput] = useState<Record<string, string>>({})
  const [runs, setRuns] = useState<AutomationRunSummary[]>([])
  const viewportRef = useRef<Viewport>(automation.viewport)
  const pendingAddRef = useRef<AddIntent | null>(null)
  const nodesRef = useRef(nodes)
  const edgesRef = useRef(edges)
  nodesRef.current = nodes
  edgesRef.current = edges
  const { screenToFlowPosition } = useReactFlow()

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

  const loadRuns = useCallback(async () => {
    if (!isTauriRuntime()) return
    try {
      setRuns(await automationApi.listRuns(automation.id))
    } catch {
      // listing is best-effort; the canvas still works
    }
  }, [automation.id])

  useEffect(() => {
    void loadRuns()
  }, [loadRuns])

  useEffect(() => {
    const exploded = explodeInlineAgents(automation.nodes, automation.edges)
    if (!exploded.changed) return
    setNodes(toRfNodes(exploded.nodes))
    setEdges(exploded.edges.map(withEdgeChrome))
    onChange({ ...automation, nodes: exploded.nodes, edges: exploded.edges })
    // Only rewrite when this automation is first opened.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [automation.id])

  useEffect(() => {
    if (!isTauriRuntime()) return
    let cancelled = false
    let unlisten: (() => void) | undefined
    void api.onAutomationRun((event: AutomationRunEvent) => {
      if (event.automationId !== automation.id) return
      if (event.kind === 'run_started') {
        setRunning(true)
        setRunError('')
        setNodeStatus({})
        setNodeOutput({})
      }
      if (event.kind === 'node_started' && event.nodeId) {
        setNodeStatus((current) => ({ ...current, [event.nodeId!]: 'running' }))
      }
      if (event.kind === 'node_finished' && event.nodeId) {
        const status: NodeRunStatus = event.status === 'error'
          ? 'error'
          : event.status === 'skipped' || event.status === 'cancelled'
            ? 'skipped'
            : 'success'
        setNodeStatus((current) => ({ ...current, [event.nodeId!]: status }))
        if (event.output) {
          setNodeOutput((current) => ({ ...current, [event.nodeId!]: event.output! }))
        }
        if (event.error) setRunError(event.error)
      }
      if (event.kind === 'run_finished') {
        setRunning(false)
        if (event.status === 'error' && event.error) setRunError(event.error)
        if (event.status === 'cancelled') setRunError(t.chatAutomationCancelled)
        void loadRuns()
      }
    }).then((fn) => {
      if (cancelled) fn()
      else unlisten = fn
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [automation.id, loadRuns, t])

  const commit = useCallback((nextNodes: AutomationRfNode[], nextEdges: Edge[]) => {
    onChange(persist(automation, nextNodes, nextEdges, viewportRef.current))
  }, [automation, onChange])

  const onConnect: OnConnect = useCallback((connection: Connection) => {
    if (!connection.source || !connection.target) return
    const currentNodes = nodesRef.current
    const currentEdges = edgesRef.current
    const model = persist(automation, currentNodes, currentEdges, viewportRef.current)
    const from = model.nodes.find((node) => node.id === connection.source)
    const slot = from ? resolveSlotConnection(from.type, connection.targetHandle) : null
    if (!canConnect(
      connection.source,
      connection.target,
      model.nodes,
      model.edges,
      connection.sourceHandle,
      slot ?? connection.targetHandle,
    )) {
      return
    }
    const edge = connectNodes(
      connection.source,
      connection.target,
      slot ? 'slot' : connection.sourceHandle,
      slot ?? connection.targetHandle,
    )
    const nextEdges = addEdge(withEdgeChrome({ ...edge, targetHandle: connection.targetHandle }), currentEdges)
    setEdges(nextEdges)
    commit(currentNodes, nextEdges)
  }, [automation, commit, setEdges])

  const isValidConnection = useCallback((connection: Edge | Connection) => {
    if (!connection.source || !connection.target) return false
    const model = persist(automation, nodes, edges, viewportRef.current)
    return canConnect(
      connection.source,
      connection.target,
      model.nodes,
      model.edges,
      connection.sourceHandle,
      connection.targetHandle,
    )
  }, [automation, edges, nodes])

  const addFromCatalog = useCallback((entry: NodeCatalogEntry) => {
    const currentNodes = nodesRef.current
    const currentEdges = edgesRef.current
    const model = persist(automation, currentNodes, currentEdges, viewportRef.current)
    if (entry.kind === 'trigger' && triggerNode(model)) {
      setPicker(null)
      pendingAddRef.current = null
      return
    }
    let intent = pendingAddRef.current
    pendingAddRef.current = null

    // 从连线中点插入：拆旧边，新节点接到中间。
    if (intent?.kind === 'edge') {
      const edgeId = intent.edgeId
      const splitEdge = model.edges.find((item) => item.id === edgeId)
      if (splitEdge) {
        const src = model.nodes.find((item) => item.id === splitEdge.source)
        const tgt = model.nodes.find((item) => item.id === splitEdge.target)
        const at = src && tgt
          ? { x: (src.position.x + tgt.position.x) / 2, y: (src.position.y + tgt.position.y) / 2 }
          : nextNodePosition(model.nodes)
        const node = createFlowNode(entry.type, entry.defaultData(t), at)
        const nextNodes: AutomationRfNode[] = [...currentNodes, {
          id: node.id,
          type: node.type,
          position: node.position,
          data: node.data,
        }]
        const kept = currentEdges.filter((item) => item.id !== splitEdge.id)
        const nextEdges: Edge[] = [
          ...kept,
          withEdgeChrome(connectNodes(splitEdge.source, node.id, splitEdge.sourceHandle)),
          withEdgeChrome(connectNodes(node.id, splitEdge.target, isIfType(node.type) ? 'true' : undefined)),
        ]
        setNodes(nextNodes)
        setEdges(nextEdges)
        setSelectedId(node.id)
        setPicker(null)
        commit(nextNodes, nextEdges)
        return
      }
      intent = null
    }

    // 底部胶囊「添加节点」没有显式来源时，自动接到还能出边的尾节点（对齐 n8n 的线性搭积木）。
    if (!intent && entry.kind !== 'trigger') {
      const tail = pickAppendSource(model.nodes, model.edges, selectedId)
      if (tail) intent = { kind: 'after', nodeId: tail.nodeId, handle: tail.handle }
    }

    const after = intent?.kind === 'after' ? intent : null
    const node = createFlowNode(
      entry.type,
      entry.defaultData(t),
      after?.at ?? nextNodePosition(model.nodes, after?.nodeId, after?.handle),
    )
    const nextNodes: AutomationRfNode[] = [...currentNodes, {
      id: node.id,
      type: node.type,
      position: node.position,
      data: node.data,
    }]
    let nextEdges = currentEdges
    if (after && canConnect(after.nodeId, node.id, [...model.nodes, node], model.edges, after.handle)) {
      nextEdges = [...currentEdges, withEdgeChrome(connectNodes(after.nodeId, node.id, after.handle))]
      setEdges(nextEdges)
    }
    setNodes(nextNodes)
    setSelectedId(node.id)
    setPicker(null)
    commit(nextNodes, nextEdges)
  }, [automation, commit, selectedId, setEdges, setNodes, t])

  const patchSelected = useCallback((next: FlowNodeModel) => {
    const nextNodes = nodesRef.current.map((node) =>
      node.id === next.id ? { ...node, data: next.data, type: next.type } : node,
    )
    setNodes(nextNodes)
    commit(nextNodes, edgesRef.current)
  }, [commit, setNodes])

  const deleteNode = useCallback((nodeId: string) => {
    const drop = new Set<string>([nodeId])
    const target = nodesRef.current.find((node) => node.id === nodeId)
    if (target?.type === 'action.agent') {
      for (const edge of edgesRef.current) {
        if (edge.target === nodeId && isSlotEdge(edge)) drop.add(edge.source)
      }
    }
    const nextNodes = nodesRef.current.filter((node) => !drop.has(node.id))
    const nextEdges = edgesRef.current.filter(
      (edge) => !drop.has(edge.source) && !drop.has(edge.target),
    )
    setNodes(nextNodes)
    setEdges(nextEdges)
    setSelectedId((current) => (current && drop.has(current) ? null : current))
    commit(nextNodes, nextEdges)
  }, [commit, setEdges, setNodes])

  const toggleNodeDisabled = useCallback((nodeId: string) => {
    const nextNodes = nodesRef.current.map((node) =>
      node.id === nodeId
        ? { ...node, data: { ...node.data, disabled: !node.data.disabled } }
        : node,
    )
    setNodes(nextNodes)
    commit(nextNodes, edgesRef.current)
  }, [commit, setNodes])

  const deleteEdge = useCallback((edgeId: string) => {
    const nextEdges = edgesRef.current.filter((edge) => edge.id !== edgeId)
    setEdges(nextEdges)
    commit(nodesRef.current, nextEdges)
  }, [commit, setEdges])

  const runGraph = useCallback(async (untilNodeId?: string) => {
    if (!isTauriRuntime()) return
    setRunError('')
    try {
      await onFlushSave()
      await automationApi.run(automation.id, untilNodeId)
      setRunning(true)
    } catch (err) {
      setRunError(err instanceof Error ? err.message : String(err))
    }
  }, [automation.id, onFlushSave])

  const cancelRun = useCallback(async () => {
    try {
      await automationApi.cancel(automation.id)
    } catch (err) {
      setRunError(err instanceof Error ? err.message : String(err))
    }
  }, [automation.id])

  const addAgentSlot = useCallback((agentId: string, slot: AgentSlot) => {
    const agent = nodesRef.current.find((node) => node.id === agentId)
    if (!agent) return
    const model = persist(automation, nodesRef.current, edgesRef.current, viewportRef.current)
    const siblings = model.edges.filter((edge) =>
      edge.target === agentId && edge.targetHandle === slot
    ).length
    const entry = SLOT_CATALOG[slot]
    const node = createFlowNode(
      entry.type,
      entry.defaultData(t),
      slotAttachPosition(agent.position, slot, siblings),
    )
    if (!canConnect(node.id, agentId, [...model.nodes, node], model.edges, 'slot', slot)) return
    const nextNodes: AutomationRfNode[] = [...nodesRef.current, {
      id: node.id,
      type: node.type,
      position: node.position,
      data: node.data,
    }]
    const nextEdges = [...edgesRef.current, withEdgeChrome(connectSlotEdge(node.id, agentId, slot))]
    setNodes(nextNodes)
    setEdges(nextEdges)
    setSelectedId(node.id)
    commit(nextNodes, nextEdges)
  }, [automation, commit, setEdges, setNodes, t])

  const canvasChrome = useMemo((): CanvasChrome => {
    const occupied = new Map<string, Set<string>>()
    const occupiedSlots = new Map<string, Set<AgentSlot>>()
    for (const edge of edges) {
      if (isSlotEdge(edge)) {
        const slots = occupiedSlots.get(edge.target) ?? new Set<AgentSlot>()
        if (AGENT_SLOTS.includes(edge.targetHandle as AgentSlot)) {
          slots.add(edge.targetHandle as AgentSlot)
        }
        occupiedSlots.set(edge.target, slots)
        continue
      }
      const handles = occupied.get(edge.source) ?? new Set<string>()
      handles.add(edge.sourceHandle ?? '')
      occupied.set(edge.source, handles)
    }
    return {
      occupied,
      occupiedSlots,
      running,
      onAddNext: (nodeId, sourceHandle) => {
        pendingAddRef.current = { kind: 'after', nodeId, handle: sourceHandle }
        setSelectedId(nodeId)
        setPicker('action')
      },
      onRunTo: (nodeId) => void runGraph(nodeId),
      onToggleDisabled: toggleNodeDisabled,
      onDeleteNode: deleteNode,
      onInsertOnEdge: (edgeId) => {
        pendingAddRef.current = { kind: 'edge', edgeId }
        setPicker('action')
      },
      onDeleteEdge: deleteEdge,
      onAddAgentSlot: addAgentSlot,
    }
  }, [addAgentSlot, deleteEdge, deleteNode, edges, runGraph, running, toggleNodeDisabled])

  // n8n 签名交互：从输出口拖线放到空白处，弹出节点选择器并在落点接上新节点。
  const onConnectEnd: OnConnectEnd = useCallback((event, connectionState) => {
    if (connectionState.isValid) return
    if (connectionState.toNode) return
    const fromNode = connectionState.fromNode
    if (!fromNode || connectionState.fromHandle?.type !== 'source') return
    if (isAttachmentType(fromNode.type ?? '')) return
    const point = 'changedTouches' in event
      ? event.changedTouches[0]
      : event
    pendingAddRef.current = {
      kind: 'after',
      nodeId: fromNode.id,
      handle: connectionState.fromHandle?.id ?? undefined,
      at: screenToFlowPosition({ x: point.clientX, y: point.clientY }),
    }
    setSelectedId(fromNode.id)
    setPicker('action')
  }, [screenToFlowPosition])

  const exportJson = useCallback(async () => {
    if (!isTauriRuntime()) return
    try {
      await onFlushSave()
      const base = (automation.name.trim() || 'automation').replace(/[\\/:*?"<>|]/g, '-')
      const path = await saveDialog({
        defaultPath: `${base}.json`,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      })
      if (!path) return
      await automationApi.exportToFile(automation.id, path)
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      window.alert(`${t.chatAutomationExportFailed}${message}`)
    }
  }, [automation.id, automation.name, onFlushSave, t])

  const defaultViewport = useMemo(
    () => ({ x: automation.viewport.x, y: automation.viewport.y, zoom: automation.viewport.zoom || 1 }),
    [automation.viewport.x, automation.viewport.y, automation.viewport.zoom],
  )

  const originLabel = (origin: string) => {
    if (origin === 'manual') return t.chatAutomationTriggerManual
    if (origin === 'schedule') return t.chatAutomationTriggerSchedule
    if (origin === 'hotkey') return t.chatAutomationTriggerHotkey
    return origin
  }
  const statusLabel = (status: string) => {
    if (status === 'success') return t.chatAutomationStatusSuccess
    if (status === 'error') return t.chatAutomationStatusError
    if (status === 'running') return t.chatAutomationStatusRunning
    if (status === 'cancelled') return t.chatAutomationCancelled
    return status
  }
  const formatRunTime = (iso: string) => {
    const date = new Date(iso)
    if (Number.isNaN(date.getTime())) return iso.replace('T', ' ').replace('Z', '')
    return date.toLocaleString(undefined, {
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    })
  }

  const openPicker = (kind: 'trigger' | 'action') => {
    pendingAddRef.current = null
    setPicker((current) => (current === kind ? null : kind))
  }

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
            onChange(persist(
              { ...automation, name: event.target.value },
              nodesRef.current,
              edgesRef.current,
              viewportRef.current,
            ))
          }
        />
        {isTauriRuntime() ? (
          <Button variant="ghost" size="sm" onClick={() => void exportJson()}>
            <Download size={14} />
            {t.chatAutomationExport}
          </Button>
        ) : null}
      </header>
      {runError ? (
        <p className="kv-automation-run-error">{runError}</p>
      ) : null}
      <div className="kv-automation-editor-body">
        <div className="kv-automation-canvas">
          <CanvasChromeContext.Provider value={canvasChrome}>
          <NodeRunStatusContext.Provider value={nodeStatus}>
            <ReactFlow
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              edgeTypes={edgeTypes}
              onNodesChange={onNodesChange}
              onEdgesChange={onEdgesChange}
              onConnect={onConnect}
              onConnectEnd={onConnectEnd}
              isValidConnection={isValidConnection}
              onNodeClick={(_, node) => {
                setSelectedId(node.id)
              }}
              onPaneClick={() => {
                setSelectedId(null)
                setPicker(null)
                pendingAddRef.current = null
              }}
              onNodeDragStop={() => commit(nodesRef.current, edgesRef.current)}
              onNodesDelete={(deleted) => {
                const ids = new Set(deleted.map((node) => node.id))
                for (const node of deleted) {
                  if (node.type !== 'action.agent') continue
                  for (const edge of edgesRef.current) {
                    if (edge.target === node.id && isSlotEdge(edge)) ids.add(edge.source)
                  }
                }
                const nextNodes = nodesRef.current.filter((node) => !ids.has(node.id))
                const nextEdges = edgesRef.current.filter(
                  (edge) => !ids.has(edge.source) && !ids.has(edge.target),
                )
                setNodes(nextNodes)
                setEdges(nextEdges)
                commit(nextNodes, nextEdges)
              }}
              onEdgesDelete={(deleted) => {
                const ids = new Set(deleted.map((edge) => edge.id))
                const nextEdges = edgesRef.current.filter((edge) => !ids.has(edge.id))
                setEdges(nextEdges)
                commit(nodesRef.current, nextEdges)
              }}
              defaultViewport={defaultViewport}
              onMoveEnd={(_, viewport) => {
                viewportRef.current = viewport
                onChange(persist(automation, nodesRef.current, edgesRef.current, viewport))
              }}
              fitView={false}
              minZoom={0.5}
              maxZoom={1.6}
              snapToGrid
              snapGrid={[24, 24]}
              deleteKeyCode={['Backspace', 'Delete']}
              connectionLineType={ConnectionLineType.Bezier}
              defaultEdgeOptions={{
                type: 'action',
                style: { strokeWidth: 1.75 },
                markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16 },
              }}
              edgesReconnectable={false}
              reconnectRadius={0}
              proOptions={{ hideAttribution: true }}
              colorMode={document.documentElement.classList.contains('dark') ? 'dark' : 'light'}
            >
              <Background variant={BackgroundVariant.Dots} gap={20} size={1.2} />
              <Controls showInteractive={false} />
            </ReactFlow>
          </NodeRunStatusContext.Provider>
          </CanvasChromeContext.Provider>
          <div className="kv-automation-capsule-wrap">
            <div className="kv-automation-capsule">
              <div className="kv-automation-enable">
                <Toggle
                  checked={automation.enabled}
                  onChange={(enabled) =>
                    onChange(persist(
                      { ...automation, enabled },
                      nodesRef.current,
                      edgesRef.current,
                      viewportRef.current,
                    ))
                  }
                  ariaLabel={t.chatAutomationEnabled}
                />
                <span>{automation.enabled ? t.chatAutomationEnabled : t.chatAutomationDisabled}</span>
              </div>
              <span className="kv-automation-capsule-rule" aria-hidden />
              {running ? (
                <Button size="sm" variant="danger" onClick={() => void cancelRun()}>
                  <Square size={14} />
                  {t.chatAutomationStop}
                </Button>
              ) : (
                <Button
                  size="sm"
                  variant="primary"
                  onClick={() => void runGraph()}
                  disabled={!hasTrigger}
                >
                  <Play size={14} />
                  {t.chatAutomationExecute}
                </Button>
              )}
              <Button
                size="sm"
                onClick={() => openPicker(hasTrigger ? 'action' : 'trigger')}
              >
                <Plus size={14} />
                {hasTrigger ? t.chatAutomationAddStep : t.chatAutomationAddTrigger}
              </Button>
            </div>
            {picker ? (
              <AddNodePicker
                kind={picker}
                onPick={addFromCatalog}
                onCancel={() => {
                  pendingAddRef.current = null
                  setPicker(null)
                }}
              />
            ) : null}
          </div>
          {nodes.length === 0 && picker !== 'trigger' ? (
            <div className="kv-automation-empty">
              <p>{t.chatAutomationEmptyCanvas}</p>
            </div>
          ) : null}
        </div>
        <div className="kv-automation-side">
          {selected ? (
            <NodeInspector
              node={selected}
              onChange={(next) => {
                setSelectedId(next.id)
                patchSelected(next)
              }}
              onExecuteStep={() => void runGraph(selected.id)}
              running={running}
              lastOutput={nodeOutput[selected.id]}
            />
          ) : (
            <aside className="kv-automation-inspector">
              <p className="kv-automation-inspector-empty">{t.chatAutomationInspectorEmpty}</p>
            </aside>
          )}
          <div className="kv-automation-runs">
            <div className="kv-automation-runs-head">{t.chatAutomationExecutions}</div>
            {runs.length === 0 ? (
              <p className="kv-automation-inspector-note">{t.chatAutomationExecutionsEmpty}</p>
            ) : (
              <ul>
                {runs.slice(0, 12).map((run) => (
                  <li key={run.id} className={`is-${run.status}`}>
                    <span className="kv-automation-run-status">{statusLabel(run.status)}</span>
                    <span className="kv-automation-run-origin">{originLabel(run.origin)}</span>
                    <time>{formatRunTime(run.startedAt)}</time>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}

export function AutomationEditor(props: {
  automation: Automation
  onChange: (next: Automation) => void
  onBack: () => void
  onFlushSave: () => Promise<void>
}) {
  return (
    <ReactFlowProvider>
      <EditorInner {...props} />
    </ReactFlowProvider>
  )
}
