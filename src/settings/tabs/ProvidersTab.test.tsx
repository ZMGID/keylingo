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
function renderTab(overrides: Partial<Props> = {}) {
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
    onRequestDeleteProvider: vi.fn(),
    onToggleGzipInfo: vi.fn(),
    onToggleKeyReveal: vi.fn(),
    onOpenModelPicker: vi.fn(),
    onOpenModelTest: vi.fn(),
    onOpenModelDrawer: vi.fn(),
    onRemoveEnabledModel: vi.fn(),
    ...overrides,
  }
  render(<ProvidersTab {...props} />)
  return props
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

  it('gzip 说明默认收起，gzipInfoOpen 命中才展开', () => {
    renderTab()
    expect(screen.queryByText(/WAF 会扫描明文请求体/)).toBeNull()
    renderTab({ gzipInfoOpen: new Set(['p1']) })
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
})
