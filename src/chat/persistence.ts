import type { Window } from '@tauri-apps/api/window'
import { isWindows } from './platform'

export const CHAT_DEFAULT_SIZE = { width: 1280, height: 800 }
/** 侧栏收起时可缩到的最小尺寸 */
export const CHAT_MIN_SIZE_COLLAPSED = { width: 400, height: 400 }
/** 侧栏展开时整窗最小尺寸（240px 侧栏 + 主内容区） */
export const CHAT_MIN_SIZE_EXPANDED = { width: 640, height: 400 }
export const CHAT_MIN_SIZE = CHAT_MIN_SIZE_COLLAPSED

export type ChatWindowGeometry = {
  width: number
  height: number
  x?: number
  y?: number
}

const CHAT_LAST_ROUTE_KEY = 'kivio-chat-last-route'
const CHAT_SIDEBAR_COLLAPSED_KEY = 'kivio-chat-sidebar-collapsed'
const CHAT_WINDOW_GEOMETRY_KEY = 'kivio-chat-window-geometry'
/** @deprecated 旧版仅持久化尺寸；读取时自动迁移到 geometry key */
const CHAT_WINDOW_SIZE_KEY = 'kivio-chat-window-size'
const WINDOWS_MINIMIZED_POSITION_SENTINEL = -10000
const MIN_VISIBLE_GEOMETRY_EDGE = 80

export function hashPath(): string {
  return window.location.hash.replace('#', '').split('?')[0]
}

export function isChatPath(path: string): boolean {
  return path === 'chat' || path.startsWith('chat/')
}

export function isChatSettingsPath(path: string): boolean {
  return path === 'chat/settings' || path.startsWith('chat/settings/')
}

export function isChatOnboardingPath(path: string): boolean {
  return path === 'chat/onboarding' || path.startsWith('chat/onboarding/')
}

function getLocalStorageItem(key: string): string | null {
  try {
    return window.localStorage?.getItem(key) ?? null
  } catch {
    return null
  }
}

function setLocalStorageItem(key: string, value: string) {
  try {
    window.localStorage?.setItem(key, value)
  } catch {
    // Storage can be unavailable in restricted previews. Chat still works without persistence.
  }
}

function removeLocalStorageItem(key: string) {
  try {
    window.localStorage?.removeItem(key)
  } catch {
    // Ignore storage errors; persistence is best-effort only.
  }
}

function forgetRememberedChatGeometry() {
  removeLocalStorageItem(CHAT_WINDOW_GEOMETRY_KEY)
  removeLocalStorageItem(CHAT_WINDOW_SIZE_KEY)
}

export function normalizeStoredChatRoute(value: string | null): string | null {
  if (!value) return null
  const route = value.startsWith('#') ? value : `#${value}`
  const path = route.replace('#', '').split('?')[0]
  if (!isChatPath(path) || isChatSettingsPath(path) || isChatOnboardingPath(path)) return null
  return route
}

export function rememberCurrentChatRoute() {
  const path = hashPath()
  if (!path.startsWith('chat/') || isChatSettingsPath(path) || isChatOnboardingPath(path)) return
  setLocalStorageItem(CHAT_LAST_ROUTE_KEY, window.location.hash || '#chat')
}

export function getRememberedChatRoute(): string | null {
  return normalizeStoredChatRoute(getLocalStorageItem(CHAT_LAST_ROUTE_KEY))
}

export function forgetRememberedChatRoute() {
  removeLocalStorageItem(CHAT_LAST_ROUTE_KEY)
}

export function getRememberedChatSidebarCollapsed(): boolean {
  return getLocalStorageItem(CHAT_SIDEBAR_COLLAPSED_KEY) === '1'
}

export function rememberChatSidebarCollapsed(collapsed: boolean) {
  setLocalStorageItem(CHAT_SIDEBAR_COLLAPSED_KEY, collapsed ? '1' : '0')
}

// ---------- Right Dock 持久化 ----------

const CHAT_DOCK_OPEN_KEY = 'kivio-chat-dock-open'
const CHAT_DOCK_WIDTH_KEY = 'kivio-chat-dock-width'
const CHAT_DOCK_TAB_KEY = 'kivio-chat-dock-tab'
const CHAT_DOCK_TREE_EXPANDED_KEY = 'kivio-chat-dock-tree-expanded'
/** 每项目展开状态 map 的最大项目键数（超出时丢弃最旧的键）。 */
const DOCK_TREE_EXPANDED_MAX_KEYS = 50

