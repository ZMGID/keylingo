import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ProvidersTab } from './ProvidersTab'
import { makeSettings, makeProvider } from './testFixtures'
import { i18n } from '../i18n'

const t = i18n.zh

vi.mock('../../api/tauri', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../api/tauri')
  return { ...actual, api: { openExternal: vi.fn() } }
})

type Props = Parameters<typeof ProvidersTab>[0]

/**
 * 回归重点（拆成 ProvidersTab + ProviderDetail 两个文件后尤其要验）：
 *   1. 选中/未选中供应商的两种形态
 *   2. 密钥池的显隐、增删按 index 作用到正确的那一条
 *   3. 各回调不串（更新 / 删除 / 模型抽屉 / 测试连接）
 */
function buildProps(overrides: Partial<Props> = {}): Props {
  const provider = makeProvider({ apiKeys: ['sk-aaa', 'sk-bbb'] })
  const props: Props = {
    settings: makeSettings({ providers: [provider] }),
    t,
    lang: 'zh',
    selectedProvider: provider,
    revealedKeys: new Set<string>(),
    gzipInfoOpen: new Set<string>(),
    fetchingProviderId: null,
    onSelectProvider: vi.fn(),
    onReorderProviders: vi.fn(),
    onAddProvider: vi.fn(),
    onAddProviderFromPreset: vi.fn(),
    onUpdateProvider: vi.fn(),
    onSetProviderIcon: vi.fn(),
    onRequestDeleteProvider: vi.fn(),
    onToggleGzipInfo: vi.fn(),
    onToggleKeyReveal: vi.fn(),
    onOpenModelPicker: vi.fn(),
    onOpenModelTest: vi.fn(),
    onOpenModelDrawer: vi.fn(),
    onRemoveEnabledModel: vi.fn(),
    ...overrides,
  }
  return props
}

function renderTab(overrides: Partial<Props> = {}) {
  const props = buildProps(overrides)
  render(<ProvidersTab {...props} />)
  return props
}

/** 需要在同一棵树上换 props 的用例（如切换供应商）用它，别重新 render 出第二棵树。 */
function renderTabWithRerender(overrides: Partial<Props> = {}) {
  const props = { ...buildProps(overrides) }
  const view = render(<ProvidersTab {...props} />)
  return { props, rerender: (next: Props) => view.rerender(<ProvidersTab {...next} />) }
}

