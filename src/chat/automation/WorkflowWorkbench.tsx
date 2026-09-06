import { useState, type ReactNode } from 'react'
import { useLang } from '../../settings/i18n'
import { Button } from '../../components/Button'
import { Select } from '../../settings/components'
import { dataFields, insertReference, templateFields, upstreamNodes } from './workflowData'
import { isAttachmentType, isStepType, type Automation, type AutomationRun, type FlowNode, type NodeOutput, type ValidationIssue } from './types'

export function WorkflowWorkbench({ graph, node, run, running, issues, onChange, onTest, children }: {
  graph: Automation; node: FlowNode; run: AutomationRun | null; running: boolean; issues: ValidationIssue[]
  onChange: (node: FlowNode) => void; onTest: (input: NodeOutput) => Promise<void>; children: ReactNode
}) {
  const english = useLang() === 'en'
  const say = (zh: string, en: string) => english ? en : zh
  const [tab, setTab] = useState('params')
  const [inputMode, setInputMode] = useState('cache')
  const [text, setText] = useState('')
  const [json, setJson] = useState('{}')
  const [error, setError] = useState('')
  const [target, setTarget] = useState('')
  const [pending, setPending] = useState(false)
  const ownerId = isAttachmentType(node.type)
    ? graph.edges.find((edge) => edge.source === node.id)?.target ?? node.id : node.id
  const record = run?.nodes.find((item) => item.nodeId === ownerId)
  const contextId = node.type === 'action.agent' ? graph.edges.find((edge) => edge.target === node.id && edge.targetHandle === 'context')?.source : undefined
  const parameterNode = graph.nodes.find((item) => item.id === contextId) ?? node
  const fields = templateFields(parameterNode)
  const targetPath = fields.some((field) => field.path === target) ? target : fields[0]?.path
  const fieldLabel = (path: string) => {
    const labels: Record<string, string> = {
      'http.url': say('请求地址', 'Request URL'), 'http.headers': say('请求头', 'Headers'), 'http.body': say('请求正文', 'Request body'),
      'notify.body': say('通知内容', 'Notification body'), 'file.path': say('文件路径', 'File path'), 'file.content': say('文件内容', 'File content'),
      'command.command': say('执行命令', 'Command'), 'command.cwd': say('工作目录', 'Working directory'), 'clipboard.text': say('剪贴板文本', 'Clipboard text'),
      'agent.prompt': say('Agent 提示词', 'Agent prompt'), 'if.value': say('比较值', 'Comparison value'),
    }
    const index = path.match(/\.(\d+)\.value$/)?.[1]
    return labels[path] ?? (index ? `${say('第', 'Value ')}${Number(index) + 1}${say('项的值', '')}` : path)
  }
  const ancestors = upstreamNodes(graph, node.id)
  const cached = record?.input
  const test = async () => {
    setError('')
    try {
      const input = inputMode === 'cache' ? cached : { text, json: JSON.parse(json) as unknown }
      if (!input) throw new Error(say('没有缓存输入，请填写测试数据。', 'No cached input. Enter test data.'))
      setPending(true)
      await onTest(input)
      setTab('output')
    } catch (err) { setError(err instanceof Error ? err.message : String(err)) }
    finally { setPending(false) }
  }
  return <div className="kv-workbench">
    <div className="kv-seg kv-workbench-tabs" role="tablist" aria-label={say('节点工作台', 'Node workbench')}>
      {(['params', 'input', 'output'] as const).map((value, index) => <button key={value} type="button" role="tab"
        className={tab === value ? 'active' : ''} aria-selected={tab === value} onClick={() => setTab(value)}>
        {[say('参数', 'Parameters'), say('输入 / 测试', 'Input / Test'), say('输出', 'Output')][index]}
      </button>)}
    </div>
    <div className="kv-workbench-content custom-scrollbar" role="tabpanel">
      {issues.length > 0 && <ul className="kv-workbench-issues" aria-label={say('配置问题', 'Configuration issues')}>
        {issues.map((issue, index) => <li key={index} data-severity={issue.severity}>{issue.message}</li>)}
      </ul>}
      {tab === 'params' && <>
        {fields.length > 0 && <details className="kv-workbench-data" open>
          <summary>{say('插入上游数据', 'Insert upstream data')}</summary>
          <label>{say('插入到参数末尾', 'Append to parameter')}</label>
          <Select
            value={targetPath ?? ''}
            onChange={setTarget}
            ariaLabel={say('插入到参数末尾', 'Append to parameter')}
            options={fields.map((field) => ({ value: field.path, label: fieldLabel(field.path) }))}
          />
          {targetPath && <details><summary>{say('当前参数内容', 'Current parameter value')}</summary><pre>{fields.find((field) => field.path === targetPath)?.value || '—'}</pre></details>}
          {ancestors.length === 0 && <p>{say('先连接上游节点，再选择数据。', 'Connect an upstream node to select its data.')}</p>}
          {ancestors.map((source) => {
            const result = run?.nodes.find((item) => item.nodeId === source.id)?.result ?? cached?.sources?.[source.id]
            const data = result ? { text: result.text, json: result.json } : null
            const options = data ? dataFields(data).filter((field) => field.pointer) : [{ pointer: '/text', value: undefined }, { pointer: '/json', value: undefined }]
            return <details key={source.id}>
              <summary>{source.data.label || source.type} {!result && say('（尚无样本）', '(no sample)')}</summary>
              {options.map((field) => <button type="button" className="kv-workbench-field" key={field.pointer}
                title={say('点击插入引用', 'Click to insert reference')}
                onClick={() => targetPath && onChange(insertReference(parameterNode, targetPath, `{{nodes.${source.id}#${field.pointer}}}`))}>
                <code>{field.pointer}</code><small>{field.value === undefined ? '—' : JSON.stringify(field.value)?.slice(0, 100)}</small>
              </button>)}
            </details>
          })}
        </details>}
        {children}
      </>}
      {tab === 'input' && <div className="kv-workbench-data">
        <h3>{say('节点实际输入', 'Recorded input')}</h3>
        {cached ? <DataTree value={{ text: cached.text, json: cached.json }} /> : <p>{say('尚无输入记录，或记录超过保存上限。', 'No input recorded, or snapshot exceeded the storage limit.')}</p>}
        {isStepType(node.type) && <>
          <h3>{say('单节点测试', 'Test this node')}</h3>
          <p>{say('仅执行当前节点。文件写入、命令和通知等操作会真实执行。', 'Runs only this node. Writes, commands and notifications execute normally.')}</p>
          <label>{say('测试输入', 'Test input')}</label>
          <Select
            value={inputMode}
            onChange={setInputMode}
            ariaLabel={say('测试输入', 'Test input')}
            options={[
              { value: 'cache', label: say('使用本次记录的输入', 'Use recorded input') },
              { value: 'custom', label: say('填写测试数据', 'Enter test data') },
            ]}
          />
          {inputMode === 'custom' && <>
            <label>{say('文本输入', 'Text input')}<textarea className="kv-textarea custom-scrollbar" rows={4} value={text} onChange={(event) => setText(event.target.value)} /></label>
            <label>JSON<textarea className="kv-textarea custom-scrollbar" rows={6} value={json} onChange={(event) => setJson(event.target.value)} /></label>
            <p>{say('自定义输入支持 {{output}} 和 {{json.*}}；跨节点引用请使用缓存输入。', 'Custom input supports {{output}} and {{json.*}}; use recorded input for node references.')}</p>
          </>}
          <Button onClick={() => void test()} disabled={running || pending || !!node.data.disabled || (inputMode === 'cache' && !cached)}>{say('测试当前节点', 'Test this node')}</Button>
          {error && <p role="alert">{error}</p>}
        </>}
      </div>}
      {tab === 'output' && <div className="kv-workbench-data">
        <h3>{say('运行输出', 'Run output')}</h3>
        {run && <p>{run.origin === 'test' ? say('单节点测试', 'Node test') : say('流程运行', 'Workflow run')} · {run.startedAt}</p>}
        {record?.error && <p role="alert">{record.error}</p>}
        {record?.result ? <DataTree value={{ text: record.result.text, json: record.result.json }} /> :
          <p>{record?.output ?? say('尚无输出，或完整记录超过保存上限。', 'No output yet, or full snapshot exceeded the storage limit.')}</p>}
      </div>}
    </div>
  </div>
}

function DataTree({ value, depth = 0 }: { value: unknown; depth?: number }) {
  if (!value || typeof value !== 'object' || depth >= 8) return <pre>{JSON.stringify(value, null, 2)?.slice(0, 10000)}</pre>
  const entries = Object.entries(value)
  return <div className="kv-data-tree">{entries.slice(0, 100).map(([key, child]) =>
    <details key={key} open={depth < 1}><summary>{key} <small>{Array.isArray(child) ? `[${child.length}]` : typeof child}</small></summary><DataTree value={child} depth={depth + 1} /></details>)}
    {entries.length > 100 && <p>… +{entries.length - 100}</p>}
  </div>
}
