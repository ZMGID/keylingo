import { fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { MessageBubble } from './MessageBubble'
import type { ChatMessage } from './types'

describe('MessageBubble mount motion', () => {
  const assistantMessage: ChatMessage = {
    id: 'assistant-motion',
    role: 'assistant',
    content: 'answer',
    timestamp: 1,
  }

  it('does not replay entrance motion for historical messages', () => {
    const { container, rerender } = render(<MessageBubble message={assistantMessage} />)
    expect(container.firstElementChild).not.toHaveClass('chat-motion-bubble-in')

    rerender(<MessageBubble message={{ ...assistantMessage, id: 'user-motion', role: 'user' }} />)
    expect(container.firstElementChild).not.toHaveClass('chat-motion-bubble-in')
  })

  it('keeps entrance motion for the live streaming preview', () => {
    const { container } = render(<MessageBubble message={assistantMessage} messageStreaming />)
    expect(container.firstElementChild).toHaveClass('chat-motion-bubble-in')
  })

  // 元信息条 hover 显隐：事件驱动（pointerenter/pointerleave 挂在消息根容器上），
  // 不用 CSS group-hover——macOS WKWebView 的 :hover 移出后粘滞不消。事件驱动的好处
  // 之一就是行为在 jsdom 里可直接断言。
  it('hides the assistant meta row until the message is hovered', () => {
    const { container } = render(<MessageBubble message={assistantMessage} />)
    const root = container.firstElementChild as HTMLElement
    expect(root).toHaveAttribute('data-hover-reveal-root')

    const meta = screen.getByLabelText('复制').closest('.transition-opacity')
    expect(meta).toHaveClass('opacity-0')

    fireEvent.pointerEnter(root)
    expect(meta).toHaveClass('opacity-100')

    fireEvent.pointerLeave(root)
    expect(meta).toHaveClass('opacity-0')

    // 光标甩出窗口漏发 pointerleave 的兜底：窗口失焦一并收起。
    fireEvent.pointerEnter(root)
    expect(meta).toHaveClass('opacity-100')
    fireEvent.blur(window)
    expect(meta).toHaveClass('opacity-0')
  })
})

describe('MessageBubble agent plan action', () => {
  it('renders execute action for a message-scoped draft plan', async () => {
    const user = userEvent.setup()
    const calls: string[] = []
    const message: ChatMessage = {
      id: 'msg-plan',
      role: 'assistant',
      content: '1. Read code\n2. Implement',
      agent_plan: {
        mode: 'plan',
        status: 'draft',
        plan: '1. Read code\n2. Implement',
        updated_at: 1,
      },
      timestamp: 1,
    }

    render(<MessageBubble message={message} onExecuteAgentPlan={(messageId) => { calls.push(messageId) }} />)

    expect(screen.getByText('计划草案')).toBeInTheDocument()
    expect(screen.queryByLabelText('计划内容')).not.toBeInTheDocument()
    const button = screen.getByRole('button', { name: '执行这条计划' })
    expect(
      button.compareDocumentPosition(screen.getByText('Read code')),
    ).toBe(Node.DOCUMENT_POSITION_PRECEDING)
    await user.click(button)
    expect(calls).toEqual(['msg-plan'])
  })

  it('keeps process timeline outside the plan label and renders the action at the bottom', () => {
    const message: ChatMessage = {
      id: 'msg-plan-with-process',
      role: 'assistant',
      content: '## 执行计划\n\n1. 调研\n2. 实现',
      agent_plan: {
        mode: 'plan',
        status: 'draft',
        plan: '## 执行计划\n\n1. 调研\n2. 实现',
        updated_at: 1,
      },
      segments: [
        { id: 'seg-reasoning', kind: 'reasoning', phase: 'plain', order: 1, text: '先调研一下' },
        { id: 'seg-tool', kind: 'tool', phase: 'tool_loop', order: 2, tool_call_id: 'tool-search' },
        { id: 'seg-text', kind: 'text', phase: 'synthesis', order: 3, text: '## 执行计划\n\n1. 调研\n2. 实现' },
      ],
      tool_calls: [
        {
          id: 'tool-search',
          name: 'web_search',
          source: 'native',
          status: 'completed',
          arguments: '{"query":"AI chat frameworks"}',
        },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} onExecuteAgentPlan={() => {}} />)

    expect(screen.queryByLabelText('计划内容')).not.toBeInTheDocument()
    const button = screen.getByRole('button', { name: '执行这条计划' })
    expect(
      button.compareDocumentPosition(screen.getByText('执行计划')),
    ).toBe(Node.DOCUMENT_POSITION_PRECEDING)
    expect(screen.getByText('计划草案')).toBeInTheDocument()
  })

  it('shows approved state without an execute button', () => {
    const message: ChatMessage = {
      id: 'msg-plan-approved',
      role: 'assistant',
      content: '1. Read code\n2. Edit',
      agent_plan: {
        mode: 'act',
        status: 'approved',
        plan: '1. Read code\n2. Edit',
        updated_at: 1,
      },
      timestamp: 1,
    }

    render(<MessageBubble message={message} onExecuteAgentPlan={() => {}} />)

    expect(screen.getByText('已按这条计划执行')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: '执行这条计划' })).not.toBeInTheDocument()
  })

  it('does not render execute action for an incomplete non-plan fragment', () => {
    const message: ChatMessage = {
      id: 'msg-plan-fragment',
      role: 'assistant',
      content: '没问题！积萌,',
      agent_plan: {
        mode: 'plan',
        status: 'draft',
        plan: '没问题！积萌,',
        updated_at: 1,
      },
      stream_outcome: 'interrupted',
      timestamp: 1,
    }

    render(<MessageBubble message={message} onExecuteAgentPlan={() => {}} />)

    expect(screen.queryByText('计划草案')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: '执行这条计划' })).not.toBeInTheDocument()
  })

  it('does not render execute action for a non-plan sentence even if persisted as draft', () => {
    const message: ChatMessage = {
      id: 'msg-plan-sentence',
      role: 'assistant',
      content: '计划：我会处理这个问题。',
      agent_plan: {
        mode: 'plan',
        status: 'draft',
        plan: '计划：我会处理这个问题。',
        updated_at: 1,
      },
      timestamp: 1,
    }

    render(<MessageBubble message={message} onExecuteAgentPlan={() => {}} />)

    expect(screen.queryByText('计划草案')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: '执行这条计划' })).not.toBeInTheDocument()
  })
})

