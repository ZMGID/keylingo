import { describe, expect, it } from 'vitest'
import {
  conversationTitleSource,
  displayConversationTitle,
  isPlaceholderTitle,
  isProvisionalTitle,
  optimisticConversationTitle,
} from './conversationTitle'

describe('optimisticConversationTitle', () => {
  it('truncates a long first message by Unicode scalar count', () => {
    const long = '这是一条非常非常非常非常非常非常非常非常非常非常非常长的第一句话'
    expect(Array.from(long).length).toBeGreaterThan(30)
    expect(optimisticConversationTitle(long)).toBe(`${Array.from(long).slice(0, 30).join('')}...`)
  })

  it('keeps a short first message intact', () => {
    expect(optimisticConversationTitle('吉林天气查询')).toBe('吉林天气查询')
  })

  it('does not fall back to 新对话 when the first message is empty', () => {
    expect(optimisticConversationTitle('')).toBe('')
    expect(optimisticConversationTitle('   \n  ')).toBe('')
  })

  it('keeps internal newlines like Rust generate_title', () => {
    expect(optimisticConversationTitle('你好\n世界')).toBe('你好\n世界')
  })

  it('does not split emoji code points', () => {
    const emoji = '😀'.repeat(40)
    expect(optimisticConversationTitle(emoji)).toBe(`${'😀'.repeat(30)}...`)
  })

  it('uses attachment names when content is empty', () => {
    expect(optimisticConversationTitle('', ['notes.pdf'])).toBe('附件: notes.pdf')
    expect(optimisticConversationTitle('hello', ['notes.pdf'])).toBe('hello')
  })
})

describe('conversationTitleSource', () => {
  it('returns empty when there is nothing to title from', () => {
    expect(conversationTitleSource('  ', [])).toBe('')
  })
})

describe('displayConversationTitle', () => {
  it('keeps a generated or renamed title', () => {
    expect(displayConversationTitle('Apex 掉帧排查', 'ignored')).toBe('Apex 掉帧排查')
  })

  it('replaces 新对话 with the first-message fallback', () => {
    expect(displayConversationTitle('新对话', '我这几天玩apex，总是突然掉帧')).toBe(
      '我这几天玩apex，总是突然掉帧',
    )
  })

  it('replaces a forked placeholder with the fallback', () => {
    expect(displayConversationTitle('新对话（分支）', '吉林天气查询')).toBe('吉林天气查询')
  })

  it('never renders 新对话 when there is no fallback yet', () => {
    expect(displayConversationTitle('新对话', '')).toBe('')
    expect(displayConversationTitle('新对话')).toBe('')
  })
})

describe('isPlaceholderTitle', () => {
  it('matches the create-time sentinel only', () => {
    expect(isPlaceholderTitle('新对话')).toBe(true)
    expect(isPlaceholderTitle(' 新对话 ')).toBe(true)
    expect(isPlaceholderTitle('新对话（分支）')).toBe(true)
    expect(isPlaceholderTitle('我这几天玩apex')).toBe(false)
    expect(isPlaceholderTitle('')).toBe(false)
  })
})

describe('isProvisionalTitle', () => {
  it('matches the optimistic truncation of a long first message', () => {
    const long = '这是一条非常非常非常非常非常非常非常非常非常非常非常长的第一句话'
    expect(Array.from(long).length).toBeGreaterThan(30)
    expect(isProvisionalTitle(`${Array.from(long).slice(0, 30).join('')}...`, long)).toBe(true)
  })

  it('matches the full preview when shorter than 30 chars', () => {
    expect(isProvisionalTitle('吉林天气查询', '吉林天气查询')).toBe(true)
  })

  it('rejects a generated title that differs from the truncation', () => {
    expect(isProvisionalTitle('吉林天气查询', '帮我查一下吉林市今天天气怎么样')).toBe(false)
  })

  it('rejects empty preview', () => {
    expect(isProvisionalTitle('新对话', '')).toBe(false)
    expect(isProvisionalTitle('新对话', '   ')).toBe(false)
  })

  it('does not collapse internal whitespace (matches Rust generate_title)', () => {
    expect(isProvisionalTitle('你好 世界', '你好\n世界')).toBe(false)
    expect(isProvisionalTitle('你好\n世界', '你好\n世界')).toBe(true)
  })
})
