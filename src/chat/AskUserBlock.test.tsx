import { act, fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { AskUserBlock } from './AskUserBlock'
import type { ToolCallRecord } from './types'

/** 待答的问用户卡片。`variant="docked"` 是吊在输入框上方的那张（用户在这里作答）；
 *  消息流里的 inline 变体只剩一行痕迹，那条契约在 ToolCallBlock.test 里。 */
function askUserCall(askUser: Record<string, unknown>): ToolCallRecord {
  return {
    id: 'tool-1',
    toolCallId: 'tool-1',
    toolName: 'AskUserQuestion',
    source: 'external_cli',
    status: 'running',
    structured_content: { askUser },
  }
}

const RETRY_QUESTION = {
  id: '0',
  prompt: '用哪种方式重试？',
  options: [
    { id: '0', label: '指数退避', description: '首次 200ms，每次翻倍' },
    { id: '1', label: '立即重试' },
    { id: '2', label: '不重试' },
  ],
  allow_multiple: false,
  allow_custom: true,
}

describe('AskUserBlock', () => {
  it('renders the question as the title with numbered options', () => {
    render(<AskUserBlock variant="docked" toolCall={askUserCall({
      phase: 'awaiting',
      questions: [RETRY_QUESTION],
      answers: {},
    })} />)
    expect(screen.getByText('用哪种方式重试？')).toBeInTheDocument()
    expect(screen.getAllByRole('option')).toHaveLength(3)
    expect(screen.getByText('指数退避')).toBeInTheDocument()
    expect(screen.getByText('首次 200ms，每次翻倍')).toBeInTheDocument()
    // 单题：不出「1/1」也不出翻页键，一道题的进度条是纯噪声。
    expect(screen.queryByText('1/1')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('上一题')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('下一题')).not.toBeInTheDocument()
    // 单题单选点一下就是答案，不该再出现「提交」。
    expect(screen.queryByText('提交')).not.toBeInTheDocument()
  })

  it('shows the pager and a submit button only when there are several questions', () => {
    render(<AskUserBlock variant="docked" toolCall={askUserCall({
      phase: 'awaiting',
      questions: [
        RETRY_QUESTION,
        { id: '1', prompt: '要补测试吗？', options: [{ id: '0', label: '要' }, { id: '1', label: '不要' }], allow_multiple: false, allow_custom: false },
      ],
      answers: {},
    })} />)
    expect(screen.getByText('1/2')).toBeInTheDocument()
    expect(screen.getByLabelText('下一题')).toBeInTheDocument()
    expect(screen.getByText('提交')).toBeInTheDocument()
  })

  // 多选时同一批次连点两项：React 会把两次点击合成一次渲染，若从 state 里读草稿，
  // 第二下会拿着旧值把第一下覆盖掉（实测只剩后一项）。
  it('keeps both picks when two multi-select options are clicked in one batch', () => {
    render(<AskUserBlock variant="docked" toolCall={askUserCall({
      phase: 'awaiting',
      questions: [{
        id: '0',
        prompt: '哪些模块要一起改？',
        options: [{ id: '0', label: '运行时' }, { id: '1', label: '用量面板' }, { id: '2', label: '右侧栏' }],
        allow_multiple: true,
        allow_custom: false,
      }],
      answers: {},
    })} />)
    const options = screen.getAllByRole('option')
    // 包在同一个 act 里才是「一次渲染两次点击」—— 单独 fireEvent 会各自 flush，复现不出来。
    act(() => {
      fireEvent.click(options[1])
      fireEvent.click(options[2])
    })
    expect(screen.getAllByRole('option')[1]).toHaveAttribute('aria-selected', 'true')
    expect(screen.getAllByRole('option')[2]).toHaveAttribute('aria-selected', 'true')
  })

  it('renders a free-text question when there are no options', () => {
    render(<AskUserBlock variant="docked" toolCall={askUserCall({
      phase: 'awaiting',
      questions: [{
        id: 'note',
        prompt: '还有补充吗？',
        options: [],
        allow_multiple: false,
        allow_custom: true,
      }],
      answers: {},
    })} />)
    expect(screen.getByText('还有补充吗？')).toBeInTheDocument()
    expect(screen.queryByRole('option')).not.toBeInTheDocument()
    expect(screen.getByPlaceholderText('自己写一个…')).toBeInTheDocument()
  })

  it('renders answers read-only once the phase is answered', () => {
    render(<AskUserBlock variant="docked" toolCall={askUserCall({
      phase: 'answered',
      questions: [RETRY_QUESTION],
      answers: { '0': { selected_option_ids: ['1'], custom_text: null } },
    })} />)
    expect(screen.getByText('已回答')).toBeInTheDocument()
    expect(screen.getByText('立即重试')).toBeInTheDocument()
    expect(screen.queryByRole('option')).not.toBeInTheDocument()
  })
})
