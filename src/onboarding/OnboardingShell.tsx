import { useCallback, useEffect, useMemo, useState } from 'react'
import { ArrowLeft, ArrowRight, Check } from 'lucide-react'
import { type Settings } from '../api/tauri'
import { getSettingsCached, saveSettingsCached } from '../api/settingsCache'
import { i18n, type Lang } from '../settings/i18n'
import { usesNativeTitlebar } from '../chat/platform'
import { Button } from '../components/Button'
import { ONBOARDING_STEPS, type OnboardingStepId } from './types'
import { canCompleteOnboarding, validateProviderStep } from './validation'
import { DoneStep } from './steps/DoneStep'
import { HotkeyStep } from './steps/HotkeyStep'
import { ProviderStep } from './steps/ProviderStep'
import { WebSearchStep } from './steps/WebSearchStep'
import { WelcomeStep } from './steps/WelcomeStep'

type OnboardingShellProps = {
  onComplete: () => void
  onSkip: () => void
  onSettingsChange?: () => void
}

/** 首次运行按系统语言（浏览器/系统 locale）自动选定界面语言：中文 locale → zh，其余 → en。 */
function detectSystemLang(): Lang {
  const raw = (
    (typeof navigator !== 'undefined' && (navigator.language || navigator.languages?.[0])) || ''
  ).toLowerCase()
  return raw.startsWith('zh') ? 'zh' : 'en'
}

