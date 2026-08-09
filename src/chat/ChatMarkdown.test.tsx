import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { ChatMarkdown } from './ChatMarkdown'
import { clearSettledMarkdownCache, settledMarkdownCacheSize } from './settledMarkdownCache'

describe('ChatMarkdown settled cache', () => {
  it('reuses a bounded settled normalization entry across remounts', () => {
    clearSettledMarkdownCache()
    const content = 'A settled answer with **markdown**.'
    const first = render(<ChatMarkdown content={content} />)
    first.unmount()
    render(<ChatMarkdown content={content} />)
    expect(settledMarkdownCacheSize()).toBe(1)
  })
})

// 回归测试：ChatMarkdown 因 props 变化（如 artifacts/citations 换引用）重渲时，公式（LazyMath）
// 不能被卸载重挂——否则真机里会闪一下「原始 LaTeX → 公式」。jsdom 无 IntersectionObserver，
// LazyMath 走同步渲染，故用 DOM 节点 identity 判定是否 remount。
// 若 kvmath 退回成 components useMemo 里的内联函数，此测试会失败（节点被替换）。
describe('ChatMarkdown 公式稳定性', () => {
  it('artifacts 换引用重渲时，公式节点不被 remount', () => {
    const { container, rerender } = render(
      <ChatMarkdown content={'目标函数 $Z_1$ 最小化'} artifacts={[]} />,
    )
    const before = container.querySelector('.katex-lazy')
    expect(before).not.toBeNull()

    // 模拟切模型/思考等级时上层重渲传入的新 artifacts 引用（内容不变）。
    rerender(<ChatMarkdown content={'目标函数 $Z_1$ 最小化'} artifacts={[]} />)
    const after = container.querySelector('.katex-lazy')

    expect(after).toBe(before) // 同一个 DOM 节点 = 未 remount
  })

  it('把 KaTeX HTML 隔离在 Shadow DOM 中，普通 DOM 只保留轻量 host', () => {
    const { container } = render(
      <ChatMarkdown content={'目标函数 $Z_1$ 最小化'} artifacts={[]} />,
    )

    const host = container.querySelector<HTMLElement>('[data-katex-shadow-host="true"]')
    expect(host).not.toBeNull()
    expect(host).toHaveClass('katex-lazy')
    expect(container.querySelector('.chat-markdown .katex')).toBeNull()
    expect(host?.shadowRoot?.querySelector('.katex')).not.toBeNull()
    expect(host?.shadowRoot?.querySelector('[data-katex-shadow-content="true"]')).not.toBeNull()
    expect(
      host?.shadowRoot?.querySelector('style[data-katex-shadow-style="true"]'),
    ).not.toBeNull()
  })
})
