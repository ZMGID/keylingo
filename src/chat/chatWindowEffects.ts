import { Effect, EffectState, type PhysicalSize } from '@tauri-apps/api/window'

export const CHAT_EFFECT_MAX_WIDTH = 3840
export const CHAT_EFFECT_MAX_HEIGHT = 2160

export type ChatEffectPlatform = 'macos' | 'windows' | 'linux'

/** Mica 的明暗变体。@tauri-apps/api 的 Effect 枚举漏了这两个，Rust 侧 WindowEffect
 *  （camelCase serde）认得，故用字面量断言塞进去。 */
export const MICA_DARK = 'micaDark' as Effect
export const MICA_LIGHT = 'micaLight' as Effect

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
): Promise<boolean> {
  if (platform === 'linux') return false
  if (!chatWindowEffectEligible(platform, enabled, focused, size)) {
    await window.clearEffects().catch(() => {})
    return false
  }

  // 裸 Effect.Mica = apply_mica(hwnd, None)，跟的是**系统**主题：亮色系统下暗色应用会
  // 从卡片缝隙里透出一片白。显式选变体（→ DWMWA_USE_IMMERSIVE_DARK_MODE）让材质
  // 和窗口描边跟应用主题走。macOS 的 Menu 材质自己跟 NSAppearance，无需分叉。
  const effects = platform === 'macos'
    ? { effects: [Effect.Menu], state: EffectState.FollowsWindowActiveState }
    : { effects: [dark ? MICA_DARK : MICA_LIGHT] }

  try {
    await window.setEffects(effects)
    return true
  } catch {
    await window.clearEffects().catch(() => {})
    return false
  }
}
