import { memo, useEffect, useState } from 'react'
import { getSnapshot } from './streamingStore'

// GRD-02 · 回波：等距矩形方格上抠出标识的动效 logo。每个点自带相位/方向 CSS 变量
// （--p/--dx/--dy），动画与颜色在 index.css 的 .kv-stream-logo 规则里。
// 用 dangerouslySetInnerHTML 挂一次静态 DOM：130+ 个 circle 不进 React 每帧 reconcile。
const LOGO_SVG = `<svg xmlns="http://www.w3.org/2000/svg" class="kv-stream-logo" viewBox="4 4 92 92" width="28" height="28"><g><circle class="dt" cx="56" cy="29" r="1.05" style="--p:0.436;--dx:0.275;--dy:-0.962"/><circle class="dt" cx="59" cy="29" r="1.05" style="--p:0.469;--dx:0.394;--dy:-0.919"/><circle class="dt" cx="62" cy="29" r="1.05" style="--p:0.513;--dx:0.496;--dy:-0.868"/><circle class="dt" cx="65" cy="29" r="1.05" style="--p:0.566;--dx:0.581;--dy:-0.814"/><circle class="dt" cx="68" cy="29" r="1.05" style="--p:0.626;--dx:0.651;--dy:-0.759"/><circle class="dt" cx="50" cy="32" r="1.05" style="--p:0.311;--dx:0.000;--dy:-1.000"/><circle class="dt" cx="53" cy="32" r="1.05" style="--p:0.319;--dx:0.164;--dy:-0.986"/><circle class="dt" cx="56" cy="32" r="1.05" style="--p:0.342;--dx:0.316;--dy:-0.949"/><circle class="dt" cx="59" cy="32" r="1.05" style="--p:0.380;--dx:0.447;--dy:-0.894"/><circle class="dt" cx="62" cy="32" r="1.05" style="--p:0.429;--dx:0.555;--dy:-0.832"/><circle class="dt" cx="65" cy="32" r="1.05" style="--p:0.488;--dx:0.640;--dy:-0.768"/><circle class="dt" cx="68" cy="32" r="1.05" style="--p:0.554;--dx:0.707;--dy:-0.707"/><circle class="dt" cx="71" cy="32" r="1.05" style="--p:0.626;--dx:0.759;--dy:-0.651"/><circle class="dt" cx="74" cy="32" r="1.05" style="--p:0.702;--dx:0.800;--dy:-0.600"/><circle class="dt" cx="47" cy="35" r="1.05" style="--p:0.222;--dx:-0.196;--dy:-0.981"/><circle class="dt" cx="50" cy="35" r="1.05" style="--p:0.213;--dx:0.000;--dy:-1.000"/><circle class="dt" cx="53" cy="35" r="1.05" style="--p:0.222;--dx:0.196;--dy:-0.981"/><circle class="dt" cx="56" cy="35" r="1.05" style="--p:0.250;--dx:0.371;--dy:-0.928"/><circle class="dt" cx="59" cy="35" r="1.05" style="--p:0.294;--dx:0.514;--dy:-0.857"/><circle class="dt" cx="62" cy="35" r="1.05" style="--p:0.350;--dx:0.625;--dy:-0.781"/><circle class="dt" cx="65" cy="35" r="1.05" style="--p:0.416;--dx:0.707;--dy:-0.707"/><circle class="dt" cx="68" cy="35" r="1.05" style="--p:0.488;--dx:0.768;--dy:-0.640"/><circle class="dt" cx="71" cy="35" r="1.05" style="--p:0.566;--dx:0.814;--dy:-0.581"/><circle class="dt" cx="74" cy="35" r="1.05" style="--p:0.647;--dx:0.848;--dy:-0.530"/><circle class="dt" cx="77" cy="35" r="1.05" style="--p:0.731;--dx:0.874;--dy:-0.486"/><circle class="dt" cx="44" cy="38" r="1.05" style="--p:0.161;--dx:-0.447;--dy:-0.894"/><circle class="dt" cx="47" cy="38" r="1.05" style="--p:0.127;--dx:-0.243;--dy:-0.970"/><circle class="dt" cx="50" cy="38" r="1.05" style="--p:0.115;--dx:0.000;--dy:-1.000"/><circle class="dt" cx="53" cy="38" r="1.05" style="--p:0.127;--dx:0.243;--dy:-0.970"/><circle class="dt" cx="56" cy="38" r="1.05" style="--p:0.161;--dx:0.447;--dy:-0.894"/><circle class="dt" cx="59" cy="38" r="1.05" style="--p:0.213;--dx:0.600;--dy:-0.800"/><circle class="dt" cx="62" cy="38" r="1.05" style="--p:0.277;--dx:0.707;--dy:-0.707"/><circle class="dt" cx="65" cy="38" r="1.05" style="--p:0.350;--dx:0.781;--dy:-0.625"/><circle class="dt" cx="68" cy="38" r="1.05" style="--p:0.429;--dx:0.832;--dy:-0.555"/><circle class="dt" cx="71" cy="38" r="1.05" style="--p:0.513;--dx:0.868;--dy:-0.496"/><circle class="dt" cx="74" cy="38" r="1.05" style="--p:0.599;--dx:0.894;--dy:-0.447"/><circle class="dt" cx="77" cy="38" r="1.05" style="--p:0.688;--dx:0.914;--dy:-0.406"/><circle class="dt" cx="80" cy="38" r="1.05" style="--p:0.778;--dx:0.928;--dy:-0.371"/><circle class="dt" cx="41" cy="41" r="1.05" style="--p:0.139;--dx:-0.707;--dy:-0.707"/><circle class="dt" cx="44" cy="41" r="1.05" style="--p:0.076;--dx:-0.555;--dy:-0.832"/><circle class="dt" cx="47" cy="41" r="1.05" style="--p:0.033;--dx:-0.316;--dy:-0.949"/><circle class="dt" cx="50" cy="41" r="1.05" style="--p:0.017;--dx:0.000;--dy:-1.000"/><circle class="dt" cx="71" cy="41" r="1.05" style="--p:0.469;--dx:0.919;--dy:-0.394"/><circle class="dt" cx="74" cy="41" r="1.05" style="--p:0.560;--dx:0.936;--dy:-0.351"/><circle class="dt" cx="77" cy="41" r="1.05" style="--p:0.652;--dx:0.949;--dy:-0.316"/><circle class="dt" cx="80" cy="41" r="1.05" style="--p:0.746;--dx:0.958;--dy:-0.287"/><circle class="dt" cx="83" cy="41" r="1.05" style="--p:0.840;--dx:0.965;--dy:-0.263"/><circle class="dt" cx="38" cy="44" r="1.05" style="--p:0.161;--dx:-0.894;--dy:-0.447"/><circle class="dt" cx="41" cy="44" r="1.05" style="--p:0.076;--dx:-0.832;--dy:-0.555"/><circle class="dt" cx="44" cy="44" r="1.05" style="--p:0.000;--dx:-0.707;--dy:-0.707"/><circle class="dt" cx="80" cy="44" r="1.05" style="--p:0.722;--dx:0.981;--dy:-0.196"/><circle class="dt" cx="83" cy="44" r="1.05" style="--p:0.818;--dx:0.984;--dy:-0.179"/><circle class="dt" cx="86" cy="44" r="1.05" style="--p:0.915;--dx:0.986;--dy:-0.164"/><circle class="dt" cx="35" cy="47" r="1.05" style="--p:0.222;--dx:-0.981;--dy:-0.196"/><circle class="dt" cx="38" cy="47" r="1.05" style="--p:0.127;--dx:-0.970;--dy:-0.243"/><circle class="dt" cx="86" cy="47" r="1.05" style="--p:0.902;--dx:0.997;--dy:-0.083"/><circle class="dt" cx="89" cy="47" r="1.05" style="--p:1.000;--dx:0.997;--dy:-0.077"/><circle class="dt" cx="11" cy="50" r="1.05" style="--p:0.996;--dx:-1.000;--dy:0.000"/><circle class="dt" cx="89" cy="50" r="1.05" style="--p:0.996;--dx:1.000;--dy:0.000"/><circle class="dt" cx="11" cy="53" r="1.05" style="--p:1.000;--dx:-0.997;--dy:0.077"/><circle class="dt" cx="14" cy="53" r="1.05" style="--p:0.902;--dx:-0.997;--dy:0.083"/><circle class="dt" cx="62" cy="53" r="1.05" style="--p:0.127;--dx:0.970;--dy:0.243"/><circle class="dt" cx="65" cy="53" r="1.05" style="--p:0.222;--dx:0.981;--dy:0.196"/><circle class="dt" cx="14" cy="56" r="1.05" style="--p:0.915;--dx:-0.986;--dy:0.164"/><circle class="dt" cx="17" cy="56" r="1.05" style="--p:0.818;--dx:-0.984;--dy:0.179"/><circle class="dt" cx="20" cy="56" r="1.05" style="--p:0.722;--dx:-0.981;--dy:0.196"/><circle class="dt" cx="56" cy="56" r="1.05" style="--p:0.000;--dx:0.707;--dy:0.707"/><circle class="dt" cx="59" cy="56" r="1.05" style="--p:0.076;--dx:0.832;--dy:0.555"/><circle class="dt" cx="62" cy="56" r="1.05" style="--p:0.161;--dx:0.894;--dy:0.447"/><circle class="dt" cx="17" cy="59" r="1.05" style="--p:0.840;--dx:-0.965;--dy:0.263"/><circle class="dt" cx="20" cy="59" r="1.05" style="--p:0.746;--dx:-0.958;--dy:0.287"/><circle class="dt" cx="23" cy="59" r="1.05" style="--p:0.652;--dx:-0.949;--dy:0.316"/><circle class="dt" cx="26" cy="59" r="1.05" style="--p:0.560;--dx:-0.936;--dy:0.351"/><circle class="dt" cx="29" cy="59" r="1.05" style="--p:0.469;--dx:-0.919;--dy:0.394"/><circle class="dt" cx="50" cy="59" r="1.05" style="--p:0.017;--dx:0.000;--dy:1.000"/><circle class="dt" cx="53" cy="59" r="1.05" style="--p:0.033;--dx:0.316;--dy:0.949"/><circle class="dt" cx="56" cy="59" r="1.05" style="--p:0.076;--dx:0.555;--dy:0.832"/><circle class="dt" cx="59" cy="59" r="1.05" style="--p:0.139;--dx:0.707;--dy:0.707"/><circle class="dt" cx="20" cy="62" r="1.05" style="--p:0.778;--dx:-0.928;--dy:0.371"/><circle class="dt" cx="23" cy="62" r="1.05" style="--p:0.688;--dx:-0.914;--dy:0.406"/><circle class="dt" cx="26" cy="62" r="1.05" style="--p:0.599;--dx:-0.894;--dy:0.447"/><circle class="dt" cx="29" cy="62" r="1.05" style="--p:0.513;--dx:-0.868;--dy:0.496"/><circle class="dt" cx="32" cy="62" r="1.05" style="--p:0.429;--dx:-0.832;--dy:0.555"/><circle class="dt" cx="35" cy="62" r="1.05" style="--p:0.350;--dx:-0.781;--dy:0.625"/><circle class="dt" cx="38" cy="62" r="1.05" style="--p:0.277;--dx:-0.707;--dy:0.707"/><circle class="dt" cx="41" cy="62" r="1.05" style="--p:0.213;--dx:-0.600;--dy:0.800"/><circle class="dt" cx="44" cy="62" r="1.05" style="--p:0.161;--dx:-0.447;--dy:0.894"/><circle class="dt" cx="47" cy="62" r="1.05" style="--p:0.127;--dx:-0.243;--dy:0.970"/><circle class="dt" cx="50" cy="62" r="1.05" style="--p:0.115;--dx:0.000;--dy:1.000"/><circle class="dt" cx="53" cy="62" r="1.05" style="--p:0.127;--dx:0.243;--dy:0.970"/><circle class="dt" cx="56" cy="62" r="1.05" style="--p:0.161;--dx:0.447;--dy:0.894"/><circle class="dt" cx="23" cy="65" r="1.05" style="--p:0.731;--dx:-0.874;--dy:0.486"/><circle class="dt" cx="26" cy="65" r="1.05" style="--p:0.647;--dx:-0.848;--dy:0.530"/><circle class="dt" cx="29" cy="65" r="1.05" style="--p:0.566;--dx:-0.814;--dy:0.581"/><circle class="dt" cx="32" cy="65" r="1.05" style="--p:0.488;--dx:-0.768;--dy:0.640"/><circle class="dt" cx="35" cy="65" r="1.05" style="--p:0.416;--dx:-0.707;--dy:0.707"/><circle class="dt" cx="38" cy="65" r="1.05" style="--p:0.350;--dx:-0.625;--dy:0.781"/><circle class="dt" cx="41" cy="65" r="1.05" style="--p:0.294;--dx:-0.514;--dy:0.857"/><circle class="dt" cx="44" cy="65" r="1.05" style="--p:0.250;--dx:-0.371;--dy:0.928"/><circle class="dt" cx="47" cy="65" r="1.05" style="--p:0.222;--dx:-0.196;--dy:0.981"/><circle class="dt" cx="50" cy="65" r="1.05" style="--p:0.213;--dx:0.000;--dy:1.000"/><circle class="dt" cx="53" cy="65" r="1.05" style="--p:0.222;--dx:0.196;--dy:0.981"/><circle class="dt" cx="26" cy="68" r="1.05" style="--p:0.702;--dx:-0.800;--dy:0.600"/><circle class="dt" cx="29" cy="68" r="1.05" style="--p:0.626;--dx:-0.759;--dy:0.651"/><circle class="dt" cx="32" cy="68" r="1.05" style="--p:0.554;--dx:-0.707;--dy:0.707"/><circle class="dt" cx="35" cy="68" r="1.05" style="--p:0.488;--dx:-0.640;--dy:0.768"/><circle class="dt" cx="38" cy="68" r="1.05" style="--p:0.429;--dx:-0.555;--dy:0.832"/><circle class="dt" cx="41" cy="68" r="1.05" style="--p:0.380;--dx:-0.447;--dy:0.894"/><circle class="dt" cx="44" cy="68" r="1.05" style="--p:0.342;--dx:-0.316;--dy:0.949"/><circle class="dt" cx="47" cy="68" r="1.05" style="--p:0.319;--dx:-0.164;--dy:0.986"/><circle class="dt" cx="50" cy="68" r="1.05" style="--p:0.311;--dx:0.000;--dy:1.000"/><circle class="dt" cx="32" cy="71" r="1.05" style="--p:0.626;--dx:-0.651;--dy:0.759"/><circle class="dt" cx="35" cy="71" r="1.05" style="--p:0.566;--dx:-0.581;--dy:0.814"/><circle class="dt" cx="38" cy="71" r="1.05" style="--p:0.513;--dx:-0.496;--dy:0.868"/><circle class="dt" cx="41" cy="71" r="1.05" style="--p:0.469;--dx:-0.394;--dy:0.919"/><circle class="dt" cx="44" cy="71" r="1.05" style="--p:0.436;--dx:-0.275;--dy:0.962"/></g></svg>`

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
