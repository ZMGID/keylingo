import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ConversationList } from './ConversationList'
import type { ConversationListItem } from './types'

const conversation: ConversationListItem = {
  id: 'conversation-1',
  title: '原会话标题',
  preview: '最近一条消息',
  provider_id: 'provider',
  model: 'model',
  message_count: 2,
  created_at: 1,
  updated_at: 1,
}

function renderList(onRenameConversation = vi.fn()) {
  render(
    <ConversationList
      conversations={[conversation]}
      projects={[]}
      sets={[]}
      lang="zh"
      onSelectConversation={vi.fn()}
      onRenameConversation={onRenameConversation}
      onExportConversation={vi.fn()}
      onDeleteConversation={vi.fn()}
      onMoveConversationToProject={vi.fn()}
      onMoveConversationToSet={vi.fn()}
    />,
  )
  return onRenameConversation
}

describe('ConversationList inline rename', () => {
  it('opens rename input on double click and commits with Enter', async () => {
    const user = userEvent.setup()
    const onRename = renderList()

    await user.dblClick(screen.getByRole('button', { name: '原会话标题' }))
    const input = screen.getByDisplayValue('原会话标题')
    expect(input).toHaveClass('border-0', 'bg-transparent')
    expect(input.closest('[data-reorder-id="conversation-1"]')).toHaveClass('kv-conv-row')
    await user.clear(input)
    await user.type(input, '改名后的会话')
    await user.keyboard('{Enter}')

    expect(onRename).toHaveBeenCalledOnce()
    expect(onRename).toHaveBeenCalledWith('conversation-1', '改名后的会话')
  })
})
