import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ProviderModelsPicker, type ProviderModelsPickerLabels } from './ProviderModelsPicker'
import { makeProvider } from './tabs/testFixtures'

vi.mock('../chat/ModelIcon', () => ({
  ModelIcon: () => null,
}))

const labels: ProviderModelsPickerLabels = {
  title: '模型',
  searchPlaceholder: '搜索模型 ID 或名称',
  fetchModels: '获取模型列表',
  fetching: '正在获取...',
  addModel: '添加',
  manualAddModel: '手动添加',
  noModels: '没有可用模型。点刷新重试，或手动添加。',
  noSearchResults: '没有匹配的模型',
  enabled: '已启用',
  addAllModels: '添加当前列表中的全部模型',
  close: '关闭',
}

describe('ProviderModelsPicker', () => {
  it('opens by refreshing even when cached models already exist', () => {
    const onFetch = vi.fn()
    render(
      <ProviderModelsPicker
        provider={makeProvider({
          availableModels: ['deepseek-v4-flash', 'deepseek-v4-pro'],
          enabledModels: ['deepseek-v4-flash'],
        })}
        lang="zh"
        labels={labels}
        fetching={false}
        onClose={() => {}}
        onFetch={onFetch}
        onAdd={() => {}}
        onAddAll={() => {}}
        onRemove={() => {}}
      />,
    )
    expect(onFetch).toHaveBeenCalledTimes(1)
    expect(screen.getByText('deepseek-v4-flash')).toBeTruthy()
  })

  it('puts refresh and manual-add in equal icon buttons beside search', () => {
    render(
      <ProviderModelsPicker
        provider={makeProvider()}
        lang="zh"
        labels={labels}
        fetching={false}
        onClose={() => {}}
        onFetch={() => {}}
        onAdd={() => {}}
        onAddAll={() => {}}
        onRemove={() => {}}
      />,
    )
    const refresh = screen.getByRole('button', { name: '获取模型列表' })
    const add = screen.getByRole('button', { name: '手动添加' })
    expect(refresh).toHaveClass('kv-icon-btn', 'sm')
    expect(add).toHaveClass('kv-icon-btn', 'sm')
    expect(refresh.textContent).not.toMatch(/获取模型列表/)
  })

  it('does not list enabled models the provider no longer offers', () => {
    render(
      <ProviderModelsPicker
        provider={makeProvider({
          availableModels: ['deepseek-v4-pro'],
          enabledModels: ['retired-model', 'deepseek-v4-pro'],
        })}
        lang="zh"
        labels={labels}
        fetching={false}
        onClose={() => {}}
        onFetch={() => {}}
        onAdd={() => {}}
        onAddAll={() => {}}
        onRemove={() => {}}
      />,
    )
    expect(screen.getByText('deepseek-v4-pro')).toBeTruthy()
    expect(screen.queryByText('retired-model')).toBeNull()
  })

  it('adds a typed model from the compact manual row', async () => {
    const user = userEvent.setup()
    const onAdd = vi.fn()
    render(
      <ProviderModelsPicker
        provider={makeProvider({ availableModels: [], enabledModels: [] })}
        lang="zh"
        labels={labels}
        fetching={false}
        onClose={() => {}}
        onFetch={() => {}}
        onAdd={onAdd}
        onAddAll={() => {}}
        onRemove={() => {}}
      />,
    )
    await user.click(screen.getByRole('button', { name: '手动添加' }))
    await user.type(screen.getByPlaceholderText('手动添加'), 'my-custom-model')
    await user.click(screen.getByRole('button', { name: '添加' }))
    expect(onAdd).toHaveBeenCalledWith('my-custom-model')
    expect(screen.getByText('my-custom-model')).toBeTruthy()
  })
})
