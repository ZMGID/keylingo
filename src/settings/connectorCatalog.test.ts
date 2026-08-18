import { describe, expect, it } from 'vitest'
import { isPluginManagedServer } from './connectorCatalog'

describe('isPluginManagedServer', () => {
  it('hides plugin MCP rows from the connectors page', () => {
    expect(
      isPluginManagedServer({
        id: 'plugin-officecli',
        connectorId: 'plugin:officecli',
      }),
    ).toBe(true)
    expect(isPluginManagedServer({ id: 'plugin-ego', connectorId: null })).toBe(true)
  })

  it('keeps real connectors', () => {
    expect(isPluginManagedServer({ id: 'connector-notion', connectorId: 'notion' })).toBe(false)
    expect(isPluginManagedServer({ id: 'connector-custom-acme', connectorId: 'custom-acme' })).toBe(
      false,
    )
  })
})
