import {
  Plus, Minus, Trash2, RefreshCw, Eye, EyeOff, Info, Wrench, Brain,
  Image as ImageIcon,
} from 'lucide-react'
import { Toggle, Select, Input, SettingRow, SettingsGroup, FieldBlock } from '../components'
import { Button, IconButton } from '../../components/Button'
import { ModelIcon } from '../../chat/ModelIcon'
import { ProviderSortableList } from '../ProviderSortableList'
import { PROVIDER_PRESETS, type ProviderPreset } from '../providerPresets'
import { isProviderEnabled } from '../utils'
import { resolveModelInfo } from '../../data/modelMatching'
import { api } from '../../api/tauri'
import type { I18n, Lang } from '../i18n'
import type {
  Settings as SettingsData,
  ModelProvider,
} from '../../api/tauri'
import { normalizeProviderApiFormat } from '../../api/tauri'

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

/** 右栏：选中供应商的启用/命名 + 端点/协议/gzip/密钥池/模型列表。 */
function ProviderDetail({
  provider,
  t,
  lang,
  revealedKeys,
  gzipInfoOpen,
  fetchingProviderId,
  onUpdateProvider,
  onToggleGzipInfo,
  onToggleKeyReveal,
  onOpenModelPicker,
  onOpenModelTest,
  onOpenModelDrawer,
  onRemoveEnabledModel,
}: {
  provider: ModelProvider
  t: I18n
  lang: Lang
  revealedKeys: Set<string>
  gzipInfoOpen: Set<string>
  fetchingProviderId: string | null
  onUpdateProvider: (id: string, updates: Partial<ModelProvider>) => void
  onToggleGzipInfo: (id: string) => void
  onToggleKeyReveal: (keyId: string) => void
  onOpenModelPicker: (id: string) => void
  onOpenModelTest: (id: string) => void
  onOpenModelDrawer: (target: { providerId: string; model: string }) => void
  onRemoveEnabledModel: (providerId: string, model: string) => void
}) {
  return (
    <SettingsGroup title={lang === 'zh' ? '配置' : 'Configuration'}>
      <FieldBlock label={t.baseUrl}>
        <div className="kv-provider-endpoint-row">
          <Input
            className="min-w-0 flex-1"
            value={provider.baseUrl}
            onChange={(v) => onUpdateProvider(provider.id, { baseUrl: v })}
            placeholder="https://api.openai.com/v1"
            mono
          />
          <Select
            className="w-[11.5rem] shrink-0"
            value={normalizeProviderApiFormat(provider.apiFormat)}
            onChange={(apiFormat) => onUpdateProvider(provider.id, { apiFormat })}
            options={[
              { value: 'openai_chat', label: 'OpenAI Chat' },
              { value: 'openai_responses', label: 'OpenAI Responses' },
              { value: 'anthropic_messages', label: 'Anthropic' },
              { value: 'gemini', label: 'Gemini' },
            ]}
          />
        </div>
      </FieldBlock>

      <SettingRow
        label={
          <span className="flex flex-col gap-1">
            <span className="flex items-center gap-1">
              <span>{lang === 'zh' ? '压缩请求体 (gzip)' : 'Compress request body (gzip)'}</span>
              <IconButton
                size="xs"
                label={lang === 'zh' ? '显示说明' : 'Show details'}
                onClick={() => onToggleGzipInfo(provider.id)}
              >
                <Info size={12} />
              </IconButton>
            </span>
            {gzipInfoOpen.has(provider.id) && (
              <span className="kv-row-desc block mt-1">
                {lang === 'zh'
                  ? '个别供应商前置的 WAF 会扫描明文请求体，把工具/系统提示里的 shell 命令、文件路径等文本误判为攻击而返回 403。开启后请求体用 gzip 压缩发送（多数网关可正常解压）。若该供应商不接受 gzip 请求（如官方 DeepSeek）会返回 400，请保持关闭。'
                  : 'Some providers sit behind a WAF that scans the plaintext request body and returns 403 for shell/path text inside tool or system-prompt content. Enable to gzip the request body (most gateways accept it). Keep off for providers that reject gzip requests (e.g. official DeepSeek), which would return 400.'}
              </span>
            )}
          </span>
        }
      >
        <Toggle
          checked={provider.compressRequestBody === true}
          onChange={(v) => onUpdateProvider(provider.id, { compressRequestBody: v })}
        />
      </SettingRow>

      <FieldBlock label={t.apiKey} description={t.apiKeysHint}>
        <div className="space-y-1.5">
          {(() => {
            // 命中快速预设 baseUrl 时，给出「获取 API Key」外链引导用户申请。
            const preset = PROVIDER_PRESETS.find(
              (p) => p.baseUrl === provider.baseUrl && p.apiKeyUrl,
            )
            if (!preset?.apiKeyUrl) return null
            return (
              <button
                type="button"
                onClick={() => void api.openExternal(preset.apiKeyUrl!)}
                className="inline-flex w-fit items-center gap-0.5 text-[12px] text-indigo-500 hover:underline dark:text-indigo-300"
                data-tauri-drag-region="false"
              >
                {lang === 'zh' ? `获取 ${preset.name} API Key ↗` : `Get ${preset.name} API key ↗`}
              </button>
            )
          })()}
          {(provider.apiKeys.length > 0 ? provider.apiKeys : ['']).map((key, idx) => {
            const total = Math.max(provider.apiKeys.length, 1)
            const keyId = `${provider.id}-${idx}`
            const revealed = revealedKeys.has(keyId)
            return (
              <div key={`${provider.id}-${total}-${idx}`} className="flex items-center gap-1.5">
                <Input
                  type={revealed ? 'text' : 'password'}
                  value={key}
                  mono
                  onChange={(v) => {
                    const base = provider.apiKeys.length > 0 ? [...provider.apiKeys] : ['']
                    base[idx] = v
                    onUpdateProvider(provider.id, { apiKeys: base })
                  }}
                  placeholder={idx === 0 ? `sk-... (${t.apiKeyPrimary})` : `sk-... (${t.apiKeyBackup})`}
                />
                <IconButton
                  size="xs"
                  onClick={() => onToggleKeyReveal(keyId)}
                  title={revealed ? (lang === 'zh' ? '隐藏密钥' : 'Hide key') : (lang === 'zh' ? '显示密钥' : 'Show key')}
                  label={revealed ? (lang === 'zh' ? '隐藏密钥' : 'Hide key') : (lang === 'zh' ? '显示密钥' : 'Show key')}
                  data-tauri-drag-region="false"
                >
                  {revealed ? <EyeOff size={12} /> : <Eye size={12} />}
                </IconButton>
                {total > 1 && (
                  <IconButton
                    variant="danger"
                    size="xs"
                    onClick={() => {
                      const next = provider.apiKeys.filter((_, i) => i !== idx)
                      onUpdateProvider(provider.id, { apiKeys: next })
                    }}
                    title={t.removeKey}
                    label={t.removeKey}
                    data-tauri-drag-region="false"
                  >
                    <Trash2 size={12} />
                  </IconButton>
                )}
              </div>
            )
          })}
        </div>
        <Button
          size="sm"
          className="mt-2"
          onClick={() => {
            const base = provider.apiKeys.length > 0 ? provider.apiKeys : ['']
            onUpdateProvider(provider.id, { apiKeys: [...base, ''] })
          }}
          data-tauri-drag-region="false"
        >
          <Plus size={11} />
          {t.addKey}
        </Button>
      </FieldBlock>

      <div className="kv-row">
        <div className="kv-row-text">
          <span className="kv-row-label">{t.testConnection}</span>
        </div>
        <div className="kv-row-control kv-row-control-cluster">
          <Button
            size="sm"
            onClick={() => onOpenModelPicker(provider.id)}
            data-tauri-drag-region="false"
          >
            <RefreshCw size={10} className={fetchingProviderId === provider.id ? 'animate-spin' : ''} />
            {provider.availableModels.length > 0
              ? (lang === 'zh' ? '管理模型' : 'Models')
              : t.fetchModels}
          </Button>
          <Button
            size="sm"
            onClick={() => onOpenModelTest(provider.id)}
            data-tauri-drag-region="false"
          >
            <RefreshCw size={10} />
            {t.testConnection}
          </Button>
        </div>
      </div>

      <FieldBlock
        label={(
          <span className="inline-flex items-center gap-2">
            <span>{lang === 'zh' ? '模型' : 'Models'}</span>
            <span className="kv-tag">{provider.enabledModels.length}</span>
          </span>
        )}
      >
        <ul className="kv-enabled-model-list">
          {provider.enabledModels.length === 0 && (
            <li className="kv-enabled-model-empty">
              {lang === 'zh' ? '点击上方「获取模型列表」拉取并添加模型。' : 'Use "Fetch Models" above to load and add models.'}
            </li>
          )}
          {provider.enabledModels.map(model => {
            const caps = resolveModelInfo(model, provider.modelOverrides).capabilities
            return (
              <li key={model} className="kv-enabled-model-row" onClick={() => onOpenModelDrawer({ providerId: provider.id, model })}>
                <ModelIcon model={model} size={16} />
                <span className="kv-enabled-model-name" title={model}>{model}</span>
                <span className="kv-enabled-model-badges">
                  {caps?.vision && (
                    <span className="kv-badge-mini kv-badge-mini--vision" title={lang === 'zh' ? '视觉' : 'Vision'}>
                      <Eye size={11} strokeWidth={2} />
                    </span>
                  )}
                  {caps?.functionCalling && (
                    <span className="kv-badge-mini kv-badge-mini--tools" title={lang === 'zh' ? '工具调用' : 'Tools'}>
                      <Wrench size={11} strokeWidth={2} />
                    </span>
                  )}
                  {caps?.reasoning && (
                    <span className="kv-badge-mini kv-badge-mini--reasoning" title={lang === 'zh' ? '推理' : 'Reasoning'}>
                      <Brain size={11} strokeWidth={2} />
                    </span>
                  )}
                  {caps?.imageGeneration && (
                    <span className="kv-badge-mini kv-badge-mini--image" title={lang === 'zh' ? '生图' : 'Image generation'}>
                      <ImageIcon size={11} strokeWidth={2} />
                    </span>
                  )}
                </span>
                <button
                  type="button"
                  onClick={(e) => { e.stopPropagation(); onRemoveEnabledModel(provider.id, model) }}
                  className="kv-enabled-model-remove"
                  data-tauri-drag-region="false"
                  aria-label={t.removeModel}
                >
                  <Minus size={14} />
                </button>
              </li>
            )
          })}
        </ul>
      </FieldBlock>
    </SettingsGroup>
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
