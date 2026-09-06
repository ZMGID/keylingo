import { isAttachmentType, isTriggerType, type Automation, type FlowNode, type ValidationIssue } from './types'
import { isSlotEdge } from './agentModel'

export function upstreamNodes(graph: Automation, selectedId: string): FlowNode[] {
  const owner = graph.edges.find((edge) => edge.source === selectedId && isSlotEdge(edge))?.target ?? selectedId
  const seen = new Set<string>([owner])
  const pending = [owner]
  while (pending.length) {
    const id = pending.pop()!
    for (const edge of graph.edges) {
      if (edge.target !== id || isSlotEdge(edge) || seen.has(edge.source)) continue
      seen.add(edge.source)
      pending.push(edge.source)
    }
  }
  return graph.nodes.filter((node) => node.id !== owner && seen.has(node.id))
}

export function dataFields(value: unknown, pointer = '', depth = 0): { pointer: string; value: unknown }[] {
  const fields = [{ pointer, value }]
  if (depth >= 8 || !value || typeof value !== 'object') return fields
  for (const [key, child] of Object.entries(value).slice(0, 100)) {
    fields.push(...dataFields(child, `${pointer}/${key.replace(/~/g, '~0').replace(/\//g, '~1')}`, depth + 1))
    if (fields.length >= 300) break
  }
  return fields.slice(0, 300)
}

export function templateFields(node: FlowNode): { path: string; value: string }[] {
  const fields: Record<string, string[]> = {
    'action.http': ['http.url', 'http.headers', ...(node.data.http?.method && node.data.http.method !== 'GET' ? ['http.body'] : [])],
    'action.notify': ['notify.body'], 'action.file': ['file.path', ...(node.data.file?.op !== 'read' ? ['file.content'] : [])],
    'action.command': ['command.command', 'command.cwd'], 'action.clipboard': node.data.clipboard?.op === 'read' ? [] : ['clipboard.text'],
    'action.agent': ['agent.prompt'], 'agent.context': ['agent.prompt'], 'logic.if': ['if.value'],
    'action.set': (node.data.set?.fields ?? []).map((_, index) => `set.fields.${index}.value`),
    'logic.switch': (node.data.switch?.cases ?? []).map((_, index) => `switch.cases.${index}.value`),
  }
  return (fields[node.type] ?? []).map((path) => ({ path, value: String(readPath(node.data, path) ?? '') }))
}

function readPath(value: unknown, path: string): unknown {
  return path.split('.').reduce<unknown>((current, key) => current && typeof current === 'object'
    ? (current as Record<string, unknown>)[key] : undefined, value)
}

export function insertReference(node: FlowNode, path: string, token: string): FlowNode {
  const data = structuredClone(node.data)
  const parts = path.split('.')
  let current: Record<string, unknown> = data
  for (const key of parts.slice(0, -1)) {
    if (!current[key] || typeof current[key] !== 'object') current[key] = {}
    current = current[key] as Record<string, unknown>
  }
  const key = parts.at(-1)!
  current[key] = `${current[key] ?? ''}${token}`
  return { ...node, data }
}

export function workflowIssues(graph: Automation, english = false): ValidationIssue[] {
  const issues: ValidationIssue[] = []
  const say = (zh: string, en: string) => english ? en : zh
  if (!graph.nodes.some((node) => isTriggerType(node.type) && !node.data.disabled))
    issues.push({ severity: 'error', message: say('添加并启用一个触发器', 'Add and enable a trigger') })
  for (const node of graph.nodes) {
    if (node.data.disabled) continue
    const issue = (message: string, severity = 'error') => issues.push({ nodeId: node.id, severity, message })
    const required: Record<string, [string, string]> = {
      'action.http': ['http.url', say('填写请求地址', 'Enter a request URL')],
      'action.file': ['file.path', say('填写文件路径', 'Enter a file path')],
      'action.command': ['command.command', say('填写要执行的命令', 'Enter a command')],
      'agent.context': ['agent.prompt', say('填写 Agent 提示词', 'Enter an Agent prompt')],
    }
    const rule = required[node.type]
    if (rule && !String(readPath(node.data, rule[0]) ?? '').trim()) issue(rule[1])
    if (!isTriggerType(node.type) && !isAttachmentType(node.type)
      && !graph.edges.some((edge) => edge.target === node.id && !isSlotEdge(edge)))
      issue(say('尚未连接输入，整体运行不会执行此节点', 'No input connection; full runs will skip this node'), 'warning')
    const ancestors = new Set(upstreamNodes(graph, node.id).map((item) => item.id))
    for (const field of templateFields(node)) {
      for (const match of field.value.matchAll(/\{\{nodes\.([^#}]+)#([^}]+)\}\}/g)) {
        if (!ancestors.has(match[1])) issue(say(`${field.path} 引用了不可用的上游节点`, `${field.path} references an unavailable upstream node`))
      }
    }
  }
  return issues
}
