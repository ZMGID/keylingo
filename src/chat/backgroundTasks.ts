import type { BackgroundTaskInfo } from '../api/tauri'

/** Running 在前（新的在上），Finished 在后（新结束的在上）。 */
export function partitionTasks(tasks: BackgroundTaskInfo[]): {
  running: BackgroundTaskInfo[]
  finished: BackgroundTaskInfo[]
} {
  const running = tasks.filter((t) => t.status === 'running')
  const finished = tasks.filter((t) => t.status !== 'running')
  running.sort((a, b) => b.startedAtMs - a.startedAtMs)
  finished.sort((a, b) => b.startedAtMs - a.startedAtMs)
  return { running, finished }
}
