import { type ModelProvider } from '../api/tauri'
import { i18n, type Lang } from './i18n'

export type Platform = 'macos' | 'windows' | 'linux'

export type SelectOption = {
  value: string
  label: string
  title?: string
}

// 修饰键集合（录制快捷键时忽略）
const modifierKeys = new Set(['Shift', 'Meta', 'Control', 'Alt', 'AltGraph'])

// 键盘按键别名映射
const keyAliasMap: Record<string, string> = {
  Escape: 'Esc',
  ' ': 'Space',
  Spacebar: 'Space',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
}

/**
 * 从键盘 code 提取字母/数字键值
 */
const normalizeKeyFromCode = (code: string) => {
  if (code.startsWith('Key')) return code.slice(3)
  if (code.startsWith('Digit')) return code.slice(5)
  return ''
}

/**
 * 将键盘事件转换为快捷键字符串
 */
export const normalizeHotkeyKey = (event: KeyboardEvent) => {
  const { key, code } = event
  if (!key) return ''
  if (modifierKeys.has(key)) return ''
  if (/^F\d{1,2}$/.test(key)) return key.toUpperCase()
  const alias = keyAliasMap[key]
  if (alias) return alias
  const fromCode = normalizeKeyFromCode(code)
  if (fromCode) return fromCode.toUpperCase()
  if (key === 'Dead' || key === 'Process') return ''
  if (key.length === 1 && key !== '+') return key.toUpperCase()
  return ''
}

/**
 * 构建完整的快捷键字符串（如 CommandOrControl+Alt+T）
 */
export const buildHotkey = (event: KeyboardEvent) => {
  const key = normalizeHotkeyKey(event)
  if (!key) return ''
  const parts: string[] = []
  if (event.metaKey || event.ctrlKey) parts.push('CommandOrControl')
  if (event.altKey || event.getModifierState('AltGraph')) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')
  parts.push(key)
  return parts.join('+')
}

/**
 * 平台检测（用于快捷键可视化）
 */
export const getPlatform = (): Platform => {
  if (navigator.platform.startsWith('Mac')) return 'macos'
  if (navigator.platform.startsWith('Win')) return 'windows'
  return 'linux'
}

/**
 * 将快捷键字符串解析为可视化按键数组
 */
export const formatHotkey = (hotkey: string, platform: 'macos' | 'windows' | 'linux'): string[] => {
  const parts = hotkey.split('+')
  return parts.map((part) => {
    switch (part) {
      case 'CommandOrControl':
        return platform === 'macos' ? '⌘' : 'Ctrl'
      case 'Command':
        return '⌘'
      case 'Control':
        return 'Ctrl'
      case 'Alt':
        return platform === 'macos' ? '⌥' : 'Alt'
      case 'Shift':
        return platform === 'macos' ? '⇧' : 'Shift'
      case 'Escape':
        return 'Esc'
      case 'Space':
        return 'Space'
      case 'ArrowUp':
        return '↑'
      case 'ArrowDown':
        return '↓'
      case 'ArrowLeft':
        return '←'
      case 'ArrowRight':
        return '→'
      default:
        return part.length === 1 ? part.toUpperCase() : part
    }
  })
}

export const modelPairValue = (providerId: string, model: string) =>
  JSON.stringify([providerId, model])

export const parseModelPairValue = (value: string): [string, string] => {
  try {
    const parsed = JSON.parse(value)
    if (Array.isArray(parsed) && parsed.length >= 2) {
      return [String(parsed[0] || ''), String(parsed[1] || '')]
    }
  } catch {
    // 兼容旧版本用 "provider:model" 拼接的下拉值。
  }
  const separator = value.indexOf(':')
  if (separator < 0) return [value, '']
  return [value.slice(0, separator), value.slice(separator + 1)]
}

export const isProviderEnabled = (provider: ModelProvider) => provider.enabled !== false

export const buildModelPairOptions = (
  providers: ModelProvider[],
  filterModel?: (provider: ModelProvider, model: string) => boolean,
): SelectOption[] =>
  providers
    .filter(provider => isProviderEnabled(provider))
    .flatMap(provider =>
      provider.enabledModels
        .filter(model => !filterModel || filterModel(provider, model))
        .map(model => ({
          value: modelPairValue(provider.id, model),
          label: `${provider.name} - ${model}`,
          title: `${provider.name} - ${model}`,
        })),
    )

