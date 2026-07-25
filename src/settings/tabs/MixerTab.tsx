import { Toggle, SettingRow, SettingsGroup } from '../components'
import { Button } from '../../components/Button'
import { ModelPairSelect } from '../ModelPairSelect'
import { resolveModelInfo } from '../../data/modelMatching'
import type { I18n, Lang } from '../i18n'
import type { Settings as SettingsData, ChatToolsConfig } from '../../api/tauri'

interface MixerTabProps {
  settings: SettingsData
  t: I18n
  lang: Lang
  chatTools: ChatToolsConfig
  /** 是否已配好聊天供应商；未配好时显示引导文案。 */
  hasChatProvider: boolean
  onUpdateDefaultModel: (
    key: keyof SettingsData['defaultModels'],
    providerId: string,
    model: string,
  ) => void
  onUpdateChatTools: (updates: Partial<ChatToolsConfig>) => void
}

/** 模型分工（Mixer）标签页。纯展示：状态留在 SettingsShell。 */
export function MixerTab({
  settings,
  t,
  lang,
  chatTools,
  hasChatProvider,
  onUpdateDefaultModel,
  onUpdateChatTools,
}: MixerTabProps) {
  return (
    <>
      <SettingsGroup title={t.mixerSection}>
        <div className="mb-3 flex items-start justify-between gap-3">
          {t.mixerSectionHint ? (
            <p className="kv-row-desc max-w-[560px]">{t.mixerSectionHint}</p>
          ) : <span />}
          <Button
            size="sm"
            className="shrink-0"
            onClick={() => {
              onUpdateDefaultModel('vision', '', '')
              onUpdateDefaultModel('titleSummary', '', '')
              onUpdateDefaultModel('compression', '', '')
              onUpdateDefaultModel('imageGeneration', '', '')
            }}
            data-tauri-drag-region="false"
          >
            {t.mixerResetAuto}
          </Button>
        </div>
        <SettingRow
          label={t.auxiliaryVisionModel}
        >
          <ModelPairSelect
            providerId={settings.defaultModels.vision.providerId || ''}
            model={settings.defaultModels.vision.model || ''}
            providers={settings.providers}
            inheritLabel={t.mixerAutoVisionModel}
            filterModel={(provider, model) =>
              resolveModelInfo(model, provider.modelOverrides).capabilities?.vision === true
            }
            onChange={(providerId, model) => {
              onUpdateDefaultModel('vision', providerId, model)
            }}
          />
        </SettingRow>
        <SettingRow
          label={t.defaultTitleSummaryModel}
        >
          <ModelPairSelect
            providerId={settings.defaultModels.titleSummary.providerId || ''}
            model={settings.defaultModels.titleSummary.model || ''}
            providers={settings.providers}
            inheritLabel={t.mixerAutoModel}
            onChange={(providerId, model) => {
              onUpdateDefaultModel('titleSummary', providerId, model)
            }}
          />
        </SettingRow>
        <SettingRow
          label={t.defaultCompressionModel}
        >
          <ModelPairSelect
            providerId={settings.defaultModels.compression.providerId || ''}
            model={settings.defaultModels.compression.model || ''}
            providers={settings.providers}
            inheritLabel={t.mixerAutoModel}
            onChange={(providerId, model) => {
              onUpdateDefaultModel('compression', providerId, model)
            }}
          />
        </SettingRow>
        <SettingRow
          label={t.defaultImageGenerationModel}
          description={t.defaultImageGenerationModelHint}
        >
          <ModelPairSelect
            providerId={settings.defaultModels.imageGeneration.providerId || ''}
            model={settings.defaultModels.imageGeneration.model || ''}
            providers={settings.providers}
            inheritLabel={t.mixerNoImageGenerationModel}
            filterModel={(provider, model) =>
              resolveModelInfo(model, provider.modelOverrides).capabilities?.imageGeneration === true
            }
            onChange={(providerId, model) => {
              onUpdateDefaultModel('imageGeneration', providerId, model)
            }}
          />
        </SettingRow>
        {!hasChatProvider && (
          <p className="kv-row-desc px-0 pb-2">
            {lang === 'zh' ? '请先在「模型」中添加并配置供应商。' : 'Add and configure a provider under Models first.'}
          </p>
        )}
      </SettingsGroup>

      <SettingsGroup title={t.mixerSubAgentSection}>
        <SettingRow
          label={t.defaultSubAgentModel}
          description={t.defaultSubAgentModelHint}
        >
          <ModelPairSelect
            providerId={chatTools.subAgentProviderId || ''}
            model={chatTools.subAgentModel || ''}
            providers={settings.providers}
            inheritLabel={t.mixerFollowChatModel}
            onChange={(providerId, model) => {
              onUpdateChatTools({ subAgentProviderId: providerId, subAgentModel: model })
            }}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup title={t.mixerAdvisorSection}>
        <SettingRow
          label={t.defaultAdvisorModel}
          description={t.defaultAdvisorModelHint}
        >
          <Toggle
            checked={Boolean(settings.defaultModels.advisor.providerId)}
            onChange={(on) => {
              if (on) {
                // 开启：若尚未选过模型，默认落到第一个可用供应商的首个模型，用户可再改。
                if (!settings.defaultModels.advisor.providerId) {
                  const p = settings.providers.find(
                    (pp) => pp.enabled && (pp.enabledModels?.length ?? 0) > 0,
                  )
                  onUpdateDefaultModel('advisor', p?.id ?? '', p?.enabledModels[0] ?? '')
                }
              } else {
                onUpdateDefaultModel('advisor', '', '')
              }
            }}
          />
        </SettingRow>
        {Boolean(settings.defaultModels.advisor.providerId) && (
          <SettingRow label={lang === 'zh' ? '顾问模型' : 'Advisor model'}>
            <ModelPairSelect
              providerId={settings.defaultModels.advisor.providerId || ''}
              model={settings.defaultModels.advisor.model || ''}
              providers={settings.providers}
              onChange={(providerId, model) => {
                onUpdateDefaultModel('advisor', providerId, model)
              }}
            />
          </SettingRow>
        )}
      </SettingsGroup>
    </>
  )
}
