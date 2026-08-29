// Chat 工具函数
export { isTauriRuntime } from '../api/tauri'

/** 用户是否偏好减少动画 */
export const prefersReducedMotion = (): boolean =>
  typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