/**
 * 与 JSON.stringify 等价但对对象 key 做递归排序,用于 dirty diff:
 * 后端 sanitize 与前端 spread 都可能改变字段顺序,普通 JSON.stringify 会
 * 把"语义无差异、字段顺序不同"误判为脏。数组顺序保留(数组顺序在 settings
 * 里语义上是有意义的,如 apiKeys 的 primary/backup 顺序)。
 */
export const stableStringify = (value: unknown): string =>
  JSON.stringify(value, (_key, v) => {
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      const sorted: Record<string, unknown> = {}
      for (const k of Object.keys(v as Record<string, unknown>).sort()) {
        sorted[k] = (v as Record<string, unknown>)[k]
      }
      return sorted
    }
    return v
  })

/**
 * 自动保存回包怎么盖回草稿。
 *
 * `save_settings` 会跑 `sanitize_settings`，丢掉尚未填完的占位行（空 API Key、
 * 空自定义请求头、空 CLI 模型 id、空 env key……）。若把回包整份 setState 回去，
 * 刚点「添加」出来的输入框会秒消失。按字段拦自动保存治标不治本——每多一种
 * 「先插空行再填」的 UI 就要再加一条白名单。
 *
 * 规则：
 * - 用户在飞行中继续改了 → 绝不盖草稿；基线推进到已落盘回包。
 * - 回包与送出的草稿相同 → 可以盖（sanitize 无实质改动）。
 * - 回包改了草稿 → 屏幕上的草稿保留；基线也钉在草稿上，避免立刻再存一遍
 *   （再存仍会被丢掉，空行又被回包盖没）。
 */
export function resolveSettingsSaveEcho(
  draftSnapshot: string,
  savedSnapshot: string,
  latestSnapshot: string,
): { applySaved: boolean; baselineSnapshot: string } {
  if (latestSnapshot !== draftSnapshot) {
    return { applySaved: false, baselineSnapshot: savedSnapshot }
  }
  if (savedSnapshot === draftSnapshot) {
    return { applySaved: true, baselineSnapshot: savedSnapshot }
  }
  return { applySaved: false, baselineSnapshot: draftSnapshot }
}

type HotkeyErrorPayload = {
  kind: 'conflict' | 'duplicate' | 'empty' | 'other'
  scope:
    | 'translator'
    | 'chat'
    | 'close_chat'
    | 'screenshot'
    | 'screenshot_text'
    | 'screenshot_replace'
    | 'screenshot_annotate'
    | 'lens'
  hotkey: string
  raw?: string
}

const SCOPE_KEY: Record<HotkeyErrorPayload['scope'], keyof typeof i18n.zh> = {
  translator: 'hotkeyScopeTranslator',
  chat: 'hotkeyScopeChat',
  close_chat: 'hotkeyScopeCloseChat',
  screenshot: 'hotkeyScopeScreenshot',
  screenshot_text: 'hotkeyScopeScreenshotText',
  screenshot_replace: 'hotkeyScopeScreenshotReplace',
  screenshot_annotate: 'annotateHotkeyLabel',
  lens: 'hotkeyScopeLens',
}

const KIND_KEY: Record<HotkeyErrorPayload['kind'], keyof typeof i18n.zh> = {
  conflict: 'hotkeyErrorConflict',
  duplicate: 'hotkeyErrorDuplicate',
  empty: 'hotkeyErrorEmpty',
  other: 'hotkeyErrorOther',
}

/**
 * 把后端 register_hotkeys 抛出的 JSON 错误数组翻译成用户语言的可读消息。
 * 解析失败(普通字符串错误)时原样返回,保证所有非热键错误也能正常显示。
 */
export const formatHotkeyError = (raw: string, lang: Lang): string => {
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return raw
  }
  if (!Array.isArray(parsed) || parsed.length === 0) return raw
  const table = i18n[lang]
  const messages: string[] = []
  for (const item of parsed) {
    if (
      !item ||
      typeof item !== 'object' ||
      !(SCOPE_KEY as Record<string, unknown>)[(item as HotkeyErrorPayload).scope] ||
      !(KIND_KEY as Record<string, unknown>)[(item as HotkeyErrorPayload).kind]
    ) {
      return raw
    }
    const e = item as HotkeyErrorPayload
    const scope = table[SCOPE_KEY[e.scope]]
    const template = table[KIND_KEY[e.kind]]
    messages.push(
      template
        .replace('{scope}', scope)
        .replace('{hotkey}', e.hotkey)
        .replace('{raw}', e.raw ?? ''),
    )
  }
  return messages.join(' / ')
}
