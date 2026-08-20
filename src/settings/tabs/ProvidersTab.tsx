import { ChevronRight, Plus, Trash2, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { Toggle, Input, SettingsGroup } from '../components'
import { IconButton } from '../../components/Button'
import { ProviderSortableList } from '../ProviderSortableList'
import { ProviderIcon, PROVIDER_PICKER_KEYS } from '../../chat/ModelIcon'
import { PROVIDER_PRESETS, type ProviderPreset } from '../providerPresets'
import { ProviderDetail } from './ProviderDetail'
import { isProviderEnabled } from '../utils'
import type { I18n, Lang } from '../i18n'
import type {
  Settings as SettingsData,
  ModelProvider,
} from '../../api/tauri'

/** 左栏：新增 + 可拖拽排序的已添加供应商。预设走弹层，不和真实供应商挤在一起。 */
function ProviderList({
  settings,
  t,
  lang,
  selectedProvider,
  onSelect,
  onReorder,
  onAdd,
}: {
  settings: SettingsData
  t: I18n
  lang: Lang
  selectedProvider: ModelProvider | undefined
  onSelect: (id: string) => void
  onReorder: (fromId: string, toId: string) => void
  onAdd: () => void
}) {
  return (
    <div className="kv-provider-list kv-split-list">
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
        icons={settings.providerIcons}
        onSelect={onSelect}
        onReorder={onReorder}
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
  onSetProviderIcon: (id: string, dataUrl: string) => void
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
  onSetProviderIcon,
  onRequestDeleteProvider,
  onToggleGzipInfo,
  onToggleKeyReveal,
  onOpenModelPicker,
  onOpenModelTest,
  onOpenModelDrawer,
  onRemoveEnabledModel,
}: ProvidersTabProps) {
  const configured = selectedProvider?.apiKeys.some((key) => key.trim())
  const [iconPickerOpen, setIconPickerOpen] = useState(false)
  const [presetPickerOpen, setPresetPickerOpen] = useState(false)
  // 切供应商时收起：展开着的选择器 / 预设弹层会直接作用到新选中的那个上。
  useEffect(() => {
    setIconPickerOpen(false)
    setPresetPickerOpen(false)
  }, [selectedProvider?.id])

  const closePresetPicker = () => setPresetPickerOpen(false)

  const addFromPreset = (preset: ProviderPreset) => {
    onAddProviderFromPreset(preset)
    closePresetPicker()
  }

  const presetEntry = (
    <button
      type="button"
      onClick={() => setPresetPickerOpen(true)}
      className="kv-subpage-entry kv-provider-preset-entry"
      data-tauri-drag-region="false"
    >
      <span className="kv-subpage-entry-body">
        <span className="kv-subpage-entry-title">{t.presetProviders}</span>
        <span className="kv-subpage-entry-hint">{t.presetProvidersHint}</span>
      </span>
      <span className="kv-subpage-entry-go">
        <ChevronRight size={14} aria-hidden="true" />
      </span>
    </button>
  )

  const presetPicker = presetPickerOpen
    ? createPortal(
        <div
          className="kv-modal-backdrop kv-modal-backdrop--portal"
          data-tauri-drag-region="false"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) closePresetPicker()
          }}
        >
          <div
            className="kv kv-modal kv-provider-preset-picker"
            role="dialog"
            aria-modal="true"
            aria-labelledby="kv-provider-preset-picker-title"
            data-tauri-drag-region="false"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div className="kv-provider-preset-picker-header">
              <h3 id="kv-provider-preset-picker-title" className="kv-provider-preset-picker-title">
                {t.presetProviders}
              </h3>
              <IconButton
                size="xs"
                onClick={closePresetPicker}
                data-tauri-drag-region="false"
                label={lang === 'zh' ? '关闭' : 'Close'}
              >
                <X size={14} />
              </IconButton>
            </div>
            <div className="kv-provider-preset-picker-body custom-scrollbar">
              {PROVIDER_PRESETS.map((preset) => (
                <button
                  key={preset.name}
                  type="button"
                  className="kv-provider-preset-row"
                  onClick={() => addFromPreset(preset)}
                  data-tauri-drag-region="false"
                >
                  <ProviderIcon name={preset.name} baseUrl={preset.baseUrl} size={22} />
                  <span className="kv-provider-preset-row-body">
                    <span className="kv-provider-preset-row-name">{preset.name}</span>
                    <span className="kv-provider-preset-row-url">{preset.baseUrl}</span>
                  </span>
                </button>
              ))}
            </div>
          </div>
        </div>,
        document.body,
      )
    : null

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
                {presetEntry}
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
                <div className="flex items-center gap-2.5">
                  <button
                    type="button"
                    onClick={() => setIconPickerOpen((v) => !v)}
                    title={lang === 'zh' ? '选择图标' : 'Choose icon'}
                    data-tauri-drag-region="false"
                    className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-white ring-1 ring-black/[0.06] transition hover:ring-black/20 dark:bg-neutral-900 dark:ring-white/[0.08] dark:hover:ring-white/25"
                  >
                    <ProviderIcon
                      name={selectedProvider.name}
                      baseUrl={selectedProvider.baseUrl}
                      iconKey={settings.providerIcons?.[selectedProvider.id]}
                      size={20}
                    />
                  </button>
                  <Input
                    className="min-w-0 flex-1"
                    value={selectedProvider.name}
                    onChange={(v) => onUpdateProvider(selectedProvider.id, { name: v })}
                    placeholder="Provider name"
                  />
                </div>
                {iconPickerOpen && (
                  <div className="rounded-lg bg-black/[0.02] p-2 ring-1 ring-black/[0.06] dark:bg-white/[0.03] dark:ring-white/[0.08]">
                    <div className="grid grid-cols-[repeat(auto-fill,minmax(2rem,1fr))] gap-1">
                      <button
                        type="button"
                        title={lang === 'zh' ? '自动匹配' : 'Auto'}
                        onClick={() => {
                          onSetProviderIcon(selectedProvider.id, '')
                          setIconPickerOpen(false)
                        }}
                        data-tauri-drag-region="false"
                        className={`flex h-8 items-center justify-center rounded-md text-[10px] text-neutral-500 hover:bg-black/[0.06] dark:text-neutral-400 dark:hover:bg-white/[0.08] ${
                          settings.providerIcons?.[selectedProvider.id] ? '' : 'bg-black/[0.07] dark:bg-white/[0.1]'
                        }`}
                      >
                        {lang === 'zh' ? '自动' : 'Auto'}
                      </button>
                      {PROVIDER_PICKER_KEYS.map((key) => (
                        <button
                          key={key}
                          type="button"
                          title={key}
                          onClick={() => {
                            onSetProviderIcon(selectedProvider.id, key)
                            setIconPickerOpen(false)
                          }}
                          data-tauri-drag-region="false"
                          className={`flex h-8 items-center justify-center rounded-md hover:bg-black/[0.06] dark:hover:bg-white/[0.08] ${
                            settings.providerIcons?.[selectedProvider.id] === key ? 'bg-black/[0.07] dark:bg-white/[0.1]' : ''
                          }`}
                        >
                          <ProviderIcon name={key} iconKey={key} size={18} />
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            ) : (
              <>
                {presetEntry}
                <p className="kv-provider-empty-hint">
                  {lang === 'zh' ? '在左侧选择供应商，或点上方「添加」新建。' : 'Select a provider on the left, or click “Add” above.'}
                </p>
              </>
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
      {presetPicker}
    </div>
  )
}
