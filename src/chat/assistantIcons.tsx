import type { ReactElement } from 'react'

// 内置专家的内联 SVG 图标（line 风格，stroke=currentColor 继承调用处的文字色）。
// 刻意不用名字首字 / emoji 这类「字体字形」：按内置专家 id 映射到手绘 SVG；
// 非内置专家返回 null，由调用处自行回退到首字头像。
//
// 实现注记：图标以「record 内的箭头函数」而非顶层组件声明的形式存在，避免
// react-refresh/only-export-components 警告（本模块只对外导出一个 helper，不是组件模块）。

const baseProps = {
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.75,
  strokeLinecap: 'round',
  strokeLinejoin: 'round',
  'aria-hidden': true,
} as const

const BUILTIN_ASSISTANT_GLYPHS: Record<string, (size: number) => ReactElement> = {
  // 写作助手：铅笔在基线上书写
  asst_builtin_writer: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <path d="M4 20h6" />
      <path d="m15 4 5 5L9.5 19.5 4 21l1.5-5.5L15 4Z" />
      <path d="m12.5 6.5 5 5" />
    </svg>
  ),
  // 编程助手：代码尖括号 + 斜杠
  asst_builtin_coder: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <polyline points="15 7 20 12 15 17" />
      <polyline points="9 7 4 12 9 17" />
      <line x1="13" y1="5" x2="11" y2="19" />
    </svg>
  ),
  // 研究助手：放大镜（带内部十字，区别于通用搜索）
  asst_builtin_researcher: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="m20 20-4.6-4.6" />
      <path d="M10.5 8v5" />
      <path d="M8 10.5h5" />
    </svg>
  ),
  // 数据分析：坐标轴 + 柱状图
  asst_builtin_data: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <path d="M4 4v16h16" />
      <rect x="7" y="12" width="2.6" height="5" rx="0.6" />
      <rect x="11.7" y="8" width="2.6" height="9" rx="0.6" />
      <rect x="16.4" y="14" width="2.6" height="3" rx="0.6" />
    </svg>
  ),
  // 前端设计师：画板/界面框 + 侧栏分区
  asst_builtin_frontend: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <rect x="3.5" y="5" width="17" height="14" rx="2" />
      <path d="M3.5 9h17" />
      <path d="M9 9v10" />
    </svg>
  ),
  // 翻译助手：地球（经纬线）
  asst_builtin_translator: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <circle cx="12" cy="12" r="8" />
      <path d="M4 12h16" />
      <path d="M12 4c2.6 2.3 2.6 13.7 0 16" />
      <path d="M12 4c-2.6 2.3-2.6 13.7 0 16" />
    </svg>
  ),
  // 文档专家：带折角的文稿 + 正文行
  asst_builtin_docsmith: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <path d="M6 3h8l4 4v14H6z" />
      <path d="M14 3v4h4" />
      <path d="M9 12.5h6" />
      <path d="M9 16h6" />
    </svg>
  ),
  // 产品经理：看板三列
  asst_builtin_pm: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <rect x="3.5" y="5" width="17" height="14" rx="2" />
      <path d="M9.2 5v14" />
      <path d="M14.8 5v14" />
      <path d="M5.5 8.5h2" />
      <path d="M11 8.5h2" />
      <path d="M16.5 8.5h2" />
    </svg>
  ),
  // 法务助手：天平
  asst_builtin_legal: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <path d="M12 4v13" />
      <path d="M7 20h10" />
      <path d="M4 8h16" />
      <path d="M6 8c0 2.8-2 5-2 5h4s-2-2.2-2-5" />
      <path d="M18 8c0 2.8 2 5 2 5h-4s2-2.2 2-5" />
    </svg>
  ),
  // 财务分析：折线向上
  asst_builtin_finance: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <path d="M4 4v16h16" />
      <polyline points="7 14 11 10 14 12.5 20 7" />
    </svg>
  ),
  // 教学助手：打开的书
  asst_builtin_teacher: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <path d="M3 5h6a4 4 0 0 1 4 4v11a3 3 0 0 0-3-3H3z" />
      <path d="M21 5h-6a4 4 0 0 0-4 4v11a3 3 0 0 1 3-3h7z" />
    </svg>
  ),
  // 代码审查：勾选的变更
  asst_builtin_reviewer: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <path d="M8 5h9.5A2.5 2.5 0 0 1 20 7.5v11A2.5 2.5 0 0 1 17.5 21H8a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z" />
      <path d="M10 4.5h5V3h-5z" />
      <path d="m10.5 13 2 2 4-4" />
    </svg>
  ),
  // 求职教练：公文包
  asst_builtin_career: (size) => (
    <svg width={size} height={size} {...baseProps}>
      <rect x="3.5" y="8" width="17" height="12" rx="2" />
      <path d="M8 8V6.5A1.5 1.5 0 0 1 9.5 5h5A1.5 1.5 0 0 1 16 6.5V8" />
      <path d="M3.5 13h17" />
    </svg>
  ),
}

/** 返回内置专家的 SVG 图标；非内置（无映射）返回 null，调用处回退到首字头像。 */
export function builtinAssistantGlyph(id: string, size = 20): ReactElement | null {
  return BUILTIN_ASSISTANT_GLYPHS[id]?.(size) ?? null
}
