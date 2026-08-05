import { describe, expect, it } from 'vitest'
import { draftKey, getComposerDraft, migrateNewChatDraft, setComposerDraft } from './composerDraft'

const NEW = draftKey(undefined)

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

  describe('migrateNewChatDraft', () => {
    it('落库拿到 id：草稿搬到真 id，占位键腾空', () => {
      setComposerDraft(NEW, { input: '还没发的字', quotes: [], attachments: [] })
      expect(migrateNewChatDraft(NEW, 'c2')).toBe(true)
      expect(getComposerDraft('c2')?.input).toBe('还没发的字')
      // 必须搬走而不是拷贝，否则下次新建对话会捡到这条已归属别人的草稿
      expect(getComposerDraft(NEW)).toBeUndefined()
    })

    it('目标键已有自己的草稿时不覆盖', () => {
      setComposerDraft(NEW, { input: 'new', quotes: [], attachments: [] })
      setComposerDraft('c3', { input: '它自己的', quotes: [], attachments: [] })
      expect(migrateNewChatDraft(NEW, 'c3')).toBe(false)
      expect(getComposerDraft('c3')?.input).toBe('它自己的')
      setComposerDraft(NEW, { input: '', quotes: [], attachments: [] })
    })

    it('真会话之间切换不搬（否则会把陈旧的新建草稿灌进别的会话）', () => {
      setComposerDraft(NEW, { input: '残留', quotes: [], attachments: [] })
      expect(migrateNewChatDraft('c4', 'c5')).toBe(false)
      expect(getComposerDraft('c5')).toBeUndefined()
      setComposerDraft(NEW, { input: '', quotes: [], attachments: [] })
    })

    it('没有草稿可搬时返回 false，让调用方走正常回填', () => {
      expect(migrateNewChatDraft(NEW, 'c6')).toBe(false)
    })
  })
})
