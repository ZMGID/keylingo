import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { HooksTab } from './HooksTab'
import { i18n } from '../i18n'
import type { HookDef } from '../../api/tauri'

function hook(overrides: Partial<HookDef> = {}): HookDef {
  return {
    id: 'h1',
    name: 'log-end',
    description: '写日志',
    event: 'agent_end',
    enabled: true,
    type: 'command',
    script: 'echo done >> /tmp/kivio-hook.log',
    url: '',
    method: 'POST',
    headers: {},
    timeoutMs: 60_000,
    ...overrides,
  }
}

function renderTab(hooks: HookDef[] = []) {
  const onChange = vi.fn()
  render(<HooksTab lang="zh" hooks={hooks} onChange={onChange} />)
  return { onChange }
}

/** 新建流程：先在事件选择弹窗里挑一个事件，再填表单。 */
async function startAdding(eventLabel: string) {
  await userEvent.click(screen.getByRole('button', { name: /新建 Hook/ }))
  await userEvent.click(screen.getByRole('button', { name: new RegExp(eventLabel) }))
}

describe('HooksTab', () => {
  it('空态不列任何事件（事件在新建时才选）', () => {
    renderTab()
    expect(screen.getByText(i18n.zh.hooksEmptyTitle)).toBeTruthy()
    // 8 个事件不再常驻页面——这是这次改版的要点，回归时会红。
    expect(screen.queryByText('工具执行前')).toBeNull()
  })

  it('所有 Hook 一次列全，不需要切换事件', () => {
    renderTab([hook(), hook({ id: 'h2', name: 'lint', event: 'tool_execution_start' })])
    expect(screen.getByText('log-end')).toBeTruthy()
    expect(screen.getByText('lint')).toBeTruthy()
  })

  it('每行标出触发时机', () => {
    renderTab([hook()])
    expect(screen.getByText('对话结束')).toBeTruthy()
  })

  it('按生命周期顺序排，而非创建顺序', () => {
    // 传入顺序是 agent_end 在前，渲染应把 tool_execution_start 排到它前面。
    renderTab([
      hook({ id: 'late', name: 'zzz-end', event: 'agent_end' }),
      hook({ id: 'early', name: 'aaa-tool', event: 'tool_execution_start' }),
    ])
    const names = screen.getAllByText(/zzz-end|aaa-tool/).map((el) => el.textContent?.trim())
    expect(names[0]).toContain('aaa-tool')
  })

  it('切换启停回调带更新后的整表', async () => {
    const { onChange } = renderTab([hook()])
    await userEvent.click(screen.getByRole('switch', { name: 'log-end' }))
    expect(onChange).toHaveBeenCalledWith([expect.objectContaining({ id: 'h1', enabled: false })])
  })

  it('新建 Hook 绑定到选中的事件', async () => {
    const { onChange } = renderTab()
    await startAdding('工具执行前')
    await userEvent.type(screen.getByPlaceholderText('lint-guard'), 'probe')
    await userEvent.type(screen.getByPlaceholderText(/kivio-hook\.log/), 'true')
    await userEvent.click(screen.getByRole('button', { name: '保存' }))
    expect(onChange).toHaveBeenCalledWith([
      expect.objectContaining({
        name: 'probe',
        event: 'tool_execution_start',
        type: 'command',
        script: 'true',
        enabled: true,
      }),
    ])
  })

  it('删除需二次确认，确认后回调去掉该条', async () => {
    const { onChange } = renderTab([hook()])
    await userEvent.click(screen.getByRole('button', { name: '删除该 Hook？' }))
    expect(onChange).not.toHaveBeenCalled()
    // 弹窗里的确认按钮与图标按钮同名，取最后一个（弹窗后渲染）。
    const confirms = screen.getAllByRole('button', { name: '删除该 Hook？' })
    await userEvent.click(confirms[confirms.length - 1])
    expect(onChange).toHaveBeenCalledWith([])
  })

  it('缺少名称时不保存，显示校验提示', async () => {
    const { onChange } = renderTab()
    await startAdding('对话结束')
    await userEvent.click(screen.getByRole('button', { name: '保存' }))
    expect(onChange).not.toHaveBeenCalled()
    expect(screen.getByText('请填写名称。')).toBeTruthy()
  })

  it('http 类型改用 URL 校验', async () => {
    const { onChange } = renderTab()
    await startAdding('对话结束')
    await userEvent.type(screen.getByPlaceholderText('lint-guard'), 'webhook')
    await userEvent.click(screen.getByRole('button', { name: 'HTTP 请求' }))
    await userEvent.click(screen.getByRole('button', { name: '保存' }))
    expect(screen.getByText('请填写 URL。')).toBeTruthy()
    await userEvent.type(screen.getByPlaceholderText('https://example.com/hook'), 'https://example.com/h')
    await userEvent.click(screen.getByRole('button', { name: '保存' }))
    expect(onChange).toHaveBeenCalledWith([
      expect.objectContaining({ type: 'http', url: 'https://example.com/h', method: 'POST' }),
    ])
  })

  it('有说明时列表优先显示说明', () => {
    renderTab([hook({ description: '写审计日志' })])
    expect(screen.getByText('写审计日志')).toBeTruthy()
    expect(screen.queryByText('echo done >> /tmp/kivio-hook.log')).toBeNull()
  })

  it('编辑时可改触发事件', async () => {
    const { onChange } = renderTab([hook()])
    await userEvent.click(screen.getByRole('button', { name: '编辑 Hook' }))
    await userEvent.click(screen.getByRole('button', { name: '对话结束' }))
    await userEvent.click(screen.getByRole('option', { name: '工具执行前' }))
    await userEvent.click(screen.getByRole('button', { name: '保存' }))
    expect(onChange).toHaveBeenCalledWith([
      expect.objectContaining({ id: 'h1', event: 'tool_execution_start' }),
    ])
  })
})
