import { useCallback } from 'react'
import { api } from '../api/tauri'
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
 * ─ / □ 用 crispEdges 贴整像素；× 必须 geometricPrecision —— 对角线 + crispEdges
 * 会把两臂锯成不对称像素块，且贴边 path 的半笔会被 viewBox 裁掉，看起来歪。
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
      {kind === 'maximize' && <rect x="0.5" y="0.5" width="9" height="9" />}
    </svg>
  )
}

export function WindowControls() {
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
        <button type="button" className="chat-win-btn" onClick={handleMinimize} aria-label="最小化">
          <CaptionIcon kind="minimize" />
        </button>
        <button type="button" className="chat-win-btn" onClick={handleMaximize} aria-label="最大化">
          <CaptionIcon kind="maximize" />
        </button>
        <button
          type="button"
          className="chat-win-btn chat-win-btn--close"
          onClick={handleClose}
          aria-label="关闭"
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
          aria-label={kind === 'close' ? '关闭' : kind === 'minimize' ? '最小化' : '最大化'}
        />
      ))}
    </div>
  )
}
