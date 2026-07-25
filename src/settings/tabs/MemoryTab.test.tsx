import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { MemoryTab } from './MemoryTab'
import { i18n } from '../i18n'

const t = i18n.zh
void t

/**
 * 回归重点：
 *   1. L1 / L2 两个编辑器的 value / savedValue / maxBytes 不串（同类型，接错不报错）
 *   2. onDraftChange 带上正确的 layer（shell 侧靠它写对 key，并清 success 提示）
 *   3. L1 超字节上限时禁用保存
 */
type Props = Parameters<typeof MemoryTab>[0]

function renderTab(overrides: Partial<Props> = {}) {
  const props = {
    lang: 'zh' as const,
    chatMemory: { enabled: true } as Props['chatMemory'],
    memoryDir: '/tmp/kivio/memory',
    memoryError: '',
    memorySuccess: '',
    memoryLoading: false,
    memorySavingLayer: null,
    memoryDrafts: { l1: 'L1 内容', l2: 'L2 内容' },
    memorySnapshots: { l1: 'L1 内容', l2: 'L2 内容' },
    onUpdateChatMemory: vi.fn(),
    onRefresh: vi.fn(),
    onOpenFolder: vi.fn(),
    onDraftChange: vi.fn(),
    onSaveLayer: vi.fn(),
    ...overrides,
  }
  render(<MemoryTab {...props} />)
  return props
}

describe('MemoryTab', () => {
  it('L1 / L2 各自回显自己的草稿（不串）', () => {
    renderTab()
    expect(screen.getByDisplayValue('L1 内容')).toBeTruthy()
    expect(screen.getByDisplayValue('L2 内容')).toBeTruthy()
  })

  it('只有 L1 显示字节上限', () => {
    renderTab()
    // L1 带 maxBytes → "x / 5000 bytes"；L2 只有 "x bytes"
    expect(screen.getByText(/\/ 5000 bytes/)).toBeTruthy()
  })

  it('编辑 L1 时 onDraftChange 带 layer=l1', async () => {
    const props = renderTab()
    const areas = screen.getAllByRole('textbox')
    await userEvent.type(areas[0], 'x')
    expect(props.onDraftChange).toHaveBeenCalledWith('l1', expect.stringContaining('x'))
  })

  it('编辑 L2 时 onDraftChange 带 layer=l2', async () => {
    const props = renderTab()
    const areas = screen.getAllByRole('textbox')
    await userEvent.type(areas[1], 'y')
    expect(props.onDraftChange).toHaveBeenCalledWith('l2', expect.stringContaining('y'))
  })

  it('草稿与快照一致时保存按钮禁用', () => {
    renderTab()
    for (const btn of screen.getAllByRole('button', { name: '保存' })) {
      expect(btn).toBeDisabled()
    }
  })

  it('草稿改动后保存按钮可用，点击带对应 layer', async () => {
    const props = renderTab({
      memoryDrafts: { l1: 'L1 改过', l2: 'L2 内容' },
    })
    const saves = screen.getAllByRole('button', { name: '保存' })
    expect(saves[0]).not.toBeDisabled()
    await userEvent.click(saves[0])
    expect(props.onSaveLayer).toHaveBeenCalledWith('l1')
  })

  it('L1 超 5000 字节时禁用保存', () => {
    renderTab({
      memoryDrafts: { l1: 'x'.repeat(5001), l2: 'L2 内容' },
      memorySnapshots: { l1: '', l2: 'L2 内容' },
    })
    expect(screen.getAllByRole('button', { name: '保存' })[0]).toBeDisabled()
  })

  it('显示记忆目录路径', () => {
    renderTab()
    expect(screen.getByText('/tmp/kivio/memory')).toBeTruthy()
  })

  it('错误与成功提示分别渲染', () => {
    renderTab({ memoryError: '读取失败' })
    expect(screen.getByText('读取失败')).toBeTruthy()
  })

  it('保存中状态只作用于对应层', () => {
    renderTab({ memorySavingLayer: 'l2' })
    expect(screen.getAllByRole('button', { name: '保存中' })).toHaveLength(1)
  })
})
