import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { BLOB_DONE_MS, StreamStatusLine } from './StreamStatusLine'
import { STATUS_QUIPS } from './blobQuips'
import { patchSnapshot, reset, setCoarse } from './streamingStore'
import type { ToolCallRecord } from './types'

vi.mock('./conversationTransitionStore', () => ({
  useConversationTransition: () => ({ loading: false, showLoading: false }),
}))

const moodProbe = vi.fn()
vi.mock('./KivioBlob', () => ({
  KivioBlob: ({ mood }: { mood: string }) => {
    moodProbe(mood)
    return <span data-testid="blob" data-mood={mood} />
  },
}))

function tool(name: string, status: ToolCallRecord['status']): ToolCallRecord {
  return { id: name, toolCallId: name, name, status } as ToolCallRecord
}

describe('StreamStatusLine', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-09-04T12:00:00Z'))
    vi.spyOn(Math, 'random').mockReturnValue(0)
    reset()
    setCoarse({ streamError: '' })
    moodProbe.mockClear()
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
    reset()
  })

  it('问用户时是 wait 脸，并很快接一句「等你呢」', () => {
    patchSnapshot({
      streaming: true,
      startedAt: Date.now() - 3000,
      content: '',
      toolCalls: [tool('ask_user', 'running')],
    })
    render(<StreamStatusLine active lang="zh" />)
    expect(screen.getByTestId('blob').dataset.mood).toBe('wait')
    expect(screen.getByText(/3s · 1 running/)).toBeTruthy()
    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(screen.getByText(new RegExp(STATUS_QUIPS.zh.wait[0]))).toBeTruthy()
  })

  it('后端状态一行字优先，墨团不抢话', () => {
    patchSnapshot({
      streaming: true,
      startedAt: Date.now(),
      toolCalls: [tool('ask_user', 'running')],
      statusNote: '上游重试中',
    })
    render(<StreamStatusLine active lang="zh" />)
    act(() => {
      vi.advanceTimersByTime(1500)
    })
    expect(screen.getByText(/上游重试中/)).toBeTruthy()
    expect(screen.queryByText(new RegExp(STATUS_QUIPS.zh.wait[0]))).toBeNull()
  })

  it('生成结束且没翻车：进「搞定」窗口，到点回闲置', () => {
    patchSnapshot({ streaming: true, startedAt: Date.now(), content: '答' })
    const { rerender } = render(<StreamStatusLine active lang="en" />)
    expect(screen.getByTestId('blob').dataset.mood).toBe('speak')
    reset()
    rerender(<StreamStatusLine active={false} lang="en" />)
    expect(screen.getByTestId('blob').dataset.mood).toBe('done')
    act(() => {
      vi.advanceTimersByTime(0)
    })
    expect(screen.getByText(STATUS_QUIPS.en.done[0])).toBeTruthy()
    act(() => {
      vi.advanceTimersByTime(BLOB_DONE_MS + 100)
    })
    expect(screen.getByTestId('blob').dataset.mood).toBe('idle')
    expect(screen.queryByText(STATUS_QUIPS.en.done[0])).toBeNull()
  })

  it('翻车收尾不得意：直接出错脸', () => {
    patchSnapshot({ streaming: true, startedAt: Date.now(), content: '答' })
    const { rerender } = render(<StreamStatusLine active lang="zh" />)
    setCoarse({ streamError: 'boom' })
    reset()
    rerender(<StreamStatusLine active={false} lang="zh" />)
    expect(screen.getByTestId('blob').dataset.mood).toBe('error')
    act(() => {
      vi.advanceTimersByTime(600)
    })
    expect(screen.getByText(STATUS_QUIPS.zh.error[0])).toBeTruthy()
  })
})
