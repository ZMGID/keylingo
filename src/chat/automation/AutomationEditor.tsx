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
  applyNodeChanges,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Connection,
  type Edge,
  type OnConnect,
  type OnConnectEnd,
  type OnNodesChange,
  type Viewport,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import './automation.css'
import { save as saveDialog } from '@tauri-apps/plugin-dialog'
import { ArrowLeft, Download, Play, Plus, Square } from 'lucide-react'
import { isTauriRuntime } from '../../api/tauri'
import { Button } from '../../components/Button'
import { Toggle } from '../../settings/components'
import { useT, useLang } from '../../settings/i18n'
import { WorkflowWorkbench } from './WorkflowWorkbench'
import { workflowIssues } from './workflowData'
import { ValidationContext } from './nodes/chrome'
import type { ValidationIssue, NodeOutput } from './types'
import { AddNodePicker } from './AddNodePicker'
import { automationApi } from './api'
import { NodeInspector } from './NodeInspector'
import { RunStatusCapsule } from './RunStatusCapsule'
import { useAutomationRunState } from './useAutomationRunState'
import {
  canConnect,
  connectNodes,
  createFlowNode,
  ensureNodeSpacing,
  flowEdgeFromConnection,
  nextNodePosition,
  nextTriggerPosition,
  pickAppendSource,
  pruneDanglingBranchEdges,
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
  isTriggerType,
  branchHandles,
  type AgentSlot,
  type Automation,
  type AutomationNodeType,
  type FlowNode as FlowNodeModel,
} from './types'

