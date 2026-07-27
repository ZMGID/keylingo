import { describe, expect, it } from 'vitest'
import { base64FromDataUrl, imageExtension } from './imageData'

describe('base64FromDataUrl', () => {
  it('extracts the payload from a base64 data URL', () => {
    expect(base64FromDataUrl('data:image/png;base64,aGVsbG8=')).toBe('aGVsbG8=')
  })

  it('returns empty for sources the clipboard command cannot decode', () => {
    // 远程 URL：没有字节可交给 arboard。
    expect(base64FromDataUrl('https://example.com/a.png')).toBe('')
    // 明文（非 base64）data URL：payload 不是 base64，解码会失败。
    expect(base64FromDataUrl('data:image/svg+xml,<svg/>')).toBe('')
    expect(base64FromDataUrl('data:image/png;base64')).toBe('')
  })
})

describe('imageExtension', () => {
  it('prefers the filename suffix', () => {
    expect(imageExtension('data:image/png;base64,x', 'shot.JPG')).toBe('jpg')
  })

  it('falls back to the data URL mime, then png', () => {
    expect(imageExtension('data:image/jpeg;base64,x')).toBe('jpg')
    expect(imageExtension('data:image/webp;base64,x')).toBe('webp')
    expect(imageExtension('https://example.com/a', 'noext')).toBe('png')
  })
})
