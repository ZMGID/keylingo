import { useRef, type ReactNode } from 'react'

interface ChatRouteKeepAliveProps {
  activeKey: string
  children: ReactNode
}
/**
 * 保留聊天/设置两棵大树的挂载状态，切换页面只切换可见性。
 *
 * 聊天历史和设置页都包含较大的 React/Markdown 子树。路由分支直接互斥渲染时，
 * 从设置返回聊天会让 MessageList 从零挂载；这里把这两个稳定路由缓存为同一个 key，
 * 让 React 复用原实例。非聊天中心页不缓存，避免把多个中心页的大树长期留在内存里。
 */
export function ChatRouteKeepAlive({ activeKey, children }: ChatRouteKeepAliveProps) {
  const cacheRef = useRef(new Map<string, ReactNode>())
  const keep = activeKey === 'conversation' || activeKey === 'settings'
  if (keep) cacheRef.current.set(activeKey, children)

  return (
    <div className="contents">
      {[...cacheRef.current.entries()].map(([key, node]) => (
        <div
          key={key}
          style={{ display: key === activeKey ? 'contents' : 'none' }}
          aria-hidden={key === activeKey ? undefined : true}
        >
          {node}
        </div>
      ))}
      {!keep && <div style={{ display: 'contents' }}>{children}</div>}
    </div>
  )
}