describe('MessageBubble timeline orphan tools', () => {
  it('renders tool calls that are missing tool segments', () => {
    const message: ChatMessage = {
      id: 'msg-1',
      role: 'assistant',
      content: 'done',
      reasoning: 'thinking',
      segments: [
        {
          id: 'seg-reasoning',
          kind: 'reasoning',
          phase: 'plain',
          order: 1,
          text: 'thinking',
        },
        {
          id: 'seg-text',
          kind: 'text',
          phase: 'plain',
          order: 2,
          text: 'done',
        },
      ],
      tool_calls: [
        {
          id: 'tool-1',
          name: 'Read',
          source: 'external_cli',
          status: 'success',
          arguments: '{"path":"README.md"}',
        },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)
    expect(screen.getByText('Read')).toBeInTheDocument()
  })
})

describe('MessageBubble timeline grouping', () => {
  it('collapses a completed group into a one-line summary by default', () => {
    const message: ChatMessage = {
      id: 'msg-2',
      role: 'assistant',
      content: 'answer',
      segments: [
        { id: 'seg-r', kind: 'reasoning', phase: 'plain', order: 1, text: 'planning' },
        { id: 'seg-t', kind: 'tool', phase: 'tool_loop', order: 2, tool_call_id: 'tool-1' },
        { id: 'seg-text', kind: 'text', phase: 'plain', order: 3, text: 'answer' },
      ],
      tool_calls: [
        {
          id: 'tool-1',
          name: 'read_file',
          source: 'native',
          status: 'completed',
          arguments: '{"path":"a.ts"}',
        },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)
    expect(screen.getByText(/读取 1 个文件/)).toBeInTheDocument()
    // collapsed historical groups keep only the summary mounted
    expect(screen.getByLabelText('过程分组')).toHaveAttribute('aria-label', '过程分组')
    expect(screen.queryByText('planning')).not.toBeInTheDocument()
    expect(screen.queryByText('read_file')).not.toBeInTheDocument()
    // final answer text still renders
    expect(screen.getByText('answer')).toBeInTheDocument()
  })

  it('mounts completed group details only after the user expands it', async () => {
    const user = userEvent.setup()
    const message: ChatMessage = {
      id: 'msg-expand',
      role: 'assistant',
      content: 'answer',
      segments: [
        { id: 'seg-r', kind: 'reasoning', phase: 'plain', order: 1, text: 'planning details' },
        { id: 'seg-t', kind: 'tool', phase: 'tool_loop', order: 2, tool_call_id: 'tool-1' },
        { id: 'seg-text', kind: 'text', phase: 'plain', order: 3, text: 'answer' },
      ],
      tool_calls: [
        {
          id: 'tool-1',
          name: 'read_file',
          source: 'native',
          status: 'completed',
          arguments: '{"path":"a.ts"}',
        },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)
    const toggle = screen.getByRole('button', { name: /读取 1 个文件/ })
    expect(toggle).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByText('planning details')).not.toBeInTheDocument()
    expect(screen.queryByText('read_file')).not.toBeInTheDocument()

    await user.click(toggle)

    expect(toggle).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getByText('planning details')).toBeInTheDocument()
    // 展开后组内工具块挂载：Cursor 式动词 Read + 目标（文件名）
    expect(screen.getByText('a.ts')).toBeInTheDocument()
  })

  it('keeps many collapsed history tools out of the DOM until expanded', async () => {
    const user = userEvent.setup()
    const toolCount = 20
    const message: ChatMessage = {
      id: 'msg-heavy',
      role: 'assistant',
      content: 'final answer',
      segments: [
        ...Array.from({ length: toolCount }, (_, index) => ({
          id: `seg-tool-${index}`,
          kind: 'tool' as const,
          phase: 'tool_loop' as const,
          order: index,
          tool_call_id: `tool-${index}`,
        })),
        {
          id: 'seg-answer',
          kind: 'text',
          phase: 'plain',
          order: toolCount,
          text: 'final answer',
        },
      ],
      tool_calls: Array.from({ length: toolCount }, (_, index) => ({
        id: `tool-${index}`,
        name: 'write',
        source: 'native',
        status: 'completed',
        structured_content: {
          operation: 'write',
          resolvedPath: `file-${index}.ts`,
          additions: index + 1,
          removals: 0,
          diff: `diff payload ${index}`,
        },
      })),
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)

    expect(screen.getByRole('button', { name: /编辑 20 个文件/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    )
    expect(screen.queryByText('write')).not.toBeInTheDocument()
    expect(screen.queryByText('diff payload 0')).not.toBeInTheDocument()
    expect(screen.getByText('final answer')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: /编辑 20 个文件/ }))

    expect(screen.getAllByText('Write')).toHaveLength(toolCount)
    expect(screen.getAllByText('file-0.ts').length).toBeGreaterThan(0)
  })

  it('renders tool → text → tool as two separate groups', () => {
    const message: ChatMessage = {
      id: 'msg-3',
      role: 'assistant',
      content: 'final',
      segments: [
        { id: 'g1', kind: 'tool', phase: 'tool_loop', order: 1, tool_call_id: 'c1' },
        { id: 'txt', kind: 'text', phase: 'plain', order: 2, text: 'middle' },
        { id: 'g2', kind: 'tool', phase: 'tool_loop', order: 3, tool_call_id: 'c2' },
      ],
      tool_calls: [
        { id: 'c1', name: 'run_command', source: 'native', status: 'completed' },
        { id: 'c2', name: 'web_fetch', source: 'native', status: 'completed' },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)
    expect(screen.getAllByLabelText('过程分组')).toHaveLength(2)
    expect(screen.getByText('middle')).toBeInTheDocument()
  })

  it('keeps the last group expanded while the message is streaming', () => {
    const message: ChatMessage = {
      id: 'msg-4',
      role: 'assistant',
      content: '',
      segments: [
        { id: 'seg-t', kind: 'tool', phase: 'tool_loop', order: 1, tool_call_id: 'tool-1' },
      ],
      tool_calls: [
        {
          id: 'tool-1',
          name: 'run_command',
          source: 'native',
          // 工具已完成、但消息整体仍在流式：末组应保持展开，不折叠抖动
          status: 'completed',
        },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} messageStreaming />)
    expect(screen.getByText(/执行 1 条命令/)).toBeInTheDocument()
    // 展开态：组内工具块细节仍渲染（动词 Run）
    expect(screen.getByText('Run')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /执行 1 条命令/ })).toHaveAttribute(
      'aria-expanded',
      'true',
    )
  })

  it('collapses non-last groups even while streaming', () => {
    const message: ChatMessage = {
      id: 'msg-5',
      role: 'assistant',
      content: '',
      segments: [
        { id: 'g1', kind: 'tool', phase: 'tool_loop', order: 1, tool_call_id: 'c1' },
        { id: 'txt', kind: 'text', phase: 'plain', order: 2, text: 'middle' },
        { id: 'g2', kind: 'tool', phase: 'tool_loop', order: 3, tool_call_id: 'c2' },
      ],
      tool_calls: [
        { id: 'c1', name: 'run_command', source: 'native', status: 'completed' },
        { id: 'c2', name: 'web_fetch', source: 'native', status: 'running' },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} messageStreaming />)
    const groups = screen.getAllByLabelText('过程分组')
    expect(groups).toHaveLength(2)
    // 前组（被正文打断、非末组）折叠；末组展开
    expect(screen.getByRole('button', { name: /执行 1 条命令/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    )
    expect(screen.getByRole('button', { name: /正在读取 1 个网页/ })).toHaveAttribute(
      'aria-expanded',
      'true',
    )
  })

  it('collapses every group once streaming has finished', () => {
    const message: ChatMessage = {
      id: 'msg-6',
      role: 'assistant',
      content: '',
      segments: [
        { id: 'seg-t', kind: 'tool', phase: 'tool_loop', order: 1, tool_call_id: 'tool-1' },
      ],
      tool_calls: [
        { id: 'tool-1', name: 'run_command', source: 'native', status: 'completed' },
      ],
      timestamp: 1,
    }

    // messageStreaming 默认 false（历史消息）→ 末组也折叠
    render(<MessageBubble message={message} />)
    expect(screen.getByRole('button', { name: /执行 1 条命令/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    )
  })
})

describe('MessageBubble 多模型所发模型标签（R8）', () => {
  const userMessage: ChatMessage = {
    id: 'msg-user',
    role: 'user',
    content: '比较这几个模型',
    group_id: 'grp-1',
    timestamp: 1,
  }

  it('多模型（≥2）时在 user 气泡顶部渲染所发模型标签', () => {
    render(
      <MessageBubble
        message={userMessage}
        sentModels={[
          { providerId: 'deepseek', model: 'deepseek-chat' },
          { providerId: 'qwen', model: 'qwen-max' },
        ]}
      />,
    )
    expect(screen.getByText('@deepseek-chat')).toBeInTheDocument()
    expect(screen.getByText('@qwen-max')).toBeInTheDocument()
  })

  it('单模型 / 缺省时不渲染标签行（无回归）', () => {
    const { rerender } = render(
      <MessageBubble message={userMessage} sentModels={[{ providerId: 'deepseek', model: 'deepseek-chat' }]} />,
    )
    expect(screen.queryByText('@deepseek-chat')).not.toBeInTheDocument()
    rerender(<MessageBubble message={userMessage} />)
    expect(screen.queryByText(/^@/)).not.toBeInTheDocument()
  })
})

describe('MessageBubble 一键 rewind', () => {
  const userMessage: ChatMessage = {
    id: 'msg-user-rewind',
    role: 'user',
    content: '原始问题',
    timestamp: 1,
  }

  it('点击直接回调 rewind，不弹编辑框', async () => {
    const onRewindMessage = vi.fn().mockResolvedValue(undefined)
    render(<MessageBubble message={userMessage} onRewindMessage={onRewindMessage} />)

    await userEvent.click(screen.getByRole('button', { name: '回到这里' }))

    expect(onRewindMessage).toHaveBeenCalledWith('msg-user-rewind')
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument()
  })

  it('无回调时不渲染按钮（生成中被 MessageList 收走）', () => {
    render(<MessageBubble message={userMessage} />)
    expect(screen.queryByRole('button', { name: '回到这里' })).not.toBeInTheDocument()
  })
})

describe('MessageBubble 建分支', () => {
  const userMessage: ChatMessage = {
    id: 'msg-user-fork',
    role: 'user',
    content: '用户问题',
    timestamp: 1,
  }
  const assistantMessage: ChatMessage = {
    id: 'msg-asst-fork',
    role: 'assistant',
    content: '助手回答',
    timestamp: 2,
  }

  it('用户消息点分支按钮调用 onForkMessage(id)', async () => {
    const onForkMessage = vi.fn().mockResolvedValue(undefined)
    render(<MessageBubble message={userMessage} onForkMessage={onForkMessage} />)

    await userEvent.click(screen.getByRole('button', { name: '建分支' }))
    expect(onForkMessage).toHaveBeenCalledWith('msg-user-fork')
  })

  it('助手消息点分支按钮调用 onForkMessage(id)', async () => {
    const onForkMessage = vi.fn().mockResolvedValue(undefined)
    render(<MessageBubble message={assistantMessage} onForkMessage={onForkMessage} />)

    await userEvent.click(screen.getByRole('button', { name: '建分支' }))
    expect(onForkMessage).toHaveBeenCalledWith('msg-asst-fork')
  })

  it('无 onForkMessage 时用户消息不渲染分支按钮', () => {
    render(<MessageBubble message={userMessage} />)
    expect(screen.queryByRole('button', { name: '建分支' })).not.toBeInTheDocument()
  })
})



describe('MessageBubble explicit artifact presentation', () => {
  const artifact = {
    id: 'art_report',
    name: 'report.txt',
    mime_type: 'text/plain',
    data_url: 'data:text/plain;base64,cmVwb3J0',
    size_bytes: 6,
  }

  it('does not automatically show newly identified artifacts', () => {
    const message: ChatMessage = {
      id: 'msg-hidden-artifact',
      role: 'assistant',
      content: 'The file is ready.',
      artifacts: [artifact],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)

    expect(screen.queryByRole('button', { name: /report\.txt/ })).not.toBeInTheDocument()
  })

  it('shows only selected artifacts at the presentation segment position', () => {
    const message: ChatMessage = {
      id: 'msg-present-artifact',
      role: 'assistant',
      content: 'before\n\nafter',
      artifacts: [
        artifact,
        { ...artifact, id: 'art_hidden', name: 'hidden.txt' },
      ],
      segments: [
        { id: 'before', kind: 'text', phase: 'plain', order: 1, text: 'before' },
        { id: 'present', kind: 'tool', phase: 'tool_loop', order: 2, tool_call_id: 'call-present' },
        { id: 'after', kind: 'text', phase: 'synthesis', order: 3, text: 'after' },
      ],
      tool_calls: [
        {
          id: 'call-present',
          name: 'present_artifacts',
          source: 'native',
          status: 'completed',
          structured_content: {
            type: 'artifact_presentation',
            artifactIds: ['art_report'],
            caption: 'Download report',
          },
        },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)

    const before = screen.getByText('before')
    const file = screen.getByRole('button', { name: /report\.txt/ })
    const after = screen.getByText('after')
    expect(screen.getByText('Download report')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /hidden\.txt/ })).not.toBeInTheDocument()
    expect(before.compareDocumentPosition(file) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(file.compareDocumentPosition(after) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('renders presentations for legacy messages without timeline segments', () => {
    const message: ChatMessage = {
      id: 'msg-present-without-segments',
      role: 'assistant',
      content: 'See the report.',
      artifacts: [artifact],
      tool_calls: [
        {
          id: 'call-present',
          name: 'present_artifacts',
          source: 'native',
          status: 'completed',
          structured_content: {
            type: 'artifact_presentation',
            artifact_ids: ['art_report'],
          },
        },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)

    expect(screen.getByRole('button', { name: /report\.txt/ })).toBeInTheDocument()
  })

  it('reports unavailable artifact IDs without falling back to paths', () => {
    const message: ChatMessage = {
      id: 'msg-missing-artifact',
      role: 'assistant',
      content: '',
      artifacts: [artifact],
      segments: [
        { id: 'present', kind: 'tool', phase: 'tool_loop', order: 1, tool_call_id: 'call-present' },
      ],
      tool_calls: [
        {
          id: 'call-present',
          name: 'present_artifacts',
          source: 'native',
          status: 'completed',
          structured_content: {
            type: 'artifact_presentation',
            artifactIds: ['art_missing'],
          },
        },
      ],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)

    expect(screen.getByText(/^1 /)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /report\.txt/ })).not.toBeInTheDocument()
  })

  it('keeps historical artifacts without IDs visible', () => {
    const message: ChatMessage = {
      id: 'msg-legacy-artifact',
      role: 'assistant',
      content: 'Legacy message',
      artifacts: [{ ...artifact, id: undefined }],
      timestamp: 1,
    }

    render(<MessageBubble message={message} />)

    expect(screen.getByRole('button', { name: /report\.txt/ })).toBeInTheDocument()
  })
})
