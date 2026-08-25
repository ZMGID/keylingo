import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { RightDock } from './RightDock'

vi.mock('./FileTreePanel', () => ({ FileTreePanel: () => <div /> }))
vi.mock('./GitPanel', () => ({ GitPanel: () => <div /> }))
vi.mock('./TerminalPanel', () => ({ TerminalPanel: () => <div data-testid="terminal-panel" /> }))
vi.mock('./BackgroundTasksPanel', () => ({ BackgroundTasksPanel: () => <div /> }))

function renderDock(activeTab: 'files' | 'git' | 'terminal' | 'tasks' = 'files') {
  return render(
    <RightDock
      open
      width={360}
      activeTab={activeTab}
      workdir="/tmp/project"
      lang="zh"
      conversationId="conv-1"
      treeExpanded={[]}
      revealRequest={null}
      previewRequest={null}
      onToggleTab={vi.fn()}
      onWidthChange={vi.fn()}
      onClose={vi.fn()}
      onTreeExpandedChange={vi.fn()}
      onRevealInTree={vi.fn()}
    />,
  )
}

describe('RightDock tabs', () => {
  it('exposes files, git, terminal, and tasks — not trajectory', () => {
    renderDock('files')
    expect(screen.getByText('文件')).toBeTruthy()
    expect(screen.getByText('Git')).toBeTruthy()
    expect(screen.getByText('终端')).toBeTruthy()
    expect(screen.getByText('任务')).toBeTruthy()
    expect(screen.queryByText('轨迹')).toBeNull()
  })

  it('does not mount the terminal until that tab is opened', () => {
    const { rerender } = renderDock('files')
    expect(screen.queryByTestId('terminal-panel')).toBeNull()

    rerender(
      <RightDock
        open
        width={360}
        activeTab="terminal"
        workdir="/tmp/project"
        lang="zh"
        conversationId="conv-1"
        treeExpanded={[]}
        revealRequest={null}
        previewRequest={null}
        onToggleTab={vi.fn()}
        onWidthChange={vi.fn()}
        onClose={vi.fn()}
        onTreeExpandedChange={vi.fn()}
        onRevealInTree={vi.fn()}
      />,
    )
    expect(screen.getByTestId('terminal-panel')).toBeTruthy()
  })
})
