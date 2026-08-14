import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'
import { vi } from 'vitest'
import { InputBar } from './InputBar'
import { draftKey, getComposerDraft, setComposerDraft } from './composerDraft'

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
  },
}))

/** 复现欢迎页→对话页切换：onSend 在同一次提交里把 InputBar 卸载。 */
function UnmountOnSend({ conversationId }: { conversationId: string }) {
  const [show, setShow] = useState(true)
  if (!show) return null
  return <InputBar onSend={() => setShow(false)} conversationId={conversationId} />
}

describe('InputBar 发送清草稿', () => {
  it('onSend 同一提交内卸载 InputBar 时，草稿 store 也被清空（欢迎页首发竞态）', async () => {
    render(<UnmountOnSend conversationId="c-draft-race" />)
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '我右键无法创建txt文件了' } })
    expect(getComposerDraft(draftKey('c-draft-race'))?.input).toBe('我右键无法创建txt文件了')

    fireEvent.keyDown(textarea, { key: 'Enter' })

    // 卸载丢弃了 setInput('') 与写回 effect —— 只有 handleSend 里的同步清 store 能保证这条。
    expect(screen.queryByPlaceholderText('Ask me anything...')).not.toBeInTheDocument()
    await waitFor(() => {
      expect(getComposerDraft(draftKey('c-draft-race'))).toBeUndefined()
    })
  })

  it('发送未被接受时保留输入草稿', async () => {
    render(<InputBar onSend={() => Promise.resolve(false)} conversationId="c-send-rejected" />)
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '不要丢掉这条消息' } })
    fireEvent.keyDown(textarea, { key: 'Enter' })

    await waitFor(() => expect(textarea).not.toHaveAttribute('aria-busy', 'true'))
    expect(textarea).toHaveValue('不要丢掉这条消息')
    expect(getComposerDraft(draftKey('c-send-rejected'))?.input).toBe('不要丢掉这条消息')
  })

  it('onAccepted 之后若发送失败且输入仍空，把原文回填', async () => {
    render(
      <InputBar
        conversationId="c-accepted-then-fail"
        onSend={(_content, _attachments, options) => {
          options?.onAccepted?.()
          return Promise.resolve(false)
        }}
      />,
    )
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '发送失败也要回来' } })
    fireEvent.keyDown(textarea, { key: 'Enter' })

    await waitFor(() => expect(textarea).toHaveValue('发送失败也要回来'))
    expect(getComposerDraft(draftKey('c-accepted-then-fail'))?.input).toBe('发送失败也要回来')
  })

  it('onAccepted 之后用户已开始打下一句，失败不覆盖新输入', async () => {
    let finishSend!: (accepted: boolean) => void
    const send = new Promise<boolean>((resolve) => {
      finishSend = resolve
    })
    render(
      <InputBar
        conversationId="c-typed-after-accept"
        onSend={(_content, _attachments, options) => {
          options?.onAccepted?.()
          return send
        }}
      />,
    )
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '第一句' } })
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(textarea).toHaveValue(''))

    fireEvent.change(textarea, { target: { value: '已经在打下一句' } })
    await waitFor(() => {
      expect(getComposerDraft(draftKey('c-typed-after-accept'))?.input).toBe('已经在打下一句')
    })
    await act(async () => { finishSend(false) })

    expect(textarea).toHaveValue('已经在打下一句')
    expect(getComposerDraft(draftKey('c-typed-after-accept'))?.input).toBe('已经在打下一句')
  })

  it('消息进入发送流程后立即清空，不等待整轮生成 Promise 完成', async () => {
    let resolveGeneration!: (accepted: boolean) => void
    let generationFinished = false
    const generation = new Promise<boolean>((resolve) => {
      resolveGeneration = (accepted) => {
        generationFinished = true
        resolve(accepted)
      }
    })
    render(
      <InputBar
        conversationId="c-accepted-early"
        onSend={(_content, _attachments, options) => {
          options?.onAccepted?.()
          return generation
        }}
      />,
    )
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '发出后马上清空' } })
    fireEvent.keyDown(textarea, { key: 'Enter' })

    await waitFor(() => expect(textarea).toHaveValue(''))
    expect(textarea).not.toHaveAttribute('aria-busy', 'true')
    expect(generationFinished).toBe(false)

    await act(async () => { resolveGeneration(true) })
  })

  it('发送中占位草稿迁移到新会话 id 后，成功时清理迁移后的草稿', async () => {
    let resolveSend!: (accepted: boolean) => void
    const send = new Promise<boolean>((resolve) => { resolveSend = resolve })
    const { rerender } = render(<InputBar onSend={() => send} conversationId={null} />)
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '首条消息' } })
    fireEvent.keyDown(textarea, { key: 'Enter' })

    rerender(<InputBar onSend={() => send} conversationId="c-created-after-send" />)
    expect(textarea).toHaveValue('首条消息')
    await act(async () => { resolveSend(true) })

    await waitFor(() => expect(textarea).toHaveValue(''))
    expect(getComposerDraft(draftKey('c-created-after-send'))).toBeUndefined()
  })

  it('等待发送时切到已有草稿的会话，不会清掉新会话草稿', async () => {
    let resolveSend!: (accepted: boolean) => void
    const send = new Promise<boolean>((resolve) => { resolveSend = resolve })
    setComposerDraft(draftKey('c-own-draft'), {
      input: '另一条会话自己的草稿',
      quotes: [],
      attachments: [],
    })
    const { rerender } = render(<InputBar onSend={() => send} conversationId="c-sending" />)
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '正在发送的消息' } })
    fireEvent.keyDown(textarea, { key: 'Enter' })

    rerender(<InputBar onSend={() => send} conversationId="c-own-draft" />)
    await waitFor(() => expect(textarea).toHaveValue('另一条会话自己的草稿'))
    await act(async () => { resolveSend(true) })

    expect(textarea).toHaveValue('另一条会话自己的草稿')
    expect(getComposerDraft(draftKey('c-own-draft'))?.input).toBe('另一条会话自己的草稿')
  })
})
