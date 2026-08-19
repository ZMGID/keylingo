import { describe, expect, it } from 'vitest'
import { isPluginManagedServer, preservePluginManagedServers } from './connectorCatalog'

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

describe('preservePluginManagedServers', () => {
  const plugin = { id: 'plugin-cua-driver', connectorId: 'plugin:cua-driver', enabled: true, command: 'cua-driver' }
  const user = { id: 'my-mcp', connectorId: null, enabled: true, command: 'npx' }

  it('keeps plugin rows when the next list deletes or edits them', () => {
    const next = [
      { ...user, enabled: false },
      { ...plugin, enabled: false, command: 'hacked' },
    ]
    expect(preservePluginManagedServers([plugin, user], next)).toEqual([
      { ...user, enabled: false },
      plugin,
    ])
  })

  it('puts a deleted plugin row back', () => {
    expect(preservePluginManagedServers([plugin, user], [user])).toEqual([user, plugin])
  })
})
