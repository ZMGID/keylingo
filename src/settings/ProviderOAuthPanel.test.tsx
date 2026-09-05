import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api, providerHasCredentials, type ProviderOAuthLogin, type ProviderOAuthPoll } from '../api/tauri'
import { ProviderOAuthPanel } from './ProviderOAuthPanel'
import { makeProvider } from './tabs/testFixtures'

vi.mock('../api/tauri', async () => {
  const actual = await vi.importActual<typeof import('../api/tauri')>('../api/tauri')
  return { ...actual, api: {
    providerOAuthStart: vi.fn(), providerOAuthPoll: vi.fn(), providerOAuthCancel: vi.fn(),
    providerOAuthDisconnect: vi.fn(), openExternal: vi.fn(),
    providerOAuthAccount: vi.fn(async () => ({ email: null, name: null, accountId: "test-account" })),
  } }
})
const login: ProviderOAuthLogin = { loginId: 'login-1', userCode: 'ABCD-1234', verificationUrl: 'https://auth.openai.com/codex/device', interval: 3, expiresAt: 1900000000 }
function setup() {
  const provider = { ...makeProvider(), request: { oauth: { provider: 'codex' as const } } }
  const update = vi.fn()
  return { ...render(<ProviderOAuthPanel provider={provider} lang="zh" onUpdateProvider={update} />), update, provider }
}
async function start() {
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: '登录授权' })) })
}
describe('model OAuth onboarding', () => {
  it('uses the browser callback flow for Antigravity without showing a device code', async () => {
    const provider = { ...makeProvider(), request: { oauth: { provider: 'antigravity' as const } } }
    const browserLogin = { ...login, userCode: '', verificationUrl: 'https://accounts.google.com/o/oauth2/v2/auth' }
    vi.mocked(api.providerOAuthStart).mockResolvedValue(browserLogin)
    render(<ProviderOAuthPanel provider={provider} lang="zh" onUpdateProvider={vi.fn()} />)
    await start()
    expect(api.providerOAuthStart).toHaveBeenCalledWith('antigravity', true)
    expect(screen.getByText(/请在浏览器中完成 Google 登录/)).toBeInTheDocument()
    expect(screen.queryByText(/输入设备码/)).toBeNull()
    expect(api.openExternal).toHaveBeenCalledWith(browserLogin.verificationUrl)
    fireEvent.click(screen.getByRole('button', { name: '取消' }))
    expect(api.providerOAuthCancel).toHaveBeenCalledWith(login.loginId)
  })
  beforeEach(() => {
    vi.useFakeTimers(); vi.resetAllMocks()
    vi.mocked(api.providerOAuthStart).mockResolvedValue(login)
    vi.mocked(api.openExternal).mockResolvedValue(undefined)
    vi.mocked(api.providerOAuthCancel).mockResolvedValue(undefined)
    vi.mocked(api.providerOAuthDisconnect).mockResolvedValue(undefined)
  })
  afterEach(() => vi.useRealTimers())
  it('shows the device code and opens the provider authorization page', async () => {
    setup(); await start()
    expect(screen.getByText('ABCD-1234')).toBeTruthy()
    expect(api.openExternal).toHaveBeenCalledWith(login.verificationUrl)
    expect(api.providerOAuthStart).toHaveBeenCalledWith('codex', true)
  })
  it('saves only the credential reference after authorization', async () => {
    const { update, provider } = setup()
    vi.mocked(api.providerOAuthPoll).mockResolvedValue({ status: 'authorized', interval: 0, auth: { provider: 'codex', credentialId: 'credential-1' } })
    await start(); await act(async () => { await vi.advanceTimersByTimeAsync(3000) })
    expect(update).toHaveBeenCalledWith(provider.id, { request: { oauth: { provider: 'codex', credentialId: 'credential-1' } } })
    expect(screen.queryByText('ABCD-1234')).toBeNull()
    expect(JSON.stringify(update.mock.calls)).not.toContain('access_token')
  })
  it('honors a longer polling interval from the provider', async () => {
    setup(); vi.mocked(api.providerOAuthPoll).mockResolvedValue({ status: 'pending', interval: 10, auth: null })
    await start(); await act(async () => { await vi.advanceTimersByTimeAsync(3000) })
    await act(async () => { await vi.advanceTimersByTimeAsync(9000) })
    expect(api.providerOAuthPoll).toHaveBeenCalledTimes(1)
    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(api.providerOAuthPoll).toHaveBeenCalledTimes(2)
  })
  it('cancels a start response that arrives after the user cancels', async () => {
    let resolve!: (value: ProviderOAuthLogin) => void
    vi.mocked(api.providerOAuthStart).mockImplementation(() => new Promise(r => { resolve = r }))
    setup(); await start()
    fireEvent.click(screen.getByRole('button', { name: '取消' }))
    await act(async () => { resolve(login) })
    expect(api.providerOAuthCancel).toHaveBeenCalledWith(login.loginId)
    expect(api.openExternal).not.toHaveBeenCalled()
  })
  it('removes a credential if authorization completes after unmount', async () => {
    let resolve!: (value: ProviderOAuthPoll) => void
    vi.mocked(api.providerOAuthPoll).mockImplementation(() => new Promise(r => { resolve = r }))
    const { unmount, update } = setup(); await start()
    await act(async () => { await vi.advanceTimersByTimeAsync(3000) })
    unmount()
    await act(async () => { resolve({ status: 'authorized', interval: 0, auth: { provider: 'codex', credentialId: 'late' } }) })
    expect(api.providerOAuthDisconnect).toHaveBeenCalledWith('late')
    expect(update).not.toHaveBeenCalled()
  })
  it('shows authorization failures and permits another attempt', async () => {
    setup(); vi.mocked(api.providerOAuthPoll).mockRejectedValue(new Error('Login expired'))
    await start(); await act(async () => { await vi.advanceTimersByTimeAsync(3000) })
    expect(screen.getByRole('alert').textContent).toContain('Login expired')
    expect(screen.getByRole('button', { name: '登录授权' })).not.toBeDisabled()
  })
  it('requires an OAuth reference even when old API keys remain', () => {
    const { provider } = setup()
    expect(providerHasCredentials({ ...provider, apiKeys: ['old-key'] })).toBe(false)
    expect(providerHasCredentials({ ...provider, apiKeys: [], request: { oauth: { provider: 'codex', credentialId: 'id' } } })).toBe(true)
  })
})
