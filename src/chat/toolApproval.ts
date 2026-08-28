import type { ChatToolConfirmPayload } from '../api/tauri'

/** 工具名 → 自然语言动词。`path` 表示操作对象是文件路径（标题里只显示文件名）。 */
const TOOL_APPROVAL_VERBS: Record<string, { verb: string; path?: boolean }> = {
  write: { verb: '写入', path: true },
  write_file: { verb: '写入', path: true },
  edit: { verb: '修改', path: true },
  edit_file: { verb: '修改', path: true },
  notebookedit: { verb: '修改', path: true },
  read: { verb: '读取', path: true },
  read_file: { verb: '读取', path: true },
  bash: { verb: '执行' },
  run_command: { verb: '执行' },
}

/**
 * 审批卡标题。后端认出操作对象（`target`）时拼「允许写入 xxx.md？」，认不出就退回工具名。
 */
export function toolApprovalTitle(payload: ChatToolConfirmPayload): string {
  const name = (payload.name || '').toLowerCase()
  const target = payload.target?.trim()
  if (name === 'exitplanmode') return '批准这份计划，开始执行？'
  if (name === 'enterplanmode') return '让 claude 先出方案，暂不改动代码？'
  if (name === 'request_permissions' || name === 'permissions') {
    const wantsNetwork = (payload.argumentsPreview || '').includes('Network access')
    if (wantsNetwork && target) return `允许 Codex 联网并使用工作区 ${target}？`
    if (wantsNetwork) return '允许 Codex 联网？'
    if (target) return `允许 Codex 使用工作区 ${target}？`
    return '允许 Codex 使用工作区 / 执行环境？'
  }
  const spec = TOOL_APPROVAL_VERBS[name]
  if (!spec || !target) return `允许调用工具 ${payload.name}？`
  const shown = spec.path ? target.split(/[\\/]/).filter(Boolean).pop() || target : target
  return `允许${spec.verb} ${shown}？`
}

export function isPlanApproval(payload: ChatToolConfirmPayload): boolean {
  return (payload.name || '').toLowerCase() === 'exitplanmode'
}

export function isEnterPlanApproval(payload: ChatToolConfirmPayload): boolean {
  return (payload.name || '').toLowerCase() === 'enterplanmode'
}

export const PLAN_APPROVAL_ACTIONS: { label: string; mode: string }[] = [
  { label: '批准，逐步确认', mode: 'default' },
  { label: '批准并自动放行', mode: 'bypassPermissions' },
]
