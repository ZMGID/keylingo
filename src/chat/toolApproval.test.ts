import { describe, expect, it } from 'vitest'
import {
  isClaudePlanApproval,
  isCursorPlanApproval,
  isPlanApproval,
  toolApprovalTitle,
} from './toolApproval'

function payload(name: string) {
  return { name, conversationId: 'c', runId: 'r', toolCallId: 't', source: 'cli' }
}

describe('toolApproval plan cards', () => {
  it('keeps claude ExitPlanMode as the Claude plan card', () => {
    const item = payload('ExitPlanMode')
    expect(isClaudePlanApproval(item)).toBe(true)
    expect(isCursorPlanApproval(item)).toBe(false)
    expect(isPlanApproval(item)).toBe(true)
    expect(toolApprovalTitle(item)).toBe('批准这份计划，开始执行？')
  })

  it('treats cursor/create_plan as its own plan card, not Claude auto-allow', () => {
    const item = payload('cursor/create_plan')
    expect(isClaudePlanApproval(item)).toBe(false)
    expect(isCursorPlanApproval(item)).toBe(true)
    expect(isPlanApproval(item)).toBe(true)
    expect(toolApprovalTitle(item)).toBe('批准这份计划？')
  })
})
