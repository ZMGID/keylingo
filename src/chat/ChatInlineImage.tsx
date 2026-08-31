import { useCallback, useState } from 'react'
import { ChatImageContextMenu, type ChatImageMenuAnchor } from './ChatImageContextMenu'

/** 聊天区预览磁贴的最长边。点开查看器仍是原图。 */
export const CHAT_IMAGE_TILE_MAX_PX = 128

const IMAGE_CLASS =
  'rounded-md border border-neutral-200/90 bg-white object-contain dark:border-neutral-700 dark:bg-neutral-900'

/**
 * 已知宽高比缓存。虚拟列表会卸载滚出视口的行，组件 state 随之丢失——若不缓存，滚回来
 * 又是「默认方块 → 解码后改比例」，跳动照旧。key 不用整个 src（data URL 可达数 MB，会把字节
 * 钉在内存里），只取长度 + 尾部片段，足够区分且不持有原串。
 * ponytail: 上限 512 条后清空重来，聊天区图片数远达不到，不值得写 LRU。
 */
const ratioCache = new Map<string, number>()
const RATIO_CACHE_MAX = 512

function ratioKey(src: string): string {
  return `${src.length}:${src.slice(-64)}`
}

function rememberRatio(src: string, ratio: number) {
  if (ratioCache.size >= RATIO_CACHE_MAX) ratioCache.clear()
  ratioCache.set(ratioKey(src), ratio)
}

function tileWidthPx(ratio: number): number {
  return Math.max(1, Math.round(Math.min(CHAT_IMAGE_TILE_MAX_PX, CHAT_IMAGE_TILE_MAX_PX * ratio)))
}

/**
 * 聊天区内联图片。两条渲染路径（markdown 内嵌图 / 生成图画廊）共用，除了右键菜单，
 * 关键职责是**先占位再解码**：
 *
 * 虚拟列表按行实测高度定位。图片解码前若高度为 0，解码后才撑开——这一
 * 次「事后长高」会让 virtualizer 重测已定位的行。
 *
 * 解法是让盒子尺寸只由**宽高比**决定：最长边恒 ≤240，多张才能在一行里并排。
 * 未知比例先按 1:1 占位，避免解码前撑满整行、把后面的图挤下去。
 */
export function ChatInlineImage({
  src,
  alt,
  name,
  onOpenViewer,
  className = '',
}: {
  src: string
  alt: string
  name?: string
  onOpenViewer?: () => void
  /** 外层按钮的附加类（如 markdown 图的外边距）。 */
  className?: string
}) {
  const [menuAnchor, setMenuAnchor] = useState<ChatImageMenuAnchor | null>(null)
  const [ratio, setRatio] = useState<number>(() => ratioCache.get(ratioKey(src)) ?? 1)

  const handleLoad = useCallback((event: React.SyntheticEvent<HTMLImageElement>) => {
    const { naturalWidth, naturalHeight, currentSrc } = event.currentTarget
    if (!naturalWidth || !naturalHeight) return
    const next = naturalWidth / naturalHeight
    rememberRatio(src, next)
    if (currentSrc && currentSrc !== src) rememberRatio(currentSrc, next)
    setRatio(next)
  }, [src])

  return (
    <>
      <button
        type="button"
        data-chat-inline-image=""
        className={`inline-block max-w-full min-w-0 cursor-zoom-in overflow-hidden rounded-md p-0 text-left align-top ${className}`}
        style={{
          aspectRatio: String(ratio),
          width: tileWidthPx(ratio),
          maxWidth: '100%',
        }}
        onClick={onOpenViewer}
        onContextMenu={(event) => {
          event.preventDefault()
          // 必须掐断冒泡：滚动容器上还挂着消息级右键菜单（MessageList.handleContextMenu），
          // 图片在消息内 ⇒ 它也会命中并开一个「复制整条消息」菜单，且 portal 挂得更晚、
          // 盖在图片菜单上面 —— 表现就是右键图片弹出来的是消息菜单。
          event.stopPropagation()
          setMenuAnchor({ left: event.clientX, top: event.clientY })
        }}
        aria-label="预览图片"
      >
        <img
          src={src}
          alt={alt}
          onLoad={handleLoad}
          loading={src.startsWith('data:') ? undefined : 'lazy'}
          className={`h-full w-full min-w-0 max-w-full ${IMAGE_CLASS}`}
        />
      </button>
      {menuAnchor ? (
        <ChatImageContextMenu
          anchor={menuAnchor}
          src={src}
          name={name}
          onOpenViewer={onOpenViewer}
          onClose={() => setMenuAnchor(null)}
        />
      ) : null}
    </>
  )
}
