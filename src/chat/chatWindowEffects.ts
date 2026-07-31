import { Effect, EffectState, type PhysicalSize } from '@tauri-apps/api/window'
import { api } from '../api/tauri'

export const CHAT_EFFECT_MAX_WIDTH = 3840
export const CHAT_EFFECT_MAX_HEIGHT = 2160

export type ChatEffectPlatform = 'macos' | 'windows' | 'linux'

/** Win10 没有 Mica，但 tauri 的 set_effects 静默吞掉 apply_mica 的失败
 *  （vibrancy/windows.rs 丢返回值），前端拿不到真话就会误判「材质已生效」而让外壳透明，
 *  于是透明窗口直接透出桌面和别的窗口。所以 Windows 走后端命令，它返回材质是否真的上了。 */
type ApplyMica = (dark: boolean) => Promise<boolean>

type EffectWindow = {
  setEffects: (effects: {
    effects: Effect[]
    state?: EffectState
  }) => Promise<void>
  clearEffects: () => Promise<void>
}

export function chatWindowEffectEligible(
  platform: ChatEffectPlatform,
  enabled: boolean,
  focused: boolean,
  size: Pick<PhysicalSize, 'width' | 'height'>,
): boolean {
  if (platform === 'linux' || !enabled) return false
  if (size.width >= CHAT_EFFECT_MAX_WIDTH && size.height >= CHAT_EFFECT_MAX_HEIGHT) return false
  return platform !== 'macos' || focused
}

export async function syncChatWindowEffect(
  window: EffectWindow,
  platform: ChatEffectPlatform,
  enabled: boolean,
  focused: boolean,
  size: Pick<PhysicalSize, 'width' | 'height'>,
  dark: boolean,
  applyMica: ApplyMica = api.chatWindowApplyMica,
): Promise<boolean> {
  if (platform === 'linux') return false
  if (!chatWindowEffectEligible(platform, enabled, focused, size)) {
    await window.clearEffects().catch(() => {})
    return false
  }

  // Windows：materials 由后端上，因为只有它能报告 Mica 是否真的生效（见 ApplyMica）。
  // 变体跟应用主题走（→ DWMWA_USE_IMMERSIVE_DARK_MODE）：裸 Mica 跟**系统**主题，
  // 亮色系统下暗色应用会从卡片缝隙里透出一片白。macOS 的 Menu 材质自己跟 NSAppearance。
  if (platform === 'windows') {
    const applied = await applyMica(dark).catch(() => false)
    if (!applied) await window.clearEffects().catch(() => {})
    return applied
  }

  try {
    await window.setEffects({ effects: [Effect.Menu], state: EffectState.FollowsWindowActiveState })
    return true
  } catch {
    await window.clearEffects().catch(() => {})
    return false
  }
}
