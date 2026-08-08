// 设置导航的自定义线性图标。接口对齐 lucide-react（size / strokeWidth / className），
// 可直接替换 <Icon size={17} strokeWidth={1.75} />。fill=none + stroke=currentColor，
// 颜色随上层文字色（选中态 / 深色模式自动跟随）。

interface IconProps {
  size?: number
  strokeWidth?: number
  className?: string
}

function svgProps({ size = 24, strokeWidth = 2, className }: IconProps) {
  return {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth,
    strokeLinecap: 'round' as const,
    strokeLinejoin: 'round' as const,
    className,
  }
}

// 基础：六角螺母 + 中心孔
export function GeneralIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M21 12 L16.5 4.2 L7.5 4.2 L3 12 L7.5 19.8 L16.5 19.8 Z" />
      <circle cx="12" cy="12" r="3.2" />
    </svg>
  )
}

// 快捷键：键盘外框 + 按键点 + 空格条
export function HotkeysIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <rect x="2.5" y="6" width="19" height="12" rx="2" />
      <path d="M6 9.5 h0.01 M9.5 9.5 h0.01 M13 9.5 h0.01 M16.5 9.5 h0.01 M8 15 h8" />
    </svg>
  )
}

// 知识库：书本 + 书脊
export function KnowledgeIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M5 4.5 H17 a2 2 0 0 1 2 2 V19 a1.5 1.5 0 0 0 -1.5 -1.5 H5 Z" />
      <path d="M5 4.5 a1.5 1.5 0 0 0 -1.5 1.5 V18 a1.5 1.5 0 0 0 1.5 1.5" />
      <path d="M8.5 9 H14.5" />
      <path d="M8.5 12 H13" />
    </svg>
  )
}

// 输入翻译：A 字 + CJK 字符
export function TranslateIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M3 19 L6.5 8 L10 19" />
      <path d="M4.2 15.5 H8.8" />
      <path d="M13 8 H21" />
      <path d="M17 6 V8" />
      <path d="M14.5 18 L17 12.5 L19.5 18" />
    </svg>
  )
}

// 快速翻译：闪电
export function ScreenshotIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M13 2 L4 14 H11 L11 22 L20 10 H13 Z" />
    </svg>
  )
}

// Lens：取景框 + 镜头
export function LensIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <circle cx="12" cy="12" r="3.5" />
      <path d="M5 8 V5 H8" />
      <path d="M16 5 H19 V8" />
      <path d="M19 16 V19 H16" />
      <path d="M8 19 H5 V16" />
    </svg>
  )
}

// AI 客户端：应用 logo mark（与 RuntimePicker / 内置 Agent 一致）
export function ChatIcon({ size = 24, className }: IconProps) {
  return (
    <img
      src="/logo-mark.png"
      alt=""
      aria-hidden="true"
      width={size}
      height={size}
      className={['kv-nav-logo-mark', className].filter(Boolean).join(' ')}
      draggable={false}
    />
  )
}

// 记忆：节点图
export function MemoryIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <circle cx="7" cy="8" r="2" />
      <circle cx="17" cy="9" r="2" />
      <circle cx="11" cy="16.5" r="2" />
      <path d="M8.9 8.6 L15.1 8.9" />
      <path d="M8.2 9.7 L9.9 14.8" />
      <path d="M15.5 10.6 L12.3 14.9" />
    </svg>
  )
}

// 混音器：推子
export function MixerIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M7 4 V20" />
      <path d="M12 4 V20" />
      <path d="M17 4 V20" />
      <circle cx="7" cy="9" r="1.8" />
      <circle cx="12" cy="14" r="1.8" />
      <circle cx="17" cy="8" r="1.8" />
    </svg>
  )
}

// Kivio Code：终端
export function CodeIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M5 5 H19 A2 2 0 0 1 21 7 V17 A2 2 0 0 1 19 19 H5 A2 2 0 0 1 3 17 V7 A2 2 0 0 1 5 5 Z" />
      <path d="M7 10 L10 12.5 L7 15" />
      <path d="M12.5 15 H16" />
    </svg>
  )
}

