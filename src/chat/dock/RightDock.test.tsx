import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { RightDock } from './RightDock'

vi.mock('./FileTreePanel', () => ({ FileTreePanel: () => <div /> }))
vi.mock('./GitPanel', () => ({ GitPanel: () => <div /> }))
vi.mock('./TerminalPanel', () => ({ TerminalPanel: () => <div /> }))
vi.mock('./BackgroundTasksPanel', () => ({ BackgroundTasksPanel: () => <div /> }))
vi.mock('../PiSessionTreePanel', () => ({
  PiSessionTreePanel: ({ active }: { active: boolean }) => <div data-testid="pi-tree">{String(active)}</div>,
}))

function renderDock(piSessionsEnabled: boolean, activeTab: 'files' | 'piSessions' = 'files') {
  return render(
    <RightDock
      open
      width={360}
      activeTab={activeTab}
      workdir="/tmp/project"
      lang="zh"
      conversationId="conv-1"
      piSessionsEnabled={piSessionsEnabled}
      treeExpanded={[]}
      revealRequest={null}
      previewRequest={null}
      onToggleTab={vi.fn()}
      onWidthChange={vi.fn()}
      onClose={vi.fn()}
      onTreeExpandedChange={vi.fn()}
      onPiConversationChanged={vi.fn()}
      onRevealInTree={vi.fn()}
    />,
  )
}

describe('RightDock Pi sessions tab', () => {
  it('shows and activates the native Pi tree only for Pi conversations', () => {
    renderDock(true, 'piSessions')
    expect(screen.getByText('Pi 会话')).toBeTruthy()
    expect(screen.getByTestId('pi-tree').textContent).toBe('true')
  })

  it('does not expose the Pi tree in other runtimes', () => {
    renderDock(false)
    expect(screen.queryByText('Pi 会话')).toBeNull()
    expect(screen.queryByTestId('pi-tree')).toBeNull()
  })
})
