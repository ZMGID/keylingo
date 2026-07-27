import { Effect, EffectState, type PhysicalSize } from '@tauri-apps/api/window'

export const CHAT_EFFECT_MAX_WIDTH = 3840
export const CHAT_EFFECT_MAX_HEIGHT = 2160

export type ChatEffectPlatform = 'macos' | 'windows' | 'linux'

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
): Promise<boolean> {
  if (platform === 'linux') return false
  if (!chatWindowEffectEligible(platform, enabled, focused, size)) {
    await window.clearEffects().catch(() => {})
    return false
  }

  const effects = platform === 'macos'
    ? { effects: [Effect.Menu], state: EffectState.FollowsWindowActiveState }
    : { effects: [Effect.Mica] }

  try {
    await window.setEffects(effects)
    return true
  } catch {
    await window.clearEffects().catch(() => {})
    return false
  }
}
