import { Plus, Trash2 } from 'lucide-react'
import { Toggle, Input, SettingsGroup } from '../components'
import { IconButton } from '../../components/Button'
import { ProviderSortableList } from '../ProviderSortableList'
import { PROVIDER_PRESETS, type ProviderPreset } from '../providerPresets'
import { ProviderDetail } from './ProviderDetail'
import { isProviderEnabled } from '../utils'
import type { I18n, Lang } from '../i18n'
import type {
  Settings as SettingsData,
  ModelProvider,
} from '../../api/tauri'

/** 左栏：新增 + 可拖拽排序的供应商列表 + 未添加的快速预设。 */
function ProviderList({
  settings,
  t,
  lang,
  selectedProvider,
  onSelect,
  onReorder,
  onAdd,
  onAddFromPreset,
}: {
  settings: SettingsData
  t: I18n
  lang: Lang
  selectedProvider: ModelProvider | undefined
  onSelect: (id: string) => void
  onReorder: (fromId: string, toId: string) => void
  onAdd: () => void
  onAddFromPreset: (preset: ProviderPreset) => void
}) {
  return (
    <div className="kv-provider-list">
      <button
        type="button"
        onClick={onAdd}
        className="kv-provider-add"
        data-tauri-drag-region="false"
      >
        <Plus />
        {t.addProvider}
      </button>

      <ProviderSortableList
        providers={settings.providers}
        selectedId={selectedProvider?.id}
        lang={lang}
        providerNameLabel={t.providerName}
        onSelect={onSelect}
        onReorder={onReorder}
        trailing={PROVIDER_PRESETS
          .filter((preset) => !settings.providers.some((p) => p.baseUrl === preset.baseUrl))
          .map((preset) => (
            <button
              key={preset.name}
              type="button"
              onClick={() => onAddFromPreset(preset)}
              className="kv-provider-item"
              title={lang === 'zh' ? `添加 ${preset.name}` : `Add ${preset.name}`}
              data-tauri-drag-region="false"
            >
              <span className="kv-provider-item-select">
                <span className="kv-provider-dot off" />
                <span className="kv-provider-name">{preset.name}</span>
              </span>
            </button>
          ))}
      />
    </div>
  )
}

interface ProvidersTabProps {
  settings: SettingsData
  t: I18n
  lang: Lang
  selectedProvider: ModelProvider | undefined
  revealedKeys: Set<string>
  gzipInfoOpen: Set<string>
  fetchingProviderId: string | null
  onSelectProvider: (id: string) => void
  onReorderProviders: (fromId: string, toId: string) => void
  onAddProvider: () => void
  onAddProviderFromPreset: (preset: ProviderPreset) => void
  onUpdateProvider: (id: string, updates: Partial<ModelProvider>) => void
  onRequestDeleteProvider: (id: string) => void
  onToggleGzipInfo: (id: string) => void
  onToggleKeyReveal: (keyId: string) => void
  onOpenModelPicker: (id: string) => void
  onOpenModelTest: (id: string) => void
  onOpenModelDrawer: (target: { providerId: string; model: string }) => void
  onRemoveEnabledModel: (providerId: string, model: string) => void
}

/** 模型（供应商）标签页。纯展示：状态留在 SettingsShell。 */
export function ProvidersTab({
  settings,
  t,
  lang,
  selectedProvider,
  revealedKeys,
  gzipInfoOpen,
  fetchingProviderId,
  onSelectProvider,
  onReorderProviders,
  onAddProvider,
  onAddProviderFromPreset,
  onUpdateProvider,
  onRequestDeleteProvider,
  onToggleGzipInfo,
  onToggleKeyReveal,
  onOpenModelPicker,
  onOpenModelTest,
  onOpenModelDrawer,
  onRemoveEnabledModel,
}: ProvidersTabProps) {
  const configured = selectedProvider?.apiKeys.some((key) => key.trim())

  return (
    <div className="kv-providers-root">
      <div className="kv-providers">
        <ProviderList
          settings={settings}
          t={t}
          lang={lang}
          selectedProvider={selectedProvider}
          onSelect={onSelectProvider}
          onReorder={onReorderProviders}
          onAdd={onAddProvider}
          onAddFromPreset={onAddProviderFromPreset}
        />

        <div className="kv-provider-detail">
          <SettingsGroup title={lang === 'zh' ? '供应商' : 'Provider'} className="!pt-0 kv-provider-section">
            {selectedProvider ? (
              <div className="kv-provider-header">
                <div className="kv-provider-header-toolbar">
                  <span className="kv-row-label">{lang === 'zh' ? '启用供应商' : 'Enable provider'}</span>
                  <Toggle
                    checked={isProviderEnabled(selectedProvider)}
                    onChange={(enabled) => onUpdateProvider(selectedProvider.id, { enabled })}
                  />
                </div>
                <div className="kv-provider-header-toolbar">
                  <span className="kv-row-label">{t.providerName}</span>
                  <div className="kv-provider-header-actions">
                    <span className={`kv-tag ${!isProviderEnabled(selectedProvider) ? 'warn' : configured ? 'ok' : 'warn'}`}>
                      {!isProviderEnabled(selectedProvider)
                        ? (lang === 'zh' ? '已禁用' : 'Disabled')
                        : configured ? t.connectionOk : t.permissionMissing}
                    </span>
                    <IconButton
                      variant="danger"
                      size="xs"
                      onClick={() => onRequestDeleteProvider(selectedProvider.id)}
                      data-tauri-drag-region="false"
                      title={t.deleteProvider}
                      label={t.deleteProvider}
                    >
                      <Trash2 size={12} />
                    </IconButton>
                  </div>
                </div>
                <Input
                  value={selectedProvider.name}
                  onChange={(v) => onUpdateProvider(selectedProvider.id, { name: v })}
                  placeholder="Provider name"
                />
              </div>
            ) : (
              <p className="kv-provider-empty-hint">
                {lang === 'zh' ? '在左侧选择供应商，或点上方「添加」新建。' : 'Select a provider on the left, or click “Add” above.'}
              </p>
            )}
          </SettingsGroup>

          {selectedProvider ? (
            <ProviderDetail
              provider={selectedProvider}
              t={t}
              lang={lang}
              revealedKeys={revealedKeys}
              gzipInfoOpen={gzipInfoOpen}
              fetchingProviderId={fetchingProviderId}
              onUpdateProvider={onUpdateProvider}
              onToggleGzipInfo={onToggleGzipInfo}
              onToggleKeyReveal={onToggleKeyReveal}
              onOpenModelPicker={onOpenModelPicker}
              onOpenModelTest={onOpenModelTest}
              onOpenModelDrawer={onOpenModelDrawer}
              onRemoveEnabledModel={onRemoveEnabledModel}
            />
          ) : null}
        </div>
      </div>
    </div>
  )
}
