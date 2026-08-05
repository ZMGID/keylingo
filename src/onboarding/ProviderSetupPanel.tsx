import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Check, Loader2, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { api, type ModelProvider, type Settings } from '../api/tauri'
import { ProviderModelsPicker } from '../settings/ProviderModelsPicker'
import { ModelPairSelect } from '../settings/ModelPairSelect'
import { Button, IconButton } from '../components/Button'
import { Input, Label } from '../settings/components'
import type { I18n } from '../settings/i18n'
import type { Lang } from '../settings/i18n'
import { PROVIDER_PRESETS, type ProviderPreset } from '../settings/providerPresets'
import { isProviderEnabled } from '../settings/utils'

type ProviderSetupPanelProps = {
  t: I18n
  lang: Lang
  settings: Settings
  onChange: (settings: Settings) => void
}

function newProviderId(): string {
  return `provider-${Date.now()}`
}

function isPresetMatch(provider: ModelProvider, preset: ProviderPreset): boolean {
  return provider.name === preset.name && provider.baseUrl === preset.baseUrl
}

function isCustomProvider(provider: ModelProvider): boolean {
  return !PROVIDER_PRESETS.some((preset) => isPresetMatch(provider, preset))
}

function resolveActiveProviderId(settings: Settings): string {
  const screenshotId = settings.screenshotTranslation?.providerId?.trim()
  if (screenshotId) {
    const byScreenshot = settings.providers.find((item) => item.id === screenshotId)
    if (byScreenshot) return byScreenshot.id
  }

  const lensId = settings.lens?.providerId?.trim()
  if (lensId) {
    const byLens = settings.providers.find((item) => item.id === lensId)
    if (byLens) return byLens.id
  }

  const chatId = settings.defaultModels.chat.providerId.trim()
  if (chatId) {
    const byChat = settings.providers.find((item) => item.id === chatId)
    if (byChat) return byChat.id
  }

  const configured = settings.providers.find((item) =>
    item.enabled !== false && item.enabledModels.length > 0,
  )
  if (configured) return configured.id

  return settings.providers[0]?.id ?? ''
}

function resolveProvider(providers: ModelProvider[], providerId: string): ModelProvider | undefined {
  const matched = providers.find((item) => item.id === providerId)
  if (matched && isProviderEnabled(matched)) return matched
  return providers.find((item) => isProviderEnabled(item))
}

function resolveModel(provider: ModelProvider | undefined, currentModel: string): string {
  if (!provider) return currentModel
  if (provider.enabledModels.includes(currentModel)) return currentModel
  return provider.enabledModels[0] || currentModel
}

function freshCustomProvider(id: string, lang: Lang, index: number): ModelProvider {
  const suffix = index > 0 ? ` ${index + 1}` : ''
  return {
    id,
    name: lang === 'zh' ? `自定义服务商${suffix}` : `Custom Provider${suffix}`,
    apiKeys: [],
    baseUrl: 'https://api.openai.com/v1',
    availableModels: [],
    enabledModels: [],
    enabled: true,
    apiFormat: 'openai_chat',
  }
}

function freshPresetProvider(preset: ProviderPreset, id: string): ModelProvider {
  return {
    id,
    name: preset.name,
    apiKeys: [],
    baseUrl: preset.baseUrl,
    availableModels: [],
    enabledModels: [],
    enabled: true,
    apiFormat: 'openai_chat',
  }
}

function maybeAutoBindDefaults(settings: Settings, provider: ModelProvider): Settings {
  if (provider.enabledModels.length === 0) return settings

  const primaryModel = provider.enabledModels[0]
  const next: Settings = { ...settings }

  const screenshotEmpty = !next.screenshotTranslation?.providerId?.trim()
    || !next.screenshotTranslation?.model?.trim()
  if (screenshotEmpty) {
    next.screenshotTranslation = {
      ...next.screenshotTranslation,
      providerId: provider.id,
      model: primaryModel,
    }
  }

  const chatEmpty = !next.defaultModels.chat.providerId.trim() || !next.defaultModels.chat.model.trim()
  if (chatEmpty) {
    next.defaultModels = {
      ...next.defaultModels,
      chat: { providerId: provider.id, model: primaryModel },
    }
    next.chatProviderId = provider.id
    next.chatModel = primaryModel
  }

  const lensEmpty = !next.lens?.providerId?.trim() || !next.lens?.model?.trim()
  if (lensEmpty) {
    next.lens = {
      ...next.lens,
      providerId: provider.id,
      model: primaryModel,
    }
  }

  return next
}

