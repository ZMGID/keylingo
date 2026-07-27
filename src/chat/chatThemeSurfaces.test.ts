// @ts-expect-error Vitest runs in Node; the app intentionally does not include Node types.
import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

const css = readFileSync(new URL('../index.css', import.meta.url), 'utf8')

type Rgb = readonly [number, number, number]

const surfaceCases = [
  {
    selector: ':root',
    mainSource: [255, 255, 255],
    sidebarSource: [249, 249, 249],
    mainValue: 255,
    sidebarValue: 249,
  },
  {
    selector: ':root[data-theme-color="warm"]',
    mainSource: [247, 243, 234],
    sidebarSource: [236, 229, 212],
    mainValue: 255,
    sidebarValue: 249,
  },
  {
    selector: ':root[data-theme-color="cool"]',
    mainSource: [238, 243, 250],
    sidebarSource: [222, 232, 242],
    mainValue: 255,
    sidebarValue: 249,
  },
  {
    selector: ':root.dark',
    mainSource: [255, 255, 255],
    sidebarSource: [20, 20, 20],
    mainValue: 23,
    sidebarValue: 20,
  },
  {
    selector: ':root.dark[data-theme-color="warm"]',
    mainSource: [247, 243, 234],
    sidebarSource: [236, 229, 212],
    mainValue: 23,
    sidebarValue: 20,
  },
  {
    selector: ':root.dark[data-theme-color="cool"]',
    mainSource: [238, 243, 250],
    sidebarSource: [222, 232, 242],
    mainValue: 23,
    sidebarValue: 20,
  },
] as const

function scaleToValue(source: Rgb, targetValue: number): Rgb {
  const scale = targetValue / Math.max(...source)
  return source.map(channel => Math.round(channel * scale)) as unknown as Rgb
}

function toHex(rgb: Rgb): string {
  return `#${rgb.map(channel => channel.toString(16).padStart(2, '0')).join('')}`
}

function declarations(selector: string): Map<string, string> {
  const marker = `${selector} {`
  const start = css.indexOf(marker)
  expect(start, `missing CSS selector ${selector}`).toBeGreaterThanOrEqual(0)
  const end = css.indexOf('}', start)
  const entries = [...css.slice(start, end).matchAll(/(--[\w-]+):\s*([^;]+);/g)]
  return new Map(entries.map(([, name, value]) => [name, value.trim()]))
}

describe('chat theme surfaces', () => {
  it.each(surfaceCases)('derives $selector colors from the theme hue and target HSV value', surface => {
    const variables = declarations(surface.selector)
    const main = scaleToValue(surface.mainSource, surface.mainValue)
    const sidebar = scaleToValue(surface.sidebarSource, surface.sidebarValue)

    expect(variables.get('--chat-main-surface')).toBe(toHex(main))
    expect(variables.get('--chat-sidebar-surface')).toBe(sidebar.join(' '))
  })

  it('uses one sidebar RGB source for opaque fallback and native material alpha', () => {
    expect(css).toContain('background: rgb(var(--chat-sidebar-surface));')
    expect(css).toContain('background: rgb(var(--chat-sidebar-surface) / 0.72);')
    expect(css).toContain('background: rgb(var(--chat-sidebar-surface) / 0.66);')
    expect(css).not.toMatch(/\.chat-sidebar-shell\s*{[^}]*backdrop-filter/s)
  })
})
