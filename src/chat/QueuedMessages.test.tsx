import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { QueuedMessages } from './QueuedMessages'
import type { QueuedMessage } from './hooks/useMessageQueue'

function queued(overrides: Partial<QueuedMessage> = {}): QueuedMessage {
  return {
    id: 'q1',
    content: '改用 rg',
    attachments: [],
    steering: false,
    ...overrides,
  }
}

function setup(messages: QueuedMessage[], canSteer = true) {
  const onSteer = vi.fn()
  const onRemove = vi.fn()
  const onRestore = vi.fn()
  render(
    <QueuedMessages
      messages={messages}
      canSteer={canSteer}
      onSteer={onSteer}
      onRemove={onRemove}
      onRestore={onRestore}
      lang="zh"
    />,
  )
  return { onSteer, onRemove, onRestore }
}

describe('QueuedMessages', () => {
  it('空队列不渲染任何东西', () => {
    const { container } = render(
      <QueuedMessages
        messages={[]}
        canSteer
        onSteer={vi.fn()}
        onRemove={vi.fn()}
        onRestore={vi.fn()}
        lang="zh"
      />,
    )
    expect(container.firstChild).toBeNull()
  })

  it('逐条列出，每条都有引导与删除入口', () => {
    const { onSteer, onRemove } = setup([queued(), queued({ id: 'q2', content: '再看下测试' })])
    expect(screen.getByText('改用 rg')).toBeTruthy()
    expect(screen.getByText('再看下测试')).toBeTruthy()
    const steerButtons = screen.getAllByLabelText(/立刻引导/)
    expect(steerButtons).toHaveLength(2)
    steerButtons[1].click()
    expect(onSteer).toHaveBeenCalledWith('q2')
    screen.getAllByLabelText('移出队列')[0].click()
    expect(onRemove).toHaveBeenCalledWith('q1')
  })

  // 外部 CLI / 多模型一问多答下后端注入不了：入口必须消失，而不是点了没反应。
  it('不支持引导时只保留删除', () => {
    setup([queued()], false)
    expect(screen.queryByLabelText(/立刻引导/)).toBeNull()
    expect(screen.getByLabelText('移出队列')).toBeTruthy()
  })

  // 等待态不写字（转圈就够看），文案只留在 title 里。
  it('已提交引导的那条转成等待态：不能撤回、不再给按钮', () => {
    const { onRestore } = setup([queued({ steering: true })])
    expect(screen.queryByLabelText(/立刻引导/)).toBeNull()
    expect(screen.queryByLabelText('移出队列')).toBeNull()
    screen.getByTitle('已提交，下一轮生效').click()
    expect(onRestore).not.toHaveBeenCalled()
  })

  it('Pi follow-up 提交中显示独立状态并锁定操作', () => {
    const { onRestore } = setup([queued({ followingUp: true })])
    expect(screen.getByTitle('正在排入下一轮')).toBeTruthy()
    expect(screen.queryByLabelText(/立刻引导/)).toBeNull()
    expect(screen.queryByLabelText('移出队列')).toBeNull()
    screen.getByTitle('正在排入下一轮').click()
    expect(onRestore).not.toHaveBeenCalled()
  })

  // 撤回只把文字还给输入框，所以带附件的不给撤——否则附件会静默消失。
  it('带附件的条目不能撤回，但仍标出附件数', () => {
    const { onRestore } = setup([queued({
      attachments: [{ id: 'a1', type: 'image', name: 'a.png', path: '/tmp/a.png' }],
    })])
    expect(screen.getByText('+1')).toBeTruthy()
    screen.getByTitle(/不能撤回/).click()
    expect(onRestore).not.toHaveBeenCalled()
  })

  it('引导被拒时在条目上说明，按钮仍在（可重试）', () => {
    setup([queued({ steerRejected: true })])
    expect(screen.getByText('插不进这轮')).toBeTruthy()
    expect(screen.getByLabelText(/立刻引导/)).toBeTruthy()
  })

  it('Pi follow-up 被拒时说明会降级为轮末发送', () => {
    setup([queued({ followUpRejected: true })])
    expect(screen.getByText('排队失败，轮末发送')).toBeTruthy()
    expect(screen.getByLabelText(/立刻引导/)).toBeTruthy()
  })

  it('普通条目点文本即撤回', () => {
    const { onRestore } = setup([queued()])
    screen.getByTitle('点击撤回到输入框').click()
    expect(onRestore).toHaveBeenCalledWith('q1')
  })
})
