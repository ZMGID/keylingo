import { describe, expect, it } from 'vitest'
import { render, screen } from '@testing-library/react'
import { LangContext, i18n } from '../settings/i18n'
import { ChatTitlebarActions } from './ChatTitlebarActions'

describe('chat shell i18n', () => {
  // 两侧键必须一对一：只往 zh 加键、忘了加 en 时，英文界面会渲染 undefined 而 tsc 抓不到
  // （`typeof i18n[Lang]` 是两者的并集类型，缺键不报错）。
  it('zh and en tables have the same keys', () => {
    expect(Object.keys(i18n.en).sort()).toEqual(Object.keys(i18n.zh).sort())
  })

  // LangContext 是外壳里二十来个组件唯一的取文案入口。Provider 一断（例如有人在 Chat.tsx
  // 里重排 JSX 时把它挪走），组件会静默退回默认 'zh'，界面看起来「语言开关没生效」。
  it('reaches shell components through LangContext', () => {
    const { unmount } = render(
      <LangContext.Provider value="en">
        <ChatTitlebarActions
          sidebarExpanded
          onToggleSidebar={() => {}}
          onNewConversation={() => {}}
        />
      </LangContext.Provider>,
    )
    expect(screen.getByTitle(i18n.en.chatNewChat)).toBeInTheDocument()
    unmount()

    render(
      <ChatTitlebarActions sidebarExpanded onToggleSidebar={() => {}} onNewConversation={() => {}} />,
    )
    expect(screen.getByTitle(i18n.zh.chatNewChat)).toBeInTheDocument()
  })
})
