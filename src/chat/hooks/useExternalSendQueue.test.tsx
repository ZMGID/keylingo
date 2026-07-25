import { renderHook, act } from '@testing-library/react'
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { useExternalSendQueue } from './useExternalSendQueue'
import { api } from '../../api/tauri'

vi.mock('../../api/tauri', () => ({
  api: { chatTakeExternalSends: vi.fn() },
}))

const mockTake = vi.mocked(api.chatTakeExternalSends)

/**
 * 回归重点：
 *   1. 单飞 —— drain 进行中再次调用只置 requested 标志，不并发取消息
 *   2. 发送被拒（正在生成）时请求留在队首、不 shift 掉，并置 requested
 *   3. 历史预置分支走 import 而非 send
 *   4. 附件映射（type/name 兜底、无 path 的被过滤）
 */
function setup() {
  const onEnterConversationView = vi.fn()
  const onImportConversation = vi.fn().mockResolvedValue(undefined)
  const onSendMessage = vi.fn().mockResolvedValue(true)
  const onError = vi.fn()
  const rendered = renderHook(() => useExternalSendQueue({
    onEnterConversationView, onImportConversation, onSendMessage, onError,
  }))
  return { ...rendered, onEnterConversationView, onImportConversation, onSendMessage, onError }
}

beforeEach(() => {
  mockTake.mockReset()
  mockTake.mockResolvedValue({ success: true, requests: [] } as never)
})

describe('useExternalSendQueue 基本流转', () => {
  it('无消息时不发送、不报错', async () => {
    const { result, onSendMessage, onError } = setup()
    await act(async () => { await result.current.drainExternalSends() })
    expect(onSendMessage).not.toHaveBeenCalled()
    expect(onError).not.toHaveBeenCalled()
  })

  it('取到消息后切视图并发送', async () => {
    mockTake.mockResolvedValueOnce({
      success: true,
      requests: [{ id: 'r1', content: '你好', attachments: [] }],
    } as never)
    const { result, onEnterConversationView, onSendMessage } = setup()
    await act(async () => { await result.current.drainExternalSends() })
    expect(onEnterConversationView).toHaveBeenCalled()
    expect(onSendMessage).toHaveBeenCalledWith('你好', [], { forceNewConversation: true })
  })

  it('带 messages 的请求走 import 而非 send', async () => {
    mockTake.mockResolvedValueOnce({
      success: true,
      requests: [{
        id: 'r2',
        messages: [{ role: 'user', content: '历史' }],
        attachments: [{ id: 'a1', type: 'image', path: '/tmp/a.png' }],
      }],
    } as never)
    const { result, onImportConversation, onSendMessage } = setup()
    await act(async () => { await result.current.drainExternalSends() })
    expect(onImportConversation).toHaveBeenCalledWith(
      [{ role: 'user', content: '历史' }],
      ['/tmp/a.png'],
    )
    expect(onSendMessage).not.toHaveBeenCalled()
  })

  it('附件映射：无 path 的被过滤，name/type 有兜底', async () => {
    mockTake.mockResolvedValueOnce({
      success: true,
      requests: [{
        id: 'r3',
        content: '看图',
        attachments: [
          { id: '', type: 'file', path: '/tmp/doc.pdf' },
          { id: 'x', type: 'image', name: '截图', path: '/tmp/s.png' },
          { id: 'no-path', type: 'image' },
        ],
      }],
    } as never)
    const { result, onSendMessage } = setup()
    await act(async () => { await result.current.drainExternalSends() })
    const attachments = onSendMessage.mock.calls[0][1]
    expect(attachments).toHaveLength(2)
    expect(attachments[0]).toMatchObject({ id: 'external-r3-0', type: 'file', name: 'Attachment' })
    expect(attachments[1]).toMatchObject({ id: 'x', type: 'image', name: '截图' })
  })
})

describe('useExternalSendQueue 单飞与重排', () => {
  it('drain 进行中再次调用不并发取消息', async () => {
    let release: (() => void) | undefined
    mockTake.mockImplementationOnce(() => new Promise((resolve) => {
      release = () => resolve({ success: true, requests: [] } as never)
    }))
    const { result, unmount } = setup()
    const drain = result.current.drainExternalSends

    const first = drain()
    // 第一次仍挂在 take 上；第二次应立即返回且不再调 take
    await drain()
    expect(mockTake).toHaveBeenCalledTimes(1)

    // 放行并收尾，避免悬挂 promise 影响后续用例
    mockTake.mockResolvedValue({ success: true, requests: [] } as never)
    release?.()
    await first
    unmount()
  })

  it('发送被拒时请求留在队首，下次重试', async () => {
    mockTake
      .mockResolvedValueOnce({ success: true, requests: [{ id: 'r4', content: 'X', attachments: [] }] } as never)
      .mockResolvedValue({ success: true, requests: [] } as never)
    const { result, onSendMessage } = setup()
    onSendMessage.mockResolvedValueOnce(false)
    await act(async () => { await result.current.drainExternalSends() })
    expect(onSendMessage).toHaveBeenCalledTimes(1)
    // 被拒后应置 requested，供流式结束后补一次
    expect(result.current.hasPendingDrainRequest()).toBe(true)

    // 第二次 drain：队列里仍有 r4，应重新尝试发送
    onSendMessage.mockResolvedValueOnce(true)
    await act(async () => { await result.current.drainExternalSends() })
    expect(onSendMessage).toHaveBeenCalledTimes(2)
    expect(onSendMessage.mock.calls[1][0]).toBe('X')
  })

  it('发送成功后请求出队，不重复发', async () => {
    mockTake
      .mockResolvedValueOnce({ success: true, requests: [{ id: 'r5', content: 'Y', attachments: [] }] } as never)
      .mockResolvedValue({ success: true, requests: [] } as never)
    const { result, onSendMessage } = setup()
    await act(async () => { await result.current.drainExternalSends() })
    await act(async () => { await result.current.drainExternalSends() })
    expect(onSendMessage).toHaveBeenCalledTimes(1)
  })
})

describe('useExternalSendQueue 错误处理', () => {
  it('take 失败时报错并释放单飞标志', async () => {
    mockTake.mockResolvedValueOnce({ success: false, error: '后端拒绝' } as never)
    const { result, onError } = setup()
    await act(async () => { await result.current.drainExternalSends() })
    expect(onError).toHaveBeenCalledWith('后端拒绝')

    // 单飞标志必须已释放，否则后续 drain 永久失效。
    // 注：失败时 requested 仍为 true，finally 会再排一次 drain，故只断言"能再取"。
    const before = mockTake.mock.calls.length
    mockTake.mockResolvedValue({ success: true, requests: [] } as never)
    await act(async () => { await result.current.drainExternalSends() })
    expect(mockTake.mock.calls.length).toBeGreaterThan(before)
  })

  it('take 抛异常时也释放单飞标志', async () => {
    mockTake.mockRejectedValueOnce(new Error('网络中断'))
    const { result, onError } = setup()
    await act(async () => { await result.current.drainExternalSends() })
    expect(onError).toHaveBeenCalledWith('网络中断')
    const before = mockTake.mock.calls.length
    mockTake.mockResolvedValue({ success: true, requests: [] } as never)
    await act(async () => { await result.current.drainExternalSends() })
    expect(mockTake.mock.calls.length).toBeGreaterThan(before)
  })
})
