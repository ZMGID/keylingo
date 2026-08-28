import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ConversationContextMenu } from './ConversationContextMenu'

function renderMenu(
  lang: 'zh' | 'en',
  onExport = vi.fn(),
  onClose = vi.fn(),
  extra: {
    onRegenerateTitle?: () => void
    canRegenerateTitle?: boolean
    showNativeSession?: boolean
    nativeSessionId?: string | null
    nativeSessionLoading?: boolean
  } = {},
) {
  render(
    <ConversationContextMenu
      anchor={{ left: 0, top: 0 }}
      projects={[]}
      sets={[]}
      lang={lang}
      canRegenerateTitle={extra.canRegenerateTitle}
      onRegenerateTitle={extra.onRegenerateTitle ?? vi.fn()}
      onExport={onExport}
      onMoveToProject={vi.fn()}
      onMoveToSet={vi.fn()}
      onDelete={vi.fn()}
      onClose={onClose}
      showNativeSession={extra.showNativeSession}
      nativeSessionId={extra.nativeSessionId}
      nativeSessionLoading={extra.nativeSessionLoading}
    />,
  )
  return { onExport, onClose, onRegenerateTitle: extra.onRegenerateTitle }
}

describe('ConversationContextMenu export', () => {
  it('renders the localized Chinese action and closes after export', async () => {
    const user = userEvent.setup()
    const { onExport, onClose } = renderMenu('zh')
    await user.click(screen.getByRole('menuitem', { name: '导出' }))
    expect(onExport).toHaveBeenCalledOnce()
    // 关闭在退场动画结束后触发（useCloseAnimation：animationend / 超时兜底）
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce())
  })

  it('renders the English action', () => {
    renderMenu('en')
    expect(screen.getByRole('menuitem', { name: 'Export' })).toBeInTheDocument()
  })
})

describe('ConversationContextMenu popout', () => {
  it('renders the localized Chinese action and closes after opening', async () => {
    const user = userEvent.setup()
    const onOpenInPopout = vi.fn()
    const onClose = vi.fn()
    render(
      <ConversationContextMenu
        anchor={{ left: 0, top: 0 }}
        projects={[]}
        sets={[]}
        lang="zh"
        onRegenerateTitle={vi.fn()}
        onExport={vi.fn()}
        onMoveToProject={vi.fn()}
        onMoveToSet={vi.fn()}
        onOpenInPopout={onOpenInPopout}
        onDelete={vi.fn()}
        onClose={onClose}
      />,
    )
    await user.click(screen.getByRole('menuitem', { name: '在新窗口打开' }))
    expect(onOpenInPopout).toHaveBeenCalledOnce()
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce())
  })

  it('renders the English action', () => {
    render(
      <ConversationContextMenu
        anchor={{ left: 0, top: 0 }}
        projects={[]}
        sets={[]}
        lang="en"
        onRegenerateTitle={vi.fn()}
        onExport={vi.fn()}
        onMoveToProject={vi.fn()}
        onMoveToSet={vi.fn()}
        onOpenInPopout={vi.fn()}
        onDelete={vi.fn()}
        onClose={vi.fn()}
      />,
    )
    expect(screen.getByRole('menuitem', { name: 'Open in new window' })).toBeInTheDocument()
  })

  it('hides the action when no handler is provided', () => {
    renderMenu('zh')
    expect(screen.queryByRole('menuitem', { name: '在新窗口打开' })).not.toBeInTheDocument()
  })
})

describe('ConversationContextMenu regenerate title', () => {
  it('renders the localized Chinese action and closes after regenerate', async () => {
    const user = userEvent.setup()
    const onRegenerateTitle = vi.fn()
    const onClose = vi.fn()
    renderMenu('zh', vi.fn(), onClose, { onRegenerateTitle })
    await user.click(screen.getByRole('menuitem', { name: '重新生成标题' }))
    expect(onRegenerateTitle).toHaveBeenCalledOnce()
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce())
  })

  it('renders the English action', () => {
    renderMenu('en')
    expect(screen.getByRole('menuitem', { name: 'Regenerate title' })).toBeInTheDocument()
  })

  it('disables regenerate when the conversation has no messages', () => {
    renderMenu('zh', vi.fn(), vi.fn(), { canRegenerateTitle: false })
    expect(screen.getByRole('menuitem', { name: '重新生成标题' })).toBeDisabled()
  })
})

describe('ConversationContextMenu removed actions', () => {
  it('does not offer rename or pin', () => {
    renderMenu('zh')
    expect(screen.queryByRole('menuitem', { name: '重命名' })).not.toBeInTheDocument()
    expect(screen.queryByRole('menuitem', { name: '置顶聊天' })).not.toBeInTheDocument()
    expect(screen.queryByRole('menuitem', { name: /原生会话/ })).not.toBeInTheDocument()
  })
})

describe('ConversationContextMenu native session', () => {
  it('shows the unbound native session id for local CLI conversations', () => {
    renderMenu('zh', vi.fn(), vi.fn(), { showNativeSession: true, nativeSessionId: null })
    const item = screen.getByRole('menuitem', { name: /原生会话/ })
    expect(item).toHaveTextContent('尚未绑定')
    expect(item).toBeDisabled()
  })

  it('copies the bound native session id', async () => {
    const user = userEvent.setup()
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    })
    const onClose = vi.fn()
    const sessionId = '0194abcd-61e2-7113-b077-58d3d91fb3d7'
    renderMenu('zh', vi.fn(), onClose, { showNativeSession: true, nativeSessionId: sessionId })
    await user.click(screen.getByRole('menuitem', { name: /原生会话/ }))
    expect(writeText).toHaveBeenCalledWith(sessionId)
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce())
  })

  it('renders the English native session label', () => {
    renderMenu('en', vi.fn(), vi.fn(), {
      showNativeSession: true,
      nativeSessionId: 'thr_live',
    })
    expect(screen.getByRole('menuitem', { name: /Native session/ })).toBeInTheDocument()
  })
})