describe('ProvidersTab', () => {
  it('未选中供应商时显示引导文案且不渲染详情', () => {
    renderTab({ selectedProvider: undefined })
    expect(screen.getByText(/在左侧选择供应商/)).toBeTruthy()
    expect(screen.queryByText(t.baseUrl)).toBeNull()
  })

  it('选中供应商时渲染详情区', () => {
    renderTab()
    expect(screen.queryByText(/在左侧选择供应商/)).toBeNull()
    expect(screen.getByText(t.baseUrl)).toBeTruthy()
    expect(screen.getByDisplayValue('https://api.openai.com/v1')).toBeTruthy()
  })

  it('密钥默认掩码显示，池中每条各一行', () => {
    renderTab()
    // 两条密钥 → 两个 password 输入
    const masked = document.querySelectorAll('input[type="password"]')
    expect(masked).toHaveLength(2)
  })

  it('revealedKeys 只解掩指定那一条', () => {
    renderTab({ revealedKeys: new Set(['p1-0']) })
    expect(document.querySelectorAll('input[type="password"]')).toHaveLength(1)
    expect(screen.getByDisplayValue('sk-aaa')).toBeTruthy()
  })

  it('删除密钥按 index 作用（删第 2 条时保留第 1 条）', async () => {
    const props = renderTab()
    const removes = document.querySelectorAll<HTMLButtonElement>('.kv-icon-btn[aria-label="移除"]')
    await userEvent.click(removes[1])
    expect(props.onUpdateProvider).toHaveBeenCalledWith('p1', { apiKeys: ['sk-aaa'] })
  })

  it('新增密钥追加空串而非覆盖', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: t.addKey }))
    expect(props.onUpdateProvider).toHaveBeenCalledWith('p1', { apiKeys: ['sk-aaa', 'sk-bbb', ''] })
  })

  it('单条密钥时不显示删除按钮', () => {
    renderTab({ selectedProvider: makeProvider({ apiKeys: ['sk-only'] }) })
    expect(document.querySelectorAll('.kv-icon-btn[aria-label="移除"]')).toHaveLength(0)
  })

  it('删除供应商走 onRequestDeleteProvider（不直接删）', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: t.deleteProvider }))
    expect(props.onRequestDeleteProvider).toHaveBeenCalledWith('p1')
    expect(props.onUpdateProvider).not.toHaveBeenCalled()
  })

  it('gzip 开关搬进「请求配置」二级页，一级页看不到', async () => {
    renderTab()
    expect(screen.queryByText(/压缩请求体/)).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.requestConfig) }))
    expect(screen.getByText(/压缩请求体/)).toBeTruthy()
    expect(screen.queryByText(/WAF 会扫描明文请求体/)).toBeNull()
  })

  it('gzipInfoOpen 命中时二级页展开 gzip 说明', async () => {
    // 单独一条：同一个 document 里 render 两棵树再靠 getAllByRole 挑最后一个，
    // 正是本文件 renderTabWithRerender 那条注释要消灭的写法。
    renderTab({ gzipInfoOpen: new Set(['p1']) })
    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.requestConfig) }))
    expect(screen.getByText(/WAF 会扫描明文请求体/)).toBeTruthy()
  })

  it('测试连接与模型管理是两个不同回调', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: t.testConnection }))
    expect(props.onOpenModelTest).toHaveBeenCalledWith('p1')
    expect(props.onOpenModelPicker).not.toHaveBeenCalled()
  })

  it('点击已启用模型打开抽屉，带 providerId + model', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByTitle('gpt-4o'))
    expect(props.onOpenModelDrawer).toHaveBeenCalledWith({ providerId: 'p1', model: 'gpt-4o' })
  })

  it('移除模型不冒泡触发抽屉', async () => {
    const props = renderTab()
    await userEvent.click(document.querySelector<HTMLButtonElement>('.kv-enabled-model-remove')!)
    expect(props.onRemoveEnabledModel).toHaveBeenCalledWith('p1', 'gpt-4o')
    expect(props.onOpenModelDrawer).not.toHaveBeenCalled()
  })

  it('新增供应商按钮走 onAddProvider', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.addProvider) }))
    expect(props.onAddProvider).toHaveBeenCalled()
  })

  it('供应商名称上方有预设入口，左侧不再放快速添加', () => {
    renderTab()
    expect(screen.getByRole('button', { name: new RegExp(t.presetProviders) })).toBeTruthy()
    expect(screen.queryByText(t.presetProvidersHint)).toBeTruthy()
    expect(document.querySelector('.kv-provider-list-presets')).toBeNull()
    expect(document.querySelectorAll('.kv-provider-item')).toHaveLength(1)
  })

  it('点预设入口弹出选择层，点一项走 onAddProviderFromPreset', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.presetProviders) }))
    expect(screen.getByRole('dialog', { name: t.presetProviders })).toBeTruthy()
    const presetRows = [...document.querySelectorAll('.kv-provider-preset-row')]
    expect(presetRows.length).toBeGreaterThan(15)
    expect(presetRows[0]?.textContent).toMatch(/Kimi for Coding/)
    expect(presetRows.at(-2)?.textContent).toMatch(/ModelScope/)
    expect(presetRows.at(-1)?.textContent).toMatch(/GitHub Models/)
    expect(screen.getByRole('button', { name: /GLM Coding Plan/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: /Xiaomi Token Plan/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: /MiniMax Token Plan/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: /OpenCode Go/ })).toBeTruthy()
    expect(screen.queryByText('已添加')).toBeNull()
    expect(screen.getByText(t.baseUrl)).toBeTruthy()

    await userEvent.click(screen.getByRole('button', { name: /DeepSeek/ }))
    expect(props.onAddProviderFromPreset).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'DeepSeek', baseUrl: 'https://api.deepseek.com/v1' }),
    )
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('预设弹层点关闭后详情仍在', async () => {
    renderTab()
    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.presetProviders) }))
    expect(screen.getByRole('dialog')).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: '关闭' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.getByText(t.baseUrl)).toBeTruthy()
  })

  it('「请求配置」入口进二级页，返回后回到一级页', async () => {
    renderTab()
    expect(screen.queryByText(t.customHeaders)).toBeNull()

    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.requestConfig) }))
    expect(screen.getByText(t.customHeaders)).toBeTruthy()
    // 二级页接管右栏：一级页的字段不该还留在 DOM 里。
    expect(screen.queryByText(t.baseUrl)).toBeNull()

    await userEvent.click(screen.getByRole('button', { name: /返回/ }))
    expect(screen.getByText(t.baseUrl)).toBeTruthy()
    expect(screen.queryByText(t.customHeaders)).toBeNull()
  })

  it('切换供应商时退回一级页，避免改到别人的请求头', async () => {
    const other = makeProvider({ id: 'p2', name: 'Anthropic' })
    const { rerender, props } = renderTabWithRerender()
    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.requestConfig) }))
    expect(screen.getByText(t.customHeaders)).toBeTruthy()

    rerender({ ...props, selectedProvider: other })
    expect(screen.queryByText(t.customHeaders)).toBeNull()
    expect(screen.getByText(t.baseUrl)).toBeTruthy()
  })
})
