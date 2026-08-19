import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ConversationContextMenu } from './ConversationContextMenu'

function renderMenu(
  lang: 'zh' | 'en',
  onExport = vi.fn(),
  onClose = vi.fn(),
  extra: { onRegenerateTitle?: () => void; canRegenerateTitle?: boolean } = {},
) {
  render(
    <ConversationContextMenu
      anchor={{ left: 0, top: 0 }}
      projects={[]}
      sets={[]}
      lang={lang}
      onRename={vi.fn()}
      canRegenerateTitle={extra.canRegenerateTitle}
      onRegenerateTitle={extra.onRegenerateTitle ?? vi.fn()}
      onExport={onExport}
      onMoveToProject={vi.fn()}
      onMoveToSet={vi.fn()}
      onDelete={vi.fn()}
      onClose={onClose}
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
