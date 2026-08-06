import { describe, expect, it } from 'vitest'
import { partitionTasks } from './backgroundTasks'
import type { BackgroundTaskInfo } from '../api/tauri'

function task(over: Partial<BackgroundTaskInfo>): BackgroundTaskInfo {
  return {
    id: 'x',
    source: 'builtin',
    kind: 'bash',
    title: 'cmd',
    status: 'running',
    elapsedSecs: 0,
    startedAtMs: 0,
    ...over,
  }
}

describe('partitionTasks', () => {
  it('splits running from finished and sorts both newest-first', () => {
    const { running, finished } = partitionTasks([
      task({ id: 'old-run', status: 'running', startedAtMs: 100 }),
      task({ id: 'done', status: 'completed', startedAtMs: 300 }),
      task({ id: 'new-run', status: 'running', startedAtMs: 200 }),
      task({ id: 'dead', status: 'failed', startedAtMs: 400 }),
      task({ id: 'killed', status: 'stopped', startedAtMs: 50 }),
    ])
    expect(running.map((t) => t.id)).toEqual(['new-run', 'old-run'])
    expect(finished.map((t) => t.id)).toEqual(['dead', 'done', 'killed'])
  })
})
