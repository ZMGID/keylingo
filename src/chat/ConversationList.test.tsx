import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { chatApi } from './api'
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

const listProps = {
  conversations: [conversation] as ConversationListItem[],
  projects: [] as [],
  sets: [] as [],
  lang: 'zh' as const,
  onSelectConversation: vi.fn(),
  onRenameConversation: vi.fn(),
  onRegenerateConversationTitle: vi.fn(),
  onTogglePinConversation: vi.fn(),
  onArchiveConversation: vi.fn(),
  onExportConversation: vi.fn(),
  onDeleteConversation: vi.fn(),
  onMoveConversationToProject: vi.fn(),
  onMoveConversationToSet: vi.fn(),
}

function renderList(onRenameConversation = vi.fn()) {
  render(
    <ConversationList
      {...listProps}
      onRenameConversation={onRenameConversation}
    />,
  )
  return onRenameConversation
}

afterEach(() => {
  vi.restoreAllMocks()
})

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

describe('ConversationList pin and archive', () => {
  it('toggles pin from the row pin button', async () => {
    const user = userEvent.setup()
    const onTogglePin = vi.fn()
    render(
      <ConversationList
        {...listProps}
        onTogglePinConversation={onTogglePin}
      />,
    )

    await user.click(screen.getByRole('button', { name: '置顶聊天' }))
    expect(onTogglePin).toHaveBeenCalledOnce()
    expect(onTogglePin).toHaveBeenCalledWith('conversation-1', true)
  })

  it('archives from the row archive button', async () => {
    const user = userEvent.setup()
    const onArchive = vi.fn()
    render(
      <ConversationList
        {...listProps}
        onArchiveConversation={onArchive}
      />,
    )

    await user.click(screen.getByRole('button', { name: '归档' }))
    expect(onArchive).toHaveBeenCalledOnce()
    expect(onArchive).toHaveBeenCalledWith('conversation-1')
  })

  it('collapses the archived row without remounting the rows below it', async () => {
    const user = userEvent.setup()
    const second: ConversationListItem = {
      ...conversation,
      id: 'conversation-2',
      title: '留下的对话',
    }
    const { container } = render(
      <ConversationList
        {...listProps}
        conversations={[conversation, second]}
      />,
    )

    await user.click(screen.getAllByRole('button', { name: '归档' })[0])

    const exiting = container.querySelector('.kv-conv-exit.is-exiting')
    expect(exiting).toBeTruthy()
    expect(exiting).toHaveTextContent('原会话标题')
    expect(screen.queryByRole('button', { name: '原会话标题' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: '留下的对话' })).toBeInTheDocument()
    const remainingWrap = screen.getByRole('button', { name: '留下的对话' }).closest('.kv-conv-exit')
    expect(remainingWrap).toBeTruthy()
    expect(remainingWrap).not.toHaveClass('is-exiting')
  })

  it('opens context menu on right-click without rename, pin, or native session', async () => {
    const user = userEvent.setup()
    const { container } = render(<ConversationList {...listProps} />)

    const row = container.querySelector('.kv-conv-row')
    expect(row).toBeTruthy()
    await user.pointer({ keys: '[MouseRight>]', target: row as Element })

    expect(screen.getByRole('menuitem', { name: '重新生成标题' })).toBeInTheDocument()
    expect(screen.queryByRole('menuitem', { name: '重命名' })).not.toBeInTheDocument()
    expect(screen.queryByRole('menuitem', { name: '置顶聊天' })).not.toBeInTheDocument()
    expect(screen.queryByRole('menuitem', { name: /原生会话/ })).not.toBeInTheDocument()
  })

  it('shows the bound native session id for local CLI conversations', async () => {
    const user = userEvent.setup()
    vi.spyOn(chatApi, 'getExternalNativeSessionId').mockResolvedValue(
      '0194abcd-61e2-7113-b077-58d3d91fb3d7',
    )
    const { container } = render(
      <ConversationList
        {...listProps}
        conversations={[
          {
            ...conversation,
            agent_runtime: { kind: 'external', externalAgentId: 'codex' },
          },
        ]}
      />,
    )

    const row = container.querySelector('.kv-conv-row')
    expect(row).toBeTruthy()
    await user.pointer({ keys: '[MouseRight>]', target: row as Element })

    await waitFor(() => {
      expect(screen.getByRole('menuitem', { name: /原生会话/ })).toHaveTextContent('0194abcd')
    })
  })

  it('regenerates title from the context menu', async () => {
    const user = userEvent.setup()
    const onRegenerate = vi.fn()
    const { container } = render(
      <ConversationList
        {...listProps}
        onRegenerateConversationTitle={onRegenerate}
      />,
    )

    const row = container.querySelector('.kv-conv-row')
    expect(row).toBeTruthy()
    await user.pointer({ keys: '[MouseRight>]', target: row as Element })

    await user.click(screen.getByRole('menuitem', { name: '重新生成标题' }))
    expect(onRegenerate).toHaveBeenCalledWith('conversation-1')
  })
})

describe('ConversationList generating wave', () => {
  it('puts wave and actions in the same replace slot while generating', () => {
    const { container } = render(
      <ConversationList
        {...listProps}
        generatingConversationIds={new Set(['conversation-1'])}
      />,
    )

    const trailing = container.querySelector('.kv-conv-trailing')
    expect(trailing).toBeTruthy()
    // data-busy 存在（值为空串也算）→ CSS 走互斥替换，而不是并排
    expect(trailing).toHaveAttribute('data-busy')
    expect(trailing?.querySelector('.kv-conv-wave.chat-gen-wave')).toBeTruthy()
    expect(trailing?.querySelector('.kv-conv-actions')).toBeTruthy()
    expect(screen.getByRole('status', { name: '正在生成' })).toBeInTheDocument()
  })

  it('still fires pin while generating — the wave must not swallow the click', async () => {
    const user = userEvent.setup()
    const onTogglePin = vi.fn()
    render(
      <ConversationList
        {...listProps}
        generatingConversationIds={new Set(['conversation-1'])}
        onTogglePinConversation={onTogglePin}
      />,
    )

    await user.click(screen.getByRole('button', { name: '置顶聊天', hidden: true }))
    expect(onTogglePin).toHaveBeenCalledWith('conversation-1', true)
  })

  it('keeps pin visible when already pinned, without the wave', () => {
    const { container } = render(
      <ConversationList
        {...listProps}
        conversations={[{ ...conversation, pinned: true }]}
        generatingConversationIds={new Set(['conversation-1'])}
      />,
    )

    const trailing = container.querySelector('.kv-conv-trailing')
    expect(trailing).not.toHaveAttribute('data-busy')
    expect(screen.queryByRole('status', { name: '正在生成' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: '取消置顶' })).toBeInTheDocument()
  })
})



