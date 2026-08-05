import { useState } from 'react'
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ProviderRequestPanel } from './ProviderRequestPanel'
import { makeProvider } from './tabs/testFixtures'
import { i18n } from './i18n'

const t = i18n.zh

function renderPanel(overrides: Partial<Parameters<typeof makeProvider>[0]> = {}) {
  const onUpdateProvider = vi.fn()
  const provider = makeProvider({ apiFormat: 'openai_chat', ...overrides })
  render(
    <ProviderRequestPanel
      provider={provider}
      t={t}
      lang="zh"
      gzipInfoOpen={new Set<string>()}
      onToggleGzipInfo={vi.fn()}
      onUpdateProvider={onUpdateProvider}
    />,
  )
  return { onUpdateProvider, provider }
}

describe('ProviderRequestPanel', () => {
  it('renders every section of the page', () => {
    renderPanel()
    for (const label of [
      '压缩请求体 (gzip)',
      t.useSystemProxy,
      t.promptCaching,
      t.cliIdentity,
      t.customHeaders,
    ]) {
      expect(screen.getByText(label)).toBeTruthy()
    }
  })

  it('offers prompt caching on OpenAI protocols too, with protocol-specific wording', () => {
    // OpenAI 侧的「缓存」= 发 prompt_cache_key，不是 Anthropic 的 cache_control 断点，
    // 两边说明文案不能混用。
    renderPanel({ apiFormat: 'openai_chat' })
    expect(screen.getByText(t.promptCachingHintOpenAI)).toBeTruthy()
    expect((screen.getByRole('switch', { name: t.promptCaching }) as HTMLButtonElement).disabled)
      .toBe(false)

    cleanup()
    renderPanel({ apiFormat: 'anthropic_messages' })
    expect(screen.getByText(t.promptCachingHintAnthropic)).toBeTruthy()
  })

  it('greys out prompt caching on the protocols with no cache field to send', () => {
    // 藏起来会让人以为这个功能没做 —— 之前就踩过。
    // Gemini 是服务端隐式缓存，xAI 直接拒收 prompt_cache_key。
    for (const apiFormat of ['gemini', 'xai_responses']) {
      cleanup()
      renderPanel({ apiFormat })
      expect(screen.getByText(t.promptCachingUnsupported), apiFormat).toBeTruthy()
      const toggle = screen.getByRole('switch', { name: t.promptCaching })
      expect((toggle as HTMLButtonElement).disabled, apiFormat).toBe(true)
    }
  })

  it('prompt caching defaults per protocol: OpenAI on, Anthropic off', async () => {
    // Anthropic 默认关：断点是净新增的线格式变化（system 字符串→块数组），而这条路
    // 没有 prompt_cache_key 那种「被拒就学会并重试」的自愈，不能悄悄替用户开。
    renderPanel({ apiFormat: 'anthropic_messages' })
    const anthropic = screen.getByRole('switch', { name: t.promptCaching })
    expect(anthropic.getAttribute('aria-checked')).toBe('false')

    cleanup()
    // OpenAI 默认开：prompt_cache_key 本来就一直在发，默认关反而是静默削弱现状。
    const { onUpdateProvider, provider } = renderPanel({ apiFormat: 'openai_chat' })
    const openai = screen.getByRole('switch', { name: t.promptCaching })
    expect(openai.getAttribute('aria-checked')).toBe('true')
    await userEvent.click(openai)
    expect(onUpdateProvider).toHaveBeenCalledWith(
      provider.id,
      expect.objectContaining({ request: expect.objectContaining({ promptCaching: false }) }),
    )
  })

  it('does not flag a freshly added empty row until it is edited', async () => {
    // 面板是受控组件，得让父层真的把新行回灌回来，否则点「添加」什么都不会发生。
    function Host() {
      const [provider, setProvider] = useState(makeProvider({ apiFormat: 'openai_chat' }))
      return (
        <ProviderRequestPanel
          provider={provider}
          t={t}
          lang="zh"
          gzipInfoOpen={new Set<string>()}
          onToggleGzipInfo={vi.fn()}
          onUpdateProvider={(_id, updates) => setProvider((p) => ({ ...p, ...updates }))}
        />
      )
    }
    render(<Host />)

    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.addCustomHeader) }))
    expect(screen.getByPlaceholderText(t.customHeaderKeyPlaceholder)).toBeTruthy()
    expect(screen.queryByText(t.headerIssueInvalidKey)).toBeNull()

    await userEvent.type(screen.getByPlaceholderText(t.customHeaderKeyPlaceholder), 'bad name')
    expect(screen.getByText(t.headerIssueInvalidKey)).toBeTruthy()
  })

  it('a new empty row stays quiet after deleting the saved ones', async () => {
    // 行的「动过没有」曾经用 index 记账：删掉两条存盘行再点添加，新行落到 index 0，
    // 被当成「存盘行」立刻飘红——正是那段注释说要避免的。换成稳定 uid 后不该再发生。
    function Host() {
      const [provider, setProvider] = useState(
        makeProvider({
          apiFormat: 'openai_chat',
          request: {
            customHeaders: [
              { key: 'X-A', value: '1' },
              { key: 'X-B', value: '2' },
            ],
          },
        }),
      )
      return (
        <ProviderRequestPanel
          provider={provider}
          t={t}
          lang="zh"
          gzipInfoOpen={new Set<string>()}
          onToggleGzipInfo={vi.fn()}
          onUpdateProvider={(_id, updates) => setProvider((p) => ({ ...p, ...updates }))}
        />
      )
    }
    render(<Host />)

    const removes = screen.getAllByRole('button', { name: t.removeCustomHeader })
    await userEvent.click(removes[0])
    await userEvent.click(screen.getByRole('button', { name: t.removeCustomHeader }))
    await userEvent.click(screen.getByRole('button', { name: new RegExp(t.addCustomHeader) }))

    expect(screen.getByPlaceholderText(t.customHeaderKeyPlaceholder)).toBeTruthy()
    expect(screen.queryByText(t.headerIssueInvalidKey)).toBeNull()
  })

  it('flags a reserved header inline', () => {
    renderPanel({ request: { customHeaders: [{ key: 'Authorization', value: 'Bearer x' }] } })
    expect(screen.getByText(t.headerIssueReserved)).toBeTruthy()
  })
})
