import { describe, expect, it, vi } from 'vitest'
import { act, render } from '@testing-library/react'
import { ChatInlineImage } from './ChatInlineImage'

const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUg'

/** 触发 <img> 的 onLoad，并伪造自然尺寸（jsdom 不解码图片）。 */
function fireLoad(img: HTMLImageElement, width: number, height: number) {
  Object.defineProperty(img, 'naturalWidth', { value: width, configurable: true })
  Object.defineProperty(img, 'naturalHeight', { value: height, configurable: true })
  Object.defineProperty(img, 'currentSrc', { value: img.getAttribute('src'), configurable: true })
  act(() => {
    img.dispatchEvent(new Event('load'))
  })
}

describe('ChatInlineImage', () => {
  it('reserves an aspect-ratio box after load so the row stops resizing', () => {
    const { container } = render(<ChatInlineImage src={`${PNG}A`} alt="x" />)
    const button = container.querySelector('button')!
    // 解码前不占宽高比盒子（还不知道比例）。
    expect(button.style.aspectRatio).toBe('')

    fireLoad(container.querySelector('img')!, 1000, 500)
    expect(button.style.aspectRatio).toBe('2')
    // 高度封顶 420 ⇒ 宽度 = 2 * 420。
    expect(button.style.width).toBe('min(100%, 840px)')
  })

  it('remembers the ratio across unmount so scrolling back does not re-measure', () => {
    const src = `${PNG}B`
    const first = render(<ChatInlineImage src={src} alt="x" />)
    fireLoad(first.container.querySelector('img')!, 800, 400)
    first.unmount()

    // 虚拟列表滚回来 = 重新挂载：必须一挂载就带着盒子，而不是从 0 高长起。
    const second = render(<ChatInlineImage src={src} alt="x" />)
    expect(second.container.querySelector('button')!.style.aspectRatio).toBe('2')
  })

  it('does not lazy-load data URLs (bytes are already in memory)', () => {
    const inline = render(<ChatInlineImage src={`${PNG}C`} alt="x" />)
    expect(inline.container.querySelector('img')!.getAttribute('loading')).toBeNull()

    const remote = render(<ChatInlineImage src="https://example.com/a.png" alt="x" />)
    expect(remote.container.querySelector('img')!.getAttribute('loading')).toBe('lazy')
  })

  // 滚动容器上挂着消息级右键菜单（MessageList.handleContextMenu）。图片的右键必须掐断冒泡,
  // 否则那个菜单也会开，且 portal 挂得更晚 ⇒ 盖住图片菜单，用户以为图片右键没了。
  it('stops the contextmenu from reaching the message-level menu', () => {
    const onParentContextMenu = vi.fn()
    const { container } = render(
      <div onContextMenu={onParentContextMenu}>
        <ChatInlineImage src={`${PNG}D`} alt="x" />
      </div>,
    )
    act(() => {
      container
        .querySelector('button')!
        .dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    })
    expect(onParentContextMenu).not.toHaveBeenCalled()
    // 自己的菜单要开出来（复制图片/另存那一层）。
    expect(document.body.textContent).toContain('复制图片')
  })
})
