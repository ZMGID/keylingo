import { describe, expect, it } from 'vitest'
import { libraryTimestamp } from './format'
import type { ConversationListItem } from '../types'

function item(partial: Partial<ConversationListItem> = {}): ConversationListItem {
  return {
    id: 'c1',
    title: 't',
    preview: '',
    provider_id: 'p',
    model: 'm',
    message_count: 1,
    created_at: 100,
    updated_at: 200,
    pinned: false,
    ...partial,
  }
}

describe('libraryTimestamp', () => {
  it('uses created_at when sorting by created', () => {
    expect(libraryTimestamp(item(), 'created')).toBe(100)
  })

  it('uses updated_at for other sorts', () => {
    expect(libraryTimestamp(item(), 'updated')).toBe(200)
    expect(libraryTimestamp(item(), 'title')).toBe(200)
    expect(libraryTimestamp(item(), 'messages')).toBe(200)
  })

  it('falls back when the preferred field is missing', () => {
    expect(libraryTimestamp(item({ created_at: 0 }), 'created')).toBe(200)
    expect(libraryTimestamp(item({ updated_at: 0 }), 'updated')).toBe(100)
  })
})
