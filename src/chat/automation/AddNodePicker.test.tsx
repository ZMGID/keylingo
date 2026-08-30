import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { AddNodePicker } from './AddNodePicker'

describe('AddNodePicker', () => {
  it('lists action nodes and reports the pick', () => {
    const onPick = vi.fn()
    render(<AddNodePicker kind="action" presentTypes={['trigger.manual']} onPick={onPick} />)
    expect(screen.getByLabelText('接下来做什么？')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /执行命令/ }))
    expect(onPick).toHaveBeenCalledWith(expect.objectContaining({ type: 'action.command' }))
  })

  it('lists unused triggers under add-another-trigger', () => {
    render(<AddNodePicker kind="action" presentTypes={['trigger.manual']} onPick={vi.fn()} />)
    expect(screen.getByText('添加另一个触发器')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /定时/ })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /手动/ })).not.toBeInTheDocument()
  })

  it('lists triggers when the graph has none', () => {
    render(<AddNodePicker kind="trigger" onPick={vi.fn()} />)
    expect(screen.getByLabelText('添加触发器')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /手动/ })).toBeInTheDocument()
  })
})
