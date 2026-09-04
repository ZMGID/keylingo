import { createContext } from 'react'

/** 加号菜单关掉自己。嵌在菜单里的来源 / 专家面板点选后调用。 */
export const ComposerAddMenuCloseContext = createContext<(() => void) | null>(null)
