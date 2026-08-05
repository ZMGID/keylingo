import type { BarRect, Metrics } from './types'

export const READY_BAR_H = 56
export const ANCHOR_GAP = 12
export const DRAG_THRESHOLD = 5
export const TRANSITION_MS = 380
export const SELECT_REVEAL_DELAY_MS = 80
// 浮动卡四周留白：给 lens-floating-surface 的投影留出渲染空间。
// Windows 走 SetWindowRgn 把全屏覆盖裁成 card 矩形再外扩这个值，=0 时投影会被整圈裁掉。
export const FLOATING_PADDING = 24
export const FLOATING_GAP = 8

// macOS 上 lens_set_floating 走的是 set_position + set_size,会真的把 OS 窗口搬到 (x, y)。
// Windows 上走 SetWindowRgn 只裁剪可见区域,窗口本身始终全屏。
// 两边对 barRect 的语义因此不同:macOS rebase 后 barRect 必须是窗口内坐标 (0,0),
// Windows 仍然是屏幕(全屏窗口本地)坐标 (finalX, finalY)。
export const isMacPlatform = typeof navigator !== 'undefined' && /Mac|iPhone|iPad|iPod/i.test(navigator.userAgent)

export const clamp = (value: number, min: number, max: number) => Math.max(min, Math.min(max, value))

/** 多屏适配：基于当前 viewport 算"比例 + 上下限"，不同分辨率/屏幕大小都能落到舒适区间。 */
export const computeMetrics = (vw: number, vh: number): Metrics => ({
  READY_W: Math.round(Math.max(420, Math.min(560, vw * 0.34))),
  SELECT_W: Math.round(Math.max(440, Math.min(640, vw * 0.42))),
  ANSWER_H: Math.round(Math.max(220, Math.min(480, vh * 0.45))),
  SELECT_BOTTOM_OFFSET: Math.round(Math.max(80, Math.min(160, vh * 0.13))),
})

/** chat 模式截图后输入栏宽度：与 select 态一致，避免缩略图/应用名挤占后发送按钮溢出。 */
export const computeChatBarWidth = (m: Metrics) => m.SELECT_W

/** 计算 select 态对话栏在 webview 内的位置（webview 全屏，所以用 viewport 大小） */
export const computeSelectBar = (vw: number, vh: number, m: Metrics): BarRect => ({
  x: Math.round(vw / 2 - m.SELECT_W / 2),
  y: Math.round(vh - m.SELECT_BOTTOM_OFFSET - READY_BAR_H),
  width: m.SELECT_W,
})
