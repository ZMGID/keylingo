/**
 * 记忆层的共享常量与工具。
 *
 * 单列一个文件而非从 SettingsShell 导出：组件文件里混着导出常量/函数会破坏
 * React Fast Refresh（react-refresh/only-export-components）。
 */

export type MemoryLayerKey = 'l1' | 'l2'

export const MEMORY_L1_MAX_BYTES = 5_000

const textEncoder = new TextEncoder()

export function utf8ByteLength(value: string): number {
  return textEncoder.encode(value).length
}
