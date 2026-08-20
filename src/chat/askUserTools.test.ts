import { describe, expect, it } from 'vitest'
import { foldToolName, hasAskUserStructuredContent, isAskUserToolName } from './askUserTools'

describe('askUserTools', () => {
  it('folds claude and dsh ask-user names onto the same key', () => {
    expect(foldToolName('AskUserQuestion')).toBe('askuserquestion')
    expect(foldToolName('ask_user_question')).toBe('askuserquestion')
    expect(foldToolName('ask_user')).toBe('askuser')
  })

  it('recognizes ask-user tools without treating claude ExitPlanMode as one', () => {
    expect(isAskUserToolName('ask_user')).toBe(true)
    expect(isAskUserToolName('AskUserQuestion')).toBe(true)
    expect(isAskUserToolName('ask_user_question')).toBe(true)
    expect(isAskUserToolName('requestUserInput')).toBe(true)
    expect(isAskUserToolName('exit_plan_mode')).toBe(true)
    expect(isAskUserToolName('ExitPlanMode')).toBe(false)
    expect(isAskUserToolName('EnterPlanMode')).toBe(false)
    expect(isAskUserToolName('bash')).toBe(false)
  })

  it('treats askUser structured content as the durable signal', () => {
    expect(hasAskUserStructuredContent({ askUser: { phase: 'awaiting' } })).toBe(true)
    expect(hasAskUserStructuredContent({ type: 'subagent' })).toBe(false)
    expect(hasAskUserStructuredContent(null)).toBe(false)
  })
})
