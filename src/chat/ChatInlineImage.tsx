import { useCallback, useState } from 'react'
import { ChatImageContextMenu, type ChatImageMenuAnchor } from './ChatImageContextMenu'

/** 聊天区内联图的最大显示高度（与旧的 max-h-[420px] 一致）。 */
const MAX_DISPLAY_HEIGHT_PX = 420
const UNKNOWN_IMAGE_MIN_HEIGHT_PX = 180

const IMAGE_CLASS =
  'rounded-md border border-neutral-200/90 bg-white object-contain dark:border-neutral-700 dark:bg-neutral-900'

/**
 * 已知宽高比缓存。虚拟列表会卸载滚出视口的行，组件 state 随之丢失——若不缓存，滚回来
 * 又是「0 高 → 解码后长高」，跳动照旧。key 不用整个 src（data URL 可达数 MB，会把字节
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

/**
 * 聊天区内联图片。两条渲染路径（markdown 内嵌图 / 生成图画廊）共用，除了右键菜单，
 * 关键职责是**先占位再解码**：
 *
 * 虚拟列表按行实测高度定位。图片解码前 <img> 高度为 0，解码后才撑开——这一
 * 次「事后长高」会让 virtualizer 重测已定位的行，表现为：滚到图片处猛地跳一段、拖到底部
 * 松手后弹回、切换会话时图片闪一下归位。
 *
 * 解法是让盒子尺寸只由**宽高比**决定，不依赖自然像素尺寸：
 * - 首次 onLoad 记下 ratio，之后盒子 = `aspect-ratio: ratio` + `width: min(100%, ratio*420px)`，
 *   高度恒 ≤420 且可预先算出，图片只是填进这个已定好的盒子。
 * - 缩略图（256px）与整图宽高比相同 ⇒ 懒加载整图替换 src 时盒子纹丝不动，不再闪。
 * - data URL 不加 `loading="lazy"`：字节已在内存里，懒加载省不了网络，只会把解码推迟到
 *   滚动到跟前，反而制造上面那次跳动。
 *
 * ponytail: ratio 靠首帧 onLoad 学，故**第一张**图仍有一帧校正。要彻底消除得让
 * artifact 自带 width/height（后端生成缩略图时顺手写入），仅新 artifact 受益，暂不做。
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
  /** 外层按钮的附加类（如 markdown 图的上下外边距）。 */
  className?: string
}) {
  const [menuAnchor, setMenuAnchor] = useState<ChatImageMenuAnchor | null>(null)
  // 初值走缓存：滚回来 / 切回会话时直接带着正确高度挂载，不再从 0 长起。
  const [ratio, setRatio] = useState<number | null>(() => ratioCache.get(ratioKey(src)) ?? null)

  const handleLoad = useCallback((event: React.SyntheticEvent<HTMLImageElement>) => {
    const { naturalWidth, naturalHeight, currentSrc } = event.currentTarget
    if (!naturalWidth || !naturalHeight) return
    const next = naturalWidth / naturalHeight
    // 缩略图与整图比例相同，两个 src 都记一份：懒加载换 src 后仍命中缓存，盒子不动。
    rememberRatio(src, next)
    if (currentSrc && currentSrc !== src) rememberRatio(currentSrc, next)
    // 缩略图→整图切换时比例通常不变；若服务端没有尺寸元数据，首帧占位
    // 使用默认比例，加载后再收敛到真实比例并交给 virtualizer 测量。
    setRatio(next)
  }, [src])

  const boxed = ratio != null
  return (
    <>
      <button
        type="button"
        className={`block cursor-zoom-in rounded-md p-0 text-left ${boxed ? '' : 'max-w-full'} ${className}`}
        style={
          boxed
            ? {
                aspectRatio: String(ratio),
                width: `min(100%, ${Math.round(ratio * MAX_DISPLAY_HEIGHT_PX)}px)`,
              }
            : { minHeight: UNKNOWN_IMAGE_MIN_HEIGHT_PX }
        }
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
          // 远程 URL 才值得懒加载；data URL 懒加载只会把解码推迟成滚动跳动。
          loading={src.startsWith('data:') ? undefined : 'lazy'}
          className={boxed ? `h-full w-full ${IMAGE_CLASS}` : `max-h-[420px] max-w-full ${IMAGE_CLASS}`}
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
