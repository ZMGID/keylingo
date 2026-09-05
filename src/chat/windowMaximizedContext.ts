import { createContext } from 'react'

/** 由窗口外壳同步原生状态，标题栏和外壳共用同一份最大化状态。 */
export const WindowMaximizedContext = createContext(false)
