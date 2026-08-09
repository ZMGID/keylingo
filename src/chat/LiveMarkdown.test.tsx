import { render } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import {
  getLiveMarkdownParsedCharacterCount,
  resetLiveMarkdownDiagnostics,
} from './liveMarkdownDiagnostics'
import { LiveMarkdown } from './LiveMarkdown'

describe('LiveMarkdown incremental parsing', () => {
  beforeEach(() => resetLiveMarkdownDiagnostics())

  it('appends plain streaming chunks without rescanning the full value', () => {
    const chunks = Array.from({ length: 200 }, (_, index) => `chunk${index} `)
    let value = chunks[0]
    const view = render(<LiveMarkdown value={value} />)
    for (const chunk of chunks.slice(1)) {
      value += chunk
      view.rerender(<LiveMarkdown value={value} />)
    }

    expect(view.container.textContent).toBe(value.trim())
    expect(getLiveMarkdownParsedCharacterCount()).toBeLessThan(value.length * 2)
  })

  it('preserves arbitrary chunk boundaries without inserting spaces', () => {
    const view = render(<LiveMarkdown value="hel" />)
    view.rerender(<LiveMarkdown value="hello" />)
    expect(view.container.textContent).toBe('hello')
  })

  it('keeps an open 20k code stream incrementally parsed and DOM-bounded', () => {
    const prefix = '```typescript\n'
    let value = prefix
    const view = render(<LiveMarkdown value={value} />)
    for (let index = 0; index < 200; index += 1) {
      value += `${String(index).padStart(3, '0')}:${'x'.repeat(92)}\n`
      view.rerender(<LiveMarkdown value={value} />)
    }

    const rendered = view.container.querySelector('code')?.textContent ?? ''
    expect(value.length).toBeGreaterThan(19_000)
    expect(rendered.length).toBeLessThanOrEqual(10_005)
    expect(rendered).toContain('000:')
    expect(rendered).toContain('199:')
    expect(rendered).toContain('…')
    expect(getLiveMarkdownParsedCharacterCount()).toBeLessThan(1_000)
  })
})
