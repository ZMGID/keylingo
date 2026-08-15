import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { AppearanceGroup, BehaviorGroup, PermissionsGroup } from './GeneralTab'
import { makeSettings } from './testFixtures'
import { i18n } from '../i18n'

const t = i18n.zh

/**
 * 回归重点：
 *   1. 字号输入的「边打字不 clamp、失焦才 clamp」两段行为（commit 标志位）
 *   2. 两个 FontPicker 各写 uiFontFamily / uiFontMono（同类型，接错不报错）
 *   3. retryEnabled 关闭时隐藏重试次数行
 */
describe('AppearanceGroup', () => {
  type Props = Parameters<typeof AppearanceGroup>[0]
  function renderGroup(overrides: Partial<Props> = {}) {
    const props: Props = {
      settings: makeSettings({ theme: 'dark', uiFontFamily: 'Inter', uiFontMono: 'Menlo' }),
      t,
      lang: 'zh' as const,
      themeColor: 'default',
      systemFonts: ['Inter', 'Menlo', 'Arial'],
      uiFontPxInput: '14',
      onUpdateSettings: vi.fn(),
      onUiFontPxInputChange: vi.fn(),
      onCommitUiFontPx: vi.fn(),
      ...overrides,
    }
    render(<AppearanceGroup {...props} />)
    return props
  }

  it('主题分段按当前 theme 高亮', () => {
    renderGroup()
    expect(screen.getByRole('button', { name: t.themeDark }).className).toContain('active')
    expect(screen.getByRole('button', { name: t.themeLight }).className).not.toContain('active')
  })

  it('主题切换写 theme', async () => {
    const props = renderGroup()
    await userEvent.click(screen.getByRole('button', { name: t.themeLight }))
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ theme: 'light' })
  })

  it('半透明侧边栏开关写 translucentSidebar', async () => {
    const props = renderGroup()
    await userEvent.click(screen.getByRole('switch', { name: '半透明侧边栏' }))
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ translucentSidebar: false })
  })

  it('字号输入时 commit=false（边打字不 clamp）', async () => {
    const props = renderGroup()
    await userEvent.type(screen.getByDisplayValue('14'), '5')
    expect(props.onUiFontPxInputChange).toHaveBeenCalled()
    expect(props.onCommitUiFontPx).toHaveBeenCalledWith(expect.any(String), false)
  })

  it('字号失焦时 commit=true（此时才 clamp）', async () => {
    const props = renderGroup()
    const input = screen.getByDisplayValue('14')
    await userEvent.click(input)
    await userEvent.tab()
    expect(props.onCommitUiFontPx).toHaveBeenCalledWith('14', true)
  })

  it('两个字体选择器各自回显 uiFontFamily / uiFontMono（不串）', () => {
    renderGroup()
    expect(screen.getByDisplayValue('Inter')).toBeTruthy()
    expect(screen.getByDisplayValue('Menlo')).toBeTruthy()
  })
})

describe('BehaviorGroup', () => {
  type Props = Parameters<typeof BehaviorGroup>[0]
  function renderGroup(overrides: Partial<Props> = {}) {
    const props: Props = {
      settings: makeSettings(),
      t,
      lang: 'zh' as const,
      retryAttemptsInput: '3',
      onUpdateSettings: vi.fn(),
      onRetryAttemptsChange: vi.fn(),
      onRetryAttemptsBlur: vi.fn(),
      ...overrides,
    }
    render(<BehaviorGroup {...props} />)
    return props
  }

  it('retryEnabled 开启时显示重试次数行', () => {
    renderGroup()
    expect(screen.getByText(t.retryAttempts)).toBeTruthy()
  })

  it('retryEnabled 关闭时隐藏重试次数行', () => {
    renderGroup({ settings: makeSettings({ retryEnabled: false }) })
    expect(screen.queryByText(t.retryAttempts)).toBeNull()
  })

  it('两个开关分别写 launchAtStartup / retryEnabled', async () => {
    const props = renderGroup()
    const toggles = screen.getAllByRole('switch')
    await userEvent.click(toggles[0])
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ launchAtStartup: true })
    await userEvent.click(toggles[2])
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ retryEnabled: false })
  })

  it('启动后最小化到托盘开关写 launchMinimizedToTray', async () => {
    const props = renderGroup()
    const toggles = screen.getAllByRole('switch')
    await userEvent.click(toggles[1])
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ launchMinimizedToTray: true })
  })
})

describe('PermissionsGroup', () => {
  it('两项权限各自透传 target', async () => {
    const onOpenPermissionSettings = vi.fn()
    render(
      <PermissionsGroup
        t={t}
        permissionStatus={{ platform: 'macos', accessibility: false, screenRecording: true } as never}
        permissionsLoading={false}
        onOpenPermissionSettings={onOpenPermissionSettings}
        onRefreshPermissions={vi.fn()}
      />,
    )
    const opens = screen.getAllByRole('button', { name: t.openSystemSettings })
    await userEvent.click(opens[0])
    expect(onOpenPermissionSettings).toHaveBeenCalledWith('accessibility')
  })

  it('已授权的项不显示前往设置按钮', () => {
    render(
      <PermissionsGroup
        t={t}
        permissionStatus={{ platform: 'macos', accessibility: true, screenRecording: true } as never}
        permissionsLoading={false}
        onOpenPermissionSettings={vi.fn()}
        onRefreshPermissions={vi.fn()}
      />,
    )
    expect(screen.getAllByText(t.permissionGranted)).toHaveLength(2)
  })
})
