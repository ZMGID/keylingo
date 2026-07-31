import { Effect, EffectState } from '@tauri-apps/api/window'
import { describe, expect, it, vi } from 'vitest'
import { chatWindowEffectEligible, syncChatWindowEffect } from './chatWindowEffects'

function effectWindow() {
  return {
    setEffects: vi.fn().mockResolvedValue(undefined),
    clearEffects: vi.fn().mockResolvedValue(undefined),
  }
}

/** 后端 chat_window_apply_mica 的替身：Win11 上 true，Win10（没有 Mica）上 false。 */
const mica = (applied: boolean) => vi.fn().mockResolvedValue(applied)

describe('chat window native effects', () => {
  it('uses the physical resize payload with an inclusive two-axis 4K cutoff', () => {
    expect(chatWindowEffectEligible('windows', true, { width: 3840, height: 2160 })).toBe(false)
    expect(chatWindowEffectEligible('windows', true, { width: 3840, height: 2159 })).toBe(true)
    expect(chatWindowEffectEligible('windows', true, { width: 3839, height: 2160 })).toBe(true)
  })

  it('keeps the macOS Menu material applied across focus changes', async () => {
    const window = effectWindow()
    expect(await syncChatWindowEffect(window, 'macos', true, { width: 1280, height: 800 }, true)).toBe(true)
    expect(window.setEffects).toHaveBeenCalledWith({
      effects: [Effect.Menu],
      state: EffectState.FollowsWindowActiveState,
    })
    // 焦点交给 FollowsWindowActiveState：失焦不得 clear，否则切窗动画中途拆装
    // NSVisualEffectView，台前调度下肉眼掉帧。
    expect(window.clearEffects).not.toHaveBeenCalled()
  })

  it('clears the macOS effect when applying it throws', async () => {
    const window = effectWindow()
    window.setEffects.mockRejectedValue(new Error('unsupported'))
    expect(await syncChatWindowEffect(window, 'macos', true, { width: 1280, height: 800 }, true)).toBe(false)
    expect(window.clearEffects).toHaveBeenCalledTimes(1)
  })

  it('applies Windows Mica through the backend command', async () => {
    const window = effectWindow()
    const applyMica = mica(true)
    expect(await syncChatWindowEffect(window, 'windows', true, { width: 1280, height: 800 }, true, applyMica)).toBe(true)
    expect(applyMica).toHaveBeenCalledWith(true)
    // setEffects 不能用：tauri 会吞掉 apply_mica 的失败，Win10 上也 resolve。
    expect(window.setEffects).not.toHaveBeenCalled()
    expect(window.clearEffects).not.toHaveBeenCalled()
  })

  // 裸 Mica 跟系统主题：亮色系统 + 暗色应用会从卡片缝里透出白条，所以必须显式选变体。
  it('picks the Mica variant from the app theme, not the system theme', async () => {
    const applyMica = mica(true)
    await syncChatWindowEffect(effectWindow(), 'windows', true, { width: 1280, height: 800 }, false, applyMica)
    expect(applyMica).toHaveBeenCalledWith(false)
  })

  it('clears effects when disabled or unsupported', async () => {
    const disabledWindow = effectWindow()
    const applyMica = mica(true)
    expect(await syncChatWindowEffect(disabledWindow, 'windows', false, { width: 1280, height: 800 }, true, applyMica)).toBe(false)
    expect(applyMica).not.toHaveBeenCalled()
    expect(disabledWindow.clearEffects).toHaveBeenCalledTimes(1)

    const linuxWindow = effectWindow()
    expect(await syncChatWindowEffect(linuxWindow, 'linux', true, { width: 1280, height: 800 }, true)).toBe(false)
    expect(linuxWindow.setEffects).not.toHaveBeenCalled()
    expect(linuxWindow.clearEffects).not.toHaveBeenCalled()
  })

  // Win10 没有 Mica → 后端返回 false → 外壳必须回到不透明，否则透明窗口透出桌面。
  it('falls back to the opaque shell when Mica did not apply', async () => {
    const window = effectWindow()
    expect(await syncChatWindowEffect(window, 'windows', true, { width: 1280, height: 800 }, true, mica(false))).toBe(false)
    expect(window.clearEffects).toHaveBeenCalledTimes(1)

    const rejected = effectWindow()
    const failing = vi.fn().mockRejectedValue(new Error('ipc failed'))
    expect(await syncChatWindowEffect(rejected, 'windows', true, { width: 1280, height: 800 }, true, failing)).toBe(false)
    expect(rejected.clearEffects).toHaveBeenCalledTimes(1)
  })
})
