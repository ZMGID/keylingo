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
  it('caps the long edge at 128px so tiles can sit in a row', () => {
    const { container } = render(<ChatInlineImage src={`${PNG}A`} alt="x" />)
    const button = container.querySelector('button')!
    // 未知比例先按 1:1 占位，避免解码前撑满整行。
    expect(button.style.aspectRatio).toBe('1')
    expect(button.style.width).toBe('128px')
    expect(button.style.maxWidth).toBe('100%')

    fireLoad(container.querySelector('img')!, 1000, 500)
    expect(button.style.aspectRatio).toBe('2')
    // 横图：宽封顶 128，高 = 64。
    expect(button.style.width).toBe('128px')
  })

  it('caps portrait tiles by height so they stay 240 tall', () => {
    const { container } = render(<ChatInlineImage src={`${PNG}P`} alt="x" />)
    fireLoad(container.querySelector('img')!, 500, 1000)
    expect(container.querySelector('button')!.style.width).toBe('64px')
  })

  it('remembers the ratio across unmount so scrolling back does not re-measure', () => {
    const src = `${PNG}B`
    const first = render(<ChatInlineImage src={src} alt="x" />)
    fireLoad(first.container.querySelector('img')!, 800, 400)
    first.unmount()

    const second = render(<ChatInlineImage src={src} alt="x" />)
    expect(second.container.querySelector('button')!.style.aspectRatio).toBe('2')
  })

  it('does not lazy-load data URLs (bytes are already in memory)', () => {
    const inline = render(<ChatInlineImage src={`${PNG}C`} alt="x" />)
    expect(inline.container.querySelector('img')!.getAttribute('loading')).toBeNull()

    const remote = render(<ChatInlineImage src="https://example.com/a.png" alt="x" />)
    expect(remote.container.querySelector('img')!.getAttribute('loading')).toBe('lazy')
  })

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
    expect(document.body.textContent).toContain('复制图片')
  })
})
