import { memo, useEffect, useState } from 'react'
import { getSnapshot } from './streamingStore'

// GRD-02 · 回波：从 logo-mark.png 在等距 3px 方格上采样（对齐原版密度语言：
// r=1.05 / step=3、28×28 正方形 viewBox、相位 = 距中心归一化距离，径向回波）。
// dangerouslySetInnerHTML 挂一次静态 DOM，不进 React 每帧 reconcile。
const LOGO_SVG = `<svg xmlns="http://www.w3.org/2000/svg" class="kv-stream-logo" viewBox="0 0 100 100" width="28" height="28"><g><circle class="dt" cx="57" cy="30" r="1.05" style="--p:0.557;--dx:0.33;--dy:-0.944"/><circle class="dt" cx="60" cy="30" r="1.05" style="--p:0.588;--dx:0.447;--dy:-0.894"/><circle class="dt" cx="63" cy="30" r="1.05" style="--p:0.627;--dx:0.545;--dy:-0.838"/><circle class="dt" cx="51" cy="33" r="1.05" style="--p:0.448;--dx:0.059;--dy:-0.998"/><circle class="dt" cx="54" cy="33" r="1.05" style="--p:0.459;--dx:0.229;--dy:-0.973"/><circle class="dt" cx="57" cy="33" r="1.05" style="--p:0.483;--dx:0.381;--dy:-0.925"/><circle class="dt" cx="60" cy="33" r="1.05" style="--p:0.518;--dx:0.507;--dy:-0.862"/><circle class="dt" cx="63" cy="33" r="1.05" style="--p:0.562;--dx:0.607;--dy:-0.794"/><circle class="dt" cx="66" cy="33" r="1.05" style="--p:0.613;--dx:0.685;--dy:-0.728"/><circle class="dt" cx="69" cy="33" r="1.05" style="--p:0.67;--dx:0.745;--dy:-0.667"/><circle class="dt" cx="45" cy="36" r="1.05" style="--p:0.391;--dx:-0.336;--dy:-0.942"/><circle class="dt" cx="48" cy="36" r="1.05" style="--p:0.372;--dx:-0.141;--dy:-0.99"/><circle class="dt" cx="51" cy="36" r="1.05" style="--p:0.369;--dx:0.071;--dy:-0.997"/><circle class="dt" cx="54" cy="36" r="1.05" style="--p:0.383;--dx:0.275;--dy:-0.962"/><circle class="dt" cx="57" cy="36" r="1.05" style="--p:0.411;--dx:0.447;--dy:-0.894"/><circle class="dt" cx="60" cy="36" r="1.05" style="--p:0.452;--dx:0.581;--dy:-0.814"/><circle class="dt" cx="63" cy="36" r="1.05" style="--p:0.502;--dx:0.68;--dy:-0.733"/><circle class="dt" cx="66" cy="36" r="1.05" style="--p:0.559;--dx:0.753;--dy:-0.659"/><circle class="dt" cx="69" cy="36" r="1.05" style="--p:0.62;--dx:0.805;--dy:-0.593"/><circle class="dt" cx="72" cy="36" r="1.05" style="--p:0.685;--dx:0.844;--dy:-0.537"/><circle class="dt" cx="75" cy="36" r="1.05" style="--p:0.753;--dx:0.873;--dy:-0.489"/><circle class="dt" cx="42" cy="39" r="1.05" style="--p:0.357;--dx:-0.588;--dy:-0.809"/><circle class="dt" cx="45" cy="39" r="1.05" style="--p:0.318;--dx:-0.414;--dy:-0.91"/><circle class="dt" cx="48" cy="39" r="1.05" style="--p:0.294;--dx:-0.179;--dy:-0.984"/><circle class="dt" cx="51" cy="39" r="1.05" style="--p:0.29;--dx:0.091;--dy:-0.996"/><circle class="dt" cx="54" cy="39" r="1.05" style="--p:0.308;--dx:0.342;--dy:-0.94"/><circle class="dt" cx="57" cy="39" r="1.05" style="--p:0.343;--dx:0.537;--dy:-0.844"/><circle class="dt" cx="60" cy="39" r="1.05" style="--p:0.391;--dx:0.673;--dy:-0.74"/><circle class="dt" cx="63" cy="39" r="1.05" style="--p:0.448;--dx:0.763;--dy:-0.646"/><circle class="dt" cx="66" cy="39" r="1.05" style="--p:0.51;--dx:0.824;--dy:-0.567"/><circle class="dt" cx="69" cy="39" r="1.05" style="--p:0.577;--dx:0.865;--dy:-0.501"/><circle class="dt" cx="72" cy="39" r="1.05" style="--p:0.646;--dx:0.894;--dy:-0.447"/><circle class="dt" cx="75" cy="39" r="1.05" style="--p:0.718;--dx:0.915;--dy:-0.403"/><circle class="dt" cx="78" cy="39" r="1.05" style="--p:0.791;--dx:0.931;--dy:-0.366"/><circle class="dt" cx="39" cy="42" r="1.05" style="--p:0.357;--dx:-0.809;--dy:-0.588"/><circle class="dt" cx="42" cy="42" r="1.05" style="--p:0.297;--dx:-0.707;--dy:-0.707"/><circle class="dt" cx="45" cy="42" r="1.05" style="--p:0.248;--dx:-0.53;--dy:-0.848"/><circle class="dt" cx="63" cy="42" r="1.05" style="--p:0.401;--dx:0.852;--dy:-0.524"/><circle class="dt" cx="66" cy="42" r="1.05" style="--p:0.47;--dx:0.894;--dy:-0.447"/><circle class="dt" cx="69" cy="42" r="1.05" style="--p:0.542;--dx:0.922;--dy:-0.388"/><circle class="dt" cx="72" cy="42" r="1.05" style="--p:0.615;--dx:0.94;--dy:-0.342"/><circle class="dt" cx="75" cy="42" r="1.05" style="--p:0.69;--dx:0.952;--dy:-0.305"/><circle class="dt" cx="78" cy="42" r="1.05" style="--p:0.765;--dx:0.962;--dy:-0.275"/><circle class="dt" cx="36" cy="45" r="1.05" style="--p:0.391;--dx:-0.942;--dy:-0.336"/><circle class="dt" cx="39" cy="45" r="1.05" style="--p:0.318;--dx:-0.91;--dy:-0.414"/><circle class="dt" cx="42" cy="45" r="1.05" style="--p:0.248;--dx:-0.848;--dy:-0.53"/><circle class="dt" cx="66" cy="45" r="1.05" style="--p:0.441;--dx:0.954;--dy:-0.298"/><circle class="dt" cx="69" cy="45" r="1.05" style="--p:0.516;--dx:0.967;--dy:-0.254"/><circle class="dt" cx="72" cy="45" r="1.05" style="--p:0.593;--dx:0.975;--dy:-0.222"/><circle class="dt" cx="75" cy="45" r="1.05" style="--p:0.67;--dx:0.981;--dy:-0.196"/><circle class="dt" cx="78" cy="45" r="1.05" style="--p:0.747;--dx:0.984;--dy:-0.176"/><circle class="dt" cx="81" cy="45" r="1.05" style="--p:0.825;--dx:0.987;--dy:-0.159"/><circle class="dt" cx="12" cy="48" r="1.05" style="--p:1.0;--dx:-0.999;--dy:-0.053"/><circle class="dt" cx="15" cy="48" r="1.05" style="--p:0.921;--dx:-0.998;--dy:-0.057"/><circle class="dt" cx="18" cy="48" r="1.05" style="--p:0.843;--dx:-0.998;--dy:-0.062"/><circle class="dt" cx="21" cy="48" r="1.05" style="--p:0.764;--dx:-0.998;--dy:-0.069"/><circle class="dt" cx="36" cy="48" r="1.05" style="--p:0.372;--dx:-0.99;--dy:-0.141"/><circle class="dt" cx="39" cy="48" r="1.05" style="--p:0.294;--dx:-0.984;--dy:-0.179"/><circle class="dt" cx="69" cy="48" r="1.05" style="--p:0.502;--dx:0.995;--dy:-0.105"/><circle class="dt" cx="72" cy="48" r="1.05" style="--p:0.581;--dx:0.996;--dy:-0.091"/><circle class="dt" cx="75" cy="48" r="1.05" style="--p:0.659;--dx:0.997;--dy:-0.08"/><circle class="dt" cx="78" cy="48" r="1.05" style="--p:0.738;--dx:0.997;--dy:-0.071"/><circle class="dt" cx="81" cy="48" r="1.05" style="--p:0.816;--dx:0.998;--dy:-0.064"/><circle class="dt" cx="84" cy="48" r="1.05" style="--p:0.895;--dx:0.998;--dy:-0.059"/><circle class="dt" cx="15" cy="51" r="1.05" style="--p:0.92;--dx:-1.0;--dy:0.029"/><circle class="dt" cx="18" cy="51" r="1.05" style="--p:0.841;--dx:-1.0;--dy:0.031"/><circle class="dt" cx="21" cy="51" r="1.05" style="--p:0.763;--dx:-0.999;--dy:0.034"/><circle class="dt" cx="24" cy="51" r="1.05" style="--p:0.684;--dx:-0.999;--dy:0.038"/><circle class="dt" cx="27" cy="51" r="1.05" style="--p:0.605;--dx:-0.999;--dy:0.043"/><circle class="dt" cx="57" cy="51" r="1.05" style="--p:0.186;--dx:0.99;--dy:0.141"/><circle class="dt" cx="60" cy="51" r="1.05" style="--p:0.264;--dx:0.995;--dy:0.1"/><circle class="dt" cx="63" cy="51" r="1.05" style="--p:0.343;--dx:0.997;--dy:0.077"/><circle class="dt" cx="75" cy="51" r="1.05" style="--p:0.658;--dx:0.999;--dy:0.04"/><circle class="dt" cx="78" cy="51" r="1.05" style="--p:0.736;--dx:0.999;--dy:0.036"/><circle class="dt" cx="81" cy="51" r="1.05" style="--p:0.815;--dx:0.999;--dy:0.032"/><circle class="dt" cx="84" cy="51" r="1.05" style="--p:0.894;--dx:1.0;--dy:0.029"/><circle class="dt" cx="87" cy="51" r="1.05" style="--p:0.973;--dx:1.0;--dy:0.027"/><circle class="dt" cx="18" cy="54" r="1.05" style="--p:0.847;--dx:-0.992;--dy:0.124"/><circle class="dt" cx="21" cy="54" r="1.05" style="--p:0.769;--dx:-0.991;--dy:0.137"/><circle class="dt" cx="24" cy="54" r="1.05" style="--p:0.691;--dx:-0.988;--dy:0.152"/><circle class="dt" cx="27" cy="54" r="1.05" style="--p:0.613;--dx:-0.985;--dy:0.171"/><circle class="dt" cx="30" cy="54" r="1.05" style="--p:0.536;--dx:-0.981;--dy:0.196"/><circle class="dt" cx="54" cy="54" r="1.05" style="--p:0.149;--dx:0.707;--dy:0.707"/><circle class="dt" cx="57" cy="54" r="1.05" style="--p:0.212;--dx:0.868;--dy:0.496"/><circle class="dt" cx="60" cy="54" r="1.05" style="--p:0.283;--dx:0.928;--dy:0.371"/><circle class="dt" cx="18" cy="57" r="1.05" style="--p:0.861;--dx:-0.977;--dy:0.214"/><circle class="dt" cx="21" cy="57" r="1.05" style="--p:0.784;--dx:-0.972;--dy:0.235"/><circle class="dt" cx="24" cy="57" r="1.05" style="--p:0.708;--dx:-0.966;--dy:0.26"/><circle class="dt" cx="27" cy="57" r="1.05" style="--p:0.632;--dx:-0.957;--dy:0.291"/><circle class="dt" cx="30" cy="57" r="1.05" style="--p:0.557;--dx:-0.944;--dy:0.33"/><circle class="dt" cx="33" cy="57" r="1.05" style="--p:0.483;--dx:-0.925;--dy:0.381"/><circle class="dt" cx="51" cy="57" r="1.05" style="--p:0.186;--dx:0.141;--dy:0.99"/><circle class="dt" cx="54" cy="57" r="1.05" style="--p:0.212;--dx:0.496;--dy:0.868"/><circle class="dt" cx="57" cy="57" r="1.05" style="--p:0.26;--dx:0.707;--dy:0.707"/><circle class="dt" cx="21" cy="60" r="1.05" style="--p:0.806;--dx:-0.945;--dy:0.326"/><circle class="dt" cx="24" cy="60" r="1.05" style="--p:0.732;--dx:-0.933;--dy:0.359"/><circle class="dt" cx="27" cy="60" r="1.05" style="--p:0.659;--dx:-0.917;--dy:0.399"/><circle class="dt" cx="30" cy="60" r="1.05" style="--p:0.588;--dx:-0.894;--dy:0.447"/><circle class="dt" cx="33" cy="60" r="1.05" style="--p:0.518;--dx:-0.862;--dy:0.507"/><circle class="dt" cx="36" cy="60" r="1.05" style="--p:0.452;--dx:-0.814;--dy:0.581"/><circle class="dt" cx="39" cy="60" r="1.05" style="--p:0.391;--dx:-0.74;--dy:0.673"/><circle class="dt" cx="45" cy="60" r="1.05" style="--p:0.294;--dx:-0.447;--dy:0.894"/><circle class="dt" cx="48" cy="60" r="1.05" style="--p:0.268;--dx:-0.196;--dy:0.981"/><circle class="dt" cx="51" cy="60" r="1.05" style="--p:0.264;--dx:0.1;--dy:0.995"/><circle class="dt" cx="54" cy="60" r="1.05" style="--p:0.283;--dx:0.371;--dy:0.928"/><circle class="dt" cx="24" cy="63" r="1.05" style="--p:0.764;--dx:-0.894;--dy:0.447"/><circle class="dt" cx="27" cy="63" r="1.05" style="--p:0.694;--dx:-0.871;--dy:0.492"/><circle class="dt" cx="30" cy="63" r="1.05" style="--p:0.627;--dx:-0.838;--dy:0.545"/><circle class="dt" cx="33" cy="63" r="1.05" style="--p:0.562;--dx:-0.794;--dy:0.607"/><circle class="dt" cx="36" cy="63" r="1.05" style="--p:0.502;--dx:-0.733;--dy:0.68"/><circle class="dt" cx="39" cy="63" r="1.05" style="--p:0.448;--dx:-0.646;--dy:0.763"/><circle class="dt" cx="42" cy="63" r="1.05" style="--p:0.401;--dx:-0.524;--dy:0.852"/><circle class="dt" cx="45" cy="63" r="1.05" style="--p:0.366;--dx:-0.359;--dy:0.933"/><circle class="dt" cx="48" cy="63" r="1.05" style="--p:0.346;--dx:-0.152;--dy:0.988"/><circle class="dt" cx="51" cy="63" r="1.05" style="--p:0.343;--dx:0.077;--dy:0.997"/><circle class="dt" cx="27" cy="66" r="1.05" style="--p:0.736;--dx:-0.821;--dy:0.571"/><circle class="dt" cx="30" cy="66" r="1.05" style="--p:0.673;--dx:-0.781;--dy:0.625"/><circle class="dt" cx="33" cy="66" r="1.05" style="--p:0.613;--dx:-0.728;--dy:0.685"/><circle class="dt" cx="36" cy="66" r="1.05" style="--p:0.559;--dx:-0.659;--dy:0.753"/><circle class="dt" cx="39" cy="66" r="1.05" style="--p:0.51;--dx:-0.567;--dy:0.824"/><circle class="dt" cx="42" cy="66" r="1.05" style="--p:0.47;--dx:-0.447;--dy:0.894"/><circle class="dt" cx="45" cy="66" r="1.05" style="--p:0.441;--dx:-0.298;--dy:0.954"/><circle class="dt" cx="48" cy="66" r="1.05" style="--p:0.424;--dx:-0.124;--dy:0.992"/><circle class="dt" cx="33" cy="69" r="1.05" style="--p:0.67;--dx:-0.667;--dy:0.745"/><circle class="dt" cx="36" cy="69" r="1.05" style="--p:0.62;--dx:-0.593;--dy:0.805"/><circle class="dt" cx="39" cy="69" r="1.05" style="--p:0.577;--dx:-0.501;--dy:0.865"/><circle class="dt" cx="42" cy="69" r="1.05" style="--p:0.542;--dx:-0.388;--dy:0.922"/><circle class="dt" cx="45" cy="69" r="1.05" style="--p:0.516;--dx:-0.254;--dy:0.967"/></g></svg>`

