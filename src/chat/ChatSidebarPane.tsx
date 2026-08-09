import { memo, Profiler, type ProfilerOnRenderCallback } from 'react'
import { Sidebar, type SidebarProps } from './Sidebar'

export interface ChatSidebarPaneProps extends SidebarProps {
  onRender: ProfilerOnRenderCallback
}

/**
 * 侧栏的 React 子树边界。
 *
 * Chat 仍负责协调路由和会话，但侧栏自己的状态/数据更新不应该把聊天主区
 * 一起带进协调。memo 让侧栏只在真正的侧栏输入变化时重渲染，Profiler 也
 * 留在这个边界内，便于分别观察侧栏和主区的成本。
 */
export const ChatSidebarPane = memo(function ChatSidebarPane({ onRender, ...props }: ChatSidebarPaneProps) {
  return (
    <Profiler id="Sidebar" onRender={onRender}>
      <Sidebar {...props} />
    </Profiler>
  )
})
