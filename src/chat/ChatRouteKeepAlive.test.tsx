import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { ChatRouteKeepAlive } from './ChatRouteKeepAlive'

describe('ChatRouteKeepAlive', () => {
  it('切换设置页后复用原聊天 DOM 实例', () => {
    const { rerender } = render(
      <ChatRouteKeepAlive activeKey="conversation">
        <main data-testid="chat-pane">chat</main>
      </ChatRouteKeepAlive>,
    )
    const firstPane = document.querySelector('[data-testid="chat-pane"]')
    expect(firstPane).not.toBeNull()

    rerender(
      <ChatRouteKeepAlive activeKey="settings">
        <section data-testid="settings-pane">settings</section>
      </ChatRouteKeepAlive>,
    )
    expect(firstPane).toBeInTheDocument()
    expect((firstPane?.parentElement as HTMLElement).style.display).toBe('none')

    rerender(
      <ChatRouteKeepAlive activeKey="conversation">
        <main data-testid="chat-pane">chat updated</main>
      </ChatRouteKeepAlive>,
    )
    expect(document.querySelector('[data-testid="chat-pane"]')).toBe(firstPane)
    expect(firstPane).toHaveTextContent('chat updated')
  })
})
