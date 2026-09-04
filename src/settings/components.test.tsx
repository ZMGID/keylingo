import { fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { Select, TextArea, Toggle } from './components'

describe('Toggle', () => {
  it('reflects checked state and toggles on click', async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    render(<Toggle checked={false} onChange={onChange} />)
    const toggle = screen.getByRole('switch')
    expect(toggle).toHaveAttribute('aria-checked', 'false')
    await user.click(toggle)
    expect(onChange).toHaveBeenCalledWith(true)
  })
})

describe('Select', () => {
  it('opens menu and selects an option', async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    render(
      <Select
        value="a"
        onChange={onChange}
        options={[
          { value: 'a', label: 'Option A' },
          { value: 'b', label: 'Option B' },
        ]}
      />,
    )
    expect(screen.getByRole('button', { name: /Option A/i })).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: /Option A/i }))
    await user.click(screen.getByRole('option', { name: 'Option B' }))
    expect(onChange).toHaveBeenCalledWith('b')
  })
})

describe('TextArea', () => {
  it('uses the shared scrollbar and opens a copy menu on right-click', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText, readText: vi.fn().mockResolvedValue('pasted') },
    })

    render(<TextArea value="hello world" onChange={() => undefined} />)
    const field = screen.getByRole('textbox') as HTMLTextAreaElement
    expect(field.className).toContain('custom-scrollbar')

    field.setSelectionRange(0, 5)
    fireEvent.contextMenu(field, { clientX: 12, clientY: 20 })
    expect(screen.getByRole('menuitem', { name: '复制' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: '剪切' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: '粘贴' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: '全选' })).toBeTruthy()

    fireEvent.click(screen.getByRole('menuitem', { name: '复制' }))
    expect(writeText).toHaveBeenCalledWith('hello')
  })
})
