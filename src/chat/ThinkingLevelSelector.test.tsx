import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ThinkingLevelSelector } from './ThinkingLevelSelector'

const { reasoningEffortsForModel } = vi.hoisted(() => ({
  reasoningEffortsForModel: vi.fn(),
}))

// api 在 jsdom 无 Tauri 环境，mock 成确定值；等级清单走兜底也是同样结果。
vi.mock('../api/tauri', () => ({
  api: {
    getSettings: () => Promise.resolve({ providers: [] }),
    reasoningEffortsForModel,
  },
}))

describe('ThinkingLevelSelector', () => {
  beforeEach(() => {
    reasoningEffortsForModel.mockReset()
    reasoningEffortsForModel.mockResolvedValue(['low', 'medium', 'high'])
  })

  it('OAuth 档位模型返回空能力时隐藏旋钮，不回写残留的会话档位', async () => {
    reasoningEffortsForModel.mockResolvedValue([])
    const onChange = vi.fn()
    await act(async () => {
      render(<ThinkingLevelSelector value="high" currentProviderId="antigravity-oauth" currentModel="gemini-3.8-flash-low" onChange={onChange} />)
    })
    expect(reasoningEffortsForModel).toHaveBeenCalledWith('gemini-3.8-flash-low', 'antigravity-oauth')
    expect(screen.queryByRole('button')).not.toBeInTheDocument()
    expect(onChange).not.toHaveBeenCalled()
  })

  it('value=null 时按默认档显示 High（不再有「跟随全局」）', () => {
    render(
      <ThinkingLevelSelector
        value={null}
        currentProviderId="p1"
        currentModel="m1"
        onChange={() => {}}
      />,
    )
    expect(screen.getByRole('button')).toHaveTextContent('High')
  })

  it('下拉项为英文标签且不含「跟随全局」', () => {
    render(
      <ThinkingLevelSelector
        value="high"
        currentProviderId="p1"
        currentModel="m1"
        onChange={() => {}}
      />,
    )
    act(() => {
      fireEvent.click(screen.getByRole('button'))
    })
    expect(screen.queryByText('跟随全局')).not.toBeInTheDocument()
    // 英文标签存在（Off + 兜底 low/medium/high）。
    expect(screen.getByText('Off')).toBeInTheDocument()
    expect(screen.getByText('Medium')).toBeInTheDocument()
  })

  it('选择某一档回调原始等级值', () => {
    const onChange = vi.fn()
    render(
      <ThinkingLevelSelector
        value="high"
        currentProviderId="p1"
        currentModel="m1"
        onChange={onChange}
      />,
    )
    act(() => {
      fireEvent.click(screen.getByRole('button'))
    })
    act(() => {
      fireEvent.click(screen.getByText('Off'))
    })
    expect(onChange).toHaveBeenCalledWith('off')
  })

  it('能力列表加载完成前不会把 xhigh 回写成 high', async () => {
    let resolveLevels!: (levels: string[]) => void
    reasoningEffortsForModel.mockImplementationOnce(() => new Promise((resolve) => {
      resolveLevels = resolve
    }))
    const onChange = vi.fn()

    render(
      <ThinkingLevelSelector
        value="xhigh"
        currentProviderId="p1"
        currentModel="gpt-5.6-sol"
        onChange={onChange}
      />,
    )

    expect(onChange).not.toHaveBeenCalled()

    await act(async () => {
      resolveLevels(['low', 'medium', 'high', 'xhigh'])
    })
    expect(screen.getByRole('button')).toHaveTextContent('XHigh')
    expect(onChange).not.toHaveBeenCalled()
  })
})