// 本地 CLI Agent：机器人
export function AgentIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M7 9 H17 A1.5 1.5 0 0 1 18.5 10.5 V16.5 A1.5 1.5 0 0 1 17 18 H7 A1.5 1.5 0 0 1 5.5 16.5 V10.5 A1.5 1.5 0 0 1 7 9 Z" />
      <path d="M12 9 V6" />
      <circle cx="12" cy="5" r="1" />
      <path d="M9.5 13 V13" />
      <path d="M14.5 13 V13" />
    </svg>
  )
}

// MCP：插头/连接
export function McpIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M9 3 V7" />
      <path d="M15 3 V7" />
      <path d="M7 7 H17 V11 A5 5 0 0 1 7 11 Z" />
      <path d="M12 16 V21" />
    </svg>
  )
}

// 连接器：两个互锁的链环（区别于 McpIcon 的插头），表达「连接外部数据源」
export function ConnectorsIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M9.5 7 H7 A4 4 0 0 0 7 15 H9.5" />
      <path d="M14.5 7 H17 A4 4 0 0 1 17 15 H14.5" />
      <path d="M8.5 11 H15.5" />
    </svg>
  )
}

// 插件：拼图块（与扩展中心里原 Puzzle 语义一致）
export function PluginsIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M12 3.5 V6.5" />
      <path d="M12 17.5 V20.5" />
      <path d="M3.5 12 H6.5" />
      <path d="M17.5 12 H20.5" />
      <path d="M8.5 6.5 H11 A1.5 1.5 0 1 1 14 6.5 H15.5 A2 2 0 0 1 17.5 8.5 V11 A1.5 1.5 0 1 1 17.5 14 V15.5 A2 2 0 0 1 15.5 17.5 H14 A1.5 1.5 0 1 1 11 17.5 H8.5 A2 2 0 0 1 6.5 15.5 V14 A1.5 1.5 0 1 1 6.5 11 V8.5 A2 2 0 0 1 8.5 6.5 Z" />
    </svg>
  )
}

// Hooks：闪电（生命周期触发）
export function HooksIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M13.5 2.5 L5 13.5 H11 L10.5 21.5 L19 10.5 H13 Z" />
    </svg>
  )
}

// Skill：卷轴
export function SkillIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M15 8 H10" />
      <path d="M15 12 H10" />
      <path d="M19 17 V5 A2 2 0 0 0 17 3 H4" />
      <path d="M8 21 H20 A2 2 0 0 0 22 19 V18 A1 1 0 0 0 21 17 H11 A1 1 0 0 0 10 18 V19 A2 2 0 1 1 6 19 V5 A2 2 0 1 0 2 5 V7 A1 1 0 0 0 3 8 H6" />
    </svg>
  )
}

// 网络搜索：地球仪（经线椭圆 + 赤道），填满 viewBox，小尺寸也清晰
export function WebSearchIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12 H21" />
      <path d="M12 3 a 4.5 9 0 0 0 0 18 a 4.5 9 0 0 0 0 -18" />
    </svg>
  )
}

// 用量统计：柱状图
export function UsageIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M4 20 H20" />
      <path d="M7.5 20 V13" />
      <path d="M12 20 V7" />
      <path d="M16.5 20 V11" />
    </svg>
  )
}

// 模型：云
export function ProvidersIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <path d="M6.5 18 A3.5 3.5 0 0 1 6.8 11 A5 5 0 0 1 16.5 10.5 A3.5 3.5 0 0 1 17 18 Z" />
    </svg>
  )
}

// 关于：信息
export function AboutIcon(props: IconProps) {
  return (
    <svg {...svgProps(props)}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11 V16.5" />
      <path d="M12 7.8 V7.81" />
    </svg>
  )
}
