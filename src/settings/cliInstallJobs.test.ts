import { describe, expect, it, vi, beforeEach } from 'vitest'
import { onExternalCliInstallLog } from '../chat/api'
import {
  getCliInstallJob,
  resetCliInstallJobsForTests,
  startCliInstall,
} from './cliInstallJobs'

vi.mock('../chat/api', () => ({
  onExternalCliInstallLog: vi.fn().mockResolvedValue(() => {}),
}))

const mockOnInstallLog = vi.mocked(onExternalCliInstallLog)

describe('cliInstallJobs', () => {
  beforeEach(() => {
    resetCliInstallJobsForTests()
    mockOnInstallLog.mockReset()
    mockOnInstallLog.mockResolvedValue(() => {})
  })

  it('marks the job running before install() is awaited', async () => {
    let finish = () => {}
    const started = startCliInstall('claude', {
      install: () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    })
    expect(getCliInstallJob('claude').running).toBe(true)
    await Promise.resolve()
    finish()
    await started
    expect(getCliInstallJob('claude')).toEqual({
      running: false,
      log: [],
      result: 'ok',
    })
  })

  it('appends log lines from the install event stream', async () => {
    let emit: Parameters<typeof mockOnInstallLog>[0] | undefined
    mockOnInstallLog.mockImplementation(async (handler) => {
      emit = handler
      return () => {}
    })
    let finish = () => {}
    const started = startCliInstall('claude', {
      install: () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    })
    await Promise.resolve()
    emit?.({ agentId: 'claude', line: '$ npm', done: false, success: false })
    emit?.({ agentId: 'codex', line: 'ignore me', done: false, success: false })
    expect(getCliInstallJob('claude').log).toEqual(['$ npm'])
    finish()
    await started
  })

  it('does not start a second install while one is in flight', async () => {
    let finish = () => {}
    const install = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    )
    const first = startCliInstall('claude', { install })
    await Promise.resolve()
    expect(install).toHaveBeenCalledTimes(1)
    await startCliInstall('claude', { install })
    expect(install).toHaveBeenCalledTimes(1)
    finish()
    await first
  })
})
