import { describe, expect, it } from 'vitest'
import { canOptimizeComposerText } from './promptOptimize'

describe('canOptimizeComposerText', () => {
  it('rejects empty and slash commands', () => {
    expect(canOptimizeComposerText('')).toBe(false)
    expect(canOptimizeComposerText('   ')).toBe(false)
    expect(canOptimizeComposerText('/compact')).toBe(false)
    expect(canOptimizeComposerText('帮我看看这个')).toBe(true)
  })
})
