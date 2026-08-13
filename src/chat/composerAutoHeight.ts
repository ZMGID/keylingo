export const COMPOSER_MAX_HEIGHT_PX = 160
export const COMPOSER_MIN_HEIGHT_PX = 28

function supportsFieldSizing(): boolean {
  return typeof CSS !== 'undefined'
    && typeof CSS.supports === 'function'
    && CSS.supports('field-sizing', 'content')
}

function lockTarget(textarea: HTMLTextAreaElement): HTMLElement | null {
  const found = textarea.closest('.chat-composer-footer')
    ?? textarea.closest('[data-chat-composer]')
    ?? textarea.parentElement
  return found instanceof HTMLElement ? found : null
}

/**
 * 输入框自适应高度。量高若把 height 塌成 auto/0 并让这次塌陷参与整列 flex 布局，
 * 贴底的聊天视口会瞬间变高、max scrollTop 被钳掉，跟随纠正器再钉回 —— 两行以上
 * 草稿每敲一字整屏抖。锁的是视口的 flex 兄弟（footer / composer shell），不是
 * textarea 里层包装；能长高就不塌；终值没变就不写 height。
 */
export function applyComposerAutoHeight(textarea: HTMLTextAreaElement) {
  if (supportsFieldSizing()) {
    textarea.style.overflowY = textarea.scrollHeight > COMPOSER_MAX_HEIGHT_PX ? 'auto' : 'hidden'
    return
  }

  const current = textarea.offsetHeight
  const overflowing = textarea.scrollHeight
  if (overflowing > current + 1) {
    const next = Math.min(overflowing, COMPOSER_MAX_HEIGHT_PX)
    if (Math.abs(next - current) > 0.5) textarea.style.height = `${next}px`
    textarea.style.overflowY = overflowing > COMPOSER_MAX_HEIGHT_PX ? 'auto' : 'hidden'
    return
  }

  const row = lockTarget(textarea)
  const prevMinHeight = row?.style.minHeight ?? ''
  if (row) row.style.minHeight = `${row.offsetHeight}px`
  textarea.style.height = '0px'
  const measured = textarea.scrollHeight
  const next = Math.min(Math.max(measured, COMPOSER_MIN_HEIGHT_PX), COMPOSER_MAX_HEIGHT_PX)
  textarea.style.height = `${next}px`
  textarea.style.overflowY = measured > COMPOSER_MAX_HEIGHT_PX ? 'auto' : 'hidden'
  if (row) row.style.minHeight = prevMinHeight
}
