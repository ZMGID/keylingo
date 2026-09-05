import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import { api, type ProviderOAuthAccount } from '../api/tauri'
import { ProviderAccountIdentity } from './ProviderAccountIdentity'
import { makeProvider } from './tabs/testFixtures'

vi.mock('../api/tauri', async () => {
  const actual = await vi.importActual<typeof import('../api/tauri')>('../api/tauri')
  return { ...actual, api: { ...actual.api, providerOAuthAccount: vi.fn() } }
})
const provider = { ...makeProvider(), request: { oauth: { provider: 'antigravity' as const, credentialId: 'one' } } }
beforeEach(() => vi.resetAllMocks())

it('shows email, name and the provider account ID', async () => {
  vi.mocked(api.providerOAuthAccount).mockResolvedValue({ email: 'person@example.com', name: 'Person', accountId: 'google-id' })
  render(<ProviderAccountIdentity provider={provider} lang="zh" />)
  expect(await screen.findByText('person@example.com')).toBeInTheDocument()
  expect(screen.getByText('Person')).toBeInTheDocument()
  expect(screen.getByText('账号 ID：google-id')).toBeInTheDocument()
})

it('ignores an old account response after switching credentials', async () => {
  let finish!: (value: ProviderOAuthAccount) => void
  vi.mocked(api.providerOAuthAccount).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    .mockResolvedValueOnce({ email: null, name: null, accountId: 'new-account' })
  const view = render(<ProviderAccountIdentity provider={provider} lang="zh" />)
  view.rerender(<ProviderAccountIdentity provider={{ ...provider, request: { oauth: { provider: 'antigravity', credentialId: 'two' } } }} lang="zh" />)
  expect(await screen.findByText('账号 ID：new-account')).toBeInTheDocument()
  await act(async () => finish({ email: 'old@example.com', name: null, accountId: 'old-account' }))
  expect(screen.queryByText('old@example.com')).not.toBeInTheDocument()
  expect(screen.getByText('账号 ID：new-account')).toBeInTheDocument()
})

it('allows retry without displaying backend errors or inventing an identity', async () => {
  vi.mocked(api.providerOAuthAccount).mockRejectedValueOnce(new Error('private backend detail'))
    .mockResolvedValueOnce({ email: null, name: null, accountId: null })
  render(<ProviderAccountIdentity provider={provider} lang="zh" />)
  expect(await screen.findByText('账号信息暂时无法读取')).toBeInTheDocument()
  expect(screen.queryByText('private backend detail')).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: '重试' }))
  expect(await screen.findByText('供应商未返回账号标识')).toBeInTheDocument()
})
