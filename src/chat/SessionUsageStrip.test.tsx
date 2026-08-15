import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { SessionUsageStrip } from './SessionUsageStrip'
import type { ChatMessage, MessageUsage } from './types'

let seq = 0

function assistant(usage: MessageUsage, providerId?: string): ChatMessage {
  seq += 1
  return {
    id: `m-${seq}`,
    role: 'assistant',
    content: 'x',
    timestamp: 1,
    usage,
    provider_id: providerId,
  }
}

describe('SessionUsageStrip', () => {
  it('shows input/output with arrows and cache hit as a percentage (Anthropic style: input excludes cache)', () => {
    render(
      <SessionUsageStrip
        lang="zh"
        apiFormats={{ anthropic: 'anthropic_messages' }}
        defaultApiFormat="anthropic_messages"
        messages={[
          assistant({ input_tokens: 1000, output_tokens: 200, cached_input_tokens: 800 }, 'anthropic'),
          assistant({ input_tokens: 500, output_tokens: 50, cached_input_tokens: 300 }, 'anthropic'),
        ]}
      />,
    )
    // 输入 = 1000+500（不减）；缓存命中率 = 1100 / (1500+1100) ≈ 42.3% → 取整 42%
    expect(screen.getByText('↑1.5K')).toBeTruthy()
    expect(screen.getByText('缓存 42%')).toBeTruthy()
    expect(screen.getByText('↓250')).toBeTruthy()
    // 悬浮提示带全量描述（在容器 span 上）
    expect(screen.getByText('↑1.5K').parentElement).toHaveAttribute(
      'title',
      '输入 1.5K · 缓存命中 42% · 输出 250',
    )
  })

  it('subtracts cached tokens from OpenAI-style input and rounds big percentages', () => {
    render(
      <SessionUsageStrip
        lang="zh"
        messages={[assistant({ input_tokens: 10_000, output_tokens: 300, cached_input_tokens: 4_000 })]}
      />,
    )
    expect(screen.getByText('↑6.0K')).toBeTruthy()
    expect(screen.getByText('缓存 40%')).toBeTruthy()
    expect(screen.getByText('↓300')).toBeTruthy()
  })

  it('resolves per-message provider format over the default', () => {
    render(
      <SessionUsageStrip
        lang="zh"
        defaultApiFormat="openai_chat"
        messages={[
          assistant({ input_tokens: 10_000, cached_input_tokens: 3_000 }, 'anthropic-p'),
          assistant({ input_tokens: 2_000, cached_input_tokens: 500 }),
        ]}
        apiFormats={{ 'anthropic-p': 'anthropic_messages' }}
      />,
    )
    // anthropic-p 不减（10000）；默认 openai_chat 的减（2000−500=1500）
    expect(screen.getByText('↑11.5K')).toBeTruthy()
    // 3500 / (11500 + 3500) = 23.3% → 取整 23%
    expect(screen.getByText('缓存 23%')).toBeTruthy()
  })

  it('does not subtract cache from dsh-style input (already exclusive)', () => {
    render(
      <SessionUsageStrip
        lang="zh"
        defaultApiFormat="openai_chat"
        cacheIncludedInInput={false}
        messages={[
          assistant({ input_tokens: 457, output_tokens: 1442, cached_input_tokens: 51_456 }),
          assistant({ input_tokens: 122, output_tokens: 600, cached_input_tokens: 54_528 }),
        ]}
      />,
    )
    // 再减一次会变成 ↑0 / 缓存 100%。新鲜输入 457+122=579。
    expect(screen.getByText('↑579')).toBeTruthy()
    expect(screen.getByText('缓存 99%')).toBeTruthy()
    expect(screen.getByText('↓2.0K')).toBeTruthy()
  })

  it('supports camelCase fields and hides the cache item when no cache was reported', () => {
    render(
      <SessionUsageStrip
        lang="zh"
        messages={[assistant({ inputTokens: 200, outputTokens: 40, cachedInputTokens: 0 })]}
      />,
    )
    expect(screen.getByText('↑200')).toBeTruthy()
    expect(screen.getByText('↓40')).toBeTruthy()
    expect(screen.queryByText(/缓存/)).toBeNull()
  })

  it('renders nothing when no message carries usage', () => {
    const { container } = render(
      <SessionUsageStrip
        lang="zh"
        messages={[
          assistant({}),
          { id: 'u-1', role: 'user', content: 'hi', timestamp: 1 },
        ]}
      />,
    )
    expect(container.firstChild).toBeNull()
  })

  it('renders nothing for an empty conversation', () => {
    const { container } = render(<SessionUsageStrip lang="zh" messages={[]} />)
    expect(container.firstChild).toBeNull()
  })
})
