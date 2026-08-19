import { renderHook, act } from '@testing-library/react'
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { useMessageQueue } from './useMessageQueue'
import { chatApi } from '../api'
import type { Conversation } from '../types'

vi.mock('../api', () => ({
  chatApi: { steerMessage: vi.fn(), followUpMessage: vi.fn() },
}))

const mockSteer = vi.mocked(chatApi.steerMessage)
const mockFollowUp = vi.mocked(chatApi.followUpMessage)

const conversation = { id: 'conv-1' } as Conversation

/**
 * 回归重点：
 *   1. 逐条交付 —— 一次 drain 只发队首一条（不合并、不一口气清空）
 *   2. 发送被拒时条目留在队首
 *   3. steer / follow-up 成功只标记不出队；出队必须等 confirm
 *   4. 收不到卡走 settleAfterRun（落库对账 + 解开 + drain）；硬失败只解开
 */
function setup(onSendResult = true) {
  const onSendMessage = vi.fn().mockResolvedValue(onSendResult)
  const onRestoreToComposer = vi.fn()
  const rendered = renderHook(() => useMessageQueue({ onSendMessage, onRestoreToComposer }))
  return { ...rendered, onSendMessage, onRestoreToComposer }
}

beforeEach(() => {
  mockSteer.mockReset()
  mockSteer.mockResolvedValue(true)
  mockFollowUp.mockReset()
  mockFollowUp.mockResolvedValue(true)
})

