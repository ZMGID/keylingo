import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SwapTitle } from './SwapTitle'

describe('SwapTitle', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('renders the initial text without typing', () => {
    render(<SwapTitle text="初始标题" />)
    expect(screen.getByText('初始标题')).toBeTruthy()
    expect(document.querySelector('.kv-title-caret')).toBeNull()
  })

  it('types the new title out character by character on change', () => {
    const { rerender } = render(<SwapTitle text="第一句消息截断..." />)
    rerender(<SwapTitle text="吉林天气" />)

    // 变化瞬间：清空旧文字、光标出现
    expect(screen.queryByText('第一句消息截断...')).toBeNull()
    expect(document.querySelector('.kv-title-caret')).not.toBeNull()

    // 逐字打出
    act(() => {
      vi.advanceTimersByTime(40)
    })
    expect(screen.getByText('吉')).toBeTruthy()
    act(() => {
      vi.advanceTimersByTime(40)
    })
    expect(screen.getByText('吉林')).toBeTruthy()
    act(() => {
      vi.advanceTimersByTime(40 * 2)
    })
    expect(screen.getByText('吉林天气')).toBeTruthy()

    // 打字结束：光标卸载
    expect(document.querySelector('.kv-title-caret')).toBeNull()
  })

  it('skips intermediate values on rapid changes and settles on the last one', () => {
    const { rerender } = render(<SwapTitle text="标题A" />)
    rerender(<SwapTitle text="标题B" />)
    // 打字途中再次变化：从空重新打最后一个
    act(() => {
      vi.advanceTimersByTime(40)
    })
    rerender(<SwapTitle text="标题C" />)
    expect(screen.queryByText('标')).toBeNull()
    act(() => {
      vi.advanceTimersByTime(40 * 3)
    })
    expect(screen.getByText('标题C')).toBeTruthy()
    expect(screen.queryByText('标题B')).toBeNull()
    expect(document.querySelector('.kv-title-caret')).toBeNull()
  })

  it('passes through the title attribute', () => {
    render(<SwapTitle text="标题" title="悬浮提示" />)
    expect(screen.getByText('标题')).toHaveAttribute('title', '悬浮提示')
  })
})
