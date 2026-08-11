import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ChatTab } from './ChatTab'

vi.mock('@tauri-apps/api/path', () => ({
  homeDir: async () => '/home/test',
  join: async (...parts: string[]) => parts.join('/'),
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
import { makeSettings, makeProvider } from './testFixtures'
import { i18n } from '../i18n'

const t = i18n.zh

type Props = Parameters<typeof ChatTab>[0]
/** 回调字段替换成 vi.fn Mock，测试里才能用 mockClear / mock.calls。 */
type MockedProps = Omit<Props, 'onUpdateChat' | 'onUpdateNativeTools' | 'onNavigateTab'> & {
  onUpdateChat: ReturnType<typeof vi.fn>
  onUpdateNativeTools: ReturnType<typeof vi.fn>
  onNavigateTab: ReturnType<typeof vi.fn>
}

/**
 * 回归重点：
 *   1. onUpdateChat vs onUpdateNativeTools 分流
 *   2. 最大输出 token 的「生效值 / 来源标签 / 模型名」不串位
 *   3. PromptField：空值显示 defaultText；恢复默认写回 ''
 */
function renderTab(overrides: Partial<MockedProps> = {}) {
  const props: MockedProps = {
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
      systemPrompt: '',
      chatMode: { systemPrompt: '' },
    } as Props['chatConfig'],
    chatTools: { enabled: false, servers: [], nativeTools: { workingDirectory: '/w' } } as unknown as Props['chatTools'],
    chatMemory: { enabled: false } as Props['chatMemory'],
    chatDefaults: 'You are the AI assistant inside Kivio.',
    chatRuntimeDefaults: 'Chat runtime (internal runtime mode): this conversation uses Kivio Chat.',
    chatFallbackMaxOutputTokens: 8192,
    effectiveChatMaxOutput: { maxOutput: 16384, source: 'override' },
    chatMaxOutputSourceLabel: '来自模型覆盖',
    chatMaxOutputModelLabel: 'p1 / gpt-4o',
    skillRuntimeEnabled: true,
    nativeBuiltinToolsEnabled: false,
    onUpdateChat: vi.fn(),
    onUpdateNativeTools: vi.fn(),
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

  it('Agent 提示词为空时显示内置 defaultText（英文原文）', () => {
    renderTab()
    expect(screen.getByDisplayValue('You are the AI assistant inside Kivio.')).toBeTruthy()
  })

  it('Chat runtime 提示词为空时显示内置 defaultText（英文原文）', () => {
    renderTab()
    expect(
      screen.getByDisplayValue('Chat runtime (internal runtime mode): this conversation uses Kivio Chat.'),
    ).toBeTruthy()
  })

  it('恢复默认写回空字符串', async () => {
    const props = renderTab({
      chatConfig: {
        streamEnabled: true,
        thinkingEnabled: true,
        maxOutputTokens: 8192,
        defaultLanguage: '',
        userDisplayName: '小明',
        userAvatar: '',
        systemPrompt: '自定义 Agent 提示',
        chatMode: { systemPrompt: '自定义 Chat 提示' },
      } as Props['chatConfig'],
    })
    const restores = screen.getAllByRole('button', { name: t.restoreDefaultPrompt })
    for (const btn of restores) {
      if ((btn as HTMLButtonElement).disabled) continue
      props.onUpdateChat.mockClear()
      await userEvent.click(btn)
      if (
        props.onUpdateChat.mock.calls.some(
          (call) => call[0] && 'systemPrompt' in call[0] && call[0].systemPrompt === '',
        )
      ) {
        break
      }
    }
    expect(props.onUpdateChat).toHaveBeenCalledWith({ systemPrompt: '' })
  })

  it('无默认提示词时 Agent 恢复默认按钮禁用', () => {
    renderTab({ chatDefaults: undefined, chatRuntimeDefaults: undefined })
    const restores = screen.getAllByRole('button', { name: t.restoreDefaultPrompt })
    expect(restores.some((btn) => (btn as HTMLButtonElement).disabled)).toBe(true)
  })

  it('编辑 Agent 提示词写回 onUpdateChat', async () => {
    const props = renderTab({
      chatConfig: {
        streamEnabled: true,
        thinkingEnabled: true,
        maxOutputTokens: 8192,
        defaultLanguage: '',
        userDisplayName: '小明',
        userAvatar: '',
        systemPrompt: '自定义 Agent 提示',
        chatMode: { systemPrompt: '' },
      } as Props['chatConfig'],
    })
    await userEvent.type(screen.getByDisplayValue('自定义 Agent 提示'), 'z')
    expect(props.onUpdateChat).toHaveBeenCalled()
  })

  it('工具状态徽标按各自开关渲染', () => {
    renderTab()
    const onTags = screen.getAllByText('已启用')
    expect(onTags).toHaveLength(1)
  })

  it('跳转按钮把目标 tab 传给 onNavigateTab', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: t.chatOpenProviders }))
    expect(props.onNavigateTab).toHaveBeenCalledWith('providers')
  })
})
