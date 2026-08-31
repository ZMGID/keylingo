import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { api } from '../api/tauri'
import { ProviderModelTestModal } from './ProviderModelTestModal'
import { MODEL_TEST_CONCURRENCY, runPool } from './providerModelTestPool'

vi.mock('../api/tauri', () => ({
  api: {
    testProviderConnection: vi.fn(),
  },
}))

vi.mock('../chat/ModelIcon', () => ({
  ModelIcon: () => null,
}))

describe('runPool', () => {
  it('caps concurrent workers', async () => {
    let inflight = 0
    let peak = 0
    await runPool(Array.from({ length: 15 }, (_, i) => i), MODEL_TEST_CONCURRENCY, async () => {
      inflight += 1
      peak = Math.max(peak, inflight)
      await new Promise((r) => setTimeout(r, 15))
      inflight -= 1
    })
    expect(peak).toBe(MODEL_TEST_CONCURRENCY)
  })
})

describe('ProviderModelTestModal', () => {
  it('does not fire every selected model at once', async () => {
    const user = userEvent.setup()
    let inflight = 0
    let peak = 0
    vi.mocked(api.testProviderConnection).mockImplementation(async () => {
      inflight += 1
      peak = Math.max(peak, inflight)
      await new Promise((r) => setTimeout(r, 20))
      inflight -= 1
      return { success: true }
    })

    const models = Array.from({ length: 14 }, (_, i) => `m${i}`)
    render(
      <ProviderModelTestModal
        providerId="p1"
        baseUrl="https://example.com/v1"
        apiKeys={['sk-test']}
        apiFormat="openai_chat"
        models={models}
        lang="zh"
        onClose={() => {}}
      />,
    )

    await user.click(screen.getByRole('button', { name: /开始测试/ }))
    await waitFor(() => {
      expect(api.testProviderConnection).toHaveBeenCalledTimes(14)
    })
    expect(peak).toBeLessThanOrEqual(MODEL_TEST_CONCURRENCY)
    expect(peak).toBeGreaterThan(1)
  })

  it('keeps a status slot on every row before results arrive', () => {
    const models = ['alpha', 'beta']
    render(
      <ProviderModelTestModal
        providerId="p1"
        baseUrl="https://example.com/v1"
        apiKeys={['sk-test']}
        apiFormat="openai_chat"
        models={models}
        lang="zh"
        onClose={() => {}}
      />,
    )
    const rows = document.querySelectorAll('.kv-mtest-row')
    expect(rows).toHaveLength(2)
    for (const row of rows) {
      expect(row.querySelector('.kv-mtest-status')).not.toBeNull()
    }
  })
})
