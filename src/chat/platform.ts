export const isMac =
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad|iPod/i.test(navigator.userAgent)

export const isWindows =
  typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)

/** macOS Chat 使用 Tauri Overlay 标题栏，交通灯由系统绘制 */
export const usesNativeTitlebar = isMac

/** 侧栏 / 主内容顶栏行高与垂直居中 */
export const chatTitlebarRowClass = usesNativeTitlebar
  ? 'flex h-[52px] shrink-0 items-center gap-2'
  : 'flex h-[52px] shrink-0 items-center gap-2 px-3 pt-2'

/**
 * 窗口左缘交通灯留白（仅侧栏顶栏、收起态主顶栏；约 66px 灯区 + 间距）。
 * 侧栏顶栏在悬浮卡片内部，坐标原点已随卡片右移 8px，而灯的 x 也 +8（windows.rs），
 * 两者抵消 → 这里保持 92px。收起态主顶栏在主区卡片内，而收起时主区卡片左缘同样在 x=8
 * （`.chat-sidebar-shell.is-collapsed` 用负侧栏宽留出那道左缝），所以收起态也是 92px，
 * `.chat-titlebar-row--collapsed-mac` 不再补 8px。
 * 垂直：不在这里定 —— 整条顶栏线对到 `--chat-traffic-center-y`（交通灯中心的实测值，
 * 由 ChatWindowHost 量出来写进 documentElement），见 index.css。
 */
export const chatTitlebarMacInsetClass = usesNativeTitlebar ? 'pl-[92px]' : ''

/**
 * 顶栏幽灵控件统一规格（去胶囊化方向 A）：
 * - 默认透明，无边框 / 无阴影；hover 才出浅背景（与图标钮 hover-bg 同语汇）。
 * - 统一高度 32px（h-8）、统一圆角 rounded-lg，所有触发控件对齐到同一视觉规格。
 * - 沿用 active:scale 按压反馈 + --kv-* 令牌；reduced-motion 靠 index.css 末尾全局兜底。
 */
export const chatTitlebarGhostHoverClass =
  'hover:bg-black/[0.05] dark:hover:bg-white/[0.07]'

/** 文字 / 复合触发控件（模型选择、侧栏操作组容器等）。 */
export const chatTitlebarPillButtonClass = [
  'chat-titlebar-pill',
  'inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-transparent bg-transparent px-2.5 text-sm transition duration-[var(--kv-dur-instant)] active:scale-[0.97]',
  chatTitlebarGhostHoverClass,
].join(' ')

/** 纯图标触发控件（Runtime / Permission 等），与 pill 同高、同圆角的方形钮。 */
export const chatTitlebarIconButtonClass = [
  'chat-titlebar-pill',
  'flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-transparent bg-transparent text-neutral-600 transition duration-[var(--kv-dur-instant)] active:scale-[0.97] dark:text-neutral-400',
  chatTitlebarGhostHoverClass,
].join(' ')

/** 复合控件内的次级图标按钮（如侧栏操作组里的切栏 / 新建）。 */
export const chatTitlebarPillIconClass =
  'chat-titlebar-pill-icon flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-neutral-600 transition duration-[var(--kv-dur-instant)] hover:bg-black/[0.05] hover:text-neutral-900 active:scale-90 dark:text-neutral-400 dark:hover:bg-white/[0.08] dark:hover:text-neutral-100'
