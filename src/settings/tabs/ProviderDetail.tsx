import { useEffect, useState } from 'react'
import {
  Plus, Minus, Trash2, RefreshCw, Eye, EyeOff, Wrench, Brain,
  ArrowLeft, ChevronRight, Globe,
  Image as ImageIcon,
} from 'lucide-react'
import { Select, Input, SettingsGroup, FieldBlock } from '../components'
import { Button, IconButton } from '../../components/Button'
import { ModelIcon } from '../../chat/ModelIcon'
import { PROVIDER_PRESETS } from '../providerPresets'
import { ProviderRequestPanel } from '../ProviderRequestPanel'
import { resolveModelInfo } from '../../data/modelMatching'
import { api, normalizeProviderApiFormat } from '../../api/tauri'
import type { I18n, Lang } from '../i18n'
import type { ModelProvider } from '../../api/tauri'

/** 右栏：选中供应商的端点/协议/gzip/密钥池/模型列表，以及通往「请求配置」二级页的入口。 */
export function ProviderDetail({
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
  const [showRequestPage, setShowRequestPage] = useState(false)
  // 切换供应商时退回一级页：否则会停在二级页上、悄悄改到另一个供应商的头。
  useEffect(() => setShowRequestPage(false), [provider.id])

  if (showRequestPage) {
    return (
      <>
        <button
          type="button"
          onClick={() => setShowRequestPage(false)}
          className="kv-subpage-back"
          data-tauri-drag-region="false"
        >
          <ArrowLeft size={14} />
          <span>{lang === 'zh' ? '返回' : 'Back'}</span>
          <span className="kv-subpage-back-title">{t.requestConfig}</span>
        </button>
        <ProviderRequestPanel
          provider={provider}
          t={t}
          lang={lang}
          gzipInfoOpen={gzipInfoOpen}
          onToggleGzipInfo={onToggleGzipInfo}
          onUpdateProvider={onUpdateProvider}
        />
      </>
    )
  }

  return (
    <>
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

      <button
        type="button"
        onClick={() => setShowRequestPage(true)}
        className="kv-subpage-entry"
        data-tauri-drag-region="false"
      >
        <span className="kv-subpage-entry-icon">
          <Globe size={15} />
        </span>
        <span className="kv-row-text">
          <span className="kv-row-label">{t.requestConfig}</span>
          <span className="kv-row-desc">{t.requestConfigHint}</span>
        </span>
        {(provider.request?.customHeaders?.length ?? 0) > 0 && (
          <span className="kv-tag ok tabular-nums">{provider.request?.customHeaders?.length}</span>
        )}
        <ChevronRight size={15} className="shrink-0 opacity-45" />
      </button>

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
    </>
  )
}
