import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { DegradedAnswerCard } from './DegradedAnswerCard'
import type { DegradedAnswer } from './types'

function makeDegraded(overrides: Partial<DegradedAnswer> = {}): DegradedAnswer {
  return {
    kind: 'rate_limited',
    reason: '模型调用被限流或配额耗尽，多次退避重试后仍失败。',
    toolSummaries: [
      { name: 'mixer_generate_image', preview: 'Generated 1 image.' },
      { name: 'present_artifacts', preview: 'Displayed 1 file in the response.' },
    ],
    text: '⚠️ ...纯文本版本...',
    ...overrides,
  }
}

describe('DegradedAnswerCard', () => {
  it('显示失败原因与工具调用计数（不复述工具列表）', () => {
    render(<DegradedAnswerCard degraded={makeDegraded()} />)
    expect(screen.getByText(/限流或配额耗尽/)).toBeTruthy()
    expect(screen.getByText(/本轮已完成 2 个工具调用/)).toBeTruthy()
    // 工具结果在上方的工具卡片里，卡片里再列一遍就是噪音
    expect(screen.queryByText('mixer_generate_image')).toBeNull()
  })

  it('有 detail 时显示供应商原始报错', () => {
    render(<DegradedAnswerCard degraded={makeDegraded({ detail: 'context deadline exceeded' })} />)
    expect(screen.getByText('context deadline exceeded')).toBeTruthy()
  })

  it('按 kind 选择标签（不解析文案）', () => {
    const cases: Array<[string, string]> = [
      ['rate_limited', '限流 / 配额'],
      ['context_overflow', '上下文超长'],
      ['timeout', '超时 / 连接中断'],
      ['moderation', '内容审核拦截'],
      ['empty_response', '空响应'],
    ]
    for (const [kind, label] of cases) {
      const { unmount } = render(<DegradedAnswerCard degraded={makeDegraded({ kind })} />)
      expect(screen.getByText(label)).toBeTruthy()
      unmount()
    }
  })

  it('未知 kind 回落到通用标签而非崩溃', () => {
    render(<DegradedAnswerCard degraded={makeDegraded({ kind: 'something_new' })} />)
    expect(screen.getByText('调用失败')).toBeTruthy()
  })

  it('没有工具摘要时不渲染摘要区', () => {
    render(<DegradedAnswerCard degraded={makeDegraded({ toolSummaries: [] })} />)
    expect(screen.queryByText(/本轮已完成/)).toBeNull()
    // 原因仍要显示
    expect(screen.getByText(/限流或配额耗尽/)).toBeTruthy()
  })

  it('toolSummaries 缺省（旧数据）不崩溃', () => {
    const d = makeDegraded()
    delete (d as Partial<DegradedAnswer>).toolSummaries
    render(<DegradedAnswerCard degraded={d} />)
    expect(screen.getByText(/限流或配额耗尽/)).toBeTruthy()
  })

  it('kind 暴露到 DOM 供样式/排查定位', () => {
    const { container } = render(<DegradedAnswerCard degraded={makeDegraded()} />)
    expect(container.querySelector('[data-degraded-kind="rate_limited"]')).toBeTruthy()
  })
})
