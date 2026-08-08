import { useEffect, useRef } from 'react'
import { ShieldQuestion } from 'lucide-react'
import { Button } from '../components/Button'

export type ApprovalAction = {
  label: string
  onSelect: () => void
  /** 主动作，额外响应 Ctrl/Cmd+Enter。 */
  primary?: boolean
  /** 按钮上跟在序号后的补充提示（如 `Ctrl+↵`）。 */
  hint?: string
}

interface ApprovalCardProps {
  title: string
  subtitle?: string
  /** 代码块正文（文件完整路径 / 命令）。 */
  detail?: string
  /** 第一项当作「拒绝」：靠左摆放，并绑定 Esc。 */
  actions: ApprovalAction[]
}

function isEditableTarget(node: EventTarget | null): boolean {
  const el = node as HTMLElement | null
  if (!el || !el.tagName) return false
  const tag = el.tagName.toLowerCase()
  return tag === 'input' || tag === 'textarea' || el.isContentEditable
}

/**
 * 内联审批卡：工具审批与会话级授权共用。刻意**不做模态遮罩**——审批要能一边看上文一边决定。
 *
 * 键盘：数字键按动作顺序触发，Ctrl/Cmd+Enter 触发主动作，Esc 触发第一项（拒绝）。
 * 挂载时把焦点移到主动作按钮上，否则焦点还在输入框里、数字键会被当成正文输入。
 */
export function ApprovalCard({ title, subtitle, detail, actions }: ApprovalCardProps) {
  const actionsRef = useRef(actions)
  actionsRef.current = actions

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      // 输入框优先：composer 的 Ctrl+Enter 是发送，数字键是正文，都不能抢。
      if (isEditableTarget(e.target)) return
      const list = actionsRef.current
      if (e.key === 'Escape') {
        if (!list.length) return
        e.preventDefault()
        list[0].onSelect()
        return
      }
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        const primary = list.find((action) => action.primary)
        if (!primary) return
        e.preventDefault()
        primary.onSelect()
        return
      }
      if (e.ctrlKey || e.metaKey || e.altKey) return
      const index = Number(e.key) - 1
      if (!Number.isInteger(index) || index < 0 || index >= list.length) return
      e.preventDefault()
      list[index].onSelect()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  const [deny, ...rest] = actions

  return (
    <div className="not-prose mx-auto mb-2 w-full rounded-md border border-neutral-200/70 bg-white/90 px-3 py-2.5 text-[12px] leading-5 text-neutral-700 shadow-[0_10px_28px_-26px_rgba(0,0,0,0.45),0_1px_2px_rgba(0,0,0,0.035)] dark:border-neutral-700/70 dark:bg-neutral-900/80 dark:text-neutral-200">
      <div className="flex items-start gap-2">
        <ShieldQuestion size={16} className="mt-0.5 shrink-0 text-[#2f6ff0] dark:text-[#5c8df7]" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold text-neutral-900 dark:text-neutral-100">
            {title}
          </div>
          {subtitle && (
            <div className="mt-0.5 text-[12px] text-neutral-500 dark:text-neutral-400">
              {subtitle}
            </div>
          )}
        </div>
      </div>
      {detail && (
        <pre className="custom-scrollbar mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-all rounded-md bg-neutral-100 px-2.5 py-2 text-[11px] leading-relaxed text-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
          {detail}
        </pre>
      )}
      <div className="mt-2.5 flex items-center gap-2">
        {deny && <ActionButton action={deny} index={0} />}
        <div className="ml-auto flex items-center gap-2">
          {rest.map((action, i) => (
            <ActionButton key={action.label} action={action} index={i + 1} />
          ))}
        </div>
      </div>
    </div>
  )
}

function ActionButton({ action, index }: { action: ApprovalAction; index: number }) {
  return (
    <Button
      size="sm"
      variant={action.primary ? 'primary' : 'default'}
      // 焦点必须落在卡片上，否则还留在输入框里、数字键会被当成正文输入。
      autoFocus={action.primary}
      onClick={action.onSelect}
    >
      <span>{action.label}</span>
      <span className="ml-1.5 opacity-55">{index + 1}</span>
      {action.hint && <span className="ml-1 opacity-45">{action.hint}</span>}
    </Button>
  )
}
