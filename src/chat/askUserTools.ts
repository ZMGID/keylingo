/** 折叠工具名：小写并去掉 `_` / `-` / 空格。
 *  `AskUserQuestion` 与 `ask_user_question` 都会变成 `askuserquestion`。 */
export function foldToolName(name: string): string {
  return name.toLowerCase().replace(/[_\-\s]/g, '')
}

/**
 * 这条工具调用该不该渲染成 Kivio 的问用户卡片。
 *
 * 后端 `external_agents/ask_user.rs` 的 CODECS 加工具名时，这里也要加一条，
 * 否则会渲染成普通工具卡、用户没法答。`askUser` 结构化载荷是兜底
 * （工具名被改过 / 缺失时仍认得出）。
 *
 * **不要**把折叠后的 `exitplanmode` 一律当成问用户：claude 的 `ExitPlanMode`
 * 是计划审批卡；只有 dsh 的 `exit_plan_mode` 走问用户缝。
 */
export function isAskUserToolName(name: string): boolean {
  const folded = foldToolName(name)
  if (folded === 'askuser' || folded === 'askuserquestion' || folded === 'requestuserinput') {
    return true
  }
  return name.toLowerCase() === 'exit_plan_mode'
}

export function hasAskUserStructuredContent(value: unknown): boolean {
  return Boolean(
    value
    && typeof value === 'object'
    && 'askUser' in (value as Record<string, unknown>),
  )
}
