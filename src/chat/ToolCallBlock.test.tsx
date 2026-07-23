import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { ToolCallBlock } from './ToolCallBlock'
import type { ToolCallRecord } from './types'

function buildToolCall(overrides: Partial<ToolCallRecord> = {}): ToolCallRecord {
  return {
    id: 'tool-1',
    toolName: 'read_file',
    status: 'success',
    result_preview: 'file contents loaded',
    ...overrides,
  }
}

describe('ToolCallBlock', () => {
  it('renders a capitalized verb + basename target, dropping status/source/duration', () => {
    render(<ToolCallBlock toolCall={buildToolCall({ arguments: { path: 'src/a/README.md' } })} />)
    const button = screen.getByRole('button', { name: /Read/ })
    // Cursor-style row: 大写动词 + 目标（文件名 basename）
    expect(within(button).getByText('Read')).toBeInTheDocument()
    expect(within(button).getByText('README.md')).toBeInTheDocument()
    // 已删除的后缀 / 全路径不再出现在折叠行
    expect(within(button).queryByText(/已完成/)).not.toBeInTheDocument()
    expect(within(button).queryByText(/Kivio/)).not.toBeInTheDocument()
    expect(within(button).queryByText(/file contents loaded/)).not.toBeInTheDocument()
    expect(within(button).queryByText(/src\/a/)).not.toBeInTheDocument()
  })

  it('shows the real read line range from structured content', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'read',
          arguments: { path: 'src/chat/Lens.tsx' },
          structured_content: { path: 'src/chat/Lens.tsx', start_line: 1880, end_line: 1939 },
        })}
      />,
    )
    const button = screen.getByRole('button', { name: /Read/ })
    expect(within(button).getByText('Lens.tsx L1880-1939')).toBeInTheDocument()
  })

  it('keeps the error out of the collapsed row and shows it (not red) in the expanded detail', async () => {
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          status: 'error',
          error: 'permission denied',
        })}
      />,
    )
    const button = screen.getByRole('button', { name: /Read/ })
    expect(within(button).queryByText(/permission denied/)).not.toBeInTheDocument()
    await user.click(button)
    const detail = screen.getByText(/permission denied/)
    expect(detail).toBeInTheDocument()
    // 错误不再标红
    expect(detail.className).not.toContain('text-red-500')
  })

  it('expands details when clicked', async () => {
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          arguments: { path: 'README.md' },
        })}
        defaultOpen={false}
      />,
    )
    await user.click(screen.getByRole('button', { name: /Read/ }))
    expect(screen.getByText('参数')).toBeInTheDocument()
    expect(screen.getAllByText(/README\.md/).length).toBeGreaterThan(0)
  })

  it('uses the search pattern as the grep target', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'grep',
          result_preview: '',
          arguments: {
            query: 'ClaudeAgentClient',
            path: 'packages/server/src/server/agent/providers/claude/agent.ts',
          },
        })}
      />,
    )
    const button = screen.getByRole('button', { name: /Grep/ })
    expect(within(button).getByText('Grep')).toBeInTheDocument()
    expect(within(button).getByText('ClaudeAgentClient')).toBeInTheDocument()
    // 目标只取 pattern，不再把 scope 塞进折叠行
    expect(within(button).queryByText(/agent\.ts/)).not.toBeInTheDocument()
  })

  it('renders glob as "Glob <pattern> in <dir>"', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'glob',
          result_preview: '',
          arguments: { pattern: '**/*overlay*', path: 'src/lens' },
        })}
      />,
    )
    const button = screen.getByRole('button', { name: /Glob/ })
    expect(within(button).getByText('Glob')).toBeInTheDocument()
    expect(within(button).getByText('**/*overlay* in lens')).toBeInTheDocument()
  })

  it('falls back to stored grep argument preview when parsed arguments are unavailable', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'grep',
          result_preview: '',
          arguments: '{"query":',
          argumentPreview: '正在生成工具参数…',
          argumentsPreview: '正在生成工具参数…',
        })}
      />,
    )
    const button = screen.getByRole('button', { name: /Grep/ })
    expect(within(button).getByText(/正在生成工具参数/)).toBeInTheDocument()
  })

  it('shows the command as the bash target', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'run_command',
          result_preview: 'exit_code: 0',
          arguments: { command: 'npm test' },
        })}
      />,
    )
    const button = screen.getByRole('button', { name: /Run/ })
    expect(within(button).getByText('Run')).toBeInTheDocument()
    expect(within(button).getByText('npm test')).toBeInTheDocument()
    expect(within(button).queryByText(/exit_code/)).not.toBeInTheDocument()
  })

  it('renders a subagent record as a SUBAGENT consult card, expandable to the task', async () => {
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'agent',
          source: 'native',
          status: 'success',
          structured_content: { type: 'subagent', agentType: 'researcher', result: '调查结论' },
          arguments: { subagent_type: 'researcher', prompt: '去调查一下这个问题' },
        })}
      />,
    )
    expect(screen.getByText('SUBAGENT')).toBeInTheDocument()
    expect(screen.getByText('researcher')).toBeInTheDocument()
    await user.click(screen.getByRole('button'))
    expect(screen.getByText('Task')).toBeInTheDocument()
    expect(screen.getByText('去调查一下这个问题')).toBeInTheDocument()
    expect(screen.getByText('调查结论')).toBeInTheDocument()
  })

  it('renders an advisor record as an ADVISOR consult card, expandable to the advice', async () => {
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'advisor',
          source: 'native',
          status: 'success',
          structured_content: { type: 'advisor', model: 'opus', question: '该怎么办', advice: '这样做' },
        })}
      />,
    )
    expect(screen.getByText('ADVISOR')).toBeInTheDocument()
    expect(screen.getByText('opus')).toBeInTheDocument()
    await user.click(screen.getByRole('button'))
    expect(screen.getByText('Question')).toBeInTheDocument()
    expect(screen.getByText('Advice')).toBeInTheDocument()
    expect(screen.getByText('这样做')).toBeInTheDocument()
  })

  it('renders a knowledge_search record as a KNOWLEDGE consult card with query and hits', async () => {
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'knowledge_search',
          source: 'native',
          status: 'success',
          arguments: { query: '替换翻译怎么工作' },
          structured_content: {
            hits: [
              { n: 1, docName: 'design.md', headingPath: '几何', score: 0.91, text: '命中片段内容' },
            ],
          },
        })}
      />,
    )
    expect(screen.getByText('KNOWLEDGE')).toBeInTheDocument()
    expect(screen.getByText('1 段')).toBeInTheDocument()
    await user.click(screen.getByRole('button'))
    expect(screen.getByText('Query')).toBeInTheDocument()
    expect(screen.getByText('替换翻译怎么工作')).toBeInTheDocument()
    expect(screen.getByText('命中片段内容')).toBeInTheDocument()
    expect(screen.getByText('[1]')).toBeInTheDocument()
  })

  it('renders a run_python record as a PYTHON consult card with code and output', async () => {
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'run_python',
          source: 'native',
          status: 'success',
          result_preview: 'hello from stdout',
          arguments: { code: 'print("hello from stdout")' },
        })}
      />,
    )
    expect(screen.getByText('PYTHON')).toBeInTheDocument()
    await user.click(screen.getByRole('button'))
    expect(screen.getByText('Code')).toBeInTheDocument()
    expect(screen.getByText('print("hello from stdout")')).toBeInTheDocument()
    expect(screen.getByText('Output')).toBeInTheDocument()
    expect(screen.getByText('hello from stdout')).toBeInTheDocument()
  })

  it('preserves newlines/indentation in the PYTHON card code block', async () => {
    const user = userEvent.setup()
    const code = 'def f():\n    return 1'
    const { container } = render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'run_python',
          source: 'native',
          status: 'success',
          arguments: { code },
        })}
      />,
    )
    await user.click(screen.getByRole('button'))
    // Code must NOT be whitespace-collapsed (regression guard against compactText):
    // assert the raw newline + indentation survive in the <pre> textContent.
    const pre = container.querySelector('pre')
    expect(pre?.textContent).toBe(code)
  })

  it('renders the built-in web search record via the default compact card (Web search · provider)', () => {
    // 任务 07-23:内置搜索复用默认 web_search 工具卡渲染(不再单独做卡片),
    // 头部显示「Web search · <provider>」,provider 取自 structured_content.provider。
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'web_search',
          source: 'native',
          status: 'success',
          arguments: JSON.stringify({ query: 'kivio release' }),
          structured_content: {
            type: 'builtin_web_search',
            provider: 'OpenAI',
            queries: ['kivio release'],
            citations: [{ title: 'A 站', url: 'https://a.com' }],
          },
        })}
      />,
    )
    expect(screen.getByText(/Web search · OpenAI/)).toBeInTheDocument()
  })
})
