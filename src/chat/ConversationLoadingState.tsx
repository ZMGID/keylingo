import { memo } from 'react'
import { StreamStatusLogo } from './StreamStatusLine'

/** 切换重型会话时的覆盖加载态：旧消息树保留在后台，避免点击侧栏时同步卸载。 */
export const ConversationLoadingState = memo(function ConversationLoadingState() {
  return (
    <div
      className="chat-conversation-loading absolute inset-0 z-30 flex items-center justify-center"
      role="status"
      aria-label="正在加载对话"
    >
      <StreamStatusLogo size={104} />
      <span className="sr-only">正在加载对话…</span>
    </div>
  )
})
