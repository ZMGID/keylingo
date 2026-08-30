import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { RunStatusCapsule } from './RunStatusCapsule'
import type { AutomationRunSummary } from './types'

const runs: AutomationRunSummary[] = [
  { id: '1', origin: 'manual', status: 'success', startedAt: '2026-08-30T04:57:00.000Z' },
  { id: '2', origin: 'hotkey', status: 'error', startedAt: '2026-08-30T03:01:00.000Z', error: 'timeout' },
]

describe('RunStatusCapsule', () => {
  it('collapsed pill shows the last run, click reveals recent history', () => {
    render(
      <RunStatusCapsule
        running={false}
        runs={runs}
        error=""
        liveStartedAt={null}
        resetKey="auto-1"
      />,
    )
    expect(screen.getByRole('button')).toHaveTextContent('成功')
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.queryByText('timeout')).toBeNull()

    fireEvent.click(screen.getByRole('button'))
    expect(screen.getByRole('dialog')).toBeInTheDocument()
    expect(screen.getByText('timeout')).toBeInTheDocument()
    expect(screen.getByText('失败')).toBeInTheDocument()
  })

  it('running state takes over the pill', () => {
    render(
      <RunStatusCapsule
        running
        runs={runs}
        error=""
        liveStartedAt="2026-08-30T05:00:00.000Z"
        resetKey="auto-1"
      />,
    )
    expect(screen.getByRole('button')).toHaveTextContent('运行中')
  })
})
