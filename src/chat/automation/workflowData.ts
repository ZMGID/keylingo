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
    if (node.type === 'action.agent') {
      const hasPrompt = Boolean(node.data.agent?.prompt?.trim())
      const hasContext = graph.edges.some((edge) =>
        edge.target === node.id && edge.targetHandle === 'context'
      )
      if (!hasPrompt && !hasContext) {
        issue(say(
          '填写 Agent 提示词，或连接一个 Context 节点；否则运行时会失败',
          'Add an Agent prompt or connect a Context node; otherwise the step will fail at run time',
        ), 'warning')
      }
    }
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

/** 后端校验不感知界面语言；所有原始英文消息在进入 UI 前统一翻译。 */
export function localizeValidationIssue(issue: ValidationIssue, english = false): ValidationIssue {
  if (english) return issue
  const message = issue.message
  const exact: Record<string, string> = {
    'automation has no nodes': '自动化中没有节点',
    'a node is missing an id': '有节点缺少 ID',
    'automation needs at least one trigger (trigger.manual, trigger.schedule, or trigger.hotkey)': '至少需要一个触发器',
    'main flow has a cycle; only tree-shaped graphs can run': '主流程存在循环；目前只支持树状流程',
    'schedule.hour must be 0–23': '小时必须在 0–23 之间',
    'schedule.minute must be 0–59': '分钟必须在 0–59 之间',
    'schedule.intervalMinutes must be at least 1': '间隔时间必须至少为 1 分钟',
    'hotkey trigger has an empty accelerator; it will not fire until one is set': '请设置快捷键，否则该触发器不会生效',
    'http.url is empty': '填写请求地址',
    'http.url should start with http:// or https://': '请求地址应以 http:// 或 https:// 开头',
    'command.command is empty': '填写要执行的命令',
    'file.path is empty': '填写文件路径',
    'set.fields is empty; this step will output {}': '字段列表为空；此步骤将输出空对象 {}',
    'switch.cases is empty; every run will take the default handle': '分支条件为空；每次运行都会走默认分支',
    'a switch case is missing id': '有分支条件缺少 ID',
    'delay.seconds must be between 1 and 600': '延迟时间必须在 1–600 秒之间',
    'action.agent has no prompt and no agent.context slot; the step will fail at run time': '填写 Agent 提示词，或连接一个 Context 节点；否则运行时会失败',
  }
  if (exact[message]) return { ...issue, message: exact[message] }

  const patterns: Array<[RegExp, (...groups: string[]) => string]> = [
    [/^duplicate node id '(.+)'$/, (id) => `节点 ID“${id}”重复`],
    [/^unknown node type '(.+)'; allowed: .+$/, (type) => `未知节点类型“${type}”`],
    [/^edge '(.+)' source '(.+)' does not exist$/, (edge, source) => `连线“${edge}”的起点“${source}”不存在`],
    [/^edge '(.+)' target '(.+)' does not exist$/, (edge, target) => `连线“${edge}”的终点“${target}”不存在`],
    [/^node '(.+)' has more than one main-flow input; only a tree is allowed$/, (id) => `节点“${id}”有多个主流程输入；目前只支持树状流程`],
    [/^slot edge '(.+)' can only plug into action\.agent, not '(.+)'$/, (_edge, type) => `能力插槽只能连接 Agent 节点，不能连接“${type}”`],
    [/^slot '(.+)' must come from (.+), not '(.+)'$/, (slot, expected, actual) => `“${slot}”插槽需要 ${expected} 节点，当前为“${actual}”`],
    [/^schedule\.kind must be daily, weekdays, or interval \(got '(.+)'\)$/, (kind) => `未知的定时类型“${kind}”`],
    [/^http\.method must be one of GET\/POST\/PUT\/PATCH\/DELETE \(got '(.+)'\)$/, (method) => `不支持 HTTP 方法“${method}”`],
    [/^file\.op must be read or write \(got '(.+)'\)$/, (op) => `不支持文件操作“${op}”`],
    [/^clipboard\.op must be copy or read \(got '(.+)'\)$/, (op) => `不支持剪贴板操作“${op}”`],
    [/^if\.op must be contains, equals, or notEmpty \(got '(.+)'\)$/, (op) => `不支持判断条件“${op}”`],
    [/^duplicate switch case id '(.+)'$/, (id) => `分支条件 ID“${id}”重复`],
    [/^switch case '(.+)' has invalid op '(.+)'$/, (id, op) => `分支条件“${id}”使用了无效操作“${op}”`],
  ]
  for (const [pattern, format] of patterns) {
    const match = message.match(pattern)
    if (match) return { ...issue, message: format(...match.slice(1)) }
  }
  return { ...issue, message: '配置不完整，请检查当前节点' }
}
