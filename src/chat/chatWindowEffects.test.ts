import { Effect, EffectState } from '@tauri-apps/api/window'
import { describe, expect, it, vi } from 'vitest'
import { chatWindowEffectEligible, MICA_DARK, MICA_LIGHT, syncChatWindowEffect } from './chatWindowEffects'

function effectWindow(failSet = false) {
  return {
    setEffects: failSet
      ? vi.fn().mockRejectedValue(new Error('unsupported'))
      : vi.fn().mockResolvedValue(undefined),
    clearEffects: vi.fn().mockResolvedValue(undefined),
  }
}

describe('chat window native effects', () => {
  it('uses the physical resize payload with an inclusive two-axis 4K cutoff', () => {
    expect(chatWindowEffectEligible('windows', true, true, { width: 3840, height: 2160 })).toBe(false)
    expect(chatWindowEffectEligible('windows', true, true, { width: 3840, height: 2159 })).toBe(true)
    expect(chatWindowEffectEligible('windows', true, true, { width: 3839, height: 2160 })).toBe(true)
  })

  it('applies macOS Menu material only while focused', async () => {
    const window = effectWindow()
    expect(await syncChatWindowEffect(window, 'macos', true, true, { width: 1280, height: 800 }, true)).toBe(true)
    expect(window.setEffects).toHaveBeenCalledWith({
      effects: [Effect.Menu],
      state: EffectState.FollowsWindowActiveState,
    })

    expect(await syncChatWindowEffect(window, 'macos', true, false, { width: 1280, height: 800 }, true)).toBe(false)
    expect(window.clearEffects).toHaveBeenCalledTimes(1)
  })

  it('keeps Windows Mica while unfocused', async () => {
    const window = effectWindow()
    expect(await syncChatWindowEffect(window, 'windows', true, false, { width: 1280, height: 800 }, true)).toBe(true)
    expect(window.setEffects).toHaveBeenCalledWith({ effects: [MICA_DARK] })
    expect(window.clearEffects).not.toHaveBeenCalled()
  })

  // 裸 Mica 跟系统主题：亮色系统 + 暗色应用会从卡片缝里透出白条，所以必须显式选变体。
  it('picks the Mica variant from the app theme, not the system theme', async () => {
    const window = effectWindow()
    await syncChatWindowEffect(window, 'windows', true, true, { width: 1280, height: 800 }, false)
    expect(window.setEffects).toHaveBeenCalledWith({ effects: [MICA_LIGHT] })
  })

  it('clears effects when disabled or unsupported', async () => {
    const disabledWindow = effectWindow()
    expect(await syncChatWindowEffect(disabledWindow, 'windows', false, true, { width: 1280, height: 800 }, true)).toBe(false)
    expect(disabledWindow.clearEffects).toHaveBeenCalledTimes(1)

    const linuxWindow = effectWindow()
    expect(await syncChatWindowEffect(linuxWindow, 'linux', true, true, { width: 1280, height: 800 }, true)).toBe(false)
    expect(linuxWindow.setEffects).not.toHaveBeenCalled()
    expect(linuxWindow.clearEffects).not.toHaveBeenCalled()
  })

  it('clears the effect when Mica application fails', async () => {
    const window = effectWindow(true)
    expect(await syncChatWindowEffect(window, 'windows', true, true, { width: 1280, height: 800 }, true)).toBe(false)
    expect(window.clearEffects).toHaveBeenCalledTimes(1)
  })
})