export type RememberedDockTab = 'files' | 'git' | 'terminal' | 'tasks'

export function getRememberedDockOpen(): boolean {
  return getLocalStorageItem(CHAT_DOCK_OPEN_KEY) === '1'
}

export function rememberDockOpen(open: boolean) {
  setLocalStorageItem(CHAT_DOCK_OPEN_KEY, open ? '1' : '0')
}

export function getRememberedDockWidth(): number {
  const parsed = Number(getLocalStorageItem(CHAT_DOCK_WIDTH_KEY))
  if (!Number.isFinite(parsed) || parsed <= 0) return 360
  return Math.min(560, Math.max(320, Math.round(parsed)))
}

export function rememberDockWidth(width: number) {
  if (!Number.isFinite(width) || width <= 0) return
  setLocalStorageItem(CHAT_DOCK_WIDTH_KEY, String(Math.round(width)))
}

export function getRememberedDockTab(): RememberedDockTab {
  const raw = getLocalStorageItem(CHAT_DOCK_TAB_KEY)
  return raw === 'git' || raw === 'terminal' || raw === 'tasks' ? raw : 'files'
}

export function rememberDockTab(tab: RememberedDockTab) {
  setLocalStorageItem(CHAT_DOCK_TAB_KEY, tab)
}

function loadTreeExpandedMap(): Record<string, string[]> {
  try {
    const raw = getLocalStorageItem(CHAT_DOCK_TREE_EXPANDED_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw)
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return {}
    const map: Record<string, string[]> = {}
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (Array.isArray(value)) {
        map[key] = value.filter((item): item is string => typeof item === 'string')
      }
    }
    return map
  } catch {
    return {}
  }
}

/** 文件树展开路径按项目键（workdir 归一化串）存一张 JSON map。 */
export function getRememberedTreeExpanded(projectKey: string): string[] {
  if (!projectKey) return []
  return loadTreeExpandedMap()[projectKey] ?? []
}

export function rememberTreeExpanded(projectKey: string, paths: string[]) {
  if (!projectKey) return
  const map = loadTreeExpandedMap()
  if (paths.length === 0) delete map[projectKey]
  else map[projectKey] = paths
  // cap：超出时按插入序丢最旧的键。
  const keys = Object.keys(map)
  while (keys.length > DOCK_TREE_EXPANDED_MAX_KEYS) {
    const oldest = keys.shift()
    if (oldest === undefined) break
    delete map[oldest]
  }
  setLocalStorageItem(CHAT_DOCK_TREE_EXPANDED_KEY, JSON.stringify(map))
}

function normalizeChatWindowGeometry(
  parsed: Partial<ChatWindowGeometry>,
): ChatWindowGeometry | null {
  const width = Number(parsed.width)
  const height = Number(parsed.height)
  if (!Number.isFinite(width) || !Number.isFinite(height)) return null
  const x = Number(parsed.x)
  const y = Number(parsed.y)
  const min = CHAT_MIN_SIZE
  const next: ChatWindowGeometry = {
    width: Math.max(min.width, Math.round(width)),
    height: Math.max(min.height, Math.round(height)),
  }
  if (Number.isFinite(x) && Number.isFinite(y)) {
    next.x = Math.round(x)
    next.y = Math.round(y)
  }
  return next
}

function hasWindowsMinimizedSentinel(geometry: ChatWindowGeometry): boolean {
  return (
    (Number.isFinite(geometry.x) && geometry.x! <= WINDOWS_MINIMIZED_POSITION_SENTINEL) ||
    (Number.isFinite(geometry.y) && geometry.y! <= WINDOWS_MINIMIZED_POSITION_SENTINEL)
  )
}

type LogicalRect = {
  x: number
  y: number
  width: number
  height: number
}

function geometryHasPosition(geometry: ChatWindowGeometry): geometry is Required<ChatWindowGeometry> {
  return Number.isFinite(geometry.x) && Number.isFinite(geometry.y)
}

function intersectsEnough(a: LogicalRect, b: LogicalRect): boolean {
  const left = Math.max(a.x, b.x)
  const top = Math.max(a.y, b.y)
  const right = Math.min(a.x + a.width, b.x + b.width)
  const bottom = Math.min(a.y + a.height, b.y + b.height)
  return right - left >= MIN_VISIBLE_GEOMETRY_EDGE && bottom - top >= MIN_VISIBLE_GEOMETRY_EDGE
}

