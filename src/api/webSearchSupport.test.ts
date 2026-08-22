import { describe, expect, it } from 'vitest'
import { builtinWebSearchSupported, isOfficialDeepSeekApi } from './tauri'

describe('isOfficialDeepSeekApi', () => {
  it('matches api.deepseek.com only', () => {
    expect(isOfficialDeepSeekApi('https://api.deepseek.com')).toBe(true)
    expect(isOfficialDeepSeekApi('https://api.deepseek.com/v1')).toBe(true)
    expect(isOfficialDeepSeekApi('https://docs.deepseek.com')).toBe(false)
    expect(isOfficialDeepSeekApi('https://openrouter.ai/api/v1')).toBe(false)
  })
})

describe('builtinWebSearchSupported', () => {
  it('allows Responses / Gemini / Anthropic regardless of host', () => {
    expect(builtinWebSearchSupported('openai_responses')).toBe(true)
    expect(builtinWebSearchSupported('gemini')).toBe(true)
    expect(builtinWebSearchSupported('anthropic_messages')).toBe(true)
  })

  it('allows official DeepSeek Chat Completions, not relays', () => {
    expect(builtinWebSearchSupported('openai_chat', 'https://api.deepseek.com/v1')).toBe(true)
    expect(builtinWebSearchSupported('openai', 'https://api.deepseek.com')).toBe(true)
    expect(builtinWebSearchSupported('openai_chat', 'https://relay.example/v1')).toBe(false)
    expect(builtinWebSearchSupported('openai_chat')).toBe(false)
  })
})
