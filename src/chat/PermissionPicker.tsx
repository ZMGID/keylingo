import { memo, useMemo, useRef, useState } from 'react'
import { Check } from 'lucide-react'
import { useT } from '../settings/i18n'
import { derivePermissionModes } from './permissionModes'
import { chatTitlebarIconButtonClass } from './platform'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import type { AgentRuntimeConfig } from './types'

interface PermissionPickerProps {
  agentRuntime: AgentRuntimeConfig
  /** Built-in agent tool-approval policy. */
  approvalPolicy?: string
  onApprovalPolicyChange?: (policy: string) => void
}

/**
 * Tool-approval policy capsule shown next to the model pill. Built-in chat only: in a local-CLI
 * conversation the CLI's own permission levels are owned by the composer mode pill (single writer),
 * so `derivePermissionModes` returns an empty option list here and the button is hidden.
 */
function PermissionPickerBase({
  agentRuntime,
  approvalPolicy,
  onApprovalPolicyChange,
}: PermissionPickerProps) {
  const t = useT()
  const [open, setOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const maxH = usePopoverMaxHeight(open, menuRef, 'down', 320)

  const { options, current } = useMemo(
    () => derivePermissionModes({ target: 'titlebar', agentRuntime, approvalPolicy }),
    [agentRuntime, approvalPolicy],
  )

  if (options.length === 0) return null
  if (!onApprovalPolicyChange) return null

  const activeOption = options.find((option) => option.value === current)
  const currentLabel = activeOption?.label ?? t.chatPermission
  const CurrentIcon = activeOption?.icon ?? options[0].icon

  const pick = (value: string) => {
    onApprovalPolicyChange(value)
    setOpen(false)
  }

  return (
    <div className="relative" data-tauri-drag-region="false">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className={`${chatTitlebarIconButtonClass} ${
          open
            ? 'bg-black/[0.06] text-neutral-800 dark:bg-white/[0.09] dark:text-neutral-100'
            : 'hover:text-neutral-800 dark:hover:text-neutral-100'
        }`}
        title={t.chatApprovalPolicy.replace('{name}', currentLabel)}
        aria-label={t.chatApprovalPolicy.replace('{name}', currentLabel)}
      >
        <CurrentIcon size={16} strokeWidth={1.8} />
      </button>
      {open && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} aria-hidden />
          <div ref={menuRef} style={{ maxHeight: maxH }} className="chat-model-selector-menu chat-motion-popover absolute left-0 top-full z-20 mt-2 min-w-[180px] overflow-y-auto kv-menu">
            {options.map((option) => {
              const active = option.value === current
              return (
                <button
                  key={option.value}
                  type="button"
                  onClick={() => pick(option.value)}
                  className={`kv-menu-row justify-between transition-colors ${
                    active
                      ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100'
                      : 'text-neutral-700 hover:bg-neutral-50 dark:text-neutral-300 dark:hover:bg-neutral-800/80'
                  }`}
                >
                  <span>{option.label}</span>
                  {active && (
                    <Check size={15} className="shrink-0 text-neutral-500 dark:text-neutral-400" />
                  )}
                </button>
              )
            })}
          </div>
        </>
      )}
    </div>
  )
}

// memo：顶栏选择器，仅在 props 变化时重渲。
export const PermissionPicker = memo(PermissionPickerBase)
