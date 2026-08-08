import { useState } from 'react'
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ProviderRequestPanel } from './ProviderRequestPanel'
import { makeProvider } from './tabs/testFixtures'
import { i18n } from './i18n'
import { resolvePromptCacheRetention } from '../api/tauri'

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

describe('resolvePromptCacheRetention', () => {
  it('defaults to short and migrates legacy bool only when retention invalid', () => {
    expect(resolvePromptCacheRetention(undefined)).toBe('short')
    expect(resolvePromptCacheRetention({})).toBe('short')
    expect(resolvePromptCacheRetention({ promptCaching: false })).toBe('none')
    expect(resolvePromptCacheRetention({ promptCaching: true })).toBe('short')
    expect(resolvePromptCacheRetention({ promptCacheRetention: 'long' })).toBe('long')
    expect(resolvePromptCacheRetention({ promptCacheRetention: 'none' })).toBe('none')
    // 合法 retention 优先于遗留 bool（与 Rust sanitize 同序）
    expect(
      resolvePromptCacheRetention({ promptCaching: false, promptCacheRetention: 'long' }),
    ).toBe('long')
    expect(
      resolvePromptCacheRetention({ promptCaching: true, promptCacheRetention: 'none' }),
    ).toBe('none')
    expect(
      resolvePromptCacheRetention({ promptCaching: false, promptCacheRetention: '???' }),
    ).toBe('none')
  })
})

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

  it('shows default retention for OpenAI and Anthropic with protocol hints', () => {
    renderPanel({ apiFormat: 'openai_chat' })
    expect(screen.getByText(t.promptCachingHintOpenAI)).toBeTruthy()
    expect(screen.getByRole('button', { name: t.promptCacheShort })).toBeTruthy()

    cleanup()
    renderPanel({ apiFormat: 'anthropic_messages' })
    expect(screen.getByText(t.promptCachingHintAnthropic)).toBeTruthy()
    expect(screen.getByRole('button', { name: t.promptCacheShort })).toBeTruthy()
  })

  it('disables retention control on protocols with no cache field', () => {
    for (const apiFormat of ['gemini', 'xai_responses']) {
      cleanup()
      renderPanel({ apiFormat })
      expect(screen.getByText(t.promptCachingUnsupported), apiFormat).toBeTruthy()
      // 缓存策略选择器在「Prompt 缓存」标签附近；禁用时显示「关闭」
      const triggers = screen.getAllByRole('button', { name: t.promptCacheNone })
      expect(triggers.some((el) => (el as HTMLButtonElement).disabled), apiFormat).toBe(true)
    }
  })

  it('can switch retention to none', async () => {
    const user = userEvent.setup()
    const { onUpdateProvider, provider } = renderPanel({ apiFormat: 'openai_chat' })
    await user.click(screen.getByRole('button', { name: t.promptCacheShort }))
    // listbox 选项里的「关闭」
    const options = await screen.findAllByRole('option')
    const noneOpt = options.find((el) => el.textContent === t.promptCacheNone)
    expect(noneOpt).toBeTruthy()
    await user.click(noneOpt!)
    expect(onUpdateProvider).toHaveBeenCalledWith(
      provider.id,
      expect.objectContaining({
        request: expect.objectContaining({ promptCacheRetention: 'none' }),
      }),
    )
  })

  it('does not flag a freshly added empty row until it is edited', async () => {
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
    await userEvent.click(screen.getByRole('button', { name: t.addCustomHeader }))
    expect(screen.queryByText(t.headerIssueInvalidKey)).toBeNull()
  })
})
