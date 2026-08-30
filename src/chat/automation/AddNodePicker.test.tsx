import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { AddNodePicker } from './AddNodePicker'

describe('AddNodePicker', () => {
  it('lists action nodes and reports the pick', () => {
    const onPick = vi.fn()
    render(<AddNodePicker kind="action" onPick={onPick} />)
    expect(screen.getByLabelText('添加节点')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /Agent 一轮/ }))
    expect(onPick).toHaveBeenCalledWith(expect.objectContaining({ type: 'action.agent' }))
  })

  it('lists triggers when the graph has none', () => {
    render(<AddNodePicker kind="trigger" onPick={vi.fn()} />)
    expect(screen.getByLabelText('添加触发器')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /手动/ })).toBeInTheDocument()
  })
})
