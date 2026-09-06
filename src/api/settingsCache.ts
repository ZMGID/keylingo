import { api, type Settings } from './tauri'

/**
 * Settings 前端内存缓存（per-webview 模块级单例）。
 *
 * 动机：后端 get_settings 是纯内存读，但每次 invoke 都要完整 clone + IPC 序列化 +
 * normalizeSettings；一次 chat 冷启动会独立发起 5-6 次。缓存后首读之外全部即时返回，
 * SettingsShell 还能用 peekSettings 做 stale-while-revalidate 首帧渲染。
 *
 * 自配置工具成功后广播 kivio-configuration-changed，各 webview 强制读取并通知
 * 订阅者。广播不携带设置或凭据。其它未广播的写入仍使用下面的现读策略。
 *
 * 已知局限（后端旁路写）：后端会直接改 settings 并落盘、不经前端 saveSettings——
 * OAuth 令牌刷新（mcp/manager.rs persist_refreshed_server）改 servers[].auth/headers；
 * 插件启用/停用（plugins::set_plugin_enabled）改 plugin-* MCP 的 enabled。
 * 本缓存无对应失效。因此“读-改-写整个 Settings”的调用方必须用 refreshSettings()（现读）
 * 而非缓存快照，否则可能把刚刷新的 token / 插件开关覆盖回旧值——Chat 的审批策略/ MCP 开关、
 * SkillCenter 保存均已如此处理。SettingsShell 可编辑 servers，其长驻草稿必须在
 * refreshSettings 后采用后端的 plugin MCP 行，否则 keep-alive 整份保存会把插件开关盖回去。
 *
 * 失败语义：读失败不写缓存（下次重试）、保存失败不动缓存——与 SettingsShell
 * “加载失败不合成默认值，避免错误状态下自动保存覆盖磁盘真实数据”的既有约定一致。
 */

let cached: Settings | null = null
let inflight: Promise<Settings> | null = null
let readGeneration = 0

/** 缓存更新订阅者。saveSettingsCached / refreshSettings / importSettingsCached 等
 *  任何写缓存的路径都会广播新 Settings，让"挂载时读一次"的消费方（如 ModelSelector）
 *  在设置自动保存后立即拿到新值——设置页没有保存按钮，落盘与返回聊天视图是并发的，
 *  只靠挂载时读缓存会读到保存回包前的旧快照。 */
type SettingsListener = (settings: Settings) => void
const listeners = new Set<SettingsListener>()

function notifySettingsUpdated(settings: Settings): void {
  for (const listener of [...listeners]) {
    try {
      listener(settings)
    } catch (err) {
      console.error('[settingsCache] listener failed', err)
    }
  }
}

/** 订阅缓存更新；返回取消订阅函数。回调拿到的对象为共享缓存引用，视为只读。 */
export function subscribeSettings(listener: SettingsListener): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** 同步读缓存；未加载过返回 null。供 SWR 首帧使用。 */
export function peekSettings(): Settings | null {
  return cached
}

/** 有缓存立即 resolve；否则发起（或复用进行中的）一次 invoke。并发首读只发一次请求。
 *  返回值视为只读，勿原地 mutate（是共享缓存引用；调用方修改请用展开生成新对象）。 */
export function getSettingsCached(): Promise<Settings> {
  if (cached) return Promise.resolve(cached)
  if (inflight) return inflight
  const generation = ++readGeneration
  inflight = api.getSettings()
    .then((settings) => {
      if (generation === readGeneration) cached = settings
      return cached ?? settings
    })
    .finally(() => {
      inflight = null
    })
  return inflight
}

/** 强制 refetch 并更新缓存（后台校准用）。失败时保留旧缓存。 */
export function refreshSettings(): Promise<Settings> {
  const generation = ++readGeneration
  return api.getSettings().then((settings) => {
    if (generation === readGeneration) {
      cached = settings
      notifySettingsUpdated(settings)
    }
    return cached ?? settings
  })
}

/** Backend agent operations update live settings outside saveSettingsCached.
 * Subscribe once per webview so existing consumers can rebase their drafts.
 * The event has no secrets; a failed refetch never synthesizes defaults. */
export async function startBackendSettingsSync(): Promise<() => void> {
  let stopped = false
  const unlisten = await api.onKivioConfigurationChanged(() => {
    if (stopped) return
    void refreshSettings().catch((error) => console.error('[settingsCache] backend refresh failed', error))
  })
  return () => { stopped = true; unlisten() }
}

/** saveSettings + 成功写通缓存并广播；失败原样抛出且不动缓存。 */
export async function saveSettingsCached(settings: Settings): Promise<Settings> {
  const saved = await api.saveSettings(settings)
  ++readGeneration
  cached = saved
  notifySettingsUpdated(saved)
  return saved
}

/**
 * importSettings + 成功写通缓存。import 会用文件内容整体覆盖磁盘 settings，
 * 返回归一化后的新 Settings，直接替换缓存。
 */
export async function importSettingsCached(path: string): Promise<Settings> {
  const imported = await api.importSettings(path)
  ++readGeneration
  cached = imported
  notifySettingsUpdated(imported)
  return imported
}

/**
 * setFavoriteModels（轻量收藏持久化，不返回 Settings）+ 成功后把新 favoriteModels
 * 补进缓存，避免收藏切换后缓存里的收藏列表变旧。失败原样抛出且不动缓存。
 */
export async function setFavoriteModelsCached(models: string[]): Promise<void> {
  await api.setFavoriteModels(models)
  // 后端 set_favorite_models 会按序去重落盘；缓存里也做同样去重，保持与磁盘一致。
  if (cached) {
    ++readGeneration
    cached = { ...cached, favoriteModels: [...new Set(models)] }
    notifySettingsUpdated(cached)
  }
}

/**
 * setTranslateCardSize（轻量翻译卡宽度持久化）+ 成功后把 clamp 后的宽度补进缓存，
 * 避免 Lens 拖拽缩放后同窗复用时 getSettingsCached 读到旧宽度、把卡片弹回默认值。
 * 失败原样抛出且不动缓存。
 */
export async function setTranslateCardSizeCached(width: number): Promise<void> {
  await api.setTranslateCardSize(width)
  const clamped = Math.max(360, Math.min(720, Math.round(width)))
  if (cached) {
    ++readGeneration
    cached = { ...cached, screenshotTranslation: { ...cached.screenshotTranslation, cardWidth: clamped } }
    notifySettingsUpdated(cached)
  }
}

/** 仅测试用：重置模块状态。 */
export function __resetSettingsCacheForTest(): void {
  cached = null
  inflight = null
  readGeneration = 0
  listeners.clear()
}
