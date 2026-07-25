import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { LensTab } from './LensTab'
import { makeSettings, makeProvider } from './testFixtures'
import { i18n } from '../i18n'

const t = i18n.zh

/**
 * 回归重点：
 *   1. enabled 总开关的三层嵌套条件（关掉时下面各组必须整体消失）
 *   2. onUpdateLens vs onUpdateSettings 分流 —— 图片归档写顶层 settings，其余写 lens
 */
function renderTab(settingsOverrides: Record<string, unknown> = {}) {
  const props = {
    settings: makeSettings({
      providers: [makeProvider()],
      lens: { enabled: true, hotkey: '', providerId: 'p1', model: 'gpt-4o' },
      ...settingsOverrides,
    } as never),
    t,
    lang: 'zh' as const,
    lensDefaults: { system: '默认系统提示', question: '默认问题提示' },
    onUpdateSettings: vi.fn(),
    onUpdateLens: vi.fn(),
  }
  render(<LensTab {...props} />)
  return props
}

describe('LensTab', () => {
  it('enabled 开启时渲染全部分组', () => {
    renderTab()
    expect(screen.getByText(t.lensSection)).toBeTruthy()
    expect(screen.getByText(t.engine)).toBeTruthy()
    // imageArchive 同时用作组标题和行标签，故 getAllByText
    expect(screen.getAllByText(t.imageArchive).length).toBeGreaterThan(0)
    expect(screen.getByText(t.customPrompts)).toBeTruthy()
  })

  it('enabled 关闭时隐藏下游全部分组（三层嵌套条件）', () => {
    renderTab({ lens: { enabled: false, hotkey: '' } })
    // 总开关所在组仍在
    expect(screen.getByText(t.lensSection)).toBeTruthy()
    // 下游各组必须整体消失
    expect(screen.queryByText(t.engine)).toBeNull()
    expect(screen.queryAllByText(t.imageArchive)).toHaveLength(0)
    expect(screen.queryByText(t.customPrompts)).toBeNull()
    expect(screen.queryByText(t.lensResponseLanguage)).toBeNull()
  })

  it('总开关写 lens.enabled 而非顶层 settings', async () => {
    const props = renderTab()
    const toggles = screen.getAllByRole('switch')
    await userEvent.click(toggles[0])
    expect(props.onUpdateLens).toHaveBeenCalledWith({ enabled: false })
    expect(props.onUpdateSettings).not.toHaveBeenCalled()
  })

  it('图片归档开关写顶层 settings（不是 lens）', async () => {
    const props = renderTab()
    const toggles = screen.getAllByRole('switch')
    // 顺序：enabled / stream / thinking / sendToChat / showCaptureHint / imageArchive
    await userEvent.click(toggles[toggles.length - 1])
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ imageArchiveEnabled: true })
  })

  it('imageArchiveEnabled 开启后才显示路径行', () => {
    renderTab()
    expect(screen.queryByText(t.imageArchivePath)).toBeNull()
    renderTab({ imageArchiveEnabled: true })
    expect(screen.getByText(t.imageArchivePath)).toBeTruthy()
  })

  it('提示词的默认值来自 lensDefaults（system / question 不串）', () => {
    renderTab()
    expect(screen.getByText(t.lensSystemPrompt)).toBeTruthy()
    expect(screen.getByText(t.lensQuestionPrompt)).toBeTruthy()
  })
})
