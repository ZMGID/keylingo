import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Background,
  BackgroundVariant,
  ConnectionLineType,
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
import { ArrowLeft, Play, Plus, Square } from 'lucide-react'
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
  triggerNode,
} from './graph'
import { type NodeCatalogEntry } from './nodeCatalog'
import {
  FlowNode,
  NodeChromeContext,
  NodeRunStatusContext,
  type AutomationRfNode,
  type NodeChrome,
} from './nodes/FlowNode'
import {
  isTriggerType,
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
  const addAfterRef = useRef<{ nodeId: string, handle?: string } | null>(null)
  const nodesRef = useRef(nodes)
  const edgesRef = useRef(edges)
  nodesRef.current = nodes
  edgesRef.current = edges

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
        const status: NodeRunStatus = event.status === 'error' ? 'error' : 'success'
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
    if (!canConnect(connection.source, connection.target, model.nodes, model.edges, connection.sourceHandle)) {
      return
    }
    const edge = connectNodes(connection.source, connection.target, connection.sourceHandle)
    const nextEdges = addEdge({ ...connection, id: edge.id }, currentEdges)
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
    )
  }, [automation, edges, nodes])

  const addFromCatalog = useCallback((entry: NodeCatalogEntry) => {
    const currentNodes = nodesRef.current
    const currentEdges = edgesRef.current
    const model = persist(automation, currentNodes, currentEdges, viewportRef.current)
    if (entry.kind === 'trigger' && triggerNode(model)) {
      setPicker(null)
      addAfterRef.current = null
      return
    }
    const after = addAfterRef.current
    addAfterRef.current = null
    const node = createFlowNode(
      entry.type,
      entry.defaultData(t),
      nextNodePosition(model.nodes, after?.nodeId, after?.handle),
    )
    const nextNodes: AutomationRfNode[] = [...currentNodes, {
      id: node.id,
      type: node.type,
      position: node.position,
      data: node.data,
    }]
    setNodes(nextNodes)
    setSelectedId(node.id)
    setPicker(null)
    commit(nextNodes, currentEdges)
  }, [automation, commit, setNodes, t])

  const patchSelected = useCallback((next: FlowNodeModel) => {
    const nextNodes = nodesRef.current.map((node) =>
      node.id === next.id ? { ...node, data: next.data, type: next.type } : node,
    )
    setNodes(nextNodes)
    commit(nextNodes, edgesRef.current)
  }, [commit, setNodes])

  const nodeChrome = useMemo((): NodeChrome => {
    const occupied = new Map<string, Set<string>>()
    for (const edge of edges) {
      const handles = occupied.get(edge.source) ?? new Set<string>()
      handles.add(edge.sourceHandle ?? '')
      occupied.set(edge.source, handles)
    }
    return {
      occupied,
      onAddNext: (nodeId, sourceHandle) => {
        addAfterRef.current = { nodeId, handle: sourceHandle }
        setSelectedId(nodeId)
        setPicker('action')
      },
    }
  }, [edges])

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
    addAfterRef.current = null
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
      </header>
      {runError ? (
        <p className="kv-automation-run-error">{runError}</p>
      ) : null}
      <div className="kv-automation-editor-body">
        <div className="kv-automation-canvas">
          <NodeChromeContext.Provider value={nodeChrome}>
          <NodeRunStatusContext.Provider value={nodeStatus}>
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
                addAfterRef.current = null
              }}
              onNodeDragStop={() => commit(nodesRef.current, edgesRef.current)}
              onNodesDelete={(deleted) => {
                const ids = new Set(deleted.map((node) => node.id))
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
              defaultEdgeOptions={{ type: 'default', style: { strokeWidth: 1.75 } }}
              edgesReconnectable={false}
              reconnectRadius={0}
              proOptions={{ hideAttribution: true }}
              colorMode={document.documentElement.classList.contains('dark') ? 'dark' : 'light'}
            >
              <Background variant={BackgroundVariant.Dots} gap={20} size={1.2} />
              <Controls showInteractive={false} />
            </ReactFlow>
          </NodeRunStatusContext.Provider>
          </NodeChromeContext.Provider>
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
                  addAfterRef.current = null
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
