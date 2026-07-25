/**
 * 界面字号的取值范围。shell 的 commitUiFontPx 与外观设置里的 Input 都要用，
 * 单列一个模块避免从组件文件导出常量（会破坏 Fast Refresh）。
 */
export const UI_FONT_PX_MIN = 12
export const UI_FONT_PX_MAX = 19
