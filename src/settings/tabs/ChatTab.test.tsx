import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ChatTab } from './ChatTab'
import { makeSettings, makeProvider } from './testFixtures'
import { i18n } from '../i18n'

const t = i18n.zh

type Props = Parameters<typeof ChatTab>[0]

/**
 * 回归重点：
 *   1. onUpdateChat vs onUpdateNativeTools 分流（工作目录写 nativeTools，其余写 chat）
 *   2. 最大输出 token 的「生效值 / 来源标签 / 模型名」三个派生值不串位
 *   3. 系统提示词的 interacted 联动（恢复默认要同时置 false）
 */
function renderTab(overrides: Partial<Props> = {}) {
  const props: Props = {
    settings: makeSettings({ providers: [makeProvider()] }),
    t,
    lang: 'zh',
    chatConfig: {
      streamEnabled: true,
      thinkingEnabled: true,
      maxOutputTokens: 8192,
      defaultLanguage: '',
      userDisplayName: '小明',
      userAvatar: '',
    } as Props['chatConfig'],
    chatTools: { enabled: false, servers: [], nativeTools: { workingDirectory: '/w' } } as unknown as Props['chatTools'],
    chatMemory: { enabled: false } as Props['chatMemory'],
    chatDefaults: '默认聊天系统提示',
    chatSystemPromptValue: '当前系统提示',
    chatSystemPromptInteracted: false,
    chatFallbackMaxOutputTokens: 8192,
    effectiveChatMaxOutput: { maxOutput: 16384, source: 'override' },
    chatMaxOutputSourceLabel: '来自模型覆盖',
    chatMaxOutputModelLabel: 'p1 / gpt-4o',
    skillRuntimeEnabled: true,
    nativeBuiltinToolsEnabled: false,
    onUpdateChat: vi.fn(),
    onUpdateNativeTools: vi.fn(),
    onSystemPromptInteractedChange: vi.fn(),
    onNavigateTab: vi.fn(),
    ...overrides,
  }
  render(<ChatTab {...props} />)
  return props
}

describe('ChatTab', () => {
  it('回显用户名与工作目录（分别来自 chatConfig / chatTools）', () => {
    renderTab()
    expect(screen.getByDisplayValue('小明')).toBeTruthy()
    expect(screen.getByDisplayValue('/w')).toBeTruthy()
  })

  it('工作目录输入走 onUpdateNativeTools 而非 onUpdateChat', async () => {
    const props = renderTab()
    await userEvent.type(screen.getByDisplayValue('/w'), 'x')
    expect(props.onUpdateNativeTools).toHaveBeenCalled()
    expect(props.onUpdateChat).not.toHaveBeenCalled()
  })

  it('用户名输入走 onUpdateChat', async () => {
    const props = renderTab()
    await userEvent.type(screen.getByDisplayValue('小明'), 'x')
    expect(props.onUpdateChat).toHaveBeenCalled()
    expect(props.onUpdateNativeTools).not.toHaveBeenCalled()
  })

  it('最大输出 token 显示生效值 + 来源标签 + 模型名（三者不串）', () => {
    renderTab()
    expect(screen.getByText('16,384 tokens')).toBeTruthy()
    expect(screen.getByText('来自模型覆盖')).toBeTruthy()
    expect(screen.getByText(/p1 \/ gpt-4o/)).toBeTruthy()
  })

  it('source=fallback 时来源标签用警示色', () => {
    renderTab({
      effectiveChatMaxOutput: { maxOutput: 8192, source: 'fallback' },
      chatMaxOutputSourceLabel: '兜底值',
    })
    expect(screen.getByText('兜底值').className).toContain('warn')
  })

  it('系统提示词回显 chatSystemPromptValue', () => {
    renderTab()
    expect(screen.getByDisplayValue('当前系统提示')).toBeTruthy()
  })

  it('恢复默认同时清空提示词并置 interacted=false', async () => {
    const props = renderTab({ chatSystemPromptInteracted: true })
    const restores = screen.getAllByRole('button', { name: t.restoreDefaultPrompt })
    await userEvent.click(restores[restores.length - 1])
    expect(props.onSystemPromptInteractedChange).toHaveBeenCalledWith(false)
    expect(props.onUpdateChat).toHaveBeenCalledWith({ systemPrompt: '' })
  })

  it('无默认提示词时恢复默认按钮禁用', () => {
    renderTab({ chatDefaults: undefined })
    const restores = screen.getAllByRole('button', { name: t.restoreDefaultPrompt })
    expect(restores[restores.length - 1]).toBeDisabled()
  })

  it('编辑提示词时置 interacted=true', async () => {
    const props = renderTab()
    await userEvent.type(screen.getByDisplayValue('当前系统提示'), 'z')
    expect(props.onSystemPromptInteractedChange).toHaveBeenCalledWith(true)
  })

  it('工具状态徽标按各自开关渲染', () => {
    renderTab()
    // skillRuntimeEnabled=true / nativeBuiltinToolsEnabled=false / chatTools.enabled=false / memory=false
    const onTags = screen.getAllByText('已启用')
    expect(onTags).toHaveLength(1)
  })

  it('跳转按钮把目标 tab 传给 onNavigateTab', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: t.chatOpenProviders }))
    expect(props.onNavigateTab).toHaveBeenCalledWith('providers')
  })
})
