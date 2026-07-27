/** data URL → 裸 base64 载荷。非 base64 data URL（远程图、`data:image/svg+xml,<svg…>`）
 *  返回空串，调用方据此禁用「复制图片 / 另存为」。 */
export function base64FromDataUrl(src: string): string {
  if (!src.startsWith('data:')) return ''
  const comma = src.indexOf(',')
  if (comma < 0) return ''
  if (!src.slice(0, comma).includes(';base64')) return ''
  return src.slice(comma + 1)
}

/** 另存为的默认扩展名：优先文件名后缀，其次按 data URL 的 mime 推，兜底 png。 */
export function imageExtension(src: string, fallbackName?: string): string {
  const fromName = /\.([a-z0-9]+)$/i.exec(fallbackName ?? '')?.[1]?.toLowerCase()
  if (fromName) return fromName
  const mime = /^data:([^;,]+)/.exec(src)?.[1] ?? ''
  if (mime === 'image/jpeg') return 'jpg'
  if (mime === 'image/webp') return 'webp'
  if (mime === 'image/gif') return 'gif'
  return 'png'
}
