import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { getSettingsCached } from '../api/settingsCache'
import { chatApi } from './api'
import { Sidebar } from './Sidebar'
import type { ChatProject, ConversationListItem } from './types'

vi.mock('../api/settingsCache', () => ({
  getSettingsCached: vi.fn().mockResolvedValue({ chat: {} }),
}))

const project1: ChatProject = {
  id: 'project-1',
  name: '项目1',
  created_at: 1,
  updated_at: 1,
}

const project2: ChatProject = {
  id: 'project-2',
  name: '项目2',
  created_at: 2,
  updated_at: 2,
}

function conversation(id: string, title: string, project: ChatProject): ConversationListItem {
  return {
    id,
    title,
    preview: '',
    provider_id: 'provider',
    model: 'model',
    message_count: 1,
    created_at: 1,
    updated_at: 1,
    folder: project.name,
    project_id: project.id,
  }
}

afterEach(() => {
  vi.restoreAllMocks()
})

beforeEach(() => {
  vi.mocked(getSettingsCached).mockResolvedValue({ chat: {} } as Awaited<ReturnType<typeof getSettingsCached>>)
})

describe('Sidebar conversation navigation', () => {
  it('selects a conversation in another project without first opening that project new-chat view', async () => {
    const user = userEvent.setup()
    const target = conversation('conversation-2b', '项目2第二个对话', project2)
    vi.spyOn(chatApi, 'getProjects').mockResolvedValue([project1, project2])
    vi.spyOn(chatApi, 'getSets').mockResolvedValue([])
    vi.spyOn(chatApi, 'getAssistants').mockResolvedValue([])
    vi.spyOn(chatApi, 'getConversations').mockResolvedValue([
      conversation('conversation-1', '项目1对话', project1),
      conversation('conversation-2a', '项目2第一个对话', project2),
      target,
    ])
    vi.spyOn(chatApi, 'getConversationPins').mockResolvedValue({})

    const onSelectProject = vi.fn()
    const onSelectConversation = vi.fn()
    render(
      <Sidebar
        lang="zh"
        currentConversationId="conversation-1"
        selectedProject={project1}
        onSelectProject={onSelectProject}
        selectedSet={null}
        onSelectSet={vi.fn()}
        onSelectConversation={onSelectConversation}
        onNewConversation={vi.fn()}
        onOpenSettings={vi.fn()}
        onOpenExtensionsItem={vi.fn()}
        onSelectLang={vi.fn()}
        onCheckUpdate={vi.fn()}
        collapsed={false}
        onToggleCollapsed={vi.fn()}
        refreshKey={0}
        searchOpen={false}
        onSearchOpenChange={vi.fn()}
      />,
    )

    await user.click(await screen.findByRole('button', { name: '项目', current: false }))
    await user.click(await screen.findByRole('button', { name: target.title }))

    expect(onSelectProject).not.toHaveBeenCalled()
    expect(onSelectConversation).toHaveBeenCalledOnce()
    expect(onSelectConversation).toHaveBeenCalledWith(
      target.id,
      target,
      { project: project2, set: null },
    )
    await waitFor(() => expect(chatApi.getConversations).toHaveBeenCalled())
  })

  it('opens the only conversation in another project on the first click', async () => {
    const user = userEvent.setup()
    const onlyConversation = conversation('conversation-only', '项目2唯一对话', project2)
    vi.spyOn(chatApi, 'getProjects').mockResolvedValue([project1, project2])
    vi.spyOn(chatApi, 'getSets').mockResolvedValue([])
    vi.spyOn(chatApi, 'getAssistants').mockResolvedValue([])
    vi.spyOn(chatApi, 'getConversations').mockResolvedValue([
      conversation('conversation-1', '项目1对话', project1),
      onlyConversation,
    ])
    vi.spyOn(chatApi, 'getConversationPins').mockResolvedValue({})

    const onSelectProject = vi.fn()
    const onSelectConversation = vi.fn()
    render(
      <Sidebar
        lang="zh"
        currentConversationId="conversation-1"
        selectedProject={project1}
        onSelectProject={onSelectProject}
        selectedSet={null}
        onSelectSet={vi.fn()}
        onSelectConversation={onSelectConversation}
        onNewConversation={vi.fn()}
        onOpenSettings={vi.fn()}
        onOpenExtensionsItem={vi.fn()}
        onSelectLang={vi.fn()}
        onCheckUpdate={vi.fn()}
        collapsed={false}
        onToggleCollapsed={vi.fn()}
        refreshKey={0}
        searchOpen={false}
        onSearchOpenChange={vi.fn()}
      />,
    )

    await user.click(await screen.findByRole('button', { name: '项目', current: false }))
    await user.click(await screen.findByRole('button', { name: onlyConversation.title }))

    expect(onSelectProject).not.toHaveBeenCalled()
    expect(onSelectConversation).toHaveBeenCalledOnce()
    expect(onSelectConversation).toHaveBeenCalledWith(
      onlyConversation.id,
      onlyConversation,
      { project: project2, set: null },
    )
  })
})
