/** @vitest-environment jsdom */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { applyComposerAutoHeight, COMPOSER_MAX_HEIGHT_PX } from './composerAutoHeight'

function mountComposer(scrollHeight: number, offsetHeight: number) {
  const footer = document.createElement('div')
  footer.className = 'chat-composer-footer'
  const textarea = document.createElement('textarea')
  footer.appendChild(textarea)
  document.body.appendChild(footer)
  Object.defineProperty(footer, 'offsetHeight', { configurable: true, get: () => 80 })
  Object.defineProperty(textarea, 'offsetHeight', { configurable: true, get: () => offsetHeight })
  Object.defineProperty(textarea, 'scrollHeight', { configurable: true, get: () => scrollHeight })
  return { footer, textarea }
}

afterEach(() => {
  document.body.replaceChildren()
  vi.unstubAllGlobals()
})

describe('applyComposerAutoHeight', () => {
  it('长高时不把 height 塌成 0，也不锁 footer', () => {
    const { footer, textarea } = mountComposer(72, 44)
    applyComposerAutoHeight(textarea)
    expect(textarea.style.height).toBe('72px')
    expect(textarea.style.height).not.toBe('0px')
    expect(footer.style.minHeight).toBe('')
  })

  it('已到上限只开滚动条，不再加高', () => {
    const { textarea } = mountComposer(240, 44)
    applyComposerAutoHeight(textarea)
    expect(textarea.style.height).toBe(`${COMPOSER_MAX_HEIGHT_PX}px`)
    expect(textarea.style.overflowY).toBe('auto')
  })

  it('同高重测结束时 footer 不残留 minHeight', () => {
    const { footer, textarea } = mountComposer(56, 56)
    applyComposerAutoHeight(textarea)
    expect(footer.style.minHeight).toBe('')
    expect(textarea.style.height).toBe('56px')
  })

  it('field-sizing 可用时不写死 height', () => {
    vi.stubGlobal('CSS', { supports: (prop: string, value: string) => prop === 'field-sizing' && value === 'content' })
    const { textarea } = mountComposer(72, 44)
    applyComposerAutoHeight(textarea)
    expect(textarea.style.height).toBe('')
    expect(textarea.style.overflowY).toBe('hidden')
  })
})
