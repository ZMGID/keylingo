import { describe, expect, it } from 'vitest'
import {
  adoptFreshPluginManagedServers,
  isPluginManagedServer,
  preservePluginManagedServers,
} from './connectorCatalog'

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

describe('adoptFreshPluginManagedServers', () => {
  const plugin = { id: 'plugin-cua-driver', connectorId: 'plugin:cua-driver', enabled: true, command: 'cua-driver' }
  const user = { id: 'my-mcp', connectorId: null, enabled: true, command: 'npx' }

  it('takes plugin enabled from fresh so a settings draft cannot revive a disabled plugin', () => {
    const freshPlugin = { ...plugin, enabled: false }
    expect(adoptFreshPluginManagedServers([plugin, user], [freshPlugin, user])).toEqual([
      freshPlugin,
      user,
    ])
  })

  it('drops a plugin row that the backend uninstalled and appends a newly registered one', () => {
    const office = { id: 'plugin-officecli', connectorId: 'plugin:officecli', enabled: true }
    expect(adoptFreshPluginManagedServers([plugin, user], [office, user])).toEqual([user, office])
  })

  it('returns the same array when plugin rows are already the fresh objects', () => {
    const draft = [plugin, user]
    expect(adoptFreshPluginManagedServers(draft, draft)).toBe(draft)
  })

  it('returns the draft when fresh plugin rows are clones with the same switch state', () => {
    const draft = [plugin, user]
    expect(adoptFreshPluginManagedServers(draft, [{ ...plugin }, user])).toBe(draft)
  })
})
