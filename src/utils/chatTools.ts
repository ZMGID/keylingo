import type { ChatNativeToolsConfig } from '../api/tauri'

const NATIVE_BUILTIN_TOOL_KEYS = [
  'webSearch',
  'webFetch',
  'readFile',
  'writeFile',
  'editFile',
  'runCommand',
] as const satisfies readonly (keyof ChatNativeToolsConfig)[]

export function hasEnabledNativeBuiltinTool(
  nativeTools?: Partial<ChatNativeToolsConfig> | null,
): boolean {
  if (!nativeTools) return false
  return NATIVE_BUILTIN_TOOL_KEYS.some((key) => nativeTools[key] === true)
}

export function hasEnabledSkillRuntime(
  nativeTools?: Partial<ChatNativeToolsConfig> | null,
): boolean {
  return nativeTools?.skillRuntime !== false
}
