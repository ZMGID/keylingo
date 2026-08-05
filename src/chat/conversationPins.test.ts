import { describe, expect, it } from 'vitest'
import { applyConversationPins, withPinAt } from './conversationPins'

const items = (...ids: string[]) => ids.map((id) => ({ id }))
const ids = (list: { id: string }[]) => list.map((item) => item.id)

describe('applyConversationPins', () => {
  it('没有钉子时原样返回时间序', () => {
    const timeOrdered = items('a', 'b', 'c')
    expect(applyConversationPins(timeOrdered, [])).toBe(timeOrdered)
  })

  it('钉子占住指定行，其余按时间填空位', () => {
    // c 本来在最后，钉到第 0 行 → c 在最上面，a b 依次填 1、2。
    expect(ids(applyConversationPins(items('a', 'b', 'c'), [{ id: 'c', row: 0 }]))).toEqual([
      'c',
      'a',
      'b',
    ])
    expect(ids(applyConversationPins(items('a', 'b', 'c'), [{ id: 'a', row: 2 }]))).toEqual([
      'b',
      'c',
      'a',
    ])
  })

  it('多个钉子按 row 升序落位', () => {
    const out = applyConversationPins(items('a', 'b', 'c', 'd'), [
      { id: 'd', row: 0 },
      { id: 'b', row: 2 },
    ])
    expect(ids(out)).toEqual(['d', 'a', 'b', 'c'])
  })

  it('两条钉同一行时后者顺延到下一个空位，不丢项', () => {
    const out = applyConversationPins(items('a', 'b', 'c'), [
      { id: 'b', row: 0 },
      { id: 'c', row: 0 },
    ])
    expect(ids(out)).toEqual(['b', 'c', 'a'])
  })

  it('行号越界夹到最后一行；钉子指向已删除的对话直接忽略', () => {
    expect(ids(applyConversationPins(items('a', 'b'), [{ id: 'a', row: 99 }]))).toEqual(['b', 'a'])
    expect(ids(applyConversationPins(items('a', 'b'), [{ id: 'gone', row: 0 }]))).toEqual(['a', 'b'])
  })

  it('输出长度恒等于输入长度', () => {
    const out = applyConversationPins(items('a', 'b', 'c'), [
      { id: 'a', row: 5 },
      { id: 'b', row: 5 },
      { id: 'c', row: 5 },
    ])
    expect(out).toHaveLength(3)
    expect(new Set(ids(out))).toEqual(new Set(['a', 'b', 'c']))
  })
})

describe('withPinAt', () => {
  it('替换同一 id 原有的钉子而不是叠加', () => {
    const pins = withPinAt(withPinAt([], 'a', 1), 'a', 3)
    expect(pins).toEqual([{ id: 'a', row: 3 }])
  })
})