const nodeTypes = {
  'trigger.manual': FlowNode,
  'trigger.schedule': FlowNode,
  'trigger.hotkey': FlowNode,
  'action.agent': FlowNode,
  'action.notify': FlowNode,
  'action.http': FlowNode,
  'action.set': FlowNode,
  'action.clipboard': FlowNode,
  'action.file': FlowNode,
  'action.command': FlowNode,
  'logic.if': FlowNode,
  'logic.switch': FlowNode,
  'logic.delay': FlowNode,
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
  return ensureNodeSpacing(nodes).map((node) => ({
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
  const [nodes, setNodes] = useNodesState<AutomationRfNode>(toRfNodes(automation.nodes))
  const [edges, setEdges, onEdgesChange] = useEdgesState(toRfEdges(automation))
  const [selectedId, setSelectedId] = useState<string | null>(
    automation.nodes[0]?.id ?? null,
  )
  const [picker, setPicker] = useState<'trigger' | 'action' | null>(null)
  const { running, setRunning, runError, setRunError, nodeStatus, nodeOutput, runs, liveStartedAt, runData } = useAutomationRunState(automation.id)
  const english = useLang() === 'en'
  const localIssues = useMemo(() => workflowIssues(automation, english), [automation, english])
  const [serverIssues, setServerIssues] = useState<ValidationIssue[]>([])
  useEffect(() => {
    let disposed = false
    setServerIssues([])
    if (!isTauriRuntime()) return
    const timer = window.setTimeout(() => {
      void automationApi.validate(automation).then((issues) => {
        if (!disposed) setServerIssues(issues.filter((issue) => !automation.nodes.find((node) => node.id === issue.nodeId)?.data.disabled))
      }).catch(() => {})
    }, 300)
    return () => { disposed = true; window.clearTimeout(timer) }
  }, [automation])
  const issues = useMemo(() => [...localIssues, ...serverIssues.filter((issue) =>
    !localIssues.some((local) => local.nodeId === issue.nodeId && local.severity === issue.severity))], [localIssues, serverIssues])
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

  useEffect(() => {
    const exploded = explodeInlineAgents(automation.nodes, automation.edges)
    const spaced = ensureNodeSpacing(exploded.nodes)
    if (!exploded.changed && spaced === exploded.nodes) return
    setNodes(toRfNodes(spaced))
    setEdges(exploded.edges.map(withEdgeChrome))
    onChange({ ...automation, nodes: spaced, edges: exploded.edges })
    // Only rewrite when this automation is first opened.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [automation.id])

  const commit = useCallback((nextNodes: AutomationRfNode[], nextEdges: Edge[]) => {
    const spaced = ensureNodeSpacing(nextNodes)
    nodesRef.current = spaced
    edgesRef.current = nextEdges
    setNodes(spaced)
    onChange(persist(automation, spaced, nextEdges, viewportRef.current))
  }, [automation, onChange, setNodes])

  const onNodesChange: OnNodesChange<AutomationRfNode> = useCallback((changes) => {
    const moving = new Set(changes.filter((change) => change.type === 'position').map((change) => change.id))
    const next = applyNodeChanges(changes, nodesRef.current)
    const spaced = moving.size ? ensureNodeSpacing(next, moving) : next
    nodesRef.current = spaced
    setNodes(spaced)
  }, [setNodes])

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
    const edge = flowEdgeFromConnection(
      from?.type ?? '',
      connection.source,
      connection.target,
      connection.sourceHandle,
      connection.targetHandle,
    )
    const nextEdges = addEdge(withEdgeChrome(edge), currentEdges)
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
    if (entry.kind === 'trigger') {
      if (model.nodes.some((node) => node.type === entry.type)) {
        setPicker(null)
        pendingAddRef.current = null
        return
      }
      const node = createFlowNode(entry.type, entry.defaultData(t), nextTriggerPosition(model.nodes))
      const nextNodes: AutomationRfNode[] = [...currentNodes, {
        id: node.id,
        type: node.type,
        position: node.position,
        data: node.data,
      }]
      setNodes(nextNodes)
      setEdges(currentEdges)
      setSelectedId(node.id)
      setPicker(null)
      pendingAddRef.current = null
      commit(nextNodes, currentEdges)
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
          withEdgeChrome(connectNodes(node.id, splitEdge.target, branchHandles(node.type, node.data)?.[0])),
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
    if (!intent) {
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
    const nextEdges = pruneDanglingBranchEdges(nextNodes, edgesRef.current)
    setNodes(nextNodes)
    if (nextEdges.length !== edgesRef.current.length) setEdges(nextEdges)
    commit(nextNodes, nextEdges)
  }, [commit, setEdges, setNodes])

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
      const problems = [...workflowIssues(automation, english), ...await automationApi.validate(automation)]
        .filter((issue) => issue.severity === 'error' && !automation.nodes.find((node) => node.id === issue.nodeId)?.data.disabled)
      if (problems.length) {
        if (problems[0].nodeId) setSelectedId(problems[0].nodeId)
        throw new Error(problems[0].message)
      }
      await onFlushSave()
      setRunning(true)
      await automationApi.run(automation.id, untilNodeId)
    } catch (err) {
      setRunning(false)
      setRunError(err instanceof Error ? err.message : String(err))
    }
  }, [automation, english, onFlushSave, setRunError, setRunning])

  const testSelected = async (input: NodeOutput) => {
    if (!isTauriRuntime()) throw new Error(english ? 'Node tests require the desktop app' : '单节点测试需要在桌面应用中执行')
    if (!selected) return
    const problems = issues.filter((issue) => issue.nodeId === selected.id && issue.severity === 'error')
    if (problems.length) throw new Error(problems[0].message)
    await onFlushSave()
    setRunError('')
    setRunning(true)
    try { await automationApi.testNode(automation.id, selected.id, input) }
    catch (err) { setRunning(false); throw err }
  }

  const cancelRun = useCallback(async () => {
    try {
      await automationApi.cancel(automation.id)
    } catch (err) {
      setRunError(err instanceof Error ? err.message : String(err))
    }
  }, [automation.id, setRunError])

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
      <div className="kv-automation-editor-body">
        <div className="kv-automation-canvas">
          <CanvasChromeContext.Provider value={canvasChrome}>
          <ValidationContext.Provider value={issues}>
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
                setPicker(null)
                pendingAddRef.current = null
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
          </ValidationContext.Provider>
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
          </div>
          {nodes.length === 0 ? (
            <div className="kv-automation-empty">
              <p>{t.chatAutomationEmptyCanvas}</p>
            </div>
          ) : null}
          <RunStatusCapsule
            running={running}
            runs={runs}
            error={runError}
            liveStartedAt={liveStartedAt}
            resetKey={automation.id}
          />
        </div>
        <div className="kv-automation-side">
          {issues.length > 0 && <details className="kv-workbench-checks">
            <summary>{english ? 'Configuration checks' : '配置检查'} · {issues.length}</summary>
            {issues.map((issue, index) => <button type="button" key={index} onClick={() => { if (issue.nodeId) { setSelectedId(issue.nodeId); setPicker(null) } }}>
              {automation.nodes.find((node) => node.id === issue.nodeId)?.data.label}: {issue.message}
            </button>)}
          </details>}
          {picker || !selected ? (
            <AddNodePicker
              kind={picker ?? (hasTrigger ? 'action' : 'trigger')}
              presentTypes={nodes.map((node) => node.type ?? '')}
              onPick={addFromCatalog}
              onCancel={picker && selected ? () => {
                pendingAddRef.current = null
                setPicker(null)
              } : undefined}
            />
          ) : (
            <WorkflowWorkbench key={selected.id} graph={automation} node={selected} run={runData} running={running}
              issues={issues.filter((issue) => issue.nodeId === selected.id)} onChange={patchSelected} onTest={testSelected}>
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
            </WorkflowWorkbench>
          )}
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
