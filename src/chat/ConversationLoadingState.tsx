import { memo, useEffect, useState } from 'react'
import { StreamDotLogo } from './StreamDotLogo'

/** 被预测为「小会话」但迟迟没揭开时，多久后把 Logo 补上。 */
const DELAYED_LOGO_MS = 150

/**
 * 会话切换覆盖层：无论是否显示 Logo，都要挡住尚未完成 Markdown/高度测量的正文。
 * showAnimation=true（消息数未知或 >12）立即播 Logo；false 时不能就此不放——
 * 代理会话把整轮 run 塞进两三条消息里，message_count 极小但内容极重，
 * 纯背景一盖几秒毫无反馈就是「切换卡死」的观感。所以 150ms 内没揭开就补 Logo：
 * 真正的轻会话在这之前已经 settle 卸载了本组件，不会闪。
 */
export const ConversationLoadingState = memo(function ConversationLoadingState({
  showAnimation,
}: {
  showAnimation: boolean
}) {
  const [delayedShow, setDelayedShow] = useState(false)
  useEffect(() => {
    if (showAnimation) return
    const timer = window.setTimeout(() => setDelayedShow(true), DELAYED_LOGO_MS)
    return () => window.clearTimeout(timer)
  }, [showAnimation])
  return (
    <div
      className="chat-conversation-loading absolute inset-0 z-30 flex items-center justify-center"
      role="status"
      aria-label="正在加载对话"
    >
      {(showAnimation || delayedShow) && <StreamDotLogo size={104} />}
      <span className="sr-only">正在加载对话…</span>
    </div>
  )
})
