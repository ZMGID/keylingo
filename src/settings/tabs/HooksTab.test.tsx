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

describe('HooksTab', () => {
  it('渲染 8 个生命周期事件，默认选中 agent_start', () => {
    renderTab()
    // 8 个事件都在导轨里；默认选中第一个（对话开始）。
    expect(screen.getAllByText('对话开始')).toHaveLength(2) // 导轨 + 右侧标题
    expect(screen.getByText('工具执行前')).toBeTruthy()
    // 描述取 i18n 而非硬编码文案：改文案不该让这条测试失败（它验的是「选中事件的描述被渲染」）。
    expect(screen.getByText(i18n.zh.hookEventDescAgentStart)).toBeTruthy()
  })

  it('未选中事件的 Hook 不显示；切换事件后显示', async () => {
    renderTab([hook()])
    // agent_start 选中态下看不到 agent_end 的 Hook。
    expect(screen.queryByText('log-end')).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: /对话结束/ }))
    expect(screen.getByText('log-end')).toBeTruthy()
  })

  it('事件点标出该事件的 Hook 数', () => {
    renderTab([hook(), hook({ id: 'h2', name: 'second' })])
    expect(screen.getAllByText('2').length).toBeGreaterThan(0)
  })

  it('切换启停回调带更新后的整表', async () => {
    const { onChange } = renderTab([hook()])
    await userEvent.click(screen.getByRole('button', { name: /对话结束/ }))
    await userEvent.click(screen.getByRole('switch', { name: 'log-end' }))
    expect(onChange).toHaveBeenCalledWith([expect.objectContaining({ id: 'h1', enabled: false })])
  })

  it('新建 Hook 保存后回调带绑定当前事件的新条目', async () => {
    const { onChange } = renderTab()
    await userEvent.click(screen.getAllByRole('button', { name: /新建 Hook/ })[0])
    await userEvent.type(screen.getByPlaceholderText('lint-guard'), 'probe')
    await userEvent.type(screen.getByPlaceholderText('echo done >> /tmp/kivio-hook.log'), 'true')
    await userEvent.click(screen.getByRole('button', { name: '保存' }))
    expect(onChange).toHaveBeenCalledWith([
      expect.objectContaining({ name: 'probe', event: 'agent_start', type: 'command', script: 'true', enabled: true }),
    ])
  })

  it('删除需二次确认，确认后回调去掉该条', async () => {
    const { onChange } = renderTab([hook()])
    await userEvent.click(screen.getByRole('button', { name: /对话结束/ }))
    await userEvent.click(screen.getByRole('button', { name: '删除该 Hook？' }))
    expect(onChange).not.toHaveBeenCalled()
    // 弹窗里的确认按钮与图标按钮同名，取最后一个（弹窗后渲染）。
    const confirms = screen.getAllByRole('button', { name: '删除该 Hook？' })
    await userEvent.click(confirms[confirms.length - 1])
    expect(onChange).toHaveBeenCalledWith([])
  })

  it('缺少名称时不保存，显示校验提示', async () => {
    const { onChange } = renderTab()
    await userEvent.click(screen.getAllByRole('button', { name: /新建 Hook/ })[0])
    await userEvent.click(screen.getByRole('button', { name: '保存' }))
    expect(onChange).not.toHaveBeenCalled()
    expect(screen.getByText('请填写名称。')).toBeTruthy()
  })

  it('http 类型改用 URL 校验', async () => {
    const { onChange } = renderTab()
    await userEvent.click(screen.getAllByRole('button', { name: /新建 Hook/ })[0])
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
})
