import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { ComposerAddMenu } from './ComposerAddMenu'

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

describe('ComposerAddMenu', () => {
  it('opens a plus menu with attachment and additional directories', () => {
    const onAddAttachment = vi.fn()
    const onChange = vi.fn()
    render(
      <ComposerAddMenu
        onAddAttachment={onAddAttachment}
        directories={[{ path: '/repos/biz', name: 'Biz' }]}
        onChangeAdditionalDirectories={onChange}
        primaryRootPath="/repos/alpha"
      />,
    )

    fireEvent.click(screen.getByLabelText('添加'))
    expect(screen.getByText('添加附件')).toBeTruthy()
    expect(screen.getByText('添加文件夹')).toBeTruthy()
    expect(screen.getByText('Biz')).toBeTruthy()
    expect(screen.queryByText('从其他项目添加')).toBeNull()

    fireEvent.click(screen.getByText('添加附件'))
    expect(onAddAttachment).toHaveBeenCalledOnce()
  })

  it('hides directory actions when the conversation cannot attach folders', () => {
    render(<ComposerAddMenu onAddAttachment={() => undefined} />)

    fireEvent.click(screen.getByLabelText('添加'))
    expect(screen.getByText('添加附件')).toBeTruthy()
    expect(screen.queryByText('添加文件夹')).toBeNull()
  })

  it('nests sources under the plus menu', () => {
    render(
      <ComposerAddMenu
        onAddAttachment={() => undefined}
        sourcesPanel={<div>sources-body</div>}
      />,
    )

    fireEvent.click(screen.getByLabelText('添加'))
    expect(screen.getByText('添加附件')).toBeTruthy()
    expect(screen.getByText('信息来源')).toBeTruthy()
    expect(screen.queryByText('sources-body')).toBeNull()

    fireEvent.click(screen.getByText('信息来源'))
    expect(screen.getByText('sources-body')).toBeTruthy()
    expect(screen.queryByText('添加附件')).toBeNull()

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.getByText('添加附件')).toBeTruthy()
    expect(screen.queryByText('sources-body')).toBeNull()
  })
})
