import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { HotkeysTab } from './HotkeysTab'
import { makeSettings } from './testFixtures'
import { i18n } from '../i18n'

const t = i18n.zh

/**
 * 回归重点不是"渲染出来了"，而是拆分时最容易错、typecheck 抓不到的两类：
 *   1. 各个快捷键行各自绑到正确的 settings 字段（类型全是 string，接错不报错）
 *   2. 清除按钮走对应的 updater（updateSettings / ScreenshotTranslation / Lens 三个不同函数）
 */
function renderTab(overrides: Partial<Parameters<typeof HotkeysTab>[0]> = {}) {
  const props = {
    settings: makeSettings({
      hotkey: 'Alt+1',
      chatHotkey: 'Alt+2',
      closeChatHotkey: 'Alt+3',
      screenshotTranslation: {
        enabled: true,
        hotkey: 'Alt+4',
        textHotkey: 'Alt+5',
        replaceHotkey: 'Alt+6',
        providerId: 'p1',
        model: 'm',
      },
      screenshotAnnotate: { hotkey: 'Alt+7' },
      lens: { hotkey: 'Alt+8' },
    } as never),
    t,
    recordingTarget: null,
    onToggleRecording: vi.fn(),
    conflictMessageFor: vi.fn().mockReturnValue(undefined),
    hotkeyConflicts: {},
    onUpdateSettings: vi.fn(),
    onUpdateScreenshotTranslation: vi.fn(),
    onUpdateScreenshotAnnotate: vi.fn(),
    onUpdateLens: vi.fn(),
    ...overrides,
  }
  render(<HotkeysTab {...props} />)
  return props
}

describe('HotkeysTab', () => {
  it('每个快捷键行显示各自 settings 字段的值（不串行）', () => {
    renderTab()
    // HotkeyInput 把值拆成按键徽章渲染（修饰键随平台变形），故断言各行独有的数字键。
    // 八个值的数字互不相同，任一 props 接错都会让某个数字缺失或重复。
    for (const digit of ['1', '2', '3', '4', '5', '6', '7', '8']) {
      expect(screen.getAllByText(digit)).toHaveLength(1)
    }
  })

  it('渲染全部八个快捷键行', () => {
    renderTab()
    // 每行一个「录制」按钮
    expect(screen.getAllByRole('button', { name: t.hotkeyRecord })).toHaveLength(8)
  })

  it('录制按钮把对应 scope 传给 onToggleRecording', async () => {
    const props = renderTab()
    const buttons = screen.getAllByRole('button', { name: t.hotkeyRecord })
    await userEvent.click(buttons[0])
    expect(props.onToggleRecording).toHaveBeenCalledWith('main')
    await userEvent.click(buttons[1])
    expect(props.onToggleRecording).toHaveBeenCalledWith('chat')
    await userEvent.click(buttons[2])
    expect(props.onToggleRecording).toHaveBeenCalledWith('closeChat')
    // 第 4-6 个是截图翻译系列，第 7 个标注，第 8 个 Lens
    await userEvent.click(buttons[7])
    expect(props.onToggleRecording).toHaveBeenCalledWith('lens')
  })

  it('清除按钮走各自的 updater 且写对字段', async () => {
    const props = renderTab()
    const clears = screen.getAllByRole('button', { name: t.hotkeyClear })

    await userEvent.click(clears[0])
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ hotkey: '' })

    await userEvent.click(clears[1])
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ chatHotkey: '' })

    await userEvent.click(clears[2])
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ closeChatHotkey: '' })

    await userEvent.click(clears[3])
    expect(props.onUpdateScreenshotTranslation).toHaveBeenCalledWith({ hotkey: '' })

    await userEvent.click(clears[4])
    expect(props.onUpdateScreenshotTranslation).toHaveBeenCalledWith({ textHotkey: '' })

    await userEvent.click(clears[5])
    expect(props.onUpdateScreenshotTranslation).toHaveBeenCalledWith({ replaceHotkey: '' })

    await userEvent.click(clears[6])
    expect(props.onUpdateScreenshotAnnotate).toHaveBeenCalledWith({ hotkey: '' })

    await userEvent.click(clears[7])
    expect(props.onUpdateLens).toHaveBeenCalledWith({ hotkey: '' })
  })

  it('冲突提示按 scope 透传', () => {
    const conflictMessageFor = vi.fn((scope: string) =>
      scope === 'chat' ? '与「翻译」冲突' : undefined,
    )
    renderTab({ conflictMessageFor: conflictMessageFor as never })
    expect(screen.getByText('与「翻译」冲突')).toBeTruthy()
  })

  it('recordingTarget 只让对应行进入录制态', () => {
    renderTab({ recordingTarget: 'lens' })
    expect(screen.getAllByRole('button', { name: t.hotkeyRecording })).toHaveLength(1)
  })

  it('恢复默认写入全部默认热键字段', async () => {
    const props = renderTab()
    await userEvent.click(screen.getByRole('button', { name: t.hotkeyRestoreDefaults }))
    expect(props.onUpdateSettings).toHaveBeenCalledWith({
      hotkey: 'CommandOrControl+Alt+T',
      chatHotkey: 'CommandOrControl+Shift+K',
      closeChatHotkey: 'CommandOrControl+Shift+W',
    })
    expect(props.onUpdateScreenshotTranslation).toHaveBeenCalledWith({
      hotkey: 'CommandOrControl+Shift+A',
      textHotkey: 'CommandOrControl+Shift+T',
      replaceHotkey: 'CommandOrControl+Shift+R',
    })
    expect(props.onUpdateScreenshotAnnotate).toHaveBeenCalledWith({
      hotkey: 'CommandOrControl+Shift+S',
    })
    expect(props.onUpdateLens).toHaveBeenCalledWith({
      hotkey: 'CommandOrControl+Shift+G',
    })
    expect(screen.getByText(t.hotkeyRestoreDone)).toBeTruthy()
  })

  it('检查冲突：无冲突显示成功提示', async () => {
    renderTab({ hotkeyConflicts: {} })
    await userEvent.click(screen.getByRole('button', { name: t.hotkeyCheckConflicts }))
    expect(screen.getByText(t.hotkeyCheckOk)).toBeTruthy()
  })

  it('检查冲突：有冲突显示配对文案', async () => {
    renderTab({
      settings: makeSettings({
        hotkey: 'Alt+1',
        chatHotkey: 'Alt+1',
        closeChatHotkey: 'Alt+3',
      } as never),
      hotkeyConflicts: { main: 'chat', chat: 'main' },
    })
    await userEvent.click(screen.getByRole('button', { name: t.hotkeyCheckConflicts }))
    expect(screen.getByText(t.hotkeyCheckFound.replace('{count}', '1'))).toBeTruthy()
    expect(
      screen.getByText(
        t.hotkeyCheckPair
          .replace('{a}', t.tabTranslate)
          .replace('{b}', t.chatHotkeyLabel)
          .replace('{hotkey}', 'Alt+1'),
      ),
    ).toBeTruthy()
  })
})
