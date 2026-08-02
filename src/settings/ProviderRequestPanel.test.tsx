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

  it('greys out prompt caching only on Gemini, where nothing can be sent', () => {
    // 藏起来会让人以为这个功能没做 —— 之前就踩过。
    renderPanel({ apiFormat: 'gemini' })
    expect(screen.getByText(t.promptCachingUnsupported)).toBeTruthy()
    const toggle = screen.getByRole('switch', { name: t.promptCaching })
    expect((toggle as HTMLButtonElement).disabled).toBe(true)
  })

  it('turning prompt caching off reports promptCaching: false', async () => {
    // 默认是开的，所以这里点一下应该是「关」。
    const { onUpdateProvider, provider } = renderPanel({ apiFormat: 'anthropic_messages' })
    const toggle = screen.getByRole('switch', { name: t.promptCaching })
    expect(toggle.getAttribute('aria-checked')).toBe('true')
    await userEvent.click(toggle)
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

  it('flags a reserved header inline', () => {
    renderPanel({ request: { customHeaders: [{ key: 'Authorization', value: 'Bearer x' }] } })
    expect(screen.getByText(t.headerIssueReserved)).toBeTruthy()
  })
})
