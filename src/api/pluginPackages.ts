import { invoke } from '@tauri-apps/api/core'

export type PluginPackage = {
  id: string
  name: string
  description: string
  version: string | null
  format: string
  source: string
  revision: string | null
  enabled: boolean
  components: Record<string, number>
  diagnostics: string[]
}

export const packageApi = {
  list: () => invoke<PluginPackage[]>('plugin_packages_list'),
  import: (source: string, subdirectory?: string) => invoke<PluginPackage>('plugin_packages_import', { source, subdirectory: subdirectory || null }),
  setEnabled: (id: string, enabled: boolean) => invoke<PluginPackage>('plugin_packages_set_enabled', { id, enabled }),
  remove: (id: string) => invoke<void>('plugin_packages_remove', { id }),
  getHooks: () => invoke<unknown>('workflow_hooks_get'),
  saveHooks: (config: unknown) => invoke<void>('workflow_hooks_save', { config }),
}
