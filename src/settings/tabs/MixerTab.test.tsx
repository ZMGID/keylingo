import { fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { MixerTab } from './MixerTab'
import { makeSettings, makeProvider } from './testFixtures'
import { i18n } from '../i18n'

const t = i18n.zh

/**
 * 回归重点：六个模型槽位各自写对 defaultModels 的 key。
 * 全是 (key, providerId, model) 同签名调用，接错 key 类型检查不报错。
 */
function renderTab(overrides: Record<string, unknown> = {}) {
  const props = {
    settings: makeSettings({
      providers: [makeProvider()],
      chatProviderId: 'p1',
      ...overrides,
    } as never),
    t,
    lang: 'zh' as const,
    chatTools: { enabled: false, servers: [] } as never,
    hasChatProvider: true,
    onUpdateDefaultModel: vi.fn(),
    onUpdateChatTools: vi.fn(),
    onUpdateChat: vi.fn(),
  }
  render(<MixerTab {...props} />)
  return props
}

describe('MixerTab', () => {
  it('渲染三个分组', () => {
    renderTab()
    expect(screen.getByText(t.mixerSection)).toBeTruthy()
    expect(screen.getByText(t.defaultPromptOptimizeModel)).toBeTruthy()
    expect(screen.getByText(t.mixerSubAgentSection)).toBeTruthy()
    expect(screen.getByText(t.mixerAdvisorSection)).toBeTruthy()
  })

  it('「全部恢复自动」一次重置五个槽位（不含 advisor）', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: t.mixerResetAuto }))
    const keys = props.onUpdateDefaultModel.mock.calls.map((c) => c[0])
    expect(keys).toEqual(['vision', 'titleSummary', 'compression', 'imageGeneration', 'promptOptimize'])
    // advisor 有独立开关，不该被批量重置清掉
    expect(keys).not.toContain('advisor')
  })

  it('未配供应商时显示引导文案', () => {
    renderTab()
    expect(screen.queryByText(/请先在「模型」中添加并配置供应商/)).toBeNull()
    render(
      <MixerTab
        settings={makeSettings({ providers: [] }) as never}
        t={t}
        lang="zh"
        chatTools={{ enabled: false, servers: [] } as never}
        hasChatProvider={false}
        onUpdateDefaultModel={vi.fn()}
        onUpdateChatTools={vi.fn()}
        onUpdateChat={vi.fn()}
      />,
    )
    expect(screen.getByText(/请先在「模型」中添加并配置供应商/)).toBeTruthy()
  })

  it('顾问开关关闭时不显示模型选择行', () => {
    renderTab()
    expect(screen.queryByText('顾问模型')).toBeNull()
  })

  it('顾问开关开启时显示模型选择行', () => {
    renderTab({
      defaultModels: {
        chat: { providerId: '', model: '' },
        vision: { providerId: '', model: '' },
        titleSummary: { providerId: '', model: '' },
        compression: { providerId: '', model: '' },
        imageGeneration: { providerId: '', model: '' },
        promptOptimize: { providerId: '', model: '' },
        advisor: { providerId: 'p1', model: 'gpt-4o' },
      },
    })
    expect(screen.getByText('顾问模型')).toBeTruthy()
  })

  it('打开顾问开关会落到第一个可用供应商的首个模型', async () => {
    const props = renderTab()
    const toggles = screen.getAllByRole('switch')
    await userEvent.click(toggles[0])
    expect(props.onUpdateDefaultModel).toHaveBeenCalledWith('advisor', 'p1', 'gpt-4o')
  })

  it('子代理模型走 onUpdateChatTools 而非 defaultModels', () => {
    const props = renderTab()
    // 该槽位读写的是 chatTools.subAgent*，不该混进 defaultModels
    expect(props.onUpdateDefaultModel).not.toHaveBeenCalled()
    expect(screen.getByText(t.defaultSubAgentModel)).toBeTruthy()
  })

  it('优化提示词写入 chat.promptOptimizePrompt', () => {
    const props = renderTab()
    const areas = screen.getAllByRole('textbox')
    fireEvent.change(areas[areas.length - 1], { target: { value: '只改写问题' } })
    expect(props.onUpdateChat).toHaveBeenCalledWith({ promptOptimizePrompt: '只改写问题' })
  })
})
