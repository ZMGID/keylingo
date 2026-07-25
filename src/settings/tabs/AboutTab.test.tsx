import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { AppInfoGroup, UpdateGroup, type UpdateFlowState } from './AboutTab'
import { makeSettings } from './testFixtures'
import { i18n } from '../i18n'

const t = i18n.zh

vi.mock('../../chat/ChatMarkdown', () => ({
  ChatMarkdown: ({ content }: { content: string }) => <div data-testid="md">{content}</div>,
}))

/**
 * 回归重点：更新流程是个状态机（idle→checking→available→downloading→downloaded/failed），
 * 拆分时把 6 个分量收进了 UpdateFlowState 对象，每个状态该显示什么按钮必须逐个验。
 */
const idle: UpdateFlowState = {
  status: 'idle',
  info: null,
  downloadState: 'idle',
  downloadPercent: 0,
  downloadError: '',
}

function renderUpdate(update: Partial<UpdateFlowState> = {}) {
  const props = {
    settings: makeSettings(),
    t,
    update: { ...idle, ...update },
    onUpdateSettings: vi.fn(),
    onCheck: vi.fn(),
    onDownloadAndInstall: vi.fn(),
    onInstall: vi.fn(),
    onOpenReleasePage: vi.fn(),
    onOpenGithubReleases: vi.fn(),
    onDismiss: vi.fn(),
  }
  render(<UpdateGroup {...props} />)
  return props
}

describe('AppInfoGroup', () => {
  it('显示版本号', () => {
    render(<AppInfoGroup t={t} lang="zh" appVersion="2.8.3" />)
    expect(screen.getByText('v2.8.3')).toBeTruthy()
  })
})

describe('UpdateGroup', () => {
  it('idle 态只有检查按钮', () => {
    renderUpdate()
    expect(screen.getByRole('button', { name: t.checkUpdate })).toBeTruthy()
    expect(screen.queryByRole('button', { name: t.downloadAndInstall })).toBeNull()
  })

  it('checking 态禁用检查按钮并改文案', () => {
    renderUpdate({ status: 'checking' })
    expect(screen.getByRole('button', { name: t.checkingUpdate })).toBeDisabled()
  })

  it('up-to-date 态显示已是最新说明', () => {
    renderUpdate({ status: 'up-to-date' })
    expect(screen.getByText(t.upToDate)).toBeTruthy()
  })

  it('check-failed 态给出 GitHub 兜底入口', async () => {
    const props = renderUpdate({ status: 'check-failed' })
    expect(screen.getByText(t.updateCheckFailed)).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: t.downloadFromGithub }))
    expect(props.onOpenGithubReleases).toHaveBeenCalled()
  })

  it('available 态显示版本与更新说明', () => {
    renderUpdate({
      status: 'available',
      info: { version: '2.9.0', body: '新增功能 X' } as never,
    })
    expect(screen.getByText('v2.9.0')).toBeTruthy()
    expect(screen.getByTestId('md').textContent).toBe('新增功能 X')
  })

  it('available + downloadState=idle 显示下载与 GitHub 两个按钮', () => {
    renderUpdate({ status: 'available', info: { version: '2.9.0' } as never })
    expect(screen.getByRole('button', { name: t.downloadAndInstall })).toBeTruthy()
    expect(screen.getByRole('button', { name: t.downloadFromGithub })).toBeTruthy()
  })

  it('downloading 态显示进度条与百分比，按钮禁用', () => {
    renderUpdate({
      status: 'available',
      info: { version: '2.9.0' } as never,
      downloadState: 'downloading',
      downloadPercent: 42,
    })
    expect(screen.getByText('42%')).toBeTruthy()
    expect(screen.getByRole('button', { name: t.downloading })).toBeDisabled()
  })

  it('downloaded 态显示安装并重启', async () => {
    const props = renderUpdate({
      status: 'available',
      info: { version: '2.9.0' } as never,
      downloadState: 'downloaded',
    })
    await userEvent.click(screen.getByRole('button', { name: t.installAndRestart }))
    expect(props.onInstall).toHaveBeenCalled()
    expect(props.onDownloadAndInstall).not.toHaveBeenCalled()
  })

  it('failed 态显示错误与重试', async () => {
    const props = renderUpdate({
      status: 'available',
      info: { version: '2.9.0' } as never,
      downloadState: 'failed',
      downloadError: '网络中断',
    })
    expect(screen.getByText(/网络中断/)).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: t.retryDownload }))
    expect(props.onDownloadAndInstall).toHaveBeenCalled()
  })

  it('稍后更新走 onDismiss（一次重置四个状态由 shell 负责）', async () => {
    const props = renderUpdate({ status: 'available', info: { version: '2.9.0' } as never })
    await userEvent.click(screen.getByRole('button', { name: t.updateLater }))
    expect(props.onDismiss).toHaveBeenCalled()
  })

  it('自动检查开关写 autoCheckUpdate', async () => {
    const props = renderUpdate()
    await userEvent.click(screen.getByRole('switch'))
    expect(props.onUpdateSettings).toHaveBeenCalledWith({ autoCheckUpdate: false })
  })
})
