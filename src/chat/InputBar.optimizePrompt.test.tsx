import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { InputBar } from './InputBar'

const optimizePrompt = vi.fn<(text: string, conversationId?: string | null) => Promise<string>>(
  async (text) => `${text.trim()}（已优化）`,
)

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ onFocusChanged: () => Promise.resolve(() => {}) }),
}))
vi.mock('../api/tauri', () => ({ api: {}, isTauriRuntime: () => false }))
vi.mock('./api', () => ({
  chatApi: {
    getProjects: () => Promise.resolve([]),
    listExternalCliSlashCommands: () => Promise.resolve({ commands: [] }),
    optimizePrompt: (text: string, conversationId?: string | null) => optimizePrompt(text, conversationId),
  },
}))

describe('InputBar 问题优化', () => {
  it('空输入时优化按钮不可用', () => {
    render(<InputBar onSend={() => {}} />)
    expect(screen.getByRole('button', { name: '先输入要优化的问题' })).toBeDisabled()
  })

  it('点击后把草稿换成优化结果，再点可撤销', async () => {
    render(<InputBar onSend={() => {}} conversationId="c1" />)
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '帮我看看这个' } })
    const button = screen.getByRole('button', { name: '优化问题' })
    expect(button).not.toBeDisabled()
    await act(async () => {
      fireEvent.click(button)
    })
    await waitFor(() => {
      expect(optimizePrompt).toHaveBeenCalledWith('帮我看看这个', 'c1')
      expect((textarea as HTMLTextAreaElement).value).toBe('帮我看看这个（已优化）')
    })
    const undo = screen.getByRole('button', { name: '撤销优化' })
    await waitFor(() => {
      expect(undo).not.toBeDisabled()
    })
    fireEvent.click(undo)
    await waitFor(() => {
      expect((textarea as HTMLTextAreaElement).value).toBe('帮我看看这个')
    })
  })

  it('优化等待时草稿呼吸，落地时旧稿先化开再换上新稿', async () => {
    let finish: (value: string) => void = () => {}
    optimizePrompt.mockImplementationOnce(
      () => new Promise<string>((resolve) => {
        finish = resolve
      }),
    )
    render(<InputBar onSend={() => {}} conversationId="c1" />)
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '帮我看看这个' } })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: '优化问题' }))
    })
    await waitFor(() => {
      expect(textarea).toHaveClass('is-optimizing')
      expect(textarea).toHaveAttribute('aria-busy', 'true')
    })
    await act(async () => {
      finish('更清楚的问题')
    })
    await waitFor(() => {
      expect(textarea).not.toHaveClass('is-optimizing')
      expect(textarea).toHaveClass('is-optimize-out')
      expect((textarea as HTMLTextAreaElement).value).toBe('帮我看看这个')
    })
    await waitFor(() => {
      expect((textarea as HTMLTextAreaElement).value).toBe('更清楚的问题')
      expect(textarea).toHaveClass('is-optimize-reveal')
    })
  })
})
