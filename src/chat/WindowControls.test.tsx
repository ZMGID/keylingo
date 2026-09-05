import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { LangContext } from '../settings/i18n'

type ResizeEvent = { payload: { width: number; height: number } }

const native = vi.hoisted(() => ({
  maximized: false,
  resizeHandlers: [] as Array<(event: ResizeEvent) => void>,
  toggle: vi.fn(),
}))

vi.mock('./platform', () => ({ isMac: false, isWindows: true, usesNativeTitlebar: false }))
vi.mock('./utils', () => ({ isTauriRuntime: () => true }))
vi.mock('./chatWindowEffects', () => ({ syncChatWindowEffect: async () => false }))
vi.mock('../api/tauri', () => ({ api: { toggleMaximizeWindow: () => native.toggle() } }))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => native.maximized,
    isFocused: async () => true,
    innerSize: async () => ({ width: 1280, height: 800 }),
    onResized: async (handler: (event: ResizeEvent) => void) => {
      native.resizeHandlers.push(handler)
      return () => { native.resizeHandlers = native.resizeHandlers.filter(cb => cb !== handler) }
    },
    onFocusChanged: async () => () => {},
  }),
}))

const { ChatWindowHost } = await import('./ChatWindowHost')
const { WindowControls } = await import('./WindowControls')

async function renderControls() {
  render(
    <LangContext.Provider value="zh">
      <ChatWindowHost translucentSidebar={false}><WindowControls /></ChatWindowHost>
    </LangContext.Provider>,
  )
  await act(async () => { await vi.advanceTimersByTimeAsync(0) })
}

beforeEach(() => {
  vi.useFakeTimers()
  native.maximized = false
  native.resizeHandlers = []
  native.toggle.mockReset().mockImplementation(async () => {
    native.maximized = !native.maximized
    native.resizeHandlers.forEach(handler => handler({ payload: { width: 1280, height: 800 } }))
  })
})
afterEach(() => { vi.useRealTimers() })

it('reflects native maximize events and switches back after restoring with the button', async () => {
  await renderControls()
  expect(screen.getByRole('button', { name: '最大化' }).querySelector('.lucide-square')).toBeTruthy()

  // 模拟双击标题栏或系统快捷键最大化，未点击 React 按钮。
  await act(async () => {
    native.maximized = true
    native.resizeHandlers.forEach(handler => handler({ payload: { width: 1920, height: 1080 } }))
    await vi.advanceTimersByTimeAsync(150)
  })
  const restore = screen.getByRole('button', { name: '还原窗口' })
  expect(restore.querySelector('.lucide-copy')).toBeTruthy()
  expect(restore).toHaveAttribute('title', '还原窗口')

  fireEvent.click(restore)
  await act(async () => { await vi.advanceTimersByTimeAsync(150) })
  expect(native.toggle).toHaveBeenCalledOnce()
  expect(screen.getByRole('button', { name: '最大化' }).querySelector('.lucide-square')).toBeTruthy()
})

it('shows restore immediately when opened in an already maximized window', async () => {
  native.maximized = true
  await renderControls()
  expect(screen.getByRole('button', { name: '还原窗口' }).querySelector('.lucide-copy')).toBeTruthy()
})
