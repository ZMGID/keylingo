import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { ApprovalCard } from './ApprovalCard'

describe('ApprovalCard', () => {
  it('展示提交错误，并阻止禁用动作被鼠标或快捷键重复触发', () => {
    const deny = vi.fn()
    const approve = vi.fn()

    render(
      <ApprovalCard
        title="工具审批"
        error="提交失败，请重试"
        actions={[
          { label: '拒绝', disabled: true, onSelect: deny },
          { label: '允许', primary: true, disabled: true, onSelect: approve },
        ]}
      />,
    )

    expect(screen.getByRole('alert')).toHaveTextContent('提交失败，请重试')
    fireEvent.click(screen.getByRole('button', { name: /拒绝/ }))
    fireEvent.click(screen.getByRole('button', { name: /允许/ }))
    fireEvent.keyDown(window, { key: 'Escape' })
    fireEvent.keyDown(window, { key: 'Enter', ctrlKey: true })
    fireEvent.keyDown(window, { key: '1' })
    fireEvent.keyDown(window, { key: '2' })

    expect(deny).not.toHaveBeenCalled()
    expect(approve).not.toHaveBeenCalled()
  })
})
