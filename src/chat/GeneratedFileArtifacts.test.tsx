import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ChatToolArtifact } from './types'

const invoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}))

import { GeneratedFileArtifacts } from './GeneratedFileArtifacts'

const pdf: ChatToolArtifact = {
  id: 'art_pdf',
  name: '简历.pdf',
  mime_type: 'application/pdf',
  data_url: 'data:application/pdf;base64,JVBERi0xLjc=',
  size_bytes: 204_900,
  path: '/tmp/简历.pdf',
}

const markdown: ChatToolArtifact = {
  id: 'art_md',
  name: 'english_essay.md',
  mime_type: 'text/markdown',
  data_url: 'data:text/markdown;base64,IyBUaGUgVmFsdWUgb2YgUmVhZGluZw==',
  size_bytes: 1200,
  path: '/tmp/english_essay.md',
}

const image: ChatToolArtifact = {
  id: 'art_img',
  name: '王积萌.jpg',
  mime_type: 'image/jpeg',
  data_url: 'data:image/jpeg;base64,/9j/4AAQ',
  path: '/tmp/wang.jpg',
}

describe('GeneratedFileArtifacts compact chips', () => {
  beforeEach(() => invoke.mockReset())

  it('lays out multiple files as compact wrapping chips, without body preview', () => {
    const { container } = render(
      <GeneratedFileArtifacts artifacts={[pdf, markdown, image]} />,
    )

    const row = container.querySelector('.flex-wrap')
    expect(row).not.toBeNull()
    expect(row?.className).toContain('flex-wrap')

    const pdfButton = screen.getByRole('button', { name: '打开文件 简历.pdf' })
    const mdButton = screen.getByRole('button', { name: '打开文件 english_essay.md' })
    expect(pdfButton.className).toContain('h-16')
    expect(pdfButton.className).toContain('w-[9.5rem]')
    expect(mdButton.className).toContain('h-16')
    expect(container.textContent).toContain('PDF')
    expect(container.textContent).toContain('MD')
    expect(container.textContent).not.toContain('%PDF')
    expect(container.textContent).not.toContain('The Value of Reading')
    expect(screen.queryByRole('button', { name: /王积萌/ })).not.toBeInTheDocument()
  })

  it('opens a file with a path via the generated-artifact command', () => {
    render(<GeneratedFileArtifacts artifacts={[pdf]} />)
    fireEvent.click(screen.getByRole('button', { name: '打开文件 简历.pdf' }))
    expect(invoke).toHaveBeenCalledWith('chat_open_generated_artifact', { path: '/tmp/简历.pdf' })
  })
})
