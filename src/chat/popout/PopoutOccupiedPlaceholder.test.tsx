/**
 * @vitest-environment jsdom
 */
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { LangContext } from '../../settings/i18n'
import { PopoutOccupiedPlaceholder } from './PopoutOccupiedPlaceholder'

describe('PopoutOccupiedPlaceholder', () => {
  it('uses the shared Button size and docks the conversation back', async () => {
    const user = userEvent.setup()
    const onFocus = vi.fn()
    const onDock = vi.fn()
    render(
      <LangContext.Provider value="zh">
        <PopoutOccupiedPlaceholder
          lang="zh"
          onFocus={onFocus}
          onDock={onDock}
          sidebarCollapsed={false}
          titlebarControls={null}
          onToggleSidebar={() => {}}
          onNewConversation={() => {}}
        />
      </LangContext.Provider>,
    )

    const focus = screen.getByRole('button', { name: '显示独立窗口' })
    const dock = screen.getByRole('button', { name: '回归窗口' })
    expect(focus.className.split(/\s+/)).toContain('kv-btn')
    expect(dock.className.split(/\s+/)).toContain('kv-btn')
    expect(focus.className.split(/\s+/)).not.toContain('sm')
    expect(dock.className.split(/\s+/)).not.toContain('sm')

    await user.click(dock)
    expect(onDock).toHaveBeenCalledOnce()
    expect(onFocus).not.toHaveBeenCalled()
  })
})
