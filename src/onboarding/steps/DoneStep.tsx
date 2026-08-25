import type { Settings } from '../../api/tauri'
import type { I18n } from '../../settings/i18n'
import { formatHotkey, getPlatform } from '../../settings/utils'
import { OnboardingStepFrame } from '../OnboardingStepFrame'
import { webSearchConfigured } from '../validation'

type DoneStepProps = {
  t: I18n
  settings: Settings
}

function resolveModelLabel(settings: Settings, providerId: string, model: string): string {
  const provider = settings.providers.find((item) => item.id === providerId)
  if (!provider) return '—'
  return `${provider.name} · ${model || '—'}`
}

function formatHotkeyLabel(hotkey: string, fallback: string): string {
  const value = hotkey.trim() || fallback
  return formatHotkey(value, getPlatform()).join(' + ') || value
}

function SummaryRow({ label, value, multiline = false }: { label: string; value: string; multiline?: boolean }) {
  return (
    <div className="onboarding-summary-row">
      <span className="onboarding-summary-label">{label}</span>
      <span className={`onboarding-summary-value${multiline ? ' onboarding-summary-value--multiline' : ''}`}>
        {value}
      </span>
    </div>
  )
}

export function DoneStep({ t, settings }: DoneStepProps) {
  const modelRows = [
    { label: t.onboardingDoneQuickTranslateModel, providerId: settings.screenshotTranslation?.providerId || '', model: settings.screenshotTranslation?.model || '' },
    { label: t.onboardingDoneLensModel, providerId: settings.lens?.providerId || '', model: settings.lens?.model || '' },
  ]

  const hotkeyRows = [
    { label: t.onboardingDoneHotkeyTranslator, value: formatHotkeyLabel(settings.hotkey, 'CommandOrControl+Alt+T') },
    { label: t.onboardingDoneHotkeyScreenshot, value: formatHotkeyLabel(settings.screenshotTranslation?.hotkey || '', 'CommandOrControl+Shift+A') },
    { label: t.onboardingDoneHotkeySelectedText, value: formatHotkeyLabel(settings.screenshotTranslation?.textHotkey || '', 'CommandOrControl+Shift+T') },
    {
      label: t.onboardingDoneHotkeyReplace,
      value: settings.screenshotTranslation?.replaceEnabled === false
        ? t.onboardingDoneNotConfigured
        : formatHotkeyLabel(settings.screenshotTranslation?.replaceHotkey || '', 'CommandOrControl+Shift+R'),
    },
    { label: t.onboardingDoneHotkeyLens, value: formatHotkeyLabel(settings.lens?.hotkey || '', 'CommandOrControl+Shift+G') },
  ]

  return (
    <OnboardingStepFrame title={t.onboardingDoneTitle} subtitle={t.onboardingDoneDesc}>
      <div className="onboarding-section">
        <div className="onboarding-section-label">{t.onboardingDoneSectionModels}</div>
        <div className="onboarding-card onboarding-card--rows">
          <div className="onboarding-summary-list">
            {modelRows.map((row) => (
              <SummaryRow
                key={row.label}
                label={row.label}
                value={resolveModelLabel(settings, row.providerId, row.model)}
              />
            ))}
            <SummaryRow
              label={t.onboardingDoneWebSearch}
              value={webSearchConfigured(settings) && settings.lens?.webSearch?.enabled
                ? t.onboardingDoneConfigured
                : t.onboardingDoneNotConfigured}
            />
          </div>
        </div>
      </div>

      <div className="onboarding-section">
        <div className="onboarding-section-label">{t.onboardingDoneHotkeys}</div>
        <div className="onboarding-card onboarding-card--rows">
          <div className="onboarding-summary-list">
            {hotkeyRows.map((row) => (
              <SummaryRow key={row.label} label={row.label} value={row.value} />
            ))}
          </div>
        </div>
      </div>
    </OnboardingStepFrame>
  )
}
