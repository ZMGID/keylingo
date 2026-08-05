import { Select, SettingRow, SettingsGroup } from '../components'
import { ModelPairSelect } from '../ModelPairSelect'
import { PromptField } from '../ScreenshotTranslationSettings'
import type { I18n, Lang } from '../i18n'
import type { Settings as SettingsData, DefaultPromptTemplates } from '../../api/tauri'

interface TranslateTabProps {
  settings: SettingsData
  t: I18n
  lang: Lang
  defaultPrompts: DefaultPromptTemplates | null
  onUpdateSettings: (updates: Partial<SettingsData>) => void
}

/**
 * 划词翻译设置（翻译标签页的上半部分）。
 *
 * 下半部分「截图翻译」是既有的 ScreenshotTranslationSettings（16 个 props），
 * 仍由 SettingsShell 直接渲染 —— 一并包进来会让本组件涨到 19 个 props，
 * 反而比留在 shell 里更难维护。
 */
export function TranslateTab({
  settings,
  t,
  lang,
  defaultPrompts,
  onUpdateSettings,
}: TranslateTabProps) {
  return (
    <>
      <div className="kv-section-title">{t.tabTranslate}</div>
      <SettingsGroup title={lang === 'zh' ? '输出' : 'Output'}>
        <SettingRow label={t.targetLang}>
          <Select
            className="w-40"
            value={settings.targetLang || 'auto'}
            onChange={(v) => onUpdateSettings({ targetLang: v })}
            options={[
              { value: 'auto', label: t.langAuto },
              { value: 'en', label: t.langEn },
              { value: 'zh', label: t.langZh },
              { value: 'zh-Hant', label: t.langZhTw },
              { value: 'ja', label: t.langJa },
              { value: 'ko', label: t.langKo },
              { value: 'fr', label: t.langFr },
              { value: 'de', label: t.langDe },
            ]}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup title={t.sectionModel}>
        <SettingRow label={t.selectModelPair}>
          <ModelPairSelect
            providerId={settings.translatorProviderId}
            model={settings.translatorModel}
            providers={settings.providers}
            onChange={(providerId, model) => {
              onUpdateSettings({ translatorProviderId: providerId, translatorModel: model })
            }}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup title={t.sectionPrompt}>
        <PromptField
          label={t.translatorPrompt}
          description={t.translatorPromptHint}
          value={settings.translatorPrompt || ''}
          defaultText={defaultPrompts?.translationTemplate || ''}
          restoreLabel={t.restoreDefaultPrompt}
          onChange={(v) => onUpdateSettings({ translatorPrompt: v })}
        />
      </SettingsGroup>
    </>
  )
}
