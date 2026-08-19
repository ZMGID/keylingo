import { act, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { TrajectoryPanel } from './TrajectoryPanel'
import { chatApi } from './api'
import type { Conversation } from './types'

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
const onFocusMessage = vi.fn()

function conversation(partial?: Partial<Conversation>): Conversation {
  return {
    id: 'conv-1',
    revision: 1,
    title: 'Demo',
    provider_id: 'local',
    model: 'kivio',
    created_at: 1,
    updated_at: 2,
    messages: [
      { id: 'u1', role: 'user', content: 'Inspect the parser', timestamp: 1 },
      {
        id: 'a1',
        role: 'assistant',
        content: 'Parser inspected',
        timestamp: 2,
        toolCalls: [{
          id: 'c1',
          toolName: 'read_file',
          argumentPreview: 'src/parser.ts',
          resultPreview: '40 lines',
          status: 'success',
        }],
      },
    ],
    ...partial,
  }
}

const snapshot = {
  tree: [],
  leafId: null,
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

describe('TrajectoryPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockTree.mockResolvedValue(snapshot)
    mockForkMessages.mockResolvedValue([{ entryId: 'u1', text: 'Inspect the parser' }])
    mockFork.mockResolvedValue(mutation)
    mockClone.mockResolvedValue({ ...mutation, text: null })
  })

  it('renders a conversation ledger for Kivio without touching Pi', async () => {
    const current = conversation()
    render(
      <TrajectoryPanel
        active
        conversation={current}
        messages={current.messages}
        lang="zh"
        piNativeEnabled={false}
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )

    expect(screen.getByText('Inspect the parser')).toBeTruthy()
    expect(screen.getByText('read_file')).toBeTruthy()
    expect(screen.getByText('Parser inspected')).toBeTruthy()
    expect(screen.getByText(/1 轮/)).toBeTruthy()
    expect(mockTree).not.toHaveBeenCalled()
    await userEvent.click(screen.getByText('Inspect the parser'))
    expect(onFocusMessage).toHaveBeenCalledWith('u1')
  })

  it('filters the ledger from the search box', async () => {
    const current = conversation()
    render(
      <TrajectoryPanel
        active
        conversation={current}
        messages={current.messages}
        lang="en"
        piNativeEnabled={false}
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    await userEvent.type(screen.getByLabelText('Search trajectory'), 'read_file')
    expect(screen.getByText('read_file')).toBeTruthy()
    expect(screen.queryByText('Inspect the parser')).toBeNull()
  })

  it('forks through Pi using the mapped user message', async () => {
    const current = conversation({
      agent_runtime: { kind: 'external', externalAgentId: 'pi' },
    })
    render(
      <TrajectoryPanel
        active
        conversation={current}
        messages={current.messages}
        lang="zh"
        piNativeEnabled
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    const button = await screen.findByLabelText('从这里分叉')
    await act(async () => { button.click() })
    await waitFor(() => expect(mockFork).toHaveBeenCalledWith('conv-1', 'u1'))
    await waitFor(() => expect(onConversationChanged).toHaveBeenCalledWith(
      'conv-2',
      undefined,
      'Inspect the parser',
    ))
  })

  it('maps duplicate user prompts to distinct Pi fork entries in order', async () => {
    mockForkMessages.mockResolvedValue([
      { entryId: 'e1', text: 'Same prompt' },
      { entryId: 'e2', text: 'Same prompt' },
    ])
    const current = conversation({
      agent_runtime: { kind: 'external', externalAgentId: 'pi' },
      messages: [
        { id: 'u1', role: 'user', content: 'Same prompt', timestamp: 1 },
        { id: 'a1', role: 'assistant', content: 'First', timestamp: 2 },
        { id: 'u2', role: 'user', content: 'Same prompt', timestamp: 3 },
        { id: 'a2', role: 'assistant', content: 'Second', timestamp: 4 },
      ],
    })
    render(
      <TrajectoryPanel
        active
        conversation={current}
        messages={current.messages}
        lang="zh"
        piNativeEnabled
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    const buttons = await screen.findAllByLabelText('从这里分叉')
    expect(buttons).toHaveLength(2)
    await act(async () => { buttons[0].click() })
    await waitFor(() => expect(mockFork).toHaveBeenCalledWith('conv-1', 'e1'))
  })

  it('clones the current Pi branch', async () => {
    const current = conversation({
      agent_runtime: { kind: 'external', externalAgentId: 'pi' },
    })
    render(
      <TrajectoryPanel
        active
        conversation={current}
        messages={current.messages}
        lang="en"
        piNativeEnabled
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    const button = await screen.findByLabelText('Clone current branch')
    await act(async () => { button.click() })
    await waitFor(() => expect(mockClone).toHaveBeenCalledWith('conv-1'))
    expect(onConversationChanged).toHaveBeenCalledWith('conv-2', undefined, undefined)
  })

  it('discards a late Pi response from the previous conversation', async () => {
    let resolveFirst!: (value: typeof snapshot) => void
    mockTree.mockImplementation((id) => {
      if (id === 'conv-1') return new Promise((resolve) => { resolveFirst = resolve })
      return Promise.resolve({ ...snapshot, sessionId: 'session-new', sessionFile: '/tmp/new.jsonl' })
    })
    mockForkMessages.mockResolvedValue([])
    const first = conversation()
    const second = conversation({
      id: 'conv-2',
      messages: [{ id: 'new-u1', role: 'user', content: 'New conversation', timestamp: 3 }],
    })
    const { rerender } = render(
      <TrajectoryPanel
        active
        conversation={first}
        messages={first.messages}
        lang="en"
        piNativeEnabled
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    await waitFor(() => expect(mockTree).toHaveBeenCalledWith('conv-1'))
    rerender(
      <TrajectoryPanel
        active
        conversation={second}
        messages={second.messages}
        lang="en"
        piNativeEnabled
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    expect(await screen.findByText('New conversation')).toBeTruthy()
    await act(async () => { resolveFirst(snapshot) })
    expect(screen.queryByText('Inspect the parser')).toBeNull()
    expect(screen.getByText('New conversation')).toBeTruthy()
  })

  it('clears Pi identity and shows an error when native session load fails', async () => {
    mockTree.mockRejectedValue(new Error('boom'))
    const current = conversation({
      agent_runtime: { kind: 'external', externalAgentId: 'pi' },
    })
    render(
      <TrajectoryPanel
        active
        conversation={current}
        messages={current.messages}
        lang="zh"
        piNativeEnabled
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    expect(await screen.findByText('无法加载 Pi 原生会话')).toBeTruthy()
    expect(screen.getByLabelText('克隆当前分支')).toBeDisabled()
  })

  it('does not keep the previous conversation Pi identity while the next load is in flight', async () => {
    let resolveSecond!: (value: typeof snapshot) => void
    mockTree.mockImplementation((id) => {
      if (id === 'conv-1') return Promise.resolve(snapshot)
      return new Promise((resolve) => { resolveSecond = resolve })
    })
    mockForkMessages.mockResolvedValue([])
    const first = conversation({
      agent_runtime: { kind: 'external', externalAgentId: 'pi' },
    })
    const second = conversation({
      id: 'conv-2',
      agent_runtime: { kind: 'external', externalAgentId: 'pi' },
      messages: [{ id: 'new-u1', role: 'user', content: 'New conversation', timestamp: 3 }],
    })
    const { rerender } = render(
      <TrajectoryPanel
        active
        conversation={first}
        messages={first.messages}
        lang="en"
        piNativeEnabled
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    expect(await screen.findByLabelText('Clone current branch')).not.toBeDisabled()
    rerender(
      <TrajectoryPanel
        active
        conversation={second}
        messages={second.messages}
        lang="en"
        piNativeEnabled
        onFocusMessage={onFocusMessage}
        onConversationChanged={onConversationChanged}
      />,
    )
    expect(screen.getByLabelText('Clone current branch')).toBeDisabled()
    await act(async () => {
      resolveSecond({ ...snapshot, sessionId: 'session-new', sessionFile: '/tmp/new.jsonl' })
    })
    expect(await screen.findByLabelText('Clone current branch')).not.toBeDisabled()
  })
})
