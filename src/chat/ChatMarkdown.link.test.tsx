import { render } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const openExternal = vi.fn(() => Promise.resolve())
const openLocalFile = vi.fn(() => Promise.resolve())
vi.mock('../api/tauri', () => ({
  api: {
    get openExternal() {
      return openExternal
    },
    get openLocalFile() {
      return openLocalFile
    },
  },
  isTauriRuntime: () => true,
}))

import { ChatMarkdown } from './ChatMarkdown'

/**
 * 链接点击的**底线**：除了系统 scheme（mailto:/tel:/sms:），默认导航一律不许发生。
 * <a> 的默认行为会把 Tauri webview 自己导航走，整个聊天 UI 连同未落盘的会话状态一起消失
 * —— 实测点一条 CLI 生成的 `file://…/index.html` 就把窗口点白了。
 */
function clickLink(content: string) {
  const { container } = render(<ChatMarkdown content={content} artifacts={[]} />)
  const link = container.querySelector('a')!
  const event = new MouseEvent('click', { bubbles: true, cancelable: true })
  link.dispatchEvent(event)
  return event
}

describe('ChatMarkdown 链接点击', () => {
  beforeEach(() => {
    openExternal.mockClear()
    openLocalFile.mockClear()
  })

  it('本地文件链接交给 openLocalFile，且不导航 webview', () => {
    // 附带钉住 urlTransform：react-markdown 默认把 `file:` 剥成空 href（死链）。
    const event = clickLink('[看板](file:///tmp/board/index.html)')
    expect(event.defaultPrevented).toBe(true)
    expect(openLocalFile).toHaveBeenCalledWith('file:///tmp/board/index.html', null)
    expect(openExternal).not.toHaveBeenCalled()
  })

  it('裸绝对路径同样不导航（这条才是把窗口点白的那种链接）', () => {
    const event = clickLink('[看板](/tmp/board/index.html)')
    expect(event.defaultPrevented).toBe(true)
    expect(openLocalFile).toHaveBeenCalledWith('/tmp/board/index.html', null)
  })

  it('相对路径也交给后端（按会话工作目录解析），绝不放行默认导航', () => {
    const event = clickLink('[看板](assets/index.html)')
    expect(event.defaultPrevented).toBe(true)
    expect(openLocalFile).toHaveBeenCalledWith('assets/index.html', null)
    expect(openExternal).not.toHaveBeenCalled()
  })

  it('不止 html：docx / zip 这类同样走默认程序', () => {
    clickLink('[报告](/tmp/out/report.docx)')
    expect(openLocalFile).toHaveBeenCalledWith('/tmp/out/report.docx', null)
  })

  it('http(s) 走系统浏览器', () => {
    const event = clickLink('[官网](https://example.com/a)')
    expect(event.defaultPrevented).toBe(true)
    expect(openExternal).toHaveBeenCalledWith('https://example.com/a')
  })

  it('mailto: 保留默认行为交给系统', () => {
    const event = clickLink('[联系](mailto:a@b.com)')
    expect(event.defaultPrevented).toBe(false)
    expect(openExternal).not.toHaveBeenCalled()
    expect(openLocalFile).not.toHaveBeenCalled()
  })
})
