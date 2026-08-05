import { describe, expect, it, vi } from 'vitest'
import { render, waitFor } from '@testing-library/react'

const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUg'

vi.mock('./attachmentPreview', () => ({
  loadAttachmentDataUrl: () => Promise.resolve(PNG),
  openAttachment: () => Promise.resolve(),
}))

import { ChatAttachments } from './ChatAttachments'

/**
 * 用户自己发的图（走 ChatAttachments）此前没有右键菜单，只有模型出图（ChatInlineImage）有。
 * 两条路现在共用 ChatImageContextMenu，这条钉住用户侧那条也接上了、且不冒泡给消息级菜单。
 */
describe('ChatAttachments 图片右键', () => {
  it('opens the image menu and does not bubble to the message menu', async () => {
    const onParentContextMenu = vi.fn()
    const { container } = render(
      <div onContextMenu={onParentContextMenu}>
        <ChatAttachments
          attachments={[
            { id: 'a1', name: 'shot.png', type: 'image', mimeType: 'image/png' } as never,
          ]}
          variant="user"
        />
      </div>,
    )

    const button = await waitFor(() => {
      const el = container.querySelector('button[aria-label="预览图片"]')
      if (!el) throw new Error('图片还没加载出来')
      return el
    })
    button.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))

    await waitFor(() => expect(document.body.textContent).toContain('复制图片'))
    expect(onParentContextMenu).not.toHaveBeenCalled()
  })
})
