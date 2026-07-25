import { useRef } from 'react'
import { Upload, X, RefreshCw, FolderOpen } from 'lucide-react'
import { open } from '@tauri-apps/plugin-dialog'
import { homeDir, join } from '@tauri-apps/api/path'
import { Toggle, Select, Input, TextArea, SettingRow, SettingsGroup, FieldBlock } from '../components'
import { Button } from '../../components/Button'
import { MemoryIcon, AgentIcon, ProvidersIcon } from '../NavIcons'
import type { I18n, Lang } from '../i18n'
import type { SettingsTab } from '../SettingsShell'
import type {
  Settings as SettingsData,
  ChatToolsConfig,
  ChatMemoryConfig,
} from '../../api/tauri'

/** 兜底最大输出 token 的可选档位。 */
const CHAT_MAX_OUTPUT_TOKEN_OPTIONS = [2048, 8192, 16384, 32768]

function formatTokenCount(tokens?: number): string {
  if (!tokens || !Number.isFinite(tokens)) return ''
  return `${tokens.toLocaleString()} tokens`
}

/**
 * 头像上传字段。原本在 SettingsShell 模块作用域，只被聊天标签页使用，
 * 随 ChatTab 一起搬过来。
 */
function AvatarField({
  value,
  onChange,
  zh,
}: {
  value: string
  onChange: (v: string) => void
  zh: boolean
}) {
  const inputRef = useRef<HTMLInputElement>(null)

  const handleFile = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    e.target.value = '' // 允许再次选同一文件
    if (!file || !file.type.startsWith('image/')) return
    const reader = new FileReader()
    reader.onload = () => {
      const img = new Image()
      img.onload = () => {
        const max = 256
        const scale = Math.min(1, max / Math.max(img.width, img.height))
        const w = Math.max(1, Math.round(img.width * scale))
        const h = Math.max(1, Math.round(img.height * scale))
        const canvas = document.createElement('canvas')
        canvas.width = w
        canvas.height = h
        const ctx = canvas.getContext('2d')
        if (!ctx) {
          onChange(String(reader.result))
          return
        }
        ctx.drawImage(img, 0, 0, w, h)
        onChange(canvas.toDataURL('image/jpeg', 0.85))
      }
      img.src = String(reader.result)
    }
    reader.readAsDataURL(file)
  }

  return (
    <div className="relative shrink-0">
      <button
        type="button"
        onClick={() => inputRef.current?.click()}
        title={zh ? '点击上传头像' : 'Click to upload avatar'}
        data-tauri-drag-region="false"
        className="group relative flex h-11 w-11 items-center justify-center overflow-hidden rounded-full bg-white ring-1 ring-black/[0.05] transition dark:bg-neutral-900 dark:ring-white/[0.08]"
      >
        {value ? (
          <img src={value} alt="" className="h-full w-full object-cover" />
        ) : (
          <img src="/icon.png" alt="" className="h-[82%] w-[82%] object-contain" draggable={false} />
        )}
        <span className="absolute inset-0 hidden items-center justify-center bg-black/45 text-white group-hover:flex">
          <Upload size={14} />
        </span>
      </button>
      {value && (
        <button
          type="button"
          onClick={() => onChange('')}
          title={zh ? '移除头像' : 'Remove avatar'}
          data-tauri-drag-region="false"
          className="absolute -right-1 -top-1 flex h-4 w-4 items-center justify-center rounded-full bg-neutral-700 text-white ring-2 ring-white hover:bg-neutral-900 dark:bg-neutral-500 dark:ring-neutral-900"
        >
          <X size={10} />
        </button>
      )}
      <input ref={inputRef} type="file" accept="image/*" className="hidden" onChange={handleFile} />
    </div>
  )
}

/**
 * 工具状态总览（只读徽标 + 跳转按钮）。
 *
 * 单列一个组件而非并入 ChatTab：它依赖的是一组「其他 tab 的启用状态」，
 * 与 ChatTab 主体的 chatConfig/updateChat 数据流无关；合在一起会让 ChatTab
 * 涨到 19 个 props。
 */
