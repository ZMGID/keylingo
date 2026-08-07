import { describe, expect, it } from 'vitest'
import { isProvisionalTitle } from './conversationTitle'

describe('isProvisionalTitle', () => {
  it('matches the optimistic truncation of a long first message', () => {
    const long = '这是一条非常非常非常非常非常非常非常非常非常非常非常长的第一句话'
    expect(long.length).toBeGreaterThan(30)
    expect(isProvisionalTitle(`${long.slice(0, 30)}...`, long)).toBe(true)
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

  it('normalizes whitespace like optimisticConversationTitle', () => {
    // 乐观标题是归一化后的单空格，preview 保留原始换行
    expect(isProvisionalTitle('你好 世界', '你好\n世界')).toBe(true)
  })
})