const Logo = memo(function Logo() {
  return (
    <span
      aria-hidden="true"
      className="inline-flex shrink-0"
      dangerouslySetInnerHTML={{ __html: LOGO_SVG }}
    />
  )
})

function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  if (h > 0) return `${h}h ${m}m ${s}s`
  if (m > 0) return `${m}m ${s}s`
  return `${s}s`
}

function formatTokens(n: number): string {
  if (n >= 10000) return `${Math.round(n / 1000)}k`
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`
  return `${n}`
}

/** CJK 按 1 字 1 token、其余 4 字符 1 token（与 Rust chunking 的口径一致的粗估）。 */
function estimateTokens(text: string): number {
  let cjk = 0
  for (const ch of text) {
    if (ch.codePointAt(0)! > 0x2e7f) cjk++
  }
  return Math.round(cjk + (text.length - cjk) / 4)
}

interface StreamStatusLineProps {
  /** 生成中（动画 + 实时信息）；false = 闲置常驻（静态 logo，无文字） */
  active: boolean
}

interface LiveStats {
  elapsedMs: number
  tokens: number
  running: number
  statusNote: string | null
}

/**
 * 消息流末尾常驻的存在标记（对标 Claude Code 的小星号）：闲置时静态 logo，
 * 生成中动效 + 「耗时 · N tokens · K running」（信息行统一英文，短）。
 *
 * 实时信息不走 props、不订阅 store（那是每帧一响）：每秒从 streamingStore 快照
 * 读一次，token 从已流出的正文 + 思维链估算 —— 任何运行时（内置 / 外部 CLI）都有数，
 * 且和 Claude 一样随输出平滑上涨；后端的 live 用量事件太稀（每 planning 轮一次）不用。
 */
export const StreamStatusLine = memo(function StreamStatusLine({ active }: StreamStatusLineProps) {
  const [stats, setStats] = useState<LiveStats | null>(null)
  useEffect(() => {
    if (!active) {
      setStats(null)
      return
    }
    const update = () => {
      const snapshot = getSnapshot()
      setStats({
        elapsedMs: snapshot.startedAt != null ? Date.now() - snapshot.startedAt : 0,
        tokens: estimateTokens(snapshot.content) + estimateTokens(snapshot.reasoning),
        running: snapshot.toolCalls.reduce(
          (n, tool) => (tool.status === 'running' ? n + 1 : n),
          0,
        ),
        statusNote: snapshot.statusNote,
      })
    }
    update()
    const id = setInterval(update, 1000)
    return () => clearInterval(id)
  }, [active])

  const parts: string[] = []
  if (stats) {
    parts.push(formatElapsed(stats.elapsedMs))
    if (stats.tokens > 0) parts.push(`${formatTokens(stats.tokens)} tokens`)
    if (stats.running > 0) parts.push(`${stats.running} running`)
    if (stats.statusNote) parts.push(stats.statusNote)
  }

  return (
    <div
      className={`flex items-center gap-2.5 py-2${active ? ' chat-motion-fade-up' : ' kv-stream-status-idle'}`}
    >
      <Logo />
      {active && stats && (
        <span className="text-xs tabular-nums text-neutral-500 dark:text-neutral-400">
          {parts.join(' · ')}
        </span>
      )}
    </div>
  )
})