function updateProviderInSettings(
  settings: Settings,
  providerId: string,
  patch: Partial<ModelProvider>,
): Settings {
  const providers = settings.providers.map((item) =>
    item.id === providerId ? { ...item, ...patch } : item,
  )
  const updated = providers.find((item) => item.id === providerId)
  if (!updated) return { ...settings, providers }
  return maybeAutoBindDefaults({ ...settings, providers }, updated)
}

function addPresetProvider(settings: Settings, preset: ProviderPreset): { settings: Settings; id: string } {
  const existing = settings.providers.find((item) => isPresetMatch(item, preset))
  if (existing) {
    return { settings, id: existing.id }
  }

  const id = newProviderId()
  const provider = freshPresetProvider(preset, id)
  return {
    settings: { ...settings, providers: [...settings.providers, provider] },
    id,
  }
}

function addCustomProvider(settings: Settings, lang: Lang): { settings: Settings; id: string } {
  const customCount = settings.providers.filter(isCustomProvider).length
  const id = newProviderId()
  const provider = freshCustomProvider(id, lang, customCount)
  return {
    settings: { ...settings, providers: [...settings.providers, provider] },
    id,
  }
}

function deleteProviderFromSettings(settings: Settings, id: string): Settings {
  const nextProviders = settings.providers.filter((item) => item.id !== id)
  const screenshotProvider = resolveProvider(nextProviders, settings.screenshotTranslation?.providerId || '')
  const lensProvider = resolveProvider(nextProviders, settings.lens?.providerId || '')
  const deletedProviderWasChatModel =
    settings.defaultModels.chat.providerId === id || settings.chatProviderId === id
  const deletedProviderWasLensModel = settings.lens?.providerId === id

  const next: Settings = {
    ...settings,
    providers: nextProviders,
    defaultModels: {
      ...settings.defaultModels,
      chat: settings.defaultModels.chat.providerId === id
        ? { providerId: '', model: '' }
        : settings.defaultModels.chat,
    },
    screenshotTranslation: {
      ...settings.screenshotTranslation,
      providerId: screenshotProvider?.id ?? '',
      model: resolveModel(screenshotProvider, settings.screenshotTranslation?.model || ''),
    },
    lens: {
      ...settings.lens,
      providerId: deletedProviderWasLensModel ? '' : (settings.lens?.providerId ?? ''),
      model: deletedProviderWasLensModel
        ? ''
        : resolveModel(lensProvider, settings.lens?.model || ''),
    },
    chatProviderId: deletedProviderWasChatModel ? '' : settings.chatProviderId,
    chatModel: deletedProviderWasChatModel ? '' : settings.chatModel,
  }

  return next
}

