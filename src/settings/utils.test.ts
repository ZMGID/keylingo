import { describe, expect, it } from 'vitest'
import { buildModelPairOptions, formatHotkey, modelPairValue, parseModelPairValue } from './utils'
import type { ModelProvider } from '../api/tauri'

describe('formatHotkey', () => {
  it('renders macOS modifier glyphs', () => {
    expect(formatHotkey('CommandOrControl+Shift+T', 'macos')).toEqual(['⌘', '⇧', 'T'])
    expect(formatHotkey('Alt+Space', 'macos')).toEqual(['⌥', 'Space'])
  })

  it('renders Windows modifier labels', () => {
    expect(formatHotkey('CommandOrControl+Shift+T', 'windows')).toEqual(['Ctrl', 'Shift', 'T'])
    expect(formatHotkey('Alt+Space', 'windows')).toEqual(['Alt', 'Space'])
  })

  it('maps arrow keys to symbols on macOS', () => {
    expect(formatHotkey('ArrowUp', 'macos')).toEqual(['↑'])
    expect(formatHotkey('ArrowDown', 'macos')).toEqual(['↓'])
  })
})

describe('modelPairValue', () => {
  it('serializes provider and model as JSON array', () => {
    expect(modelPairValue('openai', 'gpt-4o')).toBe('["openai","gpt-4o"]')
  })
})

describe('parseModelPairValue', () => {
  it('parses JSON array values', () => {
    expect(parseModelPairValue('["openai","gpt-4o"]')).toEqual(['openai', 'gpt-4o'])
  })

  it('parses legacy provider:model values', () => {
    expect(parseModelPairValue('openai:gpt-4o')).toEqual(['openai', 'gpt-4o'])
  })

  it('returns model-less pair when no separator exists', () => {
    expect(parseModelPairValue('openai')).toEqual(['openai', ''])
  })
})

describe('buildModelPairOptions', () => {
  const provider = {
    id: 'p1',
    name: 'Proxy',
    enabled: true,
    enabledModels: ['gemini-3.1-flash-image', 'gpt-5.6', 'grok-4.5'],
  } as unknown as ModelProvider

  it('lists all enabled models without a filter', () => {
    expect(buildModelPairOptions([provider]).map(o => o.value)).toEqual([
      modelPairValue('p1', 'gemini-3.1-flash-image'),
      modelPairValue('p1', 'gpt-5.6'),
      modelPairValue('p1', 'grok-4.5'),
    ])
  })

  it('keeps only models passing the filter predicate', () => {
    const opts = buildModelPairOptions([provider], (_p, model) => model.includes('image'))
    expect(opts.map(o => o.value)).toEqual([modelPairValue('p1', 'gemini-3.1-flash-image')])
  })
})
