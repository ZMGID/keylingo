import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { AssistantMessageMeta } from './AssistantMessageMeta'

describe('AssistantMessageMeta', () => {
  it('does not expose a delete action in the message footer', () => {
    render(
      <AssistantMessageMeta
        content="answer"
        timestamp={Date.now()}
        onRegenerate={vi.fn()}
        onFork={vi.fn()}
      />,
    )

    expect(screen.queryByRole('button', { name: '删除' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: '复制' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '重新生成' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '建分支' })).toBeInTheDocument()
  })
})
