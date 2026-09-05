export type CliReport = { version: 1; agent: 'antigravity'; command: string; output: string }
type Quota = { group: string; window: string; remaining: number; reset: string }

export function parseCliReport(value: string): CliReport | null {
  try {
    const v: unknown = JSON.parse(value)
    if (!v || typeof v !== 'object') return null
    const r = v as Record<string, unknown>
    return r.version === 1 && r.agent === 'antigravity' && typeof r.command === 'string'
      && typeof r.output === 'string' ? r as CliReport : null
  } catch { return null }
}

export function parseQuotas(output: string): Quota[] {
  const lines = output.trim().split(/\r?\n/).filter(Boolean)
  const rows: Quota[] = []
  for (const line of lines) {
    const match = line.match(/^(.+?)\s+(Weekly|Five Hour) Limit Remaining\s+(\d+(?:\.\d+)?)%\s+(\S+)\s*$/)
    if (!match || Number(match[3]) > 100 || !Number.isFinite(Date.parse(match[4]))) return []
    rows.push({ group: match[1].trim(), window: match[2] === 'Weekly' ? '每周额度' : '5 小时额度', remaining: Number(match[3]), reset: match[4] })
  }
  return rows
}

// Exact old agy quota reports only; ordinary prose and incomplete streams are unchanged.
export function normalizeLegacyCliReport(content: string): string {
  return parseQuotas(content).length ? '```kivio-cli-report\n' + JSON.stringify({
    version: 1, agent: 'antigravity', command: 'usage', output: content,
  }) + '\n```' : content
}

