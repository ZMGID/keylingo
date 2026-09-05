import { useCallback, useContext } from 'react'
import { Copy, Minus, Square, X } from 'lucide-react'
import { api } from '../api/tauri'
import { useT } from '../settings/i18n'
import { isMac } from './platform'
import { WindowMaximizedContext } from './windowMaximizedContext'

type TrafficButton = 'close' | 'minimize' | 'maximize'

const trafficColors: Record<TrafficButton, string> = {
  close: '#ff5f57',
  minimize: '#febc2e',
  maximize: '#28c840',
}

/** 方框的视觉面积更大，单独缩小以平衡三枚图标的视觉重量。 */
function CaptionIcon({ kind, maximized = false }: { kind: TrafficButton; maximized?: boolean }) {
  const Icon = kind === 'minimize' ? Minus : kind === 'maximize' ? (maximized ? Copy : Square) : X
  return <Icon size={kind === 'maximize' ? 13 : 16} strokeWidth={kind === 'maximize' ? 2 : 1.75} aria-hidden />
}

export function WindowControls() {
  const t = useT()
  const maximized = useContext(WindowMaximizedContext)
  const maximizeLabel = maximized ? t.chatWinRestore : t.chatWinMaximize
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
        <button type="button" className="chat-win-btn" onClick={handleMaximize} aria-label={maximizeLabel} title={maximizeLabel}>
          <CaptionIcon kind="maximize" maximized={maximized} />
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