function ChatToolsStatusGroup({
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

interface ChatTabProps {
  settings: SettingsData
  t: I18n
  lang: Lang
  chatConfig: NonNullable<SettingsData['chat']>
  chatTools: ChatToolsConfig
  chatMemory: ChatMemoryConfig
  /** 当前语言的默认聊天系统提示词，用于「恢复默认」的可用性判定。 */
  chatDefaults: string | undefined
  chatSystemPromptValue: string
  chatSystemPromptInteracted: boolean
  chatFallbackMaxOutputTokens: number
  /** 生效的最大输出 token（含来源），由 shell 依据当前聊天模型解析。 */
  effectiveChatMaxOutput: { maxOutput: number; source: string }
  chatMaxOutputSourceLabel: string
  chatMaxOutputModelLabel: string
  skillRuntimeEnabled: boolean
  nativeBuiltinToolsEnabled: boolean
  onUpdateChat: (updates: Partial<NonNullable<SettingsData['chat']>>) => void
  onUpdateNativeTools: (updates: Partial<NonNullable<ChatToolsConfig['nativeTools']>>) => void
  onSystemPromptInteractedChange: (interacted: boolean) => void
  onNavigateTab: (tab: SettingsTab) => void
}

/** AI 客户端（聊天）标签页。纯展示：状态留在 SettingsShell。 */
export function ChatTab({
  settings,
  t,
  lang,
  chatConfig,
  chatTools,
  chatMemory,
  chatDefaults,
  chatSystemPromptValue,
  chatSystemPromptInteracted,
  chatFallbackMaxOutputTokens,
  effectiveChatMaxOutput,
  chatMaxOutputSourceLabel,
  chatMaxOutputModelLabel,
  skillRuntimeEnabled,
  nativeBuiltinToolsEnabled,
  onUpdateChat,
  onUpdateNativeTools,
  onSystemPromptInteractedChange,
  onNavigateTab,
}: ChatTabProps) {
  return (
    <>
      <SettingsGroup title={lang === 'zh' ? '个人资料' : 'Profile'}>
        <div className="flex items-center gap-3 py-1">
          <AvatarField
            value={chatConfig.userAvatar || ''}
            onChange={(userAvatar) => onUpdateChat({ userAvatar })}
            zh={lang === 'zh'}
          />
          <Input
            className="min-w-0 flex-1"
            value={chatConfig.userDisplayName || ''}
            onChange={(userDisplayName) => onUpdateChat({ userDisplayName })}
            placeholder={lang === 'zh' ? '用户名（选填）' : 'Display name (optional)'}
          />
        </div>
      </SettingsGroup>

      <SettingsGroup title={lang === 'zh' ? '工作目录' : 'Workspace'}>
        <SettingRow label={lang === 'zh' ? '普通对话工作目录' : 'Conversation workspace'} stack>
          <div className="flex w-full flex-col gap-2">
            <div className="flex gap-2">
              <Input
                className="min-w-0 flex-1"
                value={chatTools.nativeTools?.workingDirectory ?? ''}
                placeholder={lang === 'zh' ? '默认：~/Kivio/workspace' : 'Default: ~/Kivio/workspace'}
                onChange={(workingDirectory) => onUpdateNativeTools({ workingDirectory })}
              />
              <Button
                size="sm"
                className="shrink-0"
                onClick={async () => {
                  const selected = await open({ directory: true, multiple: false })
                  if (!selected || typeof selected !== 'string') return
                  onUpdateNativeTools({ workingDirectory: selected })
                }}
                data-tauri-drag-region="false"
              >
                <FolderOpen size={11} />
                {lang === 'zh' ? '选择' : 'Choose'}
              </Button>
              <Button
                size="sm"
                className="shrink-0"
                onClick={async () => {
                  const defaultPath = await join(await homeDir(), 'Kivio', 'workspace')
                  onUpdateNativeTools({ workingDirectory: defaultPath })
                }}
                data-tauri-drag-region="false"
              >
                <RefreshCw size={11} />
                {lang === 'zh' ? '恢复默认' : 'Reset'}
              </Button>
            </div>
            <p className="kv-row-desc">
              {lang === 'zh'
                ? '未绑定项目的普通对话会在此目录下按对话 ID 使用独立工作台；用户明确指定的其他路径不受限制。'
                : 'Ordinary chats get a per-conversation workbench here. Explicit paths chosen by the user remain unrestricted.'}
            </p>
          </div>
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup title={lang === 'zh' ? '响应' : 'Response'}>
        <SettingRow label={t.chatStreamEnabled}>
          <Toggle
            checked={chatConfig.streamEnabled !== false}
            onChange={(streamEnabled) => onUpdateChat({ streamEnabled })}
          />
        </SettingRow>
        <SettingRow label={t.chatThinkingEnabled} description={t.chatThinkingHint}>
          <Toggle
            checked={chatConfig.thinkingEnabled !== false}
            onChange={(thinkingEnabled) => onUpdateChat({ thinkingEnabled })}
          />
        </SettingRow>
        <SettingRow label={t.chatMaxOutputTokens} stack>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-[15px] font-medium text-neutral-900 dark:text-neutral-50">
                  {formatTokenCount(effectiveChatMaxOutput.maxOutput)}
                </span>
                <span className={`kv-tag ${effectiveChatMaxOutput.source === 'fallback' ? 'warn' : 'ok'}`}>
                  {chatMaxOutputSourceLabel}
                </span>
              </div>
              <p className="kv-row-desc mt-1 min-w-0 break-all">
                {lang === 'zh' ? '当前聊天模型：' : 'Current chat model: '}
                {chatMaxOutputModelLabel}
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <span className="kv-row-desc whitespace-nowrap">
                {lang === 'zh' ? '兜底' : 'Fallback'}
              </span>
              <Select
                className="w-44"
                value={String(chatFallbackMaxOutputTokens)}
                onChange={(maxOutputTokens) => onUpdateChat({ maxOutputTokens: Number(maxOutputTokens) })}
                options={CHAT_MAX_OUTPUT_TOKEN_OPTIONS.map((tokens) => ({
                  value: String(tokens),
                  label: formatTokenCount(tokens),
                }))}
              />
            </div>
          </div>
        </SettingRow>
        <SettingRow label={t.chatDefaultLanguage}>
          <Select
            className="w-44"
            value={chatConfig.defaultLanguage || ''}
            onChange={(defaultLanguage) => onUpdateChat({ defaultLanguage })}
            options={[
              { value: '', label: t.lensLanguageInherit },
              { value: 'zh', label: '中文' },
              { value: 'zh-Hant', label: '繁體中文' },
              { value: 'en', label: 'English' },
            ]}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup title={t.customPrompts}>
        <FieldBlock label={t.chatSystemPrompt} description={t.chatSystemPromptHint}>
          <div className="mb-2 flex justify-end">
            <Button
              size="sm"
              onClick={() => {
                onSystemPromptInteractedChange(false)
                onUpdateChat({ systemPrompt: '' })
              }}
              disabled={!chatDefaults || (!chatConfig.systemPrompt && !chatSystemPromptInteracted)}
              data-tauri-drag-region="false"
            >
              <RefreshCw size={10} />
              {t.restoreDefaultPrompt}
            </Button>
          </div>
          <TextArea
            value={chatSystemPromptValue}
            onChange={(systemPrompt) => {
              onSystemPromptInteractedChange(true)
              onUpdateChat({ systemPrompt })
            }}
            rows={4}
          />
        </FieldBlock>
      </SettingsGroup>

      <ChatToolsStatusGroup
        settings={settings}
        t={t}
        lang={lang}
        chatTools={chatTools}
        chatMemory={chatMemory}
        skillRuntimeEnabled={skillRuntimeEnabled}
        nativeBuiltinToolsEnabled={nativeBuiltinToolsEnabled}
        onNavigateTab={onNavigateTab}
      />
    </>
  )
}
