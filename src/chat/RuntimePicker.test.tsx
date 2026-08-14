import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ExternalModelSelector, RuntimePicker } from './RuntimePicker'
import { LAST_AGENT_RUNTIME_KEY } from './lastAgentRuntime'
import type { AgentRuntimeConfig } from './types'

const detectModels = vi.fn()
const detectAgents = vi.fn()

vi.mock('./api', () => ({
  chatApi: {
    detectExternalAgents: (...args: unknown[]) => detectAgents(...args),
    detectExternalAgentModels: (...args: unknown[]) => detectModels(...args),
  },
}))

const runtime: AgentRuntimeConfig = {
  kind: 'external',
  externalAgentId: 'cursor',
  externalModel: 'default',
  externalReasoning: null,
}

describe('ExternalModelSelector', () => {
  beforeEach(() => {
    detectModels.mockReset()
  })

  it('探测失败降级时展示默认列表提示，重试以 force=true 重探', async () => {
    detectModels
      .mockResolvedValueOnce({
        models: [{ id: 'default', label: 'Default' }],
        reasoningOptions: [],
        source: 'fallback',
        probeError: 'boom',
      })
      .mockResolvedValueOnce({
        models: [{ id: 'gpt-5', label: 'gpt-5' }],
        reasoningOptions: [],
        source: 'probed',
      })

    render(
      <ExternalModelSelector
        agentRuntime={runtime}
        onModelChange={() => {}}
        conversationId={null}
      />,
    )

    // 初次懒查（fallback），force 未传。
    await waitFor(() => expect(detectModels).toHaveBeenCalledTimes(1))
    expect(detectModels.mock.calls[0][2]).toBeUndefined()

    // 打开下拉：出现降级提示行。
    act(() => {
      fireEvent.click(screen.getByRole('button'))
    })
    expect(screen.getByText('探测失败，显示默认列表')).toBeInTheDocument()

    // 点重试 → 以 force=true 重探。
    act(() => {
      fireEvent.click(screen.getByText('重试'))
    })
    await waitFor(() => expect(detectModels).toHaveBeenCalledTimes(2))
    expect(detectModels.mock.calls[1][2]).toBe(true)
  })

  it('probed 结果不显示降级提示', async () => {
    detectModels.mockResolvedValue({
      models: [{ id: 'gpt-5', label: 'gpt-5' }],
      reasoningOptions: [],
      source: 'probed',
    })

    render(
      <ExternalModelSelector
        agentRuntime={runtime}
        onModelChange={() => {}}
        conversationId={null}
      />,
    )
    await waitFor(() => expect(detectModels).toHaveBeenCalledTimes(1))
    act(() => {
      fireEvent.click(screen.getByRole('button'))
    })
    expect(screen.queryByText('探测失败，显示默认列表')).not.toBeInTheDocument()
    expect(screen.getByText('gpt-5')).toBeInTheDocument()
  })

  it('探测到 CLI 当前模型/推理时自动同步（未显式选择）', async () => {
    const onModelChange = vi.fn()
    detectModels.mockResolvedValue({
      models: [
        { id: 'default', label: 'Default' },
        { id: 'grok-4.5', label: 'Grok 4.5' },
      ],
      reasoningOptions: [
        { id: 'default', label: 'Default' },
        { id: 'high', label: 'High' },
      ],
      source: 'probed',
      currentModel: 'grok-4.5',
      currentReasoning: 'high',
    })

    render(
      <ExternalModelSelector
        agentRuntime={runtime}
        onModelChange={onModelChange}
        conversationId={null}
      />,
    )
    await waitFor(() =>
      expect(onModelChange).toHaveBeenCalledWith('grok-4.5', 'high'),
    )
  })

  it('用户已显式选择模型时不被当前配置覆盖', async () => {
    const onModelChange = vi.fn()
    const explicit: AgentRuntimeConfig = { ...runtime, externalModel: 'gpt-5' }
    detectModels.mockResolvedValue({
      models: [{ id: 'gpt-5', label: 'gpt-5' }],
      reasoningOptions: [],
      source: 'probed',
      currentModel: 'gpt-5.6-sol',
      currentReasoning: 'high',
    })

    render(
      <ExternalModelSelector
        agentRuntime={explicit}
        onModelChange={onModelChange}
        conversationId={null}
      />,
    )
    await waitFor(() => expect(screen.getByRole('button')).toHaveTextContent('gpt-5'))
    expect(onModelChange).not.toHaveBeenCalled()
  })

  it('dsh 的 off 是真档位：胶囊显示 Off，菜单里保留 Off', async () => {
    detectModels.mockResolvedValue({
      models: [
        { id: 'default', label: 'Default' },
        { id: 'deepseek-v4-flash', label: 'DeepSeek-V4-Flash' },
      ],
      reasoningOptions: [
        { id: 'default', label: 'Default' },
        { id: 'off', label: 'Off' },
        { id: 'high', label: 'High' },
        { id: 'max', label: 'Max' },
      ],
      source: 'probed',
      currentModel: 'deepseek-v4-flash',
      currentReasoning: 'off',
    })

    render(
      <ExternalModelSelector
        agentRuntime={runtime}
        onModelChange={() => {}}
        conversationId={null}
      />,
    )
    await waitFor(() =>
      expect(screen.getByLabelText('思考等级：Off')).toBeInTheDocument(),
    )
    act(() => {
      fireEvent.click(screen.getByLabelText('思考等级：Off'))
    })
    expect(screen.getByRole('button', { name: 'Off' })).toBeInTheDocument()
    expect(screen.queryByLabelText('思考等级：Auto')).not.toBeInTheDocument()
  })

  it('ACP 只报 on/off 开关时不显示思考档位胶囊', async () => {
    detectModels.mockResolvedValue({
      models: [{ id: 'default', label: 'Default' }],
      reasoningOptions: [
        { id: 'on', label: 'On' },
        { id: 'off', label: 'Off' },
      ],
      source: 'probed',
      currentModel: null,
      currentReasoning: 'off',
    })

    render(
      <ExternalModelSelector
        agentRuntime={runtime}
        onModelChange={() => {}}
        conversationId={null}
      />,
    )
    await waitFor(() => expect(screen.getByRole('button')).toHaveTextContent('Auto'))
    expect(screen.queryByLabelText(/思考等级/)).not.toBeInTheDocument()
  })

  it('有 CLI 当前模型/推理时胶囊显示真实名字而不是 Auto', async () => {
    detectModels.mockResolvedValue({
      models: [
        { id: 'default', label: 'Default' },
        { id: 'deepseek-v4-flash', label: 'DeepSeek-V4-Flash' },
      ],
      reasoningOptions: [
        { id: 'default', label: 'Default' },
        { id: 'off', label: 'Off' },
        { id: 'high', label: 'High' },
        { id: 'max', label: 'Max' },
      ],
      source: 'probed',
      currentModel: 'deepseek-v4-flash',
      currentReasoning: 'high',
    })

    render(
      <ExternalModelSelector
        agentRuntime={runtime}
        onModelChange={() => {}}
        conversationId={null}
      />,
    )
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /DeepSeek-V4-Flash/ })).toBeInTheDocument(),
    )
    expect(screen.getByLabelText('思考等级：High')).toBeInTheDocument()
    expect(screen.queryByText('Auto')).not.toBeInTheDocument()
  })

  it('无当前模型概念时胶囊显示「Auto」', async () => {
    detectModels.mockResolvedValue({
      models: [{ id: 'default', label: 'Default' }],
      reasoningOptions: [],
      source: 'probed',
      currentModel: null,
      currentReasoning: null,
    })

    render(
      <ExternalModelSelector
        agentRuntime={runtime}
        onModelChange={() => {}}
        conversationId={null}
      />,
    )
    await waitFor(() => expect(screen.getByRole('button')).toHaveTextContent('Auto'))
  })

  it('切换 agent 时旧 CLI 的 currentModel 立即失效（不残留在胶囊上）', async () => {
    detectModels.mockImplementation((agentId: unknown) => {
      if (agentId === 'cursor') {
        return Promise.resolve({
          models: [
            { id: 'default', label: 'Default' },
            { id: 'grok-4.5', label: 'Grok 4.5' },
          ],
          reasoningOptions: [],
          source: 'probed',
          currentModel: 'grok-4.5',
          currentReasoning: null,
        })
      }
      // 新 agent 探测 pending：胶囊应显示「获取中…」，而不是上个 CLI 的 Grok 4.5。
      return new Promise(() => {})
    })

    const { rerender } = render(
      <ExternalModelSelector
        agentRuntime={runtime}
        onModelChange={() => {}}
        conversationId={null}
      />,
    )
    await waitFor(() =>
      expect(screen.getByRole('button')).toHaveTextContent('Grok 4.5'),
    )

    rerender(
      <ExternalModelSelector
        agentRuntime={{ ...runtime, externalAgentId: 'claude', externalModel: 'default' }}
        onModelChange={() => {}}
        conversationId={null}
      />,
    )
    await waitFor(() =>
      expect(screen.getByRole('button')).toHaveTextContent('获取中…'),
    )
    expect(screen.getByRole('button')).not.toHaveTextContent('Grok 4.5')
  })
})

