import { fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'
import { vi } from 'vitest'
import { InputBar } from './InputBar'
import { draftKey, getComposerDraft } from './composerDraft'

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
  it('onSend 同一提交内卸载 InputBar 时，草稿 store 也被清空（欢迎页首发竞态）', () => {
    render(<UnmountOnSend conversationId="c-draft-race" />)
    const textarea = screen.getByPlaceholderText('Ask me anything...')
    fireEvent.change(textarea, { target: { value: '我右键无法创建txt文件了' } })
    expect(getComposerDraft(draftKey('c-draft-race'))?.input).toBe('我右键无法创建txt文件了')

    fireEvent.keyDown(textarea, { key: 'Enter' })

    // 卸载丢弃了 setInput('') 与写回 effect —— 只有 handleSend 里的同步清 store 能保证这条。
    expect(screen.queryByPlaceholderText('Ask me anything...')).not.toBeInTheDocument()
    expect(getComposerDraft(draftKey('c-draft-race'))).toBeUndefined()
  })
})
