import { describe, expect, it } from 'vitest'
import { matchComposerSlashCommand, shouldOpenSlashPopover, splitComposerSlashCommand } from './slashCommands'

describe('shouldOpenSlashPopover', () => {
  it('opens whenever a slash token is active', () => {
    expect(shouldOpenSlashPopover()).toBe(true)
  })
})

describe('splitComposerSlashCommand', () => {
  it('splits a leading slash command from its arguments', () => {
    expect(splitComposerSlashCommand('/goal')).toEqual({
      prefix: '',
      command: '/goal',
      rest: '',
    })
    expect(splitComposerSlashCommand('/goal 输入目标')).toEqual({
      prefix: '',
      command: '/goal',
      rest: ' 输入目标',
    })
  })

  it('keeps leading spaces and ignores mid-line slashes', () => {
    expect(splitComposerSlashCommand('  /plan off')).toEqual({
      prefix: '  ',
      command: '/plan',
      rest: ' off',
    })
    expect(splitComposerSlashCommand('hello /goal')).toBeNull()
    expect(splitComposerSlashCommand('Ask me anything')).toBeNull()
  })

  it('highlights only an exact known command', () => {
    const commands = [{ slash: '/goal' }, { slash: '/plan' }]
    expect(matchComposerSlashCommand('/goal', commands)?.command).toBe('/goal')
    expect(matchComposerSlashCommand('/goal 输入目标', commands)?.command).toBe('/goal')
    expect(matchComposerSlashCommand('/goalw', commands)).toBeNull()
    expect(matchComposerSlashCommand('/go', commands)).toBeNull()
    expect(matchComposerSlashCommand('/', commands)).toBeNull()
  })
})
