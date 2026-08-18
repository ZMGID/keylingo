import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

const openExternal = vi.fn(() => Promise.resolve())
vi.mock('../api/tauri', () => ({
  api: {
    get openExternal() {
      return openExternal
    },
    get openLocalFile() {
      return vi.fn(() => Promise.resolve())
    },
  },
  isTauriRuntime: () => true,
}))

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

  // 外部 CLI（claude）的子代理调用没有 structured content：按 source+名字认进同一张
  // SUBAGENT 卡，description 当人话任务名，终态结果从 result_preview 兜底。
  it('renders an external CLI Agent call as a SUBAGENT consult card', async () => {
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'Agent',
          source: 'external_cli',
          status: 'success',
          result_preview: '搜到 8 条本周 AI 资讯。',
          arguments: {
            description: '搜索最近 AI 资讯',
            prompt: '用 WebSearch 搜索最近两周的 AI 资讯',
            subagent_type: 'general-purpose',
          },
        })}
      />,
    )
    expect(screen.getByText('SUBAGENT')).toBeInTheDocument()
    expect(screen.getByText('general-purpose')).toBeInTheDocument()
    expect(screen.getByText('搜索最近 AI 资讯')).toBeInTheDocument()
    await user.click(screen.getByRole('button'))
    expect(screen.getByText('用 WebSearch 搜索最近两周的 AI 资讯')).toBeInTheDocument()
    expect(screen.getByText('搜到 8 条本周 AI 资讯。')).toBeInTheDocument()
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

  it('renders the built-in web search record as a dedicated source card', async () => {
    // 内置搜索从「默认工具卡」升级为独立 WEB SEARCH 卡：头部带 provider 与来源计数，
    // 展开后是编号可点的来源目录（标题 / 域名 / 日期 / 摘要）。
    const user = userEvent.setup()
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
            citations: [
              { title: 'A 站', url: 'https://a.com' },
              {
                title: 'B 站',
                url: 'https://www.b.com',
                snippet: '关于 kivio 的发布说明',
                published_date: '2025-06-01',
              },
            ],
          },
        })}
      />,
    )
    expect(screen.getByText('WEB SEARCH')).toBeInTheDocument()
    expect(screen.getByText('OpenAI')).toBeInTheDocument()
    expect(screen.getByText('2 来源')).toBeInTheDocument()
    // 展开前来源目录不渲染。
    expect(screen.queryByText('A 站')).not.toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: /WEB SEARCH/ }))
    expect(screen.getByText('A 站')).toBeInTheDocument()
    expect(screen.getByText('a.com')).toBeInTheDocument()
    expect(screen.getByText(/b\.com · 2025-06-01/)).toBeInTheDocument() // www 前缀剥离 + 日期
    expect(screen.getByText('关于 kivio 的发布说明')).toBeInTheDocument()
    expect(screen.getByText(/2025-06-01/)).toBeInTheDocument()
    // 点来源行 → 浏览器打开（不导航 webview）。
    await user.click(screen.getByText('A 站'))
    expect(openExternal).toHaveBeenCalledWith('https://a.com')
  })

  it('renders third-party search_web sources via the same card', async () => {
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'web_search',
          source: 'native',
          status: 'success',
          arguments: JSON.stringify({ query: '天气' }),
          structured_content: {
            type: 'third_party_web_search',
            provider: 'Tavily',
            queries: ['天气'],
            citations: [
              {
                title: '气象台',
                url: 'https://weather.example/1',
                snippet: '今天晴',
                published_date: '2025-06-02',
              },
            ],
          },
        })}
      />,
    )
    expect(screen.getByText('Tavily')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: /WEB SEARCH/ }))
    expect(screen.getByText(/weather\.example/)).toBeInTheDocument()
    expect(screen.getByText('今天晴')).toBeInTheDocument()
  })

  it('falls back to the plain result text when structured citations are absent', async () => {
    // 旧数据（structured 只有 provider / 外部 CLI 无 structured）→ 结果区原样展示文本。
    const user = userEvent.setup()
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'web_search',
          source: 'native',
          status: 'success',
          arguments: JSON.stringify({ query: 'x' }),
          structured_content: { provider: 'Tavily' },
          result_preview: 'Web search context:\n[1] A\nURL: https://a.com',
        })}
      />,
    )
    await user.click(screen.getByRole('button', { name: /WEB SEARCH/ }))
    expect(screen.getByText(/Web search context/)).toBeInTheDocument()
    expect(screen.getByText(/URL: https:\/\/a\.com/)).toBeInTheDocument()
  })

  // ---- 外部 CLI（claude Code）的内置工具：名字 PascalCase + 字段名 file_path ----
  //
  // 两处**各自独立**的错配：工具名不归一化（switch 分支写的是小写）、参数字段名只读
  // `path`（claude 用 `file_path`）。两者任一没修，折叠行都会退到
  // `previewValue(arguments)` —— 一坨 220 字符截断的 JSON，完全不可扫读。

  it('maps claude PascalCase Read + file_path to the verb/target row', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'Read',
          source: 'external_cli',
          arguments: JSON.stringify({ file_path: 'E:/proj/src/chat/Chat.tsx' }),
        })}
      />,
    )
    const button = screen.getByRole('button')
    expect(within(button).getByText('Read')).toBeInTheDocument()
    expect(within(button).getByText('Chat.tsx')).toBeInTheDocument()
    // 修复前的症状：整个参数 JSON 落在折叠行上。
    expect(within(button).queryByText(/file_path/)).not.toBeInTheDocument()
  })

  it('shows live dsh subagent steps instead of a frozen 运行中', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'subagent',
          source: 'external_cli',
          status: 'running',
          arguments: {
            description: '搜索最新AI资讯',
            prompt: '去网上搜最近的模型发布',
          },
          structured_content: {
            backgroundTaskId: 'child-9',
            subagentProgress: {
              taskId: 'child-9',
              status: 'running',
              preview: '正在检索…',
              steps: ['web_search 最新AI资讯'],
            },
          },
        })}
      />,
    )
    expect(screen.getByText('SUBAGENT')).toBeInTheDocument()
    expect(screen.getByText('web_search 最新AI资讯')).toBeInTheDocument()
    expect(screen.queryByText('运行中…')).not.toBeInTheDocument()
  })

  it('keeps a dsh background subagent launch as running, not completed', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'subagent',
          source: 'external_cli',
          status: 'success',
          arguments: {
            description: '搜索最新AI资讯',
            prompt: '去网上搜最近的模型发布',
          },
          result_preview: 'started subagent 018b08fc-ee7f-4ea5-b77c-9c5d1c6ecf50',
        })}
      />,
    )
    expect(screen.getByText('SUBAGENT')).toBeInTheDocument()
    expect(screen.getByText('运行中…')).toBeInTheDocument()
    expect(screen.queryByText('已完成')).not.toBeInTheDocument()
    expect(screen.queryByText(/started subagent/)).not.toBeInTheDocument()
  })

  it('keeps a dsh one-shot background subagent job receipt as running', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'subagent',
          source: 'external_cli',
          status: 'success',
          arguments: {
            description: '搜索最新AI资讯',
            prompt: '去网上搜最近的模型发布',
          },
          result_preview: 'started background subagent job job_9',
        })}
      />,
    )
    expect(screen.getByText('SUBAGENT')).toBeInTheDocument()
    expect(screen.getByText('运行中…')).toBeInTheDocument()
    expect(screen.queryByText('已完成')).not.toBeInTheDocument()
    expect(screen.queryByText(/started background subagent job/)).not.toBeInTheDocument()
  })

  it('renders a dsh subagent call as a SUBAGENT consult card', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'subagent',
          source: 'external_cli',
          status: 'running',
          arguments: {
            description: '读协议',
            prompt: '把 dsh 的 session 事件对上 Kivio',
          },
        })}
      />,
    )
    expect(screen.getByText('SUBAGENT')).toBeInTheDocument()
    expect(screen.getByText('读协议')).toBeInTheDocument()
  })

  it('maps dsh str_replace_editor and job_output off the raw JSON row', () => {
    const { unmount } = render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'str_replace_editor',
          source: 'external_cli',
          arguments: JSON.stringify({
            command: 'str_replace',
            path: 'E:/proj/src/chat/segments.ts',
            old_str: 'a',
            new_str: 'b',
          }),
        })}
      />,
    )
    let button = screen.getByRole('button')
    expect(within(button).getByText('Edit')).toBeInTheDocument()
    expect(within(button).getByText('segments.ts')).toBeInTheDocument()
    expect(within(button).queryByText(/str_replace_editor/)).not.toBeInTheDocument()
    unmount()

    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'job_output',
          source: 'external_cli',
          arguments: JSON.stringify({ job_id: 'job_12' }),
        })}
      />,
    )
    button = screen.getByRole('button')
    expect(within(button).getByText('Run')).toBeInTheDocument()
    expect(within(button).getByText('job_12')).toBeInTheDocument()
  })

  it('maps dsh run_code to the Python verb, not the raw name', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'run_code',
          source: 'external_cli',
          arguments: JSON.stringify({ code: 'console.log(1)' }),
        })}
      />,
    )
    const button = screen.getByRole('button')
    expect(within(button).getByText('Python')).toBeInTheDocument()
    expect(within(button).queryByText(/run_code/)).not.toBeInTheDocument()
  })

  it('maps dsh pwsh to Run + the description, not the raw JSON', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'pwsh',
          source: 'external_cli',
          arguments: JSON.stringify({
            command: 'python -X utf8 -c "print(1)"',
            description: 'Show sheet structure of three Excel files',
          }),
        })}
      />,
    )
    const button = screen.getByRole('button')
    expect(within(button).getByText('Run')).toBeInTheDocument()
    expect(within(button).getByText('Show sheet structure of three Excel files')).toBeInTheDocument()
    expect(within(button).queryByText(/pwsh/)).not.toBeInTheDocument()
    expect(within(button).queryByText(/"command"/)).not.toBeInTheDocument()
  })

  it('maps claude Bash to Run + the command', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'Bash',
          source: 'external_cli',
          arguments: JSON.stringify({ command: 'npm run typecheck', description: 'Typecheck' }),
        })}
      />,
    )
    const button = screen.getByRole('button')
    expect(within(button).getByText('Run')).toBeInTheDocument()
    expect(within(button).getByText('Typecheck')).toBeInTheDocument()
  })

  it('maps claude Grep / Glob to their pattern targets', () => {
    const { unmount } = render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'Grep',
          source: 'external_cli',
          arguments: JSON.stringify({ pattern: 'kill_process_group', path: 'src-tauri' }),
        })}
      />,
    )
    let button = screen.getByRole('button')
    expect(within(button).getByText('Grep')).toBeInTheDocument()
    expect(within(button).getByText('kill_process_group')).toBeInTheDocument()
    unmount()

    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'Glob',
          source: 'external_cli',
          arguments: JSON.stringify({ pattern: '**/*.rs', path: 'src-tauri/src' }),
        })}
      />,
    )
    button = screen.getByRole('button')
    expect(within(button).getByText('Glob')).toBeInTheDocument()
    expect(within(button).getByText(/\*\*\/\*\.rs/)).toBeInTheDocument()
  })

  it('maps claude Edit (old_string/new_string) to Edit + the file basename', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'Edit',
          source: 'external_cli',
          arguments: JSON.stringify({
            file_path: 'src-tauri/src/external_agents/run.rs',
            old_string: 'let _ = spawned.child.start_kill();',
            new_string: 'kill_agent_process_tree(&mut spawned.child);',
          }),
        })}
      />,
    )
    const button = screen.getByRole('button')
    expect(within(button).getByText('Edit')).toBeInTheDocument()
    expect(within(button).getByText('run.rs')).toBeInTheDocument()
  })

  // claude 的问用户走的是它自己的工具名。认不出来 = 渲染成普通工具卡（一坨 JSON），
  // 而不是这行「等待你回答」的痕迹 —— 整张可作答的面板吊在输入框上方，见 AskUserBlock.test。
  it('renders claude AskUserQuestion as the inline ask-user trace', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'AskUserQuestion',
          source: 'external_cli',
          status: 'running',
          structured_content: {
            askUser: {
              phase: 'awaiting',
              questions: [{
                id: '0',
                prompt: '用哪种方式重试？',
                options: [{ id: '0', label: '指数退避' }, { id: '1', label: '立即重试' }],
                allow_multiple: false,
                allow_custom: true,
              }],
              answers: {},
            },
          },
        })}
      />,
    )
    expect(screen.getByText(/等待你回答/)).toBeInTheDocument()
    // 消息流里不该出现第二份可点的选项（会和面板抢焦点、也能被点两次）。
    expect(screen.queryByRole('option')).not.toBeInTheDocument()
    expect(screen.queryByText('指数退避')).not.toBeInTheDocument()
  })

  it('does not treat claude ExitPlanMode as the ask-user card', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'ExitPlanMode',
          source: 'external_cli',
          status: 'running',
          arguments: { plan: '先改测试再改实现。' },
        })}
      />,
    )
    expect(screen.queryByText(/等待你回答/)).not.toBeInTheDocument()
    expect(screen.queryByText(/等待用户确认/)).not.toBeInTheDocument()
  })

  it('renders dsh exit_plan_mode as the ask-user card', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'exit_plan_mode',
          source: 'external_cli',
          status: 'running',
          structured_content: {
            askUser: {
              phase: 'awaiting',
              questions: [{
                id: 'plan',
                prompt: '按这份计划执行？',
                options: [{ id: '0', label: '执行' }, { id: '1', label: '再改改' }],
                allow_multiple: false,
                allow_custom: true,
              }],
              answers: {},
            },
          },
        })}
      />,
    )
    expect(screen.getByText(/等待你回答/)).toBeInTheDocument()
  })

  it('renders dsh ask_user_question as the same inline ask-user trace', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'ask_user_question',
          source: 'external_cli',
          status: 'running',
          structured_content: {
            askUser: {
              phase: 'awaiting',
              questions: [{
                id: 'runtime',
                prompt: '用哪个运行时？',
                options: [{ id: '0', label: 'Bun' }, { id: '1', label: 'Node' }],
                allow_multiple: false,
                allow_custom: true,
              }],
              answers: {},
            },
          },
        })}
      />,
    )
    expect(screen.getByText(/等待你回答/)).toBeInTheDocument()
    expect(screen.queryByRole('option')).not.toBeInTheDocument()
  })

  it('maps claude WebFetch / TodoWrite through the snake_case aliases', () => {
    const { unmount } = render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'WebFetch',
          source: 'external_cli',
          arguments: JSON.stringify({ url: 'https://example.com/docs', prompt: 'summarize' }),
        })}
      />,
    )
    let button = screen.getByRole('button')
    expect(within(button).getByText('Fetch')).toBeInTheDocument()
    expect(within(button).getByText(/example\.com\/docs/)).toBeInTheDocument()
    unmount()

    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'TodoWrite',
          source: 'external_cli',
          arguments: JSON.stringify({
            todos: [
              { content: 'a', status: 'completed' },
              { content: 'b', status: 'pending' },
            ],
          }),
        })}
      />,
    )
    button = screen.getByRole('button')
    expect(within(button).getByText('Update todos')).toBeInTheDocument()
    expect(within(button).getByText('1/2')).toBeInTheDocument()
  })

  it('renders dsh todo_write as the same todo card', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'todo_write',
          source: 'external_cli',
          arguments: JSON.stringify({
            todos: [
              { content: '读协议', status: 'completed' },
              { content: '接线', status: 'in_progress' },
            ],
          }),
        })}
      />,
    )
    const button = screen.getByRole('button')
    expect(within(button).getByText('Update todos')).toBeInTheDocument()
    expect(within(button).getByText('1/2')).toBeInTheDocument()
  })

  it('renders claude TaskCreate / TaskUpdate as readable task rows', () => {
    const { unmount } = render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'TaskCreate',
          source: 'external_cli',
          arguments: JSON.stringify({
            subject: 'Phase 0：安装 .NET 10 SDK',
            activeForm: '安装 .NET 10 SDK',
          }),
        })}
      />,
    )
    let button = screen.getByRole('button')
    expect(within(button).getByText('Create task')).toBeInTheDocument()
    expect(within(button).getByText('Phase 0：安装 .NET 10 SDK')).toBeInTheDocument()
    unmount()

    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'TaskUpdate',
          source: 'external_cli',
          arguments: JSON.stringify({ taskId: '1', status: 'in_progress' }),
        })}
      />,
    )
    button = screen.getByRole('button')
    expect(within(button).getByText('Update task')).toBeInTheDocument()
    expect(within(button).getByText(/进行中/)).toBeInTheDocument()
    expect(within(button).getByText(/1/)).toBeInTheDocument()
  })

  it('renders claude TaskCreate with todoState as the shared todo card', () => {
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'TaskCreate',
          source: 'external_cli',
          status: 'success',
          arguments: JSON.stringify({ subject: '整理工作目录文件' }),
          structured_content: {
            subject: '整理工作目录文件',
            todoState: {
              items: [
                { id: '1', content: '整理工作目录文件', status: 'pending' },
                { id: '2', content: '写一个示例脚本', status: 'pending' },
              ],
              updated_at: 1,
            },
          },
        })}
      />,
    )
    const button = screen.getByRole('button')
    expect(within(button).getByText('Update todos')).toBeInTheDocument()
    expect(within(button).getByText('0/2')).toBeInTheDocument()
  })

  it('keeps MCP tool names verbatim (normalization must not lowercase the display name)', () => {
    // 归一化只用于 switch 匹配。MCP 工具名的大小写有意义，把它小写化会改坏显示。
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'mcp__notion__searchPages',
          source: 'mcp',
          arguments: JSON.stringify({ q: 'kivio' }),
        })}
      />,
    )
    expect(screen.getByText('mcp__notion__searchPages')).toBeInTheDocument()
  })

  it('still renders Kivio native snake_case tools unchanged', () => {
    // 归一化不得让原生工具落空：`read_file` / `search_files` 等必须仍命中各自分支。
    render(
      <ToolCallBlock
        toolCall={buildToolCall({
          toolName: 'search_files',
          source: 'native',
          arguments: JSON.stringify({ query: 'usage_parts_all_zero', path: 'src-tauri' }),
        })}
      />,
    )
    const button = screen.getByRole('button')
    expect(within(button).getByText('Grep')).toBeInTheDocument()
    expect(within(button).getByText('usage_parts_all_zero')).toBeInTheDocument()
  })
})