describe('useMessageQueue', () => {
  it('入队后 drain 只发队首一条', async () => {
    const { result, onSendMessage } = setup()
    act(() => {
      result.current.enqueue('conv-1', '第一条', [])
      result.current.enqueue('conv-1', '第二条', [])
    })
    expect(result.current.queued['conv-1']).toHaveLength(2)

    await act(async () => { await result.current.drain(conversation) })

    expect(onSendMessage).toHaveBeenCalledTimes(1)
    expect(onSendMessage).toHaveBeenCalledWith('第一条', [], {
      conversationOverride: conversation,
    })
    expect(result.current.queued['conv-1'].map((item) => item.content)).toEqual(['第二条'])
  })

  it('空白内容且无附件不入队', () => {
    const { result } = setup()
    act(() => { result.current.enqueue('conv-1', '   \n ', []) })
    expect(result.current.queued['conv-1']).toBeUndefined()
  })

  it('发送被拒时条目留在队首', async () => {
    const { result } = setup(false)
    act(() => { result.current.enqueue('conv-1', '留下', []) })
    await act(async () => { await result.current.drain(conversation) })
    expect(result.current.queued['conv-1'].map((item) => item.content)).toEqual(['留下'])
  })

  // 回归：drain 要 await 一整轮，而那一轮结束时它自己又会调 drain。早前用「每会话一个
  // draining 标志」时这里会被自己挡住，第二条永远发不出去。
  it('发送过程中被再次触发时接着发下一条，而不是把自己挡住', async () => {
    const sent: string[] = []
    const onRestoreToComposer = vi.fn()
    let reenter: (() => Promise<void>) | null = null
    const onSendMessage = vi.fn().mockImplementation(async (content: string) => {
      sent.push(content)
      // 模拟这一轮结束时（还在 await 里）由 run 的 finally 再次触发 drain。
      if (reenter) await reenter()
      return true
    })
    const { result } = renderHook(() => useMessageQueue({ onSendMessage, onRestoreToComposer }))
    act(() => {
      result.current.enqueue('conv-1', '第一条', [])
      result.current.enqueue('conv-1', '第二条', [])
    })
    reenter = async () => {
      reenter = null
      await result.current.drain(conversation)
    }

    await act(async () => { await result.current.drain(conversation) })

    expect(sent).toEqual(['第一条', '第二条'])
    expect(result.current.queued['conv-1']).toBeUndefined()
  })

  it('同一条不会被两个并发 drain 各发一遍', async () => {
    const onSendMessage = vi.fn().mockImplementation(
      () => new Promise<boolean>((resolve) => setTimeout(() => resolve(true), 0)),
    )
    const { result } = renderHook(() => useMessageQueue({
      onSendMessage,
      onRestoreToComposer: vi.fn(),
    }))
    act(() => { result.current.enqueue('conv-1', '只发一次', []) })

    await act(async () => {
      await Promise.all([
        result.current.drain(conversation),
        result.current.drain(conversation),
      ])
    })

    expect(onSendMessage).toHaveBeenCalledTimes(1)
  })

  it('立刻引导成功只标记 steering，不出队；确认后才出队', async () => {
    const { result } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '改用 rg', [])!.id })

    await act(async () => { await result.current.steer('conv-1', id) })
    expect(mockSteer).toHaveBeenCalledWith('conv-1', id, '改用 rg', [])
    expect(result.current.queued['conv-1']).toHaveLength(1)
    expect(result.current.queued['conv-1'][0].steering).toBe(true)

    act(() => { result.current.confirm('conv-1', id) })
    expect(result.current.queued['conv-1']).toBeUndefined()
  })

  it('引导已受理但未收到确认卡时，run 收尾仍降级为普通发送', async () => {
    const { result, onSendMessage } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '没赶上边界', [])!.id })
    await act(async () => { await result.current.steer('conv-1', id) })

    await act(async () => { await result.current.settleAfterRun('conv-1', conversation) })

    expect(onSendMessage).toHaveBeenCalledWith('没赶上边界', [], {
      conversationOverride: conversation,
    })
    expect(result.current.queued['conv-1']).toBeUndefined()
  })

  it('立刻引导会把虚拟文本附件交给通用文本协议', async () => {
    const { result } = setup()
    const attachment = {
      id: 'paste-1',
      type: 'file' as const,
      name: '已粘贴的文本.txt',
      path: 'memory://paste-1',
      content: '很长的正文',
    }
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '', [attachment])!.id })

    await act(async () => { await result.current.steer('conv-1', id) })

    expect(mockSteer).toHaveBeenCalledWith('conv-1', id, '', [attachment])
    expect(result.current.queued['conv-1'][0].steering).toBe(true)
  })

  it('磁盘附件不能通过文本 steering 时留在队列等待正常发送', async () => {
    const { result } = setup()
    let id = ''
    act(() => {
      id = result.current.enqueue('conv-1', '看附件', [{
        id: 'disk-1',
        type: 'file',
        name: 'report.pdf',
        path: '/tmp/report.pdf',
      }])!.id
    })

    const accepted = await act(async () => await result.current.steer('conv-1', id))

    expect(accepted).toBe(false)
    expect(mockSteer).not.toHaveBeenCalled()
    expect(result.current.queued['conv-1'][0].steerRejected).toBe(true)
  })

  it('没有活跃 run 时引导失败，条目留着并仍能被自动发送', async () => {
    mockSteer.mockResolvedValue(false)
    const { result, onSendMessage } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '兜住我', [])!.id })

    const accepted = await act(async () => await result.current.steer('conv-1', id))
    expect(accepted).toBe(false)
    expect(result.current.queued['conv-1'][0].steering).toBe(false)
    // 被拒要在条目上说一句话，否则用户点了按钮什么都看不见
    // （claude / ACP 那种协议压根不支持注入的情况就走这条）。
    expect(result.current.queued['conv-1'][0].steerRejected).toBe(true)

    await act(async () => { await result.current.drain(conversation) })
    expect(onSendMessage).toHaveBeenCalledWith('兜住我', [], {
      conversationOverride: conversation,
    })
    expect(result.current.queued['conv-1']).toBeUndefined()
  })

  it('Pi follow-up 成功只标记 followingUp，不出队；确认后才出队', async () => {
    const { result, onSendMessage } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '稍后再总结', [])!.id })

    const accepted = await act(async () => await result.current.followUp(conversation, id))

    expect(accepted).toBe(true)
    expect(mockFollowUp).toHaveBeenCalledWith('conv-1', id, '稍后再总结', [])
    expect(result.current.queued['conv-1']).toHaveLength(1)
    expect(result.current.queued['conv-1'][0].followingUp).toBe(true)
    await act(async () => { await result.current.drain(conversation) })
    expect(onSendMessage).not.toHaveBeenCalled()

    act(() => { result.current.confirm('conv-1', id) })
    expect(result.current.queued['conv-1']).toBeUndefined()
    expect(onSendMessage).not.toHaveBeenCalled()
  })

  it('Pi 拒绝 follow-up 时恢复本地队列并尝试普通发送兜底', async () => {
    mockFollowUp.mockResolvedValue(false)
    const { result, onSendMessage } = setup(false)
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '别丢我', [])!.id })

    const accepted = await act(async () => await result.current.followUp(conversation, id))

    expect(accepted).toBe(false)
    expect(onSendMessage).toHaveBeenCalledWith('别丢我', [], {
      conversationOverride: conversation,
    })
    expect(result.current.queued['conv-1'][0]).toEqual(expect.objectContaining({
      id,
      followingUp: false,
      followUpRejected: true,
    }))
  })

  it('follow-up 等确认时普通 drain 不能抢发同一条', async () => {
    let resolveFollowUp: ((accepted: boolean) => void) | null = null
    mockFollowUp.mockImplementation(() => new Promise<boolean>((resolve) => {
      resolveFollowUp = resolve
    }))
    const { result, onSendMessage } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '只处理一次', [])!.id })

    let pending!: Promise<boolean>
    act(() => { pending = result.current.followUp(conversation, id) })
    expect(result.current.queued['conv-1'][0].followingUp).toBe(true)
    await act(async () => { await result.current.drain(conversation) })
    expect(onSendMessage).not.toHaveBeenCalled()

    await act(async () => {
      resolveFollowUp?.(true)
      await pending
    })
    expect(result.current.queued['conv-1'][0].followingUp).toBe(true)
    expect(onSendMessage).not.toHaveBeenCalled()

    act(() => { result.current.confirm('conv-1', id) })
    expect(result.current.queued['conv-1']).toBeUndefined()
  })

  it('follow-up 已受理但未收到确认卡时，run 收尾降级为普通发送', async () => {
    const { result, onSendMessage } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '没吐卡', [])!.id })
    await act(async () => { await result.current.followUp(conversation, id) })

    await act(async () => { await result.current.settleAfterRun('conv-1', conversation) })

    expect(onSendMessage).toHaveBeenCalledWith('没吐卡', [], {
      conversationOverride: conversation,
    })
    expect(result.current.queued['conv-1']).toBeUndefined()
  })

  it('落库对话已有插话卡时先对账出队，收尾不再重发', async () => {
    const { result, onSendMessage } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '已经在历史上', [])!.id })
    await act(async () => { await result.current.followUp(conversation, id) })

    await act(async () => {
      await result.current.settleAfterRun('conv-1', {
        id: 'conv-1',
        messages: [{
          toolCalls: [{
            source: 'native',
            name: 'user_follow_up',
            structured_content: { type: 'user_follow_up', follow_up_id: id, text: '已经在历史上' },
          }],
        }],
      } as Conversation)
    })

    expect(result.current.queued['conv-1']).toBeUndefined()
    expect(onSendMessage).not.toHaveBeenCalled()
  })

  it('失败收尾解开已提交条目，转圈条可以再撤或再发', async () => {
    const { result, onRestoreToComposer } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '解开我', [])!.id })
    await act(async () => { await result.current.steer('conv-1', id) })
    expect(result.current.queued['conv-1'][0].steering).toBe(true)

    act(() => { result.current.settleAfterRun('conv-1') })
    act(() => { result.current.restoreToComposer('conv-1', id) })
    expect(onRestoreToComposer).toHaveBeenCalledWith(
      expect.objectContaining({ content: '解开我' }),
    )
  })

  it('已提交的条目不能直接移出队列', async () => {
    const { result } = setup()
    let id = ''
    act(() => { id = result.current.enqueue('conv-1', '别删', [])!.id })
    await act(async () => { await result.current.steer('conv-1', id) })
    act(() => { result.current.remove('conv-1', id) })
    expect(result.current.queued['conv-1']).toHaveLength(1)
  })

  it('撤回到输入框：出队并交还内容；已提交引导的不给撤', async () => {
    const { result, onRestoreToComposer } = setup()
    let a = ''
    let b = ''
    act(() => {
      a = result.current.enqueue('conv-1', '可撤', [])!.id
      b = result.current.enqueue('conv-1', '已引导', [])!.id
    })
    await act(async () => { await result.current.steer('conv-1', b) })

    act(() => { result.current.restoreToComposer('conv-1', b) })
    expect(onRestoreToComposer).not.toHaveBeenCalled()

    act(() => { result.current.restoreToComposer('conv-1', a) })
    expect(onRestoreToComposer).toHaveBeenCalledWith(
      expect.objectContaining({ content: '可撤' }),
    )
    expect(result.current.queued['conv-1'].map((item) => item.content)).toEqual(['已引导'])
  })

  it('队列按会话隔离', async () => {
    const { result, onSendMessage } = setup()
    act(() => {
      result.current.enqueue('conv-1', 'A', [])
      result.current.enqueue('conv-2', 'B', [])
    })
    await act(async () => { await result.current.drain({ id: 'conv-2' } as Conversation) })
    expect(onSendMessage).toHaveBeenCalledWith('B', [], expect.anything())
    expect(result.current.queued['conv-1'].map((item) => item.content)).toEqual(['A'])
  })
})
