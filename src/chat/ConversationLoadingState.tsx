import { memo } from 'react'
import { StreamDotLogo } from './StreamDotLogo'

/**
 * 会话切换覆盖层：无论是否显示 Logo，都要挡住尚未完成 Markdown/高度测量的正文。
 * 小会话只用纯背景覆盖；预测为重会话时才播放 Logo（原来的点阵脉冲，不是墨团）。
 */
export const ConversationLoadingState = memo(function ConversationLoadingState({
  showAnimation,
}: {
  showAnimation: boolean
}) {
  return (
    <div
      className="chat-conversation-loading absolute inset-0 z-30 flex items-center justify-center"
      role="status"
      aria-label="正在加载对话"
    >
      {showAnimation && <StreamDotLogo size={104} />}
      <span className="sr-only">正在加载对话…</span>
    </div>
  )
})
