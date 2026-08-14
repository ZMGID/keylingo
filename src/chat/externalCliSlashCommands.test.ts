import { describe, expect, it } from 'vitest'
import { mapExternalCliSlashCommands } from './externalCliSlashCommands'
import { commandMatches } from './slashCommands'

describe('mapExternalCliSlashCommands', () => {
  it('maps probed Claude commands into slash popover items', () => {
    const commands = mapExternalCliSlashCommands('claude', [
      { name: 'compact', slash: '/compact', description: 'Compact history' },
      { name: 'frontend-design:frontend-design', slash: '/frontend-design:frontend-design' },
    ])
    expect(commands.some((item) => item.slash === '/compact')).toBe(true)
    expect(commands.some((item) => item.slash === '/frontend-design:frontend-design')).toBe(true)
    expect(commands.every((item) => item.kind === 'cli')).toBe(true)
  })

  it('maps dsh official commands as passthrough CLI items', () => {
    const commands = mapExternalCliSlashCommands('dsh', [
      { name: 'compact', slash: '/compact', description: 'Compact older conversation history' },
      { name: 'model', slash: '/model', description: '选择本会话使用的模型' },
    ])
    expect(commands.every((item) => item.kind === 'cli')).toBe(true)
    expect(commands.every((item) => item.category === 'DeepSeek Harness')).toBe(true)
    expect(commands.some((item) => item.slash === '/compact')).toBe(true)
  })

  it('filters by query like builtin slash popover', () => {
    const commands = mapExternalCliSlashCommands('claude', [
      { name: 'compact', slash: '/compact' },
      { name: 'context', slash: '/context' },
    ])
    const filtered = commands.filter((item) => commandMatches(item, 'comp'))
    expect(filtered.some((item) => item.slash === '/compact')).toBe(true)
    expect(filtered.some((item) => item.slash === '/context')).toBe(false)
  })
})
