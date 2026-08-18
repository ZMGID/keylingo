import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { RightDock } from './RightDock'
import type { Conversation } from '../types'

vi.mock('./FileTreePanel', () => ({ FileTreePanel: () => <div /> }))
vi.mock('./GitPanel', () => ({ GitPanel: () => <div /> }))
vi.mock('./TerminalPanel', () => ({ TerminalPanel: () => <div /> }))
vi.mock('./BackgroundTasksPanel', () => ({ BackgroundTasksPanel: () => <div /> }))
vi.mock('../TrajectoryPanel', () => ({
  TrajectoryPanel: ({ active }: { active: boolean }) => <div data-testid="trajectory">{String(active)}</div>,
}))

const conversation: Conversation = {
  id: 'conv-1',
  revision: 1,
  title: 'Demo',
  provider_id: 'local',
  model: 'kivio',
  created_at: 1,
  updated_at: 2,
  messages: [{ id: 'u1', role: 'user', content: 'hello', timestamp: 1 }],
}

function renderDock(activeTab: 'files' | 'trajectory' = 'files') {
  return render(
    <RightDock
      open
      width={360}
      activeTab={activeTab}
      workdir="/tmp/project"
      lang="zh"
      conversationId="conv-1"
      conversation={conversation}
      messages={conversation.messages}
      piNativeEnabled={false}
      treeExpanded={[]}
      revealRequest={null}
      previewRequest={null}
      onToggleTab={vi.fn()}
      onWidthChange={vi.fn()}
      onClose={vi.fn()}
      onTreeExpandedChange={vi.fn()}
      onPiConversationChanged={vi.fn()}
      onFocusMessage={vi.fn()}
      onRevealInTree={vi.fn()}
    />,
  )
}

describe('RightDock trajectory tab', () => {
  it('always exposes the generic trajectory tab', () => {
    renderDock('trajectory')
    expect(screen.getByText('轨迹')).toBeTruthy()
    expect(screen.getByTestId('trajectory').textContent).toBe('true')
  })

  it('does not mount the trajectory panel until the tab is opened', () => {
    renderDock('files')
    expect(screen.getByText('轨迹')).toBeTruthy()
    expect(screen.queryByTestId('trajectory')).toBeNull()
  })
})
