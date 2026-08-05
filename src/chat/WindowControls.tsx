import { useCallback } from 'react'
import { api } from '../api/tauri'
import { useT } from '../settings/i18n'
import { isMac } from './platform'

type TrafficButton = 'close' | 'minimize' | 'maximize'

const trafficColors: Record<TrafficButton, string> = {
  close: '#ff5f57',
  minimize: '#febc2e',
  maximize: '#28c840',
}

/**
 * Windows caption 图标：10×10 viewBox 内自绘，笔画恒为 1px。
 *
 * 刻意不用 lucide：lucide 的 strokeWidth 是 24-viewBox 内的比例，缩到 ~11px 后实际笔画
 * 只剩 0.6px，抗锯齿会把它糊成一条浅灰虚影；且 lucide `Square` 自带 rx=2 圆角，
 * 而原生最大化键是直角方框。
 *
 * ─ 用 crispEdges 贴整像素（`y=5.5` 是整数对齐的，缩放下不会掉）；× 必须
 * geometricPrecision —— 对角线 + crispEdges 会把两臂锯成不对称像素块，且贴边 path 的半笔
 * 会被 viewBox 裁掉，看起来歪。□ 用填充而非描边，理由见下方注释。
 */
function CaptionIcon({ kind }: { kind: TrafficButton }) {
  if (kind === 'close') {
    return (
      <svg
        width="10"
        height="10"
        viewBox="0 0 10 10"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.1"
        strokeLinecap="square"
        shapeRendering="geometricPrecision"
        aria-hidden
      >
        {/* 内收 1.5：整笔都在 viewBox 内，十字中心落在 (5,5) */}
        <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" />
      </svg>
    )
  }

  return (
    <svg
      width="10"
      height="10"
      viewBox="0 0 10 10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1"
      shapeRendering="crispEdges"
      aria-hidden
    >
      {kind === 'minimize' && <path d="M0 5.5h10" />}
      {kind === 'maximize' && (
        /*
         * 用「填充的方框」而不是描边矩形。
         *
         * 描边是以路径为中心向两侧各摊半个笔宽的，在 125% / 150% 这类**非整数显示缩放**下，
         * `crispEdges` 会把某一侧摊到 0 个设备像素——那条边整条消失，实测表现为只剩上、右
         * 两条边的「⌐」形。填充几何全是整数坐标，四条边在任何缩放下都在。
         *
         * `stroke="none"` 是必须的：外层 svg 上的 stroke/strokeWidth 会被继承，
         * 不关掉就会在填充框外再描一圈，方框变粗一倍。
         */
        <path
          d="M0 0h10v10H0V0zm1 1v8h8V1H1z"
          fill="currentColor"
          fillRule="evenodd"
          stroke="none"
        />
      )}
    </svg>
  )
}

export function WindowControls() {
  const t = useT()
  const handleClose = useCallback(() => {
    void api.closeWindow()
  }, [])

  const handleMinimize = useCallback(() => {
    void api.minimizeWindow()
  }, [])

  const handleMaximize = useCallback(() => {
    void api.toggleMaximizeWindow()
  }, [])

  if (!isMac) {
    return (
      <div className="chat-win-controls chat-win-controls--win" data-tauri-drag-region="false">
        <button type="button" className="chat-win-btn" onClick={handleMinimize} aria-label={t.chatWinMinimize}>
          <CaptionIcon kind="minimize" />
        </button>
        <button type="button" className="chat-win-btn" onClick={handleMaximize} aria-label={t.chatWinMaximize}>
          <CaptionIcon kind="maximize" />
        </button>
        <button
          type="button"
          className="chat-win-btn chat-win-btn--close"
          onClick={handleClose}
          aria-label={t.chatWinClose}
        >
          <CaptionIcon kind="close" />
        </button>
      </div>
    )
  }

  return (
    <div className="chat-traffic" data-tauri-drag-region="false">
      {(['close', 'minimize', 'maximize'] as TrafficButton[]).map((kind) => (
        <button
          key={kind}
          type="button"
          className={`chat-traffic-dot chat-traffic-dot--${kind}`}
          style={{ ['--dot-color' as string]: trafficColors[kind] }}
          onClick={
            kind === 'close'
              ? handleClose
              : kind === 'minimize'
                ? handleMinimize
                : handleMaximize
          }
          aria-label={
            kind === 'close'
              ? t.chatWinClose
              : kind === 'minimize'
                ? t.chatWinMinimize
                : t.chatWinMaximize
          }
        />
      ))}
    </div>
  )
}
