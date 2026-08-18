import { act, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { PiSessionTreePanel } from './PiSessionTreePanel'
import { chatApi } from './api'

vi.mock('./api', () => ({
  chatApi: {
    piSessionTree: vi.fn(),
    piForkMessages: vi.fn(),
    piSessionFork: vi.fn(),
    piSessionClone: vi.fn(),
    piSessionSwitch: vi.fn(),
  },
}))

const mockTree = vi.mocked(chatApi.piSessionTree)
const mockForkMessages = vi.mocked(chatApi.piForkMessages)
const mockFork = vi.mocked(chatApi.piSessionFork)
const mockClone = vi.mocked(chatApi.piSessionClone)
const onConversationChanged = vi.fn()

const snapshot = {
  tree: [
    {
      entry: { type: 'message', id: 'u1', parentId: null, message: { role: 'user', content: 'Inspect the parser' } },
      children: [
        {
          entry: {
            type: 'message',
            id: 'a1',
            parentId: 'u1',
            message: { role: 'assistant', content: [{ type: 'text', text: 'Parser inspected' }] },
          },
          children: [],
        },
      ],
    },
  ],
  leafId: 'a1',
  sessionId: 'session-1',
  sessionFile: '/tmp/session-1.jsonl',
}

const mutation = {
  cancelled: false,
  text: 'Inspect the parser',
  sessionId: 'session-2',
  sessionFile: '/tmp/session-2.jsonl',
  previousSessionId: 'session-1',
  previousSessionFile: '/tmp/session-1.jsonl',
  conversationId: 'conv-2',
  conversation: null,
}

describe('PiSessionTreePanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockTree.mockResolvedValue(snapshot)
    mockForkMessages.mockResolvedValue([{ entryId: 'u1', text: 'Inspect the parser' }])
    mockFork.mockResolvedValue(mutation)
    mockClone.mockResolvedValue({ ...mutation, text: null })
  })

  it('loads and expands the active Pi branch', async () => {
    render(<PiSessionTreePanel active conversationId="conv-1" lang="zh" onConversationChanged={onConversationChanged} />)

    expect(await screen.findByText('Inspect the parser')).toBeTruthy()
    expect(screen.getByText('Parser inspected')).toBeTruthy()
    expect(screen.getByTitle('当前 leaf')).toBeTruthy()
    expect(mockTree).toHaveBeenCalledWith('conv-1')
    expect(mockForkMessages).toHaveBeenCalledWith('conv-1')
  })

  it('forks through Pi and hands the new Kivio conversation to the host', async () => {
    render(<PiSessionTreePanel active conversationId="conv-1" lang="zh" onConversationChanged={onConversationChanged} />)
    const button = await screen.findByLabelText('从这里创建 Pi 原生 fork')

    await act(async () => { button.click() })

    await waitFor(() => expect(mockFork).toHaveBeenCalledWith('conv-1', 'u1'))
    await waitFor(() => expect(onConversationChanged).toHaveBeenCalledWith(
      'conv-2',
      undefined,
      'Inspect the parser',
    ))
  })

  it('clones the current Pi branch through the distinct clone command', async () => {
    render(<PiSessionTreePanel active conversationId="conv-1" lang="en" onConversationChanged={onConversationChanged} />)
    const button = await screen.findByLabelText('Clone current Pi branch')

    await act(async () => { button.click() })

    await waitFor(() => expect(mockClone).toHaveBeenCalledWith('conv-1'))
    expect(onConversationChanged).toHaveBeenCalledWith('conv-2', undefined, undefined)
  })

  it('discards a late tree response from the previous conversation', async () => {
    let resolveFirst!: (value: typeof snapshot) => void
    mockTree.mockImplementation((conversationId) => {
      if (conversationId === 'conv-1') {
        return new Promise((resolve) => { resolveFirst = resolve })
      }
      return Promise.resolve({
        ...snapshot,
        sessionId: 'session-new',
        tree: [{
          entry: { type: 'message', id: 'new-u1', parentId: null, message: { role: 'user', content: 'New conversation' } },
          children: [],
        }],
        leafId: 'new-u1',
      })
    })
    mockForkMessages.mockResolvedValue([])
    const { rerender } = render(
      <PiSessionTreePanel active conversationId="conv-1" lang="en" onConversationChanged={onConversationChanged} />,
    )
    await waitFor(() => expect(mockTree).toHaveBeenCalledWith('conv-1'))
    expect(mockForkMessages).not.toHaveBeenCalledWith('conv-1')

    rerender(<PiSessionTreePanel active conversationId="conv-2" lang="en" onConversationChanged={onConversationChanged} />)
    expect(await screen.findByText('New conversation')).toBeTruthy()
    await act(async () => { resolveFirst(snapshot) })

    expect(screen.queryByText('Inspect the parser')).toBeNull()
    expect(screen.getByText('New conversation')).toBeTruthy()
  })
})
