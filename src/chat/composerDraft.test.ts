import { describe, expect, it } from 'vitest'
import { draftKey, getComposerDraft, setComposerDraft } from './composerDraft'

describe('composerDraft', () => {
  it('新建对话回退到固定 key', () => {
    expect(draftKey(null)).toBe(draftKey(undefined))
    expect(draftKey('c1')).toBe('c1')
  })

  it('存取往返 + 空草稿删除', () => {
    const k = draftKey('c1')
    setComposerDraft(k, { input: 'hi', quotes: [], attachments: [] })
    expect(getComposerDraft(k)?.input).toBe('hi')
    // 全空 => 从 store 移除，避免留空壳
    setComposerDraft(k, { input: '', quotes: [], attachments: [] })
    expect(getComposerDraft(k)).toBeUndefined()
  })
})