export function OnboardingShell({ onComplete, onSkip, onSettingsChange }: OnboardingShellProps) {
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [settings, setSettings] = useState<Settings | null>(null)
  const [stepIndex, setStepIndex] = useState(0)
  const [skipConfirmOpen, setSkipConfirmOpen] = useState(false)
  const [providerBypass, setProviderBypass] = useState(false)

  const stepId = ONBOARDING_STEPS[stepIndex] ?? 'welcome'
  const lang = (settings?.settingsLanguage || 'zh') as Lang
  const t = i18n[lang]

  const loadSettings = useCallback(async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const loaded = await getSettingsCached()
      // 首次运行按系统语言自动设定界面语言（欢迎页起即本地化）；但若用户此前已选过语言
      // （如重跑引导的老用户），沿用其选择，不要用系统 locale 覆盖。
      setSettings({
        ...loaded,
        settingsLanguage: loaded.settingsLanguage || detectSystemLang(),
      })
    } catch (err) {
      console.error('Failed to load settings for onboarding:', err)
      setLoadError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadSettings()
  }, [loadSettings])

  const updateSettings = useCallback((next: Settings) => {
    setSettings(next)
  }, [])

  const providerValidation = useMemo(
    () => (settings ? validateProviderStep(settings) : { ok: false }),
    [settings],
  )

  const canAdvanceFromProvider = providerValidation.ok || providerBypass

  const canGoNext = useMemo(() => {
    switch (stepId) {
      case 'provider':
        return canAdvanceFromProvider
      default:
        return true
    }
  }, [canAdvanceFromProvider, stepId])

  const persistSettings = useCallback(async (status: 'completed' | 'skipped') => {
    if (!settings) return false
    setSaving(true)
    setSaveError(null)
    try {
      const saved = await saveSettingsCached({
        ...settings,
        onboardingStatus: status,
      })
      setSettings(saved)
      onSettingsChange?.()
      return true
    } catch (err) {
      // save_settings 在热键注册失败时会 Err 并回滚——必须把错误呈现给用户，
      // 否则用户点 Finish/Skip 无反应、卡在页面上不知所以。
      console.error('Failed to save onboarding settings:', err)
      setSaveError(err instanceof Error ? err.message : String(err))
      return false
    } finally {
      setSaving(false)
    }
  }, [onSettingsChange, settings])

  const handleSkip = useCallback(async () => {
    const ok = await persistSettings('skipped')
    if (ok) onSkip()
  }, [onSkip, persistSettings])

  const handleSkipAfterLoadFailure = useCallback(async () => {
    setSaving(true)
    try {
      const loaded = await getSettingsCached()
      await saveSettingsCached({ ...loaded, onboardingStatus: 'skipped' })
      onSettingsChange?.()
    } catch (err) {
      console.error('Failed to skip onboarding after load error:', err)
    } finally {
      setSaving(false)
    }
    onSkip()
  }, [onSettingsChange, onSkip])

  const handleFinish = useCallback(async () => {
    if (!settings) return
    // 正常完成需通过供应商校验；若用户显式「继续（跳过校验）」，则以 skipped 状态完成——
    // 供应商未验证，标 skipped 比 completed 诚实，且同样不会再次弹引导。避免 bypass 后
    // Finish 永久禁用、只能走 Skip 的死路。
    if (canCompleteOnboarding(settings)) {
      const ok = await persistSettings('completed')
      if (ok) onComplete()
    } else if (providerBypass) {
      const ok = await persistSettings('skipped')
      if (ok) onComplete()
    }
  }, [onComplete, persistSettings, providerBypass, settings])

  const goNext = () => {
    if (stepIndex >= ONBOARDING_STEPS.length - 1) return
    setStepIndex((index) => Math.min(index + 1, ONBOARDING_STEPS.length - 1))
  }

  const goBack = () => {
    setStepIndex((index) => Math.max(index - 1, 0))
  }

  if (loading) {
    return (
      <div className="onboarding-shell onboarding-shell--loading settings-embedded kv">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-neutral-300 border-t-neutral-800 dark:border-neutral-700 dark:border-t-neutral-200" />
      </div>
    )
  }

  if (!settings) {
    const errorT = i18n.zh
    return (
      <div className="onboarding-shell onboarding-shell--loading settings-embedded kv">
        <div className="onboarding-error-panel">
          <h2 className="onboarding-title">{errorT.onboardingLoadErrorTitle}</h2>
          <p className="onboarding-subtitle">{errorT.onboardingLoadErrorDesc}</p>
          {loadError ? <p className="onboarding-panel-note">{loadError}</p> : null}
          <div className="onboarding-error-actions">
            <Button
              variant="primary"
              onClick={() => void loadSettings()}
              disabled={saving}
              data-tauri-drag-region="false"
            >
              {errorT.onboardingRetry}
            </Button>
            <Button
              variant="ghost"
              onClick={() => void handleSkipAfterLoadFailure()}
              disabled={saving}
              data-tauri-drag-region="false"
            >
              {errorT.onboardingSkip}
            </Button>
          </div>
        </div>
      </div>
    )
  }

  const stepLabels: Record<OnboardingStepId, string> = {
    welcome: t.onboardingStepWelcome,
    provider: t.onboardingWelcomeStepProvider,
    webSearch: t.onboardingWelcomeStepWebSearch,
    hotkey: t.onboardingWelcomeStepHotkey,
    done: t.onboardingStepDone,
  }

  const primaryLabel = stepId === 'welcome'
    ? t.onboardingStart
    : stepId === 'done'
      ? t.onboardingFinish
      : t.onboardingNext

  const handlePrimary = () => {
    if (stepId === 'done') {
      void handleFinish()
      return
    }
    goNext()
  }

  return (
    <div className="onboarding-shell settings-embedded kv">
      <aside
        className={`onboarding-side${usesNativeTitlebar ? ' onboarding-side--mac' : ''}`}
        data-tauri-drag-region
      >
        <div className="onboarding-side-brand" data-tauri-drag-region>
          <img src="/logo-mark.png" alt="" className="onboarding-side-logo" draggable={false} />
          <span className="onboarding-side-brand-name">Kivio Desktop</span>
        </div>
        <nav className="onboarding-side-steps">
          {ONBOARDING_STEPS.map((step, index) => {
            const done = index < stepIndex
            const active = index === stepIndex
            return (
              <button
                key={step}
                type="button"
                className={`onboarding-side-step${active ? ' active' : ''}${done ? ' done' : ''}`}
                data-clickable={done ? 'true' : 'false'}
                disabled={!done}
                onClick={() => {
                  if (done) setStepIndex(index)
                }}
                data-tauri-drag-region="false"
              >
                <span className="onboarding-side-step-bullet">
                  {done ? <Check size={11} strokeWidth={3} /> : index + 1}
                </span>
                <span className="onboarding-side-step-label">{stepLabels[step]}</span>
              </button>
            )
          })}
        </nav>
      </aside>

      <div className="onboarding-main">
        <div className="onboarding-topbar" data-tauri-drag-region>
          <Button
            variant="ghost"
            onClick={() => setSkipConfirmOpen(true)}
            data-tauri-drag-region="false"
          >
            {t.onboardingSkip}
          </Button>
        </div>

        <div className="onboarding-body kv-scroll custom-scrollbar" data-tauri-drag-region="false">
          {stepId === 'welcome' ? <WelcomeStep t={t} /> : null}
          {stepId === 'provider' ? (
            <ProviderStep
              t={t}
              lang={lang}
              settings={settings}
              onChange={updateSettings}
              showValidationWarning={!providerValidation.ok}
              validationBypassed={providerBypass}
              onBypassValidation={() => setProviderBypass(true)}
            />
          ) : null}
          {stepId === 'webSearch' ? (
            <WebSearchStep t={t} settings={settings} onChange={updateSettings} />
          ) : null}
          {stepId === 'hotkey' ? (
            <HotkeyStep t={t} settings={settings} onChange={updateSettings} />
          ) : null}
          {stepId === 'done' ? <DoneStep t={t} settings={settings} /> : null}
        </div>

        <div className="onboarding-footer" data-tauri-drag-region="false">
          {saveError ? (
            <div className="onboarding-footer-error" role="alert">
              <span>{t.onboardingSaveError}</span>
              <span className="onboarding-footer-error-detail">{saveError}</span>
            </div>
          ) : null}
          <div className="onboarding-footer-inner">
            {stepIndex > 0 ? (
              <Button
                variant="ghost"
                onClick={goBack}
                disabled={saving}
                data-tauri-drag-region="false"
              >
                <ArrowLeft size={14} />
                {t.onboardingBack}
              </Button>
            ) : null}
            <div className="onboarding-footer-spacer" />
            <div className="onboarding-footer-actions">
              {stepId === 'webSearch' ? (
                <Button
                  variant="ghost"
                  onClick={goNext}
                  disabled={saving}
                  data-tauri-drag-region="false"
                >
                  {t.onboardingWebSearchSkipStep}
                </Button>
              ) : null}
              <Button
                variant="primary"
                onClick={handlePrimary}
                disabled={saving || (stepId !== 'done' && !canGoNext) || (stepId === 'done' && !canCompleteOnboarding(settings) && !providerBypass)}
                data-tauri-drag-region="false"
              >
                {primaryLabel}
                {stepId !== 'done' ? <ArrowRight size={14} /> : null}
              </Button>
            </div>
          </div>
        </div>
      </div>

      {skipConfirmOpen ? (
        <div
          className="kv-modal-backdrop kv-modal-backdrop--portal"
          data-tauri-drag-region="false"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) setSkipConfirmOpen(false)
          }}
        >
          <div
            className="kv-modal"
            role="dialog"
            aria-modal="true"
            data-tauri-drag-region="false"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <h3 className="kv-modal-title">{t.onboardingSkipConfirmTitle}</h3>
            <p className="kv-row-desc">{t.onboardingSkipConfirmDesc}</p>
            <div className="flex justify-end gap-2 pt-4">
              <Button
                variant="ghost"
                onClick={() => setSkipConfirmOpen(false)}
                data-tauri-drag-region="false"
              >
                {t.cancel}
              </Button>
              <Button
                variant="primary"
                onClick={() => {
                  setSkipConfirmOpen(false)
                  void handleSkip()
                }}
                disabled={saving}
                data-tauri-drag-region="false"
              >
                {t.onboardingSkipConfirm}
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}
