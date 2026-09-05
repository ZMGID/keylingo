// @vitest-environment jsdom
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { CliCommandReport } from './CliCommandReport'
import { normalizeLegacyCliReport, parseCliReport, parseQuotas } from './cliCommandReportData'
import { ChatMarkdown } from './ChatMarkdown'

const output = 'Gemini Models\tWeekly Limit Remaining\t89%\t2026-09-11T15:23:59Z\nGemini Models\tFive Hour Limit Remaining\t0%\t2026-09-05T10:39:37Z\nClaude and GPT models\tWeekly Limit Remaining\t100%\t2026-09-12T07:09:02Z'

describe('CLI command panels', () => {
  it('renders old persisted quota text as remaining bars with reset times', () => {
    const { container } = render(<ChatMarkdown content={output} />)
    const bars = screen.getAllByRole('progressbar')
    expect(bars).toHaveLength(3)
    expect(bars[0]).toHaveAttribute('aria-valuenow', '89')
    expect(bars[1]).toHaveAttribute('aria-valuenow', '0')
    expect(bars[2]).toHaveAttribute('aria-valuenow', '100')
    expect(container.querySelector('time')).toHaveAttribute('datetime', '2026-09-11T15:23:59Z')
    expect(container.querySelector('details')).not.toHaveAttribute('open')
  })

  it('does not discard unrecognized rows or invent quota values', () => {
    for (const raw of [output + '\nUnexpected new quota type', output.replace('89%', '189%'), output.replace('2026-09-11T15:23:59Z', 'unknown')]) {
      expect(parseQuotas(raw)).toEqual([])
      expect(normalizeLegacyCliReport(raw)).toBe(raw)
    }
  })

  it('renders versioned reports and filters skills without rendering HTML', () => {
    const report = { version: 1 as const, agent: 'antigravity' as const, command: 'skills', output: 'review\tInspect changes\ndesign\t<script>alert(1)</script>' }
    render(<CliCommandReport report={report} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Inspect' } })
    expect(screen.getByRole('button', { name: '复制 review' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: '复制 design' })).toBeNull()
    expect(document.querySelector('script')).toBeNull()
    expect(parseCliReport(JSON.stringify(report))).toEqual(report)
    expect(parseCliReport('{"version":2}')).toBeNull()
  })

  it('uses the same panel for newly persisted fenced results', () => {
    render(<ChatMarkdown content={'```kivio-cli-report\n' + JSON.stringify({ version: 1, agent: 'antigravity', command: 'model', output: 'model-id\tModel display name' }) + '\n```'} />)
    expect(screen.getByRole('region', { name: '当前模型' })).toBeInTheDocument()
    expect(screen.getByText('Model display name')).toBeInTheDocument()
  })
})
