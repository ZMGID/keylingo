import { providerHasCredentials } from '../../api/tauri'
import { ArrowDownAZ, Heart, Plus, Search, Trash2, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
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

/** 左栏：一个添加按钮打开预设弹层（含自定义），下面是可拖拽的已添加供应商。 */
function ProviderList({
  settings,
  t,
  lang,
  selectedProvider,
  onSelect,
  onReorder,
  onOpenPresets,
}: {
  settings: SettingsData
  t: I18n
  lang: Lang
  selectedProvider: ModelProvider | undefined
  onSelect: (id: string) => void
  onReorder: (fromId: string, toId: string) => void
  onOpenPresets: () => void
}) {
  return (
    <div className="kv-provider-list kv-split-list">
      <div className="kv-provider-list-actions">
        <button
          type="button"
          onClick={onOpenPresets}
          className="kv-provider-add kv-provider-add--preset"
          title={t.presetProvidersHint}
          data-tauri-drag-region="false"
        >
          <Plus />
          {t.addProvider}
        </button>
      </div>

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
  const configured = selectedProvider && providerHasCredentials(selectedProvider)
  const [iconPickerOpen, setIconPickerOpen] = useState(false)
  const [presetPickerOpen, setPresetPickerOpen] = useState(false)
  const [presetQuery, setPresetQuery] = useState('')
  const [presetSortAz, setPresetSortAz] = useState(false)
  // 切供应商时收起：展开着的选择器 / 预设弹层会直接作用到新选中的那个上。
  useEffect(() => {
    setIconPickerOpen(false)
    setPresetPickerOpen(false)
    setPresetQuery('')
    setPresetSortAz(false)
  }, [selectedProvider?.id])

  const closePresetPicker = () => {
    setPresetPickerOpen(false)
    setPresetQuery('')
    setPresetSortAz(false)
  }

  const addFromPreset = (preset: ProviderPreset) => {
    onAddProviderFromPreset(preset)
    closePresetPicker()
  }

  const addCustomProvider = () => {
    onAddProvider()
    closePresetPicker()
  }

  const presetMatches = useMemo(() => {
    const q = presetQuery.trim().toLowerCase()
    let list = PROVIDER_PRESETS
    if (q) {
      list = list.filter(
        (preset) =>
          preset.name.toLowerCase().includes(q) || preset.baseUrl.toLowerCase().includes(q),
      )
    }
    const sponsored = list.filter((preset) => preset.sponsored)
    const rest = list.filter((preset) => !preset.sponsored)
    if (presetSortAz) {
      rest.sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }))
    }
    return [...sponsored, ...rest]
  }, [presetQuery, presetSortAz])

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
              <div className="kv-provider-preset-picker-heading">
                <h3 id="kv-provider-preset-picker-title" className="kv-provider-preset-picker-title">
                  {t.presetProviders}
                </h3>
                <p className="kv-provider-preset-picker-hint">{t.presetProvidersHint}</p>
              </div>
              <IconButton
                size="xs"
                onClick={closePresetPicker}
                data-tauri-drag-region="false"
                label={lang === 'zh' ? '关闭' : 'Close'}
              >
                <X size={14} />
              </IconButton>
            </div>
            <div className="kv-provider-preset-picker-toolbar">
              <div className="kv-provider-preset-picker-search">
                <Search size={14} className="kv-provider-preset-picker-search-icon" />
                <Input
                  value={presetQuery}
                  onChange={setPresetQuery}
                  placeholder={t.presetProvidersSearch}
                  mono={false}
                />
              </div>
              <IconButton
                size="sm"
                className={presetSortAz ? 'is-active' : ''}
                onClick={() => setPresetSortAz((on) => !on)}
                data-tauri-drag-region="false"
                aria-pressed={presetSortAz}
                label={t.presetSortAz}
              >
                <ArrowDownAZ size={14} />
              </IconButton>
            </div>
            <div className="kv-provider-preset-picker-body custom-scrollbar">
              <button
                type="button"
                className="kv-provider-preset-tile is-custom"
                onClick={addCustomProvider}
                data-tauri-drag-region="false"
              >
                <Plus size={16} strokeWidth={2.25} />
                <span className="kv-provider-preset-tile-name">{t.presetCustom}</span>
              </button>
              {presetMatches.map((preset) => (
                <button
                  key={preset.name}
                  type="button"
                  className="kv-provider-preset-tile"
                  title={preset.baseUrl}
                  onClick={() => addFromPreset(preset)}
                  data-tauri-drag-region="false"
                >
                  <ProviderIcon name={preset.name} baseUrl={preset.baseUrl} size={18} />
                  <span className="kv-provider-preset-tile-name">{preset.name}</span>
                  {preset.sponsored ? (
                    <Heart
                      size={12}
                      strokeWidth={2}
                      fill="currentColor"
                      className="kv-provider-preset-tile-heart"
                      aria-label={t.presetSponsored}
                    />
                  ) : null}
                </button>
              ))}
              {presetMatches.length === 0 && (
                <p className="kv-provider-preset-picker-empty">{t.presetNoSearchResults}</p>
              )}
            </div>
            <p className="kv-provider-preset-picker-foot">{t.presetCustomHint}</p>
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
          onOpenPresets={() => setPresetPickerOpen(true)}
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
              <p className="kv-provider-empty-hint">
                {lang === 'zh' ? '在左侧选择供应商，或点「添加驱动」从预设加入。' : 'Select a provider on the left, or click Add to pick a preset.'}
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
