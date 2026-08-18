import type { Settings, WebSearchProviderId } from '../../api/tauri'
import { Input, Select, Toggle } from '../../settings/components'
import type { I18n } from '../../settings/i18n'
import { isWebSearchConfigured, webSearchKeyField } from '../../settings/webSearch'
import { OnboardingFormRow } from '../OnboardingFormRow'
import { OnboardingStepFrame } from '../OnboardingStepFrame'

const ONBOARDING_PROVIDERS: { value: WebSearchProviderId; label: string; placeholder: string }[] = [
  { value: 'tavily', label: 'Tavily', placeholder: 'tvly-...' },
  { value: 'exa', label: 'Exa', placeholder: 'exa-...' },
  { value: 'brave', label: 'Brave', placeholder: 'BSA...' },
  { value: 'serper', label: 'Serper', placeholder: 'serper key' },
  { value: 'tinyfish', label: 'TinyFish', placeholder: 'tinyfish key' },
  { value: 'bocha', label: 'Bocha', placeholder: 'bocha key' },
  { value: 'zhipu', label: 'Zhipu', placeholder: 'zhipu key' },
]

type WebSearchStepProps = {
  t: I18n
  settings: Settings
  onChange: (settings: Settings) => void
}

export function WebSearchStep({ t, settings, onChange }: WebSearchStepProps) {
  const webSearch = settings.lens?.webSearch ?? {
    enabled: false,
    provider: 'tavily' as const,
    tavilyApiKey: '',
    exaApiKey: '',
    maxResults: 5,
    searchDepth: 'basic' as const,
  }
  const chatWebSearchEnabled = settings.chatTools.nativeTools?.webSearch !== false
  const selected = ONBOARDING_PROVIDERS.find((item) => item.value === webSearch.provider)
  const keyField = webSearchKeyField(webSearch.provider)
  const keyValue = keyField ? String(webSearch[keyField] ?? '') : ''

  const updateWebSearch = (updates: Partial<NonNullable<Settings['lens']['webSearch']>>) => {
    onChange({
      ...settings,
      lens: {
        ...settings.lens,
        webSearch: {
          ...webSearch,
          ...updates,
        },
      },
    })
  }

  const updateChatWebSearch = (enabled: boolean) => {
    onChange({
      ...settings,
      chatTools: {
        ...settings.chatTools,
        nativeTools: {
          ...settings.chatTools.nativeTools,
          webSearch: enabled,
        },
      },
    })
  }

  const hasApiKey = isWebSearchConfigured(webSearch)

  return (
    <OnboardingStepFrame title={t.onboardingWebSearchTitle} subtitle={t.onboardingWebSearchDesc}>
      <div className="onboarding-section">
        <div className="onboarding-section-label">{t.webSearchApiSection}</div>
        <div className="onboarding-card onboarding-card--rows">
          <OnboardingFormRow label={t.lensWebSearchProvider}>
            <Select
              className="w-full max-w-[220px]"
              value={webSearch.provider}
              onChange={(value) => updateWebSearch({ provider: value as WebSearchProviderId })}
              options={ONBOARDING_PROVIDERS.map((item) => ({ value: item.value, label: item.label }))}
            />
          </OnboardingFormRow>
          {keyField && (
            <OnboardingFormRow
              label={t.lensWebSearchApiKey}
              hint={!hasApiKey ? t.onboardingWebSearchKeyRequired : undefined}
              stack
            >
              <Input
                type="password"
                value={keyValue}
                onChange={(value) => updateWebSearch({ [keyField]: value })}
                placeholder={selected?.placeholder ?? 'API key'}
                mono
              />
            </OnboardingFormRow>
          )}
        </div>
      </div>

      <div className="onboarding-section">
        <div className="onboarding-section-label">{t.onboardingWebSearchEnableSection}</div>
        <div className="onboarding-card onboarding-card--rows">
          <OnboardingFormRow label={t.webSearchChatSection} hint={t.onboardingWebSearchChatHint}>
            <Toggle
              checked={hasApiKey && chatWebSearchEnabled}
              onChange={(enabled) => {
                if (!hasApiKey) return
                updateChatWebSearch(enabled)
              }}
            />
          </OnboardingFormRow>
          <OnboardingFormRow label={t.webSearchLensSection} hint={t.lensWebSearchHint}>
            <Toggle
              checked={hasApiKey && webSearch.enabled}
              onChange={(enabled) => {
                if (!hasApiKey) return
                updateWebSearch({ enabled })
              }}
            />
          </OnboardingFormRow>
        </div>
      </div>
    </OnboardingStepFrame>
  )
}
