import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { api, type ProviderOAuthUsage } from '../api/tauri'
import { ProviderUsageCard } from './ProviderUsageCard'
import { makeProvider } from './tabs/testFixtures'
vi.mock('../api/tauri', async () => {
  const actual = await vi.importActual<typeof import('../api/tauri')>('../api/tauri')
  return { ...actual, api: { ...actual.api, providerOAuthUsage: vi.fn() } }
})
const provider = { ...makeProvider(), request: { oauth: { provider: 'kimi' as const, credentialId: 'test-only' } } }
const usage: ProviderOAuthUsage = { plan: null, fetchedAt: 1700000000, windows: [{ label: 'Weekly', usedPercent: 75, used: 75, limit: 100, resetsAt: 1700600000 }] }
beforeEach(() => { vi.resetAllMocks(); vi.mocked(api.providerOAuthUsage).mockResolvedValue(usage) })
describe('ProviderUsageCard', () => {
  it('loads Antigravity quota groups and preserves zero remaining', async () => {
    vi.mocked(api.providerOAuthUsage).mockResolvedValue({ ...usage, windows: [{ ...usage.windows[0], label: 'Gemini · Weekly', usedPercent: 100, used: null, limit: null, resetsAt: null, resetHint: 'Resets every week' }] })
    render(<ProviderUsageCard provider={{ ...provider, request: { oauth: { provider: 'antigravity', credentialId: 'agy' } } }} lang="zh" />)
    expect(await screen.findByText('剩余 0%')).toBeInTheDocument()
    expect(screen.getByText('Resets every week')).toBeInTheDocument()
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '0')
    expect(api.providerOAuthUsage).toHaveBeenCalledTimes(1)
  })

  it('loads account limits and shows remaining rather than used percent', async () => {
    render(<ProviderUsageCard provider={provider} lang="zh" />)
    expect(await screen.findByText('剩余 25%')).toBeInTheDocument()
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '25')
    fireEvent.click(screen.getByRole('button', { name: '刷新' }))
    await waitFor(() => expect(api.providerOAuthUsage).toHaveBeenCalledTimes(2))
  })
  it('shows unavailable data and errors without a misleading full bar', async () => {
    vi.mocked(api.providerOAuthUsage).mockResolvedValue({ ...usage, windows: [{ ...usage.windows[0], usedPercent: null, used: null }] })
    render(<ProviderUsageCard provider={provider} lang="zh" />)
    expect(await screen.findByText('暂无比例')).toBeInTheDocument()
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument()
    vi.mocked(api.providerOAuthUsage).mockRejectedValue(new Error('network'))
    fireEvent.click(screen.getByRole('button', { name: '刷新' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('暂时无法获取')
  })
  it('never fetches unsupported or disconnected accounts and ignores stale responses', async () => {
    let finish!: (result: ProviderOAuthUsage) => void
    vi.mocked(api.providerOAuthUsage).mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const view = render(<ProviderUsageCard provider={provider} lang="zh" />)
    view.rerender(<ProviderUsageCard provider={{ ...provider, request: { oauth: { provider: 'codex' } } }} lang="zh" />)
    await act(async () => finish(usage))
    expect(screen.queryByRole('region')).not.toBeInTheDocument()
    expect(api.providerOAuthUsage).toHaveBeenCalledTimes(1)
    view.rerender(<ProviderUsageCard provider={{ ...provider, request: { oauth: { provider: 'antigravity' } } }} lang="zh" />)
    expect(api.providerOAuthUsage).toHaveBeenCalledTimes(1)
  })
})
