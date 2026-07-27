import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { Check, Eye, FilePen, ShieldAlert, ShieldCheck, ShieldQuestion } from 'lucide-react'
import { APPROVAL_POLICY_OPTIONS } from './approvalPolicies'
import { chatApi, type DetectedExternalAgent } from './api'
import { chatTitlebarIconButtonClass } from './platform'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import type { AgentRuntimeConfig } from './types'

interface Option {
  value: string
  label: string
}

/** Distinct icon per permission level so the capsule reflects the active mode at a glance.
 *  Covers built-in approval policies (by value) and external CLI sandbox levels (by label). */
function modeIcon(value: string, label: string) {
  if (value === 'always_confirm') return ShieldAlert
  if (value === 'readonly_auto_sensitive_confirm') return ShieldQuestion
  if (value === 'auto') return ShieldCheck
  if (/计划|只读|read|plan/i.test(label)) return Eye
  if (/编辑|edit/i.test(label)) return FilePen
  if (/完全|默认|full|default/i.test(label)) return ShieldCheck
  return ShieldAlert
}

interface PermissionPickerProps {
  agentRuntime: AgentRuntimeConfig
  conversationId?: string | null
  /** Set the external-CLI sandbox level (claude --permission-mode / codex --sandbox). */
  onSandboxChange: (level: string) => void
  /** Built-in agent tool-approval policy. */
  approvalPolicy?: string
  onApprovalPolicyChange?: (policy: string) => void
}

/**
 * Unified permission capsule shown next to the model pill. Adapts to the active runtime:
 * external CLI → sandbox level (per-agent); built-in chat → tool-approval policy. Hidden when
 * there is nothing to control (e.g. an external CLI with no sandbox flag).
 */
function PermissionPickerBase({
  agentRuntime,
  conversationId,
  onSandboxChange,
  approvalPolicy,
  onApprovalPolicyChange,
}: PermissionPickerProps) {
  const [open, setOpen] = useState(false)
  const [agents, setAgents] = useState<DetectedExternalAgent[]>([])
  const menuRef = useRef<HTMLDivElement>(null)
  const maxH = usePopoverMaxHeight(open, menuRef, 'down', 320)

  useEffect(() => {
    let active = true
    void chatApi.detectExternalAgents(false, conversationId)
      .then((list) => {
        if (active) setAgents(list)
      })
      .catch(() => {
        if (active) setAgents([])
      })
    return () => {
      active = false
    }
  }, [conversationId])

  const usesExternal = agentRuntime.kind === 'external' && !!agentRuntime.externalAgentId
  const agent = agents.find((item) => item.id === agentRuntime.externalAgentId)
  const sandboxOptions = useMemo(
    () => agent?.sandboxOptions ?? agent?.sandbox_options ?? [],
    [agent],
  )

  const options: Option[] = useMemo(
    () =>
      usesExternal
        ? sandboxOptions.map((o) => ({ value: o.id, label: o.label }))
        : APPROVAL_POLICY_OPTIONS.map((o) => ({ value: o.value, label: o.label })),
    [usesExternal, sandboxOptions],
  )

  const current = useMemo(() => {
    if (usesExternal) {
      if (agentRuntime.externalSandbox) return agentRuntime.externalSandbox
      const def = sandboxOptions.find((o) => o.label.includes('默认')) ?? sandboxOptions[0]
      return def?.id ?? ''
    }
    return approvalPolicy ?? APPROVAL_POLICY_OPTIONS[1]?.value ?? ''
  }, [usesExternal, agentRuntime.externalSandbox, sandboxOptions, approvalPolicy])

  if (usesExternal && options.length === 0) return null
  if (!usesExternal && !onApprovalPolicyChange) return null

  const currentLabel = options.find((o) => o.value === current)?.label ?? '权限'
  const CurrentIcon = modeIcon(current, currentLabel)

  const pick = (value: string) => {
    if (usesExternal) {
      onSandboxChange(value)
    } else {
      onApprovalPolicyChange?.(value)
    }
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
        title={`${usesExternal ? '沙盒 / 权限等级' : '工具审批策略'}：${currentLabel}`}
        aria-label={`${usesExternal ? '沙盒 / 权限等级' : '工具审批策略'}：${currentLabel}`}
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
