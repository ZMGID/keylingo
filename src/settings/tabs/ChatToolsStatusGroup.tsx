import { SettingRow, SettingsGroup } from '../components'
import { Button } from '../../components/Button'
import { MemoryIcon, AgentIcon, ProvidersIcon } from '../NavIcons'
import type { I18n, Lang } from '../i18n'
import type { SettingsTab } from '../SettingsShell'
import type {
  Settings as SettingsData,
  ChatToolsConfig,
  ChatMemoryConfig,
} from '../../api/tauri'

/**
 * 工具状态总览（只读徽标 + 跳转按钮）。
 *
 * 单列一个组件而非并入 ChatTab：它依赖的是一组「其他 tab 的启用状态」，
 * 与 ChatTab 主体的 chatConfig/updateChat 数据流无关；合在一起会让 ChatTab
 * 涨到 19 个 props。
 */
export function ChatToolsStatusGroup({
  settings,
  t,
  lang,
  chatTools,
  chatMemory,
  skillRuntimeEnabled,
  nativeBuiltinToolsEnabled,
  onNavigateTab,
}: {
  settings: SettingsData
  t: I18n
  lang: Lang
  chatTools: ChatToolsConfig
  chatMemory: ChatMemoryConfig
  skillRuntimeEnabled: boolean
  nativeBuiltinToolsEnabled: boolean
  onNavigateTab: (tab: SettingsTab) => void
}) {
  const onLabel = lang === 'zh' ? '已启用' : 'On'
  const offLabel = lang === 'zh' ? '未启用' : 'Off'
  const webSearchOn = Boolean(settings.lens?.webSearch?.enabled || chatTools.nativeTools?.webSearch)

  return (
    <SettingsGroup title={t.chatToolsSection}>
      <div className="flex flex-wrap gap-2 pb-2">
        <Button
          size="sm"
          onClick={() => onNavigateTab('memory')}
          data-tauri-drag-region="false"
        >
          <MemoryIcon size={11} />
          {t.tabMemory}
        </Button>
        <Button
          size="sm"
          onClick={() => onNavigateTab('externalAgents')}
          data-tauri-drag-region="false"
        >
          <AgentIcon size={11} />
          {t.chatOpenExternalAgents}
        </Button>
        <Button
          size="sm"
          onClick={() => onNavigateTab('providers')}
          data-tauri-drag-region="false"
        >
          <ProvidersIcon size={11} />
          {t.chatOpenProviders}
        </Button>
      </div>
      <SettingRow
        label={lang === 'zh' ? 'MCP 工具' : 'MCP tools'}
      >
        <span className={`kv-tag ${chatTools.enabled ? 'ok' : ''}`}>
          {chatTools.enabled ? onLabel : offLabel}
        </span>
      </SettingRow>
      <SettingRow
        label={lang === 'zh' ? 'Skill 运行时' : 'Skill runtime'}
      >
        <span className={`kv-tag ${skillRuntimeEnabled ? 'ok' : ''}`}>
          {skillRuntimeEnabled ? onLabel : offLabel}
        </span>
      </SettingRow>
      <SettingRow
        label={lang === 'zh' ? '内置工具' : 'Native tools'}
      >
        <span className={`kv-tag ${nativeBuiltinToolsEnabled ? 'ok' : ''}`}>
          {nativeBuiltinToolsEnabled ? onLabel : offLabel}
        </span>
      </SettingRow>
      <SettingRow
        label={t.tabMemory}
      >
        <span className={`kv-tag ${chatMemory.enabled ? 'ok' : ''}`}>
          {chatMemory.enabled ? onLabel : offLabel}
        </span>
      </SettingRow>
      <SettingRow
        label={lang === 'zh' ? '联网搜索' : 'Web search'}
      >
        <span className={`kv-tag ${webSearchOn ? 'ok' : ''}`}>
          {webSearchOn
            ? (lang === 'zh' ? '部分启用' : 'Partially on')
            : offLabel}
        </span>
      </SettingRow>
    </SettingsGroup>
  )
}