export function ProviderSetupPanel({ t, lang, settings, onChange }: ProviderSetupPanelProps) {
  const initializedRef = useRef(false)
  const [activeProviderId, setActiveProviderId] = useState(() => resolveActiveProviderId(settings))
  const [modelPickerOpen, setModelPickerOpen] = useState(false)
  const [fetching, setFetching] = useState(false)
  const [testing, setTesting] = useState(false)
  const [testFeedback, setTestFeedback] = useState<{ ok: boolean; message: string } | null>(null)
  const [confirmDelete, setConfirmDelete] = useState(false)

  const provider = useMemo(
    () => settings.providers.find((item) => item.id === activeProviderId),
    [activeProviderId, settings.providers],
  )

  useEffect(() => {
    if (initializedRef.current) return
    initializedRef.current = true
    const resolvedId = resolveActiveProviderId(settings)
    if (resolvedId) setActiveProviderId(resolvedId)
  }, [settings])

  useEffect(() => {
    if (activeProviderId && settings.providers.some((item) => item.id === activeProviderId)) return
    const resolvedId = resolveActiveProviderId(settings)
    setActiveProviderId(resolvedId)
  }, [activeProviderId, settings])

  const selectProvider = useCallback((providerId: string) => {
    setActiveProviderId(providerId)
    setTestFeedback(null)
    setConfirmDelete(false)
  }, [])

  const handleAddPreset = useCallback((preset: ProviderPreset) => {
    const { settings: next, id } = addPresetProvider(settings, preset)
    onChange(next)
    selectProvider(id)
  }, [onChange, selectProvider, settings])

  const handleAddCustom = useCallback(() => {
    const { settings: next, id } = addCustomProvider(settings, lang)
    onChange(next)
    selectProvider(id)
  }, [lang, onChange, selectProvider, settings])

  const updateProvider = useCallback((patch: Partial<ModelProvider>) => {
    if (!provider) return
    onChange(updateProviderInSettings(settings, provider.id, patch))
  }, [onChange, provider, settings])

  const handleDeleteProvider = useCallback(() => {
    if (!provider) return
    const next = deleteProviderFromSettings(settings, provider.id)
    onChange(next)
    setConfirmDelete(false)
    setModelPickerOpen(false)
    setTestFeedback(null)
    setActiveProviderId(resolveActiveProviderId(next))
  }, [onChange, provider, settings])

  const fetchModels = async () => {
    if (!provider || fetching) return
    setFetching(true)
    try {
      const models = await api.fetchModels(provider.id, {
        id: provider.id,
        baseUrl: provider.baseUrl,
        apiKeys: provider.apiKeys,
      })
      updateProvider({ availableModels: models })
    } catch (err) {
      console.error('Failed to fetch models:', err)
    } finally {
      setFetching(false)
    }
  }

  const handleTestConnection = async () => {
    if (!provider) return
    setTesting(true)
    setTestFeedback(null)
    try {
      const result = await api.testProviderConnection(provider.id, {
        id: provider.id,
        baseUrl: provider.baseUrl,
        apiKeys: provider.apiKeys,
        apiFormat: provider.apiFormat,
        model: provider.enabledModels[0] ?? provider.availableModels[0],
      })
      if (result.success) {
        setTestFeedback({ ok: true, message: t.connectionOk })
      } else {
        setTestFeedback({
          ok: false,
          message: `${t.connectionFailed}${result.error || 'Unknown error'}`,
        })
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setTestFeedback({ ok: false, message: `${t.connectionFailed}${message}` })
    } finally {
      setTesting(false)
    }
  }

  const openModelPicker = () => {
    if (!provider) return
    setModelPickerOpen(true)
    if (provider.availableModels.length === 0 && !fetching) {
      void fetchModels()
    }
  }

  const hasAnyEnabledModels = settings.providers.some((item) => item.enabledModels.length > 0)
  // 未添加过的预设直接以「+ 名称」虚线 chip 混排在服务商列表尾部，避免「已添加」和「快速添加」两块重复列表。
  const unaddedPresets = PROVIDER_PRESETS.filter(
    (preset) => !settings.providers.some((item) => isPresetMatch(item, preset)),
  )

  return (
    <div className="onboarding-provider-panel">
      <div className="onboarding-section">
        <div className="onboarding-section-label">{t.onboardingProviderList}</div>
        <div className="onboarding-pick-grid">
          {settings.providers.map((item) => (
            <button
              key={item.id}
              type="button"
              className={`onboarding-pick${item.id === activeProviderId ? ' active' : ''}`}
              onClick={() => selectProvider(item.id)}
              data-tauri-drag-region="false"
            >
              {item.name.trim() || item.id}
            </button>
          ))}
          {unaddedPresets.map((preset) => (
            <button
              key={preset.name}
              type="button"
              className="onboarding-pick onboarding-pick--add"
              onClick={() => handleAddPreset(preset)}
              data-tauri-drag-region="false"
            >
              <Plus size={12} />
              {preset.name}
            </button>
          ))}
          <button
            type="button"
            className="onboarding-pick onboarding-pick--add"
            onClick={handleAddCustom}
            data-tauri-drag-region="false"
          >
            <Plus size={12} />
            {t.onboardingProviderAddCustom}
          </button>
        </div>
        {settings.providers.length === 0 ? (
          <p className="onboarding-panel-note">{t.onboardingProviderEmpty}</p>
        ) : null}

        {provider ? (
          <div className="onboarding-card onboarding-provider-card">
            <div className="onboarding-provider-grid">
              <div className="onboarding-field">
                <Label>{t.onboardingProviderName}</Label>
                <Input
                  value={provider.name}
                  onChange={(value) => updateProvider({ name: value })}
                  placeholder={t.onboardingProviderCustom}
                />
              </div>
              <div className="onboarding-field">
                <Label>{t.baseUrl}</Label>
                <Input
                  value={provider.baseUrl}
                  onChange={(value) => {
                    const baseUrlChanged = value !== provider.baseUrl
                    updateProvider({
                      baseUrl: value,
                      ...(baseUrlChanged ? { availableModels: [], enabledModels: [] } : {}),
                    })
                  }}
                  placeholder="https://api.example.com/v1"
                  mono
                />
              </div>
            </div>

            <div className="onboarding-field">
              <Label>{t.onboardingProviderApiKey}</Label>
              <Input
                type="password"
                value={provider.apiKeys[0] || ''}
                onChange={(value) => updateProvider({ apiKeys: value.trim() ? [value.trim()] : [] })}
                placeholder="sk-..."
                mono
              />
              {(() => {
                // 命中快速预设 baseUrl 时，给出「获取 API Key」外链引导申请（与设置页一致）。
                const preset = PROVIDER_PRESETS.find(
                  (p) => p.baseUrl === provider.baseUrl && p.apiKeyUrl,
                )
                if (!preset?.apiKeyUrl) return null
                return (
                  <button
                    type="button"
                    onClick={() => void api.openExternal(preset.apiKeyUrl!)}
                    className="inline-flex w-fit items-center text-[12px] text-indigo-500 hover:underline dark:text-indigo-300"
                    data-tauri-drag-region="false"
                  >
                    {lang === 'zh' ? `获取 ${preset.name} API Key ↗` : `Get ${preset.name} API key ↗`}
                  </button>
                )
              })()}
            </div>

            <div className="onboarding-action-row">
              <Button
                onClick={() => void handleTestConnection()}
                disabled={testing}
                data-tauri-drag-region="false"
              >
                {testing ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
                {t.onboardingProviderTest}
              </Button>
              <Button
                variant="primary"
                onClick={openModelPicker}
                data-tauri-drag-region="false"
              >
                {t.onboardingProviderManageModels}
                {provider.enabledModels.length > 0 ? ` · ${provider.enabledModels.length}` : ''}
              </Button>
              {testFeedback ? (
                <span className={`kv-tag ${testFeedback.ok ? 'ok' : 'danger'}`}>{testFeedback.message}</span>
              ) : null}
              <IconButton
                variant="danger"
                size="xs"
                className="onboarding-action-trailing"
                onClick={() => setConfirmDelete(true)}
                data-tauri-drag-region="false"
                title={t.deleteProvider}
                label={t.deleteProvider}
              >
                <Trash2 size={13} />
              </IconButton>
            </div>

            {provider.enabledModels.length > 0 ? (
              <div className="onboarding-model-list">
                {provider.enabledModels.map((model) => (
                  <span key={model} className="onboarding-model-chip">
                    <Check size={12} />
                    {model}
                  </span>
                ))}
              </div>
            ) : null}
          </div>
        ) : null}
      </div>

      {hasAnyEnabledModels ? (
        <div className="onboarding-section">
          <div className="onboarding-section-label">{t.onboardingProviderDefaultModels}</div>
          <div className="onboarding-defaults-grid">
            <div className="onboarding-default-cell">
              <span className="onboarding-field-label">{t.onboardingProviderQuickTranslateModel}</span>
              <ModelPairSelect
                providerId={settings.screenshotTranslation?.providerId || ''}
                model={settings.screenshotTranslation?.model || ''}
                providers={settings.providers}
                className="w-full"
                onChange={(providerId, model) => {
                  onChange({
                    ...settings,
                    screenshotTranslation: {
                      ...settings.screenshotTranslation,
                      providerId,
                      model,
                    },
                  })
                }}
              />
              <p className="onboarding-field-hint">{t.onboardingProviderQuickTranslateHint}</p>
            </div>
            <div className="onboarding-default-cell">
              <span className="onboarding-field-label">{t.onboardingProviderLensModel}</span>
              <ModelPairSelect
                providerId={settings.lens?.providerId || ''}
                model={settings.lens?.model || ''}
                providers={settings.providers}
                className="w-full"
                onChange={(providerId, model) => {
                  onChange({
                    ...settings,
                    lens: {
                      ...settings.lens,
                      providerId,
                      model,
                    },
                  })
                }}
              />
              <p className="onboarding-field-hint">{t.onboardingProviderLensHint}</p>
            </div>
            <div className="onboarding-default-cell">
              <span className="onboarding-field-label">{t.onboardingProviderChatModel}</span>
              <ModelPairSelect
                providerId={settings.defaultModels.chat.providerId}
                model={settings.defaultModels.chat.model}
                providers={settings.providers}
                className="w-full"
                onChange={(providerId, model) => {
                  onChange({
                    ...settings,
                    defaultModels: {
                      ...settings.defaultModels,
                      chat: { providerId, model },
                    },
                    chatProviderId: providerId,
                    chatModel: model,
                  })
                }}
              />
              <p className="onboarding-field-hint">{t.onboardingProviderChatHint}</p>
            </div>
          </div>
        </div>
      ) : null}

      {confirmDelete && provider ? (
        <div
          className="kv-modal-backdrop kv-modal-backdrop--portal"
          data-tauri-drag-region="false"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) setConfirmDelete(false)
          }}
        >
          <div className="kv-modal space-y-3" data-tauri-drag-region="false">
            <h3 className="text-[14px] font-semibold">{t.confirmDeleteProvider}</h3>
            <p className="kv-panel-body">{t.onboardingProviderDeleteDesc}</p>
            <div className="flex justify-end gap-2 pt-1">
              <Button
                onClick={() => setConfirmDelete(false)}
                data-tauri-drag-region="false"
              >
                {t.cancel}
              </Button>
              <Button
                variant="danger"
                onClick={handleDeleteProvider}
                data-tauri-drag-region="false"
              >
                {t.deleteProvider}
              </Button>
            </div>
          </div>
        </div>
      ) : null}

      {modelPickerOpen && provider ? (
        <ProviderModelsPicker
          provider={provider}
          lang={lang}
          labels={{
            title: t.onboardingProviderManageModels,
            searchPlaceholder: lang === 'zh' ? '搜索模型 ID 或名称' : 'Search model ID or name',
            fetchModels: t.fetchModels,
            fetching: t.fetching,
            addModel: t.addModel,
            manualAddModel: t.manualAddModel,
            noModels: lang === 'zh' ? '尚未获取模型，请点击上方按钮拉取，或使用手动添加。' : 'No models yet. Fetch from API or add manually.',
            noSearchResults: lang === 'zh' ? '没有匹配的模型' : 'No matching models',
            enabled: t.enabled,
            addAllModels: lang === 'zh' ? '添加当前列表中的全部模型' : 'Add all models in the current list',
            close: lang === 'zh' ? '关闭' : 'Close',
          }}
          fetching={fetching}
          onClose={() => setModelPickerOpen(false)}
          onFetch={() => void fetchModels()}
          onAdd={(model) => {
            if (provider.enabledModels.includes(model)) return
            onChange(updateProviderInSettings(settings, provider.id, {
              enabledModels: [...provider.enabledModels, model],
            }))
          }}
          onAddAll={(models) => {
            const merged = [...provider.enabledModels]
            for (const model of models) {
              if (!merged.includes(model)) merged.push(model)
            }
            onChange(updateProviderInSettings(settings, provider.id, { enabledModels: merged }))
          }}
          onRemove={(model) => {
            onChange(updateProviderInSettings(settings, provider.id, {
              enabledModels: provider.enabledModels.filter((item) => item !== model),
            }))
          }}
        />
      ) : null}
    </div>
  )
}