describe('RuntimePicker（一 agent 一对话绑定锁）', () => {
  beforeEach(() => {
    detectAgents.mockReset()
    detectAgents.mockResolvedValue([
      { id: 'cursor', name: 'Cursor Agent', available: true, models: [{ id: 'default', label: 'Default' }] },
      { id: 'claude', name: 'Claude Code', available: true, models: [{ id: 'default', label: 'Default' }] },
    ])
  })

  it('locked 时不展示绑定横幅，非当前项置灰且不可切换', async () => {
    const onRuntimeChange = vi.fn()
    render(
      <RuntimePicker
        agentRuntime={runtime}
        onRuntimeChange={onRuntimeChange}
        conversationId="c1"
        locked
      />,
    )
    await waitFor(() => expect(detectAgents).toHaveBeenCalled())
    // 打开 popover（chip 是第一个按钮）。
    act(() => {
      fireEvent.click(screen.getAllByRole('button')[0])
    })
    expect(screen.getByText('已绑定，新建可切换')).toBeInTheDocument()
    // 非当前代理全部禁用：Kivio Agent / Chat 与 claude；当前 agent（cursor）保持可选。
    expect(screen.getByRole('radio', { name: 'Kivio Agent' })).toBeDisabled()
    expect(screen.getByRole('radio', { name: 'Kivio Chat' })).toBeDisabled()
    act(() => {
      fireEvent.click(screen.getByRole('radio', { name: 'Kivio Agent' }))
    })
    expect(onRuntimeChange).not.toHaveBeenCalled()
    expect(screen.getByRole('radio', { name: /Claude Code/ })).toBeDisabled()
    expect(screen.getByRole('radio', { name: /Cursor Agent/ })).not.toBeDisabled()
  })


  it('再点当前外部代理不把模型和思考重置成 default', async () => {
    const onRuntimeChange = vi.fn()
    const claudeRuntime: AgentRuntimeConfig = {
      kind: 'external',
      externalAgentId: 'claude',
      externalModel: 'claude-opus-5',
      externalReasoning: 'high',
    }
    render(
      <RuntimePicker
        agentRuntime={claudeRuntime}
        onRuntimeChange={onRuntimeChange}
        conversationId="c1"
      />,
    )
    await waitFor(() => expect(detectAgents).toHaveBeenCalled())
    act(() => {
      fireEvent.click(screen.getAllByRole('button')[0])
    })
    act(() => {
      fireEvent.click(screen.getByRole('radio', { name: /Claude Code/ }))
    })
    expect(onRuntimeChange).not.toHaveBeenCalled()
  })

  it('切到 Claude Code 时带回该代理上次的模型和思考档', async () => {
    window.localStorage.setItem(LAST_AGENT_RUNTIME_KEY, JSON.stringify({
      kind: 'external',
      externalAgentId: 'dsh',
      externalModel: 'deepseek-v4-flash',
      byAgent: {
        claude: { externalModel: 'claude-opus-5', externalReasoning: 'high' },
      },
    }))
    const onRuntimeChange = vi.fn()
    render(
      <RuntimePicker
        agentRuntime={runtime}
        onRuntimeChange={onRuntimeChange}
        conversationId={null}
      />,
    )
    await waitFor(() => expect(detectAgents).toHaveBeenCalled())
    act(() => {
      fireEvent.click(screen.getAllByRole('button')[0])
    })
    act(() => {
      fireEvent.click(screen.getByRole('radio', { name: /Claude Code/ }))
    })
    expect(onRuntimeChange).toHaveBeenCalledWith({
      kind: 'external',
      externalAgentId: 'claude',
      externalModel: 'claude-opus-5',
      externalReasoning: 'high',
      externalSandbox: null,
      externalAgentPreset: null,
    })
    window.localStorage.removeItem(LAST_AGENT_RUNTIME_KEY)
  })

  it('未 locked 时切换项可用', async () => {
    const onRuntimeChange = vi.fn()
    render(
      <RuntimePicker
        agentRuntime={runtime}
        onRuntimeChange={onRuntimeChange}
        conversationId="c1"
      />,
    )
    await waitFor(() => expect(detectAgents).toHaveBeenCalled())
    act(() => {
      fireEvent.click(screen.getAllByRole('button')[0])
    })
    expect(screen.queryByText('已绑定，新建可切换')).not.toBeInTheDocument()
    expect(screen.getByRole('radio', { name: 'Kivio Agent' })).not.toBeDisabled()
    act(() => {
      fireEvent.click(screen.getByRole('radio', { name: 'Kivio Agent' }))
    })
    expect(onRuntimeChange).toHaveBeenCalled()
  })
})
