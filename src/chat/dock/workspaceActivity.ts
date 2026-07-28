// workspace:activity 事件单例客户端。
// 多个订阅方（文件树 / Git 面板）共享一个 Tauri listener；每次订阅集合变化时
// 用全量 workdir 集重调 dock_workspace_watch_set（后端语义是整体替换）。
// 非 Tauri 运行时（npm run dev:ui）subscribe 为 no-op，isAvailable() 为 false，
// 调用方据此退回 10s 轮询。
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { isTauriRuntime } from '../utils'
import { dockApi } from './api'
import { normalizeWorkspaceActivityEvent, type WorkspaceActivityEvent } from './types'

export type WorkspaceActivityListener = (event: WorkspaceActivityEvent) => void

const listenersByWorkdir = new Map<string, Set<WorkspaceActivityListener>>()

let unlistenPromise: Promise<UnlistenFn> | null = null
let syncPending = false

function ensureEventListener(): void {
  if (!isTauriRuntime() || unlistenPromise) return
  unlistenPromise = listen('workspace:activity', (event) => {
    dispatch(normalizeWorkspaceActivityEvent(event.payload))
  })
  // listener 注册失败时允许下次订阅重试，而不是永久哑掉。
  unlistenPromise.catch(() => {
    unlistenPromise = null
  })
}

function dispatch(activity: WorkspaceActivityEvent): void {
  const listeners = listenersByWorkdir.get(activity.workdir)
  if (!listeners) return
  for (const listener of [...listeners]) {
    try {
      listener(activity)
    } catch (err) {
      console.error('[dock] workspace activity listener failed:', err)
    }
  }
}

/** 把当前订阅全集推给后端。合并同一微任务里的多次增删，避免连发 watch_set。 */
function scheduleWatchSync(): void {
  if (!isTauriRuntime() || syncPending) return
  syncPending = true
  queueMicrotask(() => {
    syncPending = false
    void dockApi.workspaceWatchSet([...listenersByWorkdir.keys()]).catch((err) => {
      console.error('[dock] workspace watch set failed:', err)
    })
  })
}

export const workspaceActivity = {
  /** watcher 服务是否可用（决定调用方是否需要 10s 兜底轮询）。 */
  isAvailable(): boolean {
    return isTauriRuntime()
  },

  subscribe(workdir: string, listener: WorkspaceActivityListener): () => void {
    if (!isTauriRuntime() || !workdir) return () => {}
    ensureEventListener()
    let listeners = listenersByWorkdir.get(workdir)
    if (!listeners) {
      listeners = new Set()
      listenersByWorkdir.set(workdir, listeners)
    }
    listeners.add(listener)
    scheduleWatchSync()
    return () => {
      const current = listenersByWorkdir.get(workdir)
      if (!current) return
      current.delete(listener)
      if (current.size === 0) listenersByWorkdir.delete(workdir)
      scheduleWatchSync()
    }
  },
}
