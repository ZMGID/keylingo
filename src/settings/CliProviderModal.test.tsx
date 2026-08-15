import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { CliProviderModal } from './CliProviderModal'

vi.mock('../chat/api', () => ({
  chatApi: {
    externalCliFetchRelayModels: vi.fn(),
  },
}))

describe('CliProviderModal native providers', () => {
  it('saves OpenCode as native provider JSON instead of environment variables', () => {
    const onSave = vi.fn()
    render(
      <CliProviderModal
        lang="zh"
        agentId="opencode"
        agentName="OpenCode"
        onSave={onSave}
        onClose={vi.fn()}
      />,
    )

    fireEvent.change(screen.getByPlaceholderText('例如：我的中转站'), {
      target: { value: 'Relay' },
    })
    fireEvent.change(screen.getByPlaceholderText('https://api.example.com/v1'), {
      target: { value: 'https://relay.example/v1' },
    })
    fireEvent.change(screen.getByPlaceholderText('sk-…'), {
      target: { value: 'sk-test' },
    })
    fireEvent.change(screen.getByLabelText('模型 ID'), { target: { value: 'gpt-test' } })
    expect(screen.queryByText('启动默认值')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: '当前默认模型' })).toHaveAttribute('aria-pressed', 'true')
    fireEvent.click(screen.getByRole('button', { name: '添加' }))

    expect(onSave).toHaveBeenCalledTimes(1)
    const saved = onSave.mock.calls[0][0]
    expect(saved.env).toEqual([])
    expect(saved.nativeProviderId).toBe('relay')
    expect(saved.defaultModel).toBe('gpt-test')
    expect(JSON.parse(saved.configJson)).toMatchObject({
      npm: '@ai-sdk/openai-compatible',
      options: { baseURL: 'https://relay.example/v1' },
      models: { 'gpt-test': { name: 'gpt-test' } },
    })
    expect(JSON.parse(saved.authJson)).toEqual({ type: 'api', key: 'sk-test' })
  })

  it('allows an OpenCode native SDK provider without URL or API key', () => {
    const onSave = vi.fn()
    render(
      <CliProviderModal
        lang="zh"
        agentId="opencode"
        agentName="OpenCode"
        onSave={onSave}
        onClose={vi.fn()}
      />,
    )

    fireEvent.change(screen.getByPlaceholderText('例如：我的中转站'), {
      target: { value: 'Local Anthropic' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'OpenAI Compatible' }))
    fireEvent.click(screen.getByRole('option', { name: 'Claude' }))
    fireEvent.change(screen.getByLabelText('模型 ID'), {
      target: { value: 'claude-sonnet-4.6' },
    })
    fireEvent.click(screen.getByRole('button', { name: '添加' }))

    expect(onSave).toHaveBeenCalledTimes(1)
    const saved = onSave.mock.calls[0][0]
    expect(saved.nativeProviderId).toBe('local-anthropic')
    expect(saved.authJson).toBe('')
    expect(JSON.parse(saved.configJson)).toMatchObject({
      npm: '@ai-sdk/anthropic',
      options: {},
      models: {
        'claude-sonnet-4.6': {
          reasoning: true,
          attachment: true,
        },
      },
    })
  })

  it('selects the OpenCode default model from the model row', () => {
    const onSave = vi.fn()
    render(
      <CliProviderModal
        lang="zh"
        agentId="opencode"
        agentName="OpenCode"
        onSave={onSave}
        onClose={vi.fn()}
      />,
    )

    fireEvent.change(screen.getByPlaceholderText('例如：我的中转站'), {
      target: { value: 'Relay' },
    })
    fireEvent.change(screen.getByPlaceholderText('https://api.example.com/v1'), {
      target: { value: 'https://relay.example/v1' },
    })
    fireEvent.change(screen.getByLabelText('模型 ID'), { target: { value: 'model-a' } })
    fireEvent.click(screen.getByRole('button', { name: '添加模型' }))
    fireEvent.change(screen.getAllByLabelText('模型 ID')[1], { target: { value: 'model-b' } })
    fireEvent.click(screen.getByRole('button', { name: '设为默认模型' }))
    fireEvent.click(screen.getByRole('button', { name: '添加' }))

    expect(onSave).toHaveBeenCalledTimes(1)
    expect(onSave.mock.calls[0][0].defaultModel).toBe('model-b')
  })

  it('auto-fills Pi metadata from a model id and allows advanced overrides', () => {
    const onSave = vi.fn()
    render(
      <CliProviderModal
        lang="zh"
        agentId="pi"
        agentName="Pi"
        onSave={onSave}
        onClose={vi.fn()}
      />,
    )

    fireEvent.change(screen.getByPlaceholderText('例如：我的中转站'), {
      target: { value: 'Grok Relay' },
    })
    fireEvent.change(screen.getByPlaceholderText('https://api.example.com/v1'), {
      target: { value: 'https://relay.example/v1' },
    })
    fireEvent.change(screen.getByPlaceholderText('sk-…'), {
      target: { value: 'sk-pi' },
    })
    fireEvent.change(screen.getByLabelText('模型 ID'), {
      target: { value: 'grok-4.5' },
    })
    expect(screen.getByLabelText('默认模型')).toHaveValue('grok-4.5')
    expect(screen.getByText('Grok 4.5')).toBeInTheDocument()
    expect(screen.getByText('500K 上下文')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '模型高级设置' }))
    expect(screen.getByRole('switch', { name: '支持推理' })).toHaveAttribute('aria-checked', 'true')
    expect(screen.getByRole('switch', { name: '支持图片输入' })).toHaveAttribute('aria-checked', 'true')
    fireEvent.change(screen.getByLabelText('上下文窗口'), { target: { value: '256000' } })
    fireEvent.change(screen.getByLabelText('最大输出 Token'), { target: { value: '32768' } })
    fireEvent.click(screen.getByRole('button', { name: '添加模型' }))
    fireEvent.change(screen.getAllByLabelText('模型 ID')[1], {
      target: { value: 'relay-fast-model' },
    })
    fireEvent.click(screen.getByRole('button', { name: '添加' }))

    expect(onSave).toHaveBeenCalledTimes(1)
    const saved = onSave.mock.calls[0][0]
    expect(saved.defaultReasoning).toBe('high')
    expect(JSON.parse(saved.configJson)).toMatchObject({
      api: 'openai-completions',
      models: [
        {
          id: 'grok-4.5',
          name: 'Grok 4.5',
          reasoning: true,
          input: ['text', 'image'],
          contextWindow: 256000,
          maxTokens: 32768,
        },
        {
          id: 'relay-fast-model',
          name: 'relay-fast-model',
          reasoning: false,
          input: ['text'],
          contextWindow: 128000,
          maxTokens: 16384,
        },
      ],
    })
  })

  it('uses the first unknown Pi model as default with thinking off', () => {
    const onSave = vi.fn()
    render(
      <CliProviderModal
        lang="zh"
        agentId="pi"
        agentName="Pi"
        onSave={onSave}
        onClose={vi.fn()}
      />,
    )

    fireEvent.change(screen.getByPlaceholderText('例如：我的中转站'), {
      target: { value: 'Private Relay' },
    })
    fireEvent.change(screen.getByPlaceholderText('https://api.example.com/v1'), {
      target: { value: 'https://relay.example/v1' },
    })
    fireEvent.change(screen.getByPlaceholderText('sk-…'), {
      target: { value: 'sk-pi' },
    })
    fireEvent.change(screen.getByLabelText('模型 ID'), {
      target: { value: 'company-private-model' },
    })

    expect(screen.getByLabelText('默认模型')).toHaveValue('company-private-model')
    fireEvent.click(screen.getByRole('button', { name: '添加' }))

    expect(onSave).toHaveBeenCalledTimes(1)
    const saved = onSave.mock.calls[0][0]
    expect(saved.defaultModel).toBe('company-private-model')
    expect(saved.defaultReasoning).toBe('off')
    expect(JSON.parse(saved.configJson).models[0]).toMatchObject({
      id: 'company-private-model',
      reasoning: false,
      contextWindow: 128000,
      maxTokens: 16384,
    })
  })

  it('keeps the dsh provider form to the three supported protocols', () => {
    const onSave = vi.fn()
    render(
      <CliProviderModal
        lang="zh"
        agentId="dsh"
        agentName="DeepSeek Harness"
        onSave={onSave}
        onClose={vi.fn()}
      />,
    )

    fireEvent.change(screen.getByPlaceholderText('例如：我的中转站'), {
      target: { value: 'GPT' },
    })
    fireEvent.change(screen.getByPlaceholderText('https://api.example.com/v1'), {
      target: { value: 'https://relay.example/v1' },
    })
    fireEvent.change(screen.getByPlaceholderText('sk-…'), {
      target: { value: 'sk-dsh' },
    })
    fireEvent.change(screen.getByLabelText('模型 ID'), {
      target: { value: 'gpt-test' },
    })

    expect(screen.queryByText('启动默认值')).not.toBeInTheDocument()
    expect(screen.queryByText('Pi 默认值')).not.toBeInTheDocument()
    expect(screen.getByText('默认值')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '当前默认模型' })).toHaveAttribute('aria-pressed', 'true')

    fireEvent.click(screen.getByRole('button', { name: 'openai-completions' }))
    expect(screen.getByRole('option', { name: 'openai-responses' })).toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'anthropic-messages' })).toBeInTheDocument()
    expect(screen.queryByRole('option', { name: 'google-generative-ai' })).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: '添加' }))
    expect(onSave).toHaveBeenCalledTimes(1)
    expect(JSON.parse(onSave.mock.calls[0][0].configJson)).toMatchObject({
      api: 'openai-completions',
      baseURL: 'https://relay.example/v1',
      models: [{ id: 'gpt-test' }],
    })
  })

  it('exposes dsh image input and reasoning efforts on the model row', () => {
    const onSave = vi.fn()
    render(
      <CliProviderModal
        lang="zh"
        agentId="dsh"
        agentName="DeepSeek Harness"
        onSave={onSave}
        onClose={vi.fn()}
      />,
    )

    fireEvent.change(screen.getByPlaceholderText('例如：我的中转站'), {
      target: { value: 'GPT' },
    })
    fireEvent.change(screen.getByPlaceholderText('https://api.example.com/v1'), {
      target: { value: 'https://relay.example/v1' },
    })
    fireEvent.change(screen.getByPlaceholderText('sk-…'), {
      target: { value: 'sk-dsh' },
    })
    fireEvent.change(screen.getByLabelText('模型 ID'), {
      target: { value: 'gpt-5.6-sol' },
    })

    expect(screen.queryByRole('button', { name: '模型高级设置' })).toBeInTheDocument()
    expect(screen.getByRole('switch', { name: '支持图片输入' })).toHaveAttribute('aria-checked', 'true')
    expect(screen.getByRole('switch', { name: '支持推理' })).toHaveAttribute('aria-checked', 'true')
    expect(screen.getByRole('group', { name: '推理档位' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'xhigh' })).toHaveAttribute('aria-pressed', 'true')

    fireEvent.click(screen.getByRole('button', { name: 'max' }))
    fireEvent.click(screen.getByRole('button', { name: '添加' }))

    expect(onSave).toHaveBeenCalledTimes(1)
    const config = JSON.parse(onSave.mock.calls[0][0].configJson)
    expect(config.defaultInput).toEqual(['text', 'image'])
    expect(config.models[0]).toMatchObject({
      id: 'gpt-5.6-sol',
      input: ['text', 'image'],
    })
    expect(config.models[0].reasoningEfforts).toMatchObject({
      off: null,
      low: 'low',
      high: 'high',
      xhigh: 'xhigh',
    })
    expect(config.models[0].reasoningEfforts.max).toBeUndefined()
  })
})