async function isChatGeometryOnAnyMonitor(geometry: ChatWindowGeometry): Promise<boolean> {
  if (!geometryHasPosition(geometry)) return true
  if (hasWindowsMinimizedSentinel(geometry)) return false

  try {
    const { availableMonitors } = await import('@tauri-apps/api/window')
    const monitors = await availableMonitors()
    if (monitors.length === 0) return true

    const windowRect: LogicalRect = {
      x: geometry.x,
      y: geometry.y,
      width: geometry.width,
      height: geometry.height,
    }

    return monitors.some((monitor) => {
      const scaleFactor = monitor.scaleFactor || 1
      const monitorRect: LogicalRect = {
        x: monitor.workArea.position.x / scaleFactor,
        y: monitor.workArea.position.y / scaleFactor,
        width: monitor.workArea.size.width / scaleFactor,
        height: monitor.workArea.size.height / scaleFactor,
      }
      return intersectsEnough(windowRect, monitorRect)
    })
  } catch {
    return true
  }
}

export function getRememberedChatGeometry(): ChatWindowGeometry {
  try {
    const rawGeometry = getLocalStorageItem(CHAT_WINDOW_GEOMETRY_KEY)
    if (rawGeometry) {
      const parsed = JSON.parse(rawGeometry) as Partial<ChatWindowGeometry>
      const normalized = normalizeChatWindowGeometry(parsed)
      if (normalized) return normalized
    }
    const rawSize = getLocalStorageItem(CHAT_WINDOW_SIZE_KEY)
    if (rawSize) {
      const parsed = JSON.parse(rawSize) as Partial<{ width: number; height: number }>
      const normalized = normalizeChatWindowGeometry(parsed)
      if (normalized) return normalized
    }
  } catch {
    // fall through
  }
  return CHAT_DEFAULT_SIZE
}

export function rememberChatGeometry(geometry: ChatWindowGeometry) {
  const normalized = normalizeChatWindowGeometry(geometry)
  if (!normalized) return
  setLocalStorageItem(CHAT_WINDOW_GEOMETRY_KEY, JSON.stringify(normalized))
}

export function rememberChatSize(width: number, height: number) {
  const current = getRememberedChatGeometry()
  rememberChatGeometry({ ...current, width, height })
}

/** 在 show 之前恢复上次窗口尺寸与位置，避免先闪默认 1280×800 再跳变。 */
export async function restoreChatWindowGeometry(win: Window): Promise<void> {
  if (await win.isMaximized()) return

  const { LogicalPosition, LogicalSize } = await import('@tauri-apps/api/window')
  const geo = getRememberedChatGeometry()
  const canRestorePosition = !isWindows || await isChatGeometryOnAnyMonitor(geo)
  if (isWindows && !canRestorePosition) {
    forgetRememberedChatGeometry()
  }

  await win.setSize(new LogicalSize(geo.width, geo.height))
  if (canRestorePosition && geometryHasPosition(geo)) {
    await win.setPosition(new LogicalPosition(geo.x!, geo.y!))
  } else {
    await win.center()
  }
}

export async function isChatWindowPlacementVisible(win: Window): Promise<boolean> {
  if (!isWindows) return true

  const geometry = await snapshotChatWindowGeometry(win)
  if (!geometry || !geometryHasPosition(geometry)) return false
  return isChatGeometryOnAnyMonitor(geometry)
}

export async function snapshotChatWindowGeometry(win: Window): Promise<ChatWindowGeometry | null> {
  try {
    if (isWindows) {
      const [visible, minimized] = await Promise.all([win.isVisible(), win.isMinimized()])
      if (!visible || minimized) return null
    }

    const scaleFactor = await win.scaleFactor()
    const [size, position] = await Promise.all([win.innerSize(), win.outerPosition()])
    const logicalSize = size.toLogical(scaleFactor)
    const logicalPosition = position.toLogical(scaleFactor)
    const geometry = normalizeChatWindowGeometry({
      width: logicalSize.width,
      height: logicalSize.height,
      x: logicalPosition.x,
      y: logicalPosition.y,
    })
    if (geometry && isWindows && !await isChatGeometryOnAnyMonitor(geometry)) return null
    return geometry
  } catch {
    return null
  }
}
