/**
 * @vitest-environment jsdom
 */
import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { LangContext } from '../settings/i18n'
import { ChatTitlebar } from './ChatTitlebar'

describe('ChatTitlebar', () => {
  it('popout hideNav keeps a layout-occupying strip, not the floating settings overlay', () => {
    const { container } = render(
      <LangContext.Provider value="zh">
        <ChatTitlebar
          hideNav
          sidebarExpanded={false}
          onToggleSidebar={() => {}}
          onNewConversation={() => {}}
        >
          <span>pills</span>
        </ChatTitlebar>
      </LangContext.Provider>,
    )
    const strip = container.querySelector('.chat-titlebar-strip')
    expect(strip).toBeTruthy()
    expect(strip?.classList.contains('chat-titlebar-strip--settings')).toBe(false)
    expect(strip?.classList.contains('chat-titlebar-strip--solid')).toBe(true)
  })
})
