import { forwardRef, useImperativeHandle, useState, useEffect, useCallback, useMemo, useRef, useReducer } from 'react'
import {
  X, Check, Plus, Minus, Trash2, RefreshCw,
  ExternalLink, Download, Upload, Wrench, FolderOpen, Eye, EyeOff, Info,
  Brain, Image as ImageIcon, ArrowLeft,
} from 'lucide-react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { homeDir, join } from '@tauri-apps/api/path'
import { ChatMarkdown } from '../chat/ChatMarkdown'
import {
  api,
  type Settings as SettingsType,
  type ModelProvider,
  type ModelInfo,
  type DefaultPromptTemplates,
  type PermissionStatus,
  type UpdateInfo,
  type ChatToolsConfig,
  type ChatNativeToolsConfig,
  type ChatMemoryConfig,
  defaultNativeTools,
  normalizeProviderApiFormat,
  type ReplaceTranslationPackStatus,
  type RapidOcrTier,
} from '../api/tauri'
import {
  getSettingsCached,
  importSettingsCached,
  peekSettings,
  refreshSettings,
  saveSettingsCached,
} from '../api/settingsCache'
import { i18n } from './i18n'
import {
  GeneralIcon, HotkeysIcon, TranslateIcon, LensIcon, ChatIcon, MemoryIcon, MixerIcon,
  AgentIcon, WebSearchIcon, ConnectorsIcon, UsageIcon, ProvidersIcon, AboutIcon,
} from './NavIcons'
import { buildHotkey, formatHotkeyError, getPlatform, isProviderEnabled, stableStringify } from './utils'
import { PROVIDER_PRESETS, type ProviderPreset } from './providerPresets'
import { ProviderModelsPicker } from './ProviderModelsPicker'
import { ModelIcon } from '../chat/ModelIcon'
import { ProviderSortableList } from './ProviderSortableList'
import { ScreenshotTranslationSettings } from './ScreenshotTranslationSettings'
import { initialReplacePackProgressState, reduceReplacePackProgress } from './replacePackProgress'
import { UsageStatsPanel } from './UsageStatsPanel'
import { RequestDebugPanel } from './RequestDebugPanel'
import { ExternalAgentsSettings } from './ExternalAgentsSettings'
import { HotkeysTab } from './tabs/HotkeysTab'
import { LensTab } from './tabs/LensTab'
import { MixerTab } from './tabs/MixerTab'
import { TranslateTab } from './tabs/TranslateTab'
import { ModelDetailDrawer } from '../components/ModelDetailDrawer'
import { ProviderModelTestModal } from '../components/ProviderModelTestModal'
import { Button, IconButton } from '../components/Button'
import { resolveModelInfo } from '../data/modelMatching'
import { useWindowInteractionFocus } from '../utils/windowFocus'
import { hasEnabledNativeBuiltinTool, hasEnabledSkillRuntime } from '../utils/chatTools'
import { THEME_COLOR_PRESETS, normalizeThemeColorId } from '../themeColors'
import {
  Toggle, Select, Input, TextArea,
  SettingRow, PermissionItem,
  SettingsGroup,
} from './components'
import { ConnectorsPanel } from './ConnectorsPanel'
import { WebSearchPanel } from './WebSearchPanel'
import { defaultChatTools } from './chatToolsShared'

export type SettingsTab = 'general' | 'hotkeys' | 'translate' | 'lens' | 'chat' | 'memory' | 'mixer' | 'externalAgents' | 'webSearch' | 'connectors' | 'usage' | 'providers' | 'about'

type SettingsData = SettingsType
type MemoryLayerKey = 'l1' | 'l2'

const MEMORY_L1_MAX_BYTES = 5_000
const CHAT_MAX_OUTPUT_TOKEN_OPTIONS = [2048, 8192, 16384, 32768]
// UI 字号：以 px 展示、以整体缩放（zoom）实现。CSS 全是 px 硬编码，做不了真正的 rem 基准字号，
// 故 14px 锚定为 100%，输入 px → scale = px/14。ponytail: zoom 代理，若将来全量 rem 化可换真基准。
const UI_FONT_BASE_PX = 14
const UI_FONT_PX_MIN = 12
const UI_FONT_PX_MAX = 19

// 可搜索字体选择器：聚焦展开、输入过滤，每项以自身字体预览。界面字体/代码字体共用。
function FontPicker({ value, systemFonts, placeholder, defaultLabel, emptyText, onChange }: {
  value: string
  systemFonts: string[]
  placeholder: string
  defaultLabel: string
  emptyText: string
  onChange: (v: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const q = query.trim().toLowerCase()
  const filtered = (q ? systemFonts.filter((f) => f.toLowerCase().includes(q)) : systemFonts).slice(0, 100)
  const select = (name: string) => {
    onChange(name)
    setOpen(false)
    setQuery('')
  }
  return (
    <div className="relative w-56">
      <Input
        value={open ? query : (value || defaultLabel)}
        onChange={(v) => { setQuery(v); if (!open) setOpen(true) }}
        onFocus={() => { setQuery(''); setOpen(true) }}
        onBlur={() => window.setTimeout(() => setOpen(false), 120)}
        placeholder={placeholder}
      />
      {open && (
        <div className="absolute right-0 z-50 mt-1 max-h-64 w-full overflow-auto rounded-lg border border-neutral-200 bg-white py-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-900">
          <button
            type="button"
            className={`block w-full truncate px-3 py-1.5 text-left text-[13px] hover:bg-black/[0.05] dark:hover:bg-white/[0.08] ${value === '' ? 'font-semibold text-neutral-900 dark:text-neutral-100' : 'text-neutral-700 dark:text-neutral-300'}`}
            onMouseDown={(e) => { e.preventDefault(); select('') }}
          >
            {defaultLabel}
          </button>
          {filtered.map((f) => (
            <button
              key={f}
              type="button"
              className={`block w-full truncate px-3 py-1.5 text-left text-[13px] hover:bg-black/[0.05] dark:hover:bg-white/[0.08] ${value === f ? 'bg-black/[0.04] font-semibold dark:bg-white/[0.06]' : 'text-neutral-700 dark:text-neutral-300'}`}
              style={{ fontFamily: `"${f}"` }}
              onMouseDown={(e) => { e.preventDefault(); select(f) }}
            >
              {f}
            </button>
          ))}
          {filtered.length === 0 && (
            <div className="px-3 py-2 text-[12px] text-neutral-400">{emptyText}</div>
          )}
        </div>
      )}
    </div>
  )
}
const textEncoder = new TextEncoder()

function utf8ByteLength(value: string): number {
  return textEncoder.encode(value).length
}

export interface SettingsShellProps {
  variant: 'standalone' | 'embedded'
  onClose: () => void
  onSettingsChange: () => void
  onReady?: () => void
  reserveTrafficLightSpace?: boolean
  /** 打开设置面板时选中的侧栏项（如 Chat 内嵌设置默认 AI 客户端） */
  initialTab?: SettingsTab
  /** embedded 单页模式：隐藏左侧设置导航，只显示 initialTab 对应页（如从扩展点「知识库」进入） */
  hideNav?: boolean
}

export interface SettingsShellHandle {
  requestClose: () => void
}

/** 快捷键作用域。原本是组件体内的局部 type，抽 HotkeysTab 后需要跨模块共享，提到模块作用域。 */
export type HotkeyScopeKey =
  | 'main'
  | 'chat'
  | 'screenshotTranslation'
  | 'screenshotTranslationText'
  | 'screenshotTranslationReplace'
  | 'screenshotAnnotate'
  | 'lens'

function FieldBlock({
  label,
  description,
  children,
  className = '',
}: {
  label: React.ReactNode
  description?: string
  children: React.ReactNode
  className?: string
}) {
  return (
    <div className={`py-2 ${className}`}>
      <div className="mb-2">
        <div className="kv-row-label">{label}</div>
        {description && <p className="kv-row-desc">{description}</p>}
      </div>
      {children}
    </div>
  )
}

function MemoryEditor({
  layer,
  title,
  description,
  value,
  savedValue,
  maxBytes,
  rows,
  loading,
  saving,
  lang,
  onChange,
  onSave,
  onReload,
}: {
  layer: MemoryLayerKey
  title: string
  description: string
  value: string
  savedValue: string
  maxBytes?: number
  rows: number
  loading: boolean
  saving: boolean
  lang: string
  onChange: (value: string) => void
  onSave: () => void
  onReload: () => void
}) {
  const bytes = utf8ByteLength(value)
  const overLimit = maxBytes !== undefined && bytes > maxBytes
  const dirty = value !== savedValue
  return (
    <div className="kv-panel">
      <div className="mb-2 flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="kv-panel-title !mb-1">
            {title}
            <span className={`kv-tag ${overLimit ? 'danger' : dirty ? 'warn' : 'ok'}`}>
              {maxBytes ? `${bytes} / ${maxBytes} bytes` : `${bytes} bytes`}
            </span>
          </div>
          <div className="kv-panel-body">{description}</div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <Button
            size="sm"
            onClick={onReload}
            disabled={loading || saving}
            data-tauri-drag-region="false"
          >
            <RefreshCw size={10} className={loading ? 'animate-spin' : ''} />
            {lang === 'zh' ? '重载' : 'Reload'}
          </Button>
          <Button
            variant="primary"
            size="sm"
            onClick={onSave}
            disabled={loading || saving || !dirty || overLimit}
            data-tauri-drag-region="false"
          >
            {saving ? (lang === 'zh' ? '保存中' : 'Saving') : (lang === 'zh' ? '保存' : 'Save')}
          </Button>
        </div>
      </div>
      <textarea
        value={value}
        onChange={(event) => onChange(event.target.value)}
        rows={rows}
        className="kv-textarea mono custom-scrollbar min-h-[160px]"
        spellCheck={false}
        data-tauri-drag-region="false"
        aria-label={title}
      />
      {overLimit && (
        <p className="mt-1.5 text-[11px] leading-snug text-red-500 dark:text-red-400">
          {lang === 'zh'
            ? `${layer.toUpperCase()} 超出字节上限，保存前需要精简。`
            : `${layer.toUpperCase()} is over its byte limit.`}
        </p>
      )}
    </div>
  )
}

function defaultChatConfig(): NonNullable<SettingsData['chat']> {
  return {
    streamEnabled: true,
    thinkingEnabled: true,
    maxOutputTokens: 8192,
    defaultLanguage: '',
    systemPrompt: '',
    userDisplayName: '',
    userAvatar: '',
    defaultAgentRuntime: {
      kind: 'builtin',
      externalAgentId: null,
      externalModel: null,
      externalReasoning: null,
    },
  }
}

function defaultChatMemory(): ChatMemoryConfig {
  return {
    enabled: false,
    toolWriteConfirm: false,
  }
}

function formatTokenCount(tokens?: number): string {
  if (!tokens || !Number.isFinite(tokens)) return ''
  return `${tokens.toLocaleString()} tokens`
}

function resolveEffectiveChatModel(settings: SettingsData): { provider?: ModelProvider, model: string } {
  const configuredChat = settings.defaultModels.chat.providerId
    ? settings.defaultModels.chat
    : settings.chatProviderId
      ? { providerId: settings.chatProviderId, model: settings.chatModel }
      : settings.lens?.providerId
        ? { providerId: settings.lens.providerId, model: settings.lens.model || '' }
        : { providerId: settings.translatorProviderId, model: settings.translatorModel }

  return {
    provider: settings.providers.find((provider) => provider.id === configuredChat.providerId),
    model: configuredChat.model || '',
  }
}

function resolveEffectiveChatMaxOutput(settings: SettingsData, fallbackTokens: number) {
  const { provider, model } = resolveEffectiveChatModel(settings)
  const override = model ? provider?.modelOverrides?.[model]?.maxOutput : undefined
  const modelInfo = model ? resolveModelInfo(model, provider?.modelOverrides) : {}
  const maxOutput = override || modelInfo.maxOutput || fallbackTokens
  const source: 'override' | 'database' | 'fallback' = override
    ? 'override'
    : modelInfo.maxOutput
      ? 'database'
      : 'fallback'

  return { maxOutput, source, model, provider }
}

function defaultDefaultModels(chatProviderId = '', chatModel = ''): SettingsData['defaultModels'] {
  return {
    chat: { providerId: chatProviderId, model: chatModel },
    vision: { providerId: '', model: '' },
    titleSummary: { providerId: '', model: '' },
    compression: { providerId: '', model: '' },
    imageGeneration: { providerId: '', model: '' },
    advisor: { providerId: '', model: '' },
  }
}

function clearDefaultModelProvider(
  defaultModels: SettingsData['defaultModels'],
  providerId: string,
): SettingsData['defaultModels'] {
  return {
    chat: defaultModels.chat.providerId === providerId ? { providerId: '', model: '' } : defaultModels.chat,
    vision: defaultModels.vision.providerId === providerId
      ? { providerId: '', model: '' }
      : defaultModels.vision,
    titleSummary: defaultModels.titleSummary.providerId === providerId
      ? { providerId: '', model: '' }
      : defaultModels.titleSummary,
    compression: defaultModels.compression.providerId === providerId
      ? { providerId: '', model: '' }
      : defaultModels.compression,
    imageGeneration: defaultModels.imageGeneration.providerId === providerId
      ? { providerId: '', model: '' }
      : defaultModels.imageGeneration,
    advisor: defaultModels.advisor.providerId === providerId
      ? { providerId: '', model: '' }
      : defaultModels.advisor,
  }
}

function resolveDefaultModelsAfterModelRemoval(
  defaultModels: SettingsData['defaultModels'],
  providerId: string,
  resolveAfterRemoval: (currentModel: string) => string,
): SettingsData['defaultModels'] {
  return {
    chat: defaultModels.chat.providerId === providerId
      ? { ...defaultModels.chat, model: resolveAfterRemoval(defaultModels.chat.model) }
      : defaultModels.chat,
    vision: defaultModels.vision.providerId === providerId
      ? { ...defaultModels.vision, model: resolveAfterRemoval(defaultModels.vision.model) }
      : defaultModels.vision,
    titleSummary: defaultModels.titleSummary.providerId === providerId
      ? { ...defaultModels.titleSummary, model: resolveAfterRemoval(defaultModels.titleSummary.model) }
      : defaultModels.titleSummary,
    compression: defaultModels.compression.providerId === providerId
      ? { ...defaultModels.compression, model: resolveAfterRemoval(defaultModels.compression.model) }
      : defaultModels.compression,
    imageGeneration: defaultModels.imageGeneration.providerId === providerId
      ? { ...defaultModels.imageGeneration, model: resolveAfterRemoval(defaultModels.imageGeneration.model) }
      : defaultModels.imageGeneration,
    advisor: defaultModels.advisor.providerId === providerId
      ? { ...defaultModels.advisor, model: resolveAfterRemoval(defaultModels.advisor.model) }
      : defaultModels.advisor,
  }
}

/**
 * 头像：可点击的圆形头像，点击=选本地图片 → canvas 缩到 256px → 存 data URL
 * （避免把大图 base64 塞进 settings.json）。空时显示应用默认 logo。value 仍是字符串，
 * 可直接当 <img src>，与旧的 URL 值兼容。
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
 * 设置面板主组件（standalone / embedded 双宿主）
 */
export const SettingsShell = forwardRef<SettingsShellHandle, SettingsShellProps>(function SettingsShell(
  { variant, onClose, onSettingsChange, onReady, reserveTrafficLightSpace = false, initialTab, hideNav = false },
  ref,
) {
  const [settings, setSettings] = useState<SettingsData | null>(null)
  const [initialSettingsSnapshot, setInitialSettingsSnapshot] = useState('')
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [appVersion, setAppVersion] = useState('')
  const [activeTab, setActiveTab] = useState<SettingsTab>(initialTab ?? 'general')
  // 用量统计页内的二级视图：用量统计 / 请求调试（请求调试原为独立导航项，现并入用量统计）
  const [usageView, setUsageView] = useState<'stats' | 'debug'>('stats')
  useEffect(() => {
    if (initialTab) setActiveTab(initialTab)
  }, [initialTab])
  const [saveError, setSaveError] = useState('')
  // 热键被占用未能注册的警告（保存已成功，只是提醒，不阻断）。
  const [saveWarning, setSaveWarning] = useState('')
  const [saveSuccess, setSaveSuccess] = useState(false)
  const [closeConfirmOpen, setCloseConfirmOpen] = useState(false)
  const [confirmDeleteProviderId, setConfirmDeleteProviderId] = useState<string | null>(null)
  const [recordingTarget, setRecordingTarget] = useState<null | 'main' | 'chat' | 'screenshotTranslation' | 'screenshotTranslationText' | 'screenshotTranslationReplace' | 'screenshotAnnotate' | 'lens'>(null)
  const [defaultPrompts, setDefaultPrompts] = useState<DefaultPromptTemplates | null>(null)
  const [chatSystemPromptInteracted, setChatSystemPromptInteracted] = useState(false)
  const [retryAttemptsInput, setRetryAttemptsInput] = useState('')
  const [uiFontPxInput, setUiFontPxInput] = useState('')
  const [systemFonts, setSystemFonts] = useState<string[]>([])
  const [permissionStatus, setPermissionStatus] = useState<PermissionStatus | null>(null)
  const [permissionsLoading, setPermissionsLoading] = useState(false)
  const [fetchingProviderId, setFetchingProviderId] = useState<string | null>(null)
  const [modelPickerProviderId, setModelPickerProviderId] = useState<string | null>(null)
  const [drawerModel, setDrawerModel] = useState<{ providerId: string; model: string } | null>(null)
  const [modelTestProviderId, setModelTestProviderId] = useState<string | null>(null)
  const [selectedProviderId, setSelectedProviderId] = useState('')
  const [memoryDrafts, setMemoryDrafts] = useState<Record<MemoryLayerKey, string>>({ l1: '', l2: '' })
  const [memorySnapshots, setMemorySnapshots] = useState<Record<MemoryLayerKey, string>>({ l1: '', l2: '' })
  const [memoryDir, setMemoryDir] = useState('')
  const [memoryLoading, setMemoryLoading] = useState(false)
  const [memorySavingLayer, setMemorySavingLayer] = useState<MemoryLayerKey | null>(null)
  const [memoryError, setMemoryError] = useState('')
  const [memorySuccess, setMemorySuccess] = useState('')
  // 更新检查状态：'idle' / 'checking' / 'up-to-date' / 'available' / 'check-failed'
  const [updateStatus, setUpdateStatus] = useState<'idle' | 'checking' | 'up-to-date' | 'available' | 'check-failed'>('idle')
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null)
  // 下载/安装两段式状态机:idle → downloading(进度条) → downloaded(显示安装按钮) → 用户点击 → 应用退出
  // failed 时显示错误 + 重试 + 跳 GitHub 兜底
  const [downloadState, setDownloadState] = useState<'idle' | 'downloading' | 'downloaded' | 'failed'>('idle')
  const [downloadPercent, setDownloadPercent] = useState(0)
  const requestWindowFocus = useWindowInteractionFocus()
  const [downloadedPath, setDownloadedPath] = useState('')
  const [downloadError, setDownloadError] = useState('')
  // RapidOCR 离线 OCR 状态:检查 app data 目录里 dylib + 模型 4 个文件齐不齐。
  const [rapidOcrStatus, setRapidOcrStatus] = useState<import('../api/tauri').RapidOcrStatus | null>(null)
  // 下载临时状态:'idle' / 'downloading' / 'failed'(success 后自动 refresh status 到已就绪,
  // 没有专门的 success 终态)
  const [rapidOcrDownloadState, setRapidOcrDownloadState] = useState<'idle' | 'downloading' | 'failed'>('idle')
  const [rapidOcrDownloadError, setRapidOcrDownloadError] = useState('')
  const [replacePackStatus, setReplacePackStatus] = useState<ReplaceTranslationPackStatus | null>(null)
  const [replacePackDownload, dispatchReplacePackDownload] = useReducer(
    reduceReplacePackProgress,
    initialReplacePackProgressState,
  )
  const platform = getPlatform()
  const isMac = platform === 'macos'
  const hasSystemOcr = isMac || platform === 'windows'
  // 加载失败时的错误信息；非空则渲染错误 UI 而不是用合成默认值进入正常视图
  // （否则用户可能没察觉就 Save 把磁盘真实数据覆盖掉）
  const [loadError, setLoadError] = useState('')
  const [reloadKey, setReloadKey] = useState(0)
  const saveSuccessTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const readyEmittedRef = useRef(false)
  // 镜像当前草稿的序列化快照，供后台 SWR 校准回调判断“用户是否已改动”而无需闭包捕获最新 state。
  const currentSettingsSnapshotRef = useRef('')

  const lang = settings?.settingsLanguage || 'zh'
  const t = i18n[lang]
  const themeColor = normalizeThemeColorId(settings?.themeColor)
  const chatTools = settings?.chatTools || defaultChatTools()
  const nativeBuiltinToolsEnabled = hasEnabledNativeBuiltinTool(chatTools.nativeTools)
  const skillRuntimeEnabled = hasEnabledSkillRuntime(chatTools.nativeTools)
  // 判断是否有未保存的更改
  const hasUnsavedChanges = settings ? stableStringify(settings) !== initialSettingsSnapshot : false
  // 同步当前草稿快照到 ref（SWR 校准回调据此判断草稿是否 pristine）。
  useEffect(() => {
    currentSettingsSnapshotRef.current = settings ? stableStringify(settings) : ''
  }, [settings])

  // 客户端热键冲突检测:在保存前发现"两个启用功能用了同一个组合"。
  // OS 层面的冲突(Spotlight 占用 Cmd+Space 等)仍需保存后从后端拿到结果。
  // 返回每个 scope 对应的"和谁冲突"——前端各 HotkeyInput 拿到对应 scope 的伙伴名后,
  // 用 hotkeyScope* 模板自己拼本地化字符串。
  const hotkeyConflicts = useMemo<Partial<Record<HotkeyScopeKey, HotkeyScopeKey>>>(() => {
    if (!settings) return {}
    const slots: Array<{ scope: HotkeyScopeKey; hotkey: string; enabled: boolean }> = [
      { scope: 'main', hotkey: settings.hotkey || '', enabled: !!(settings.hotkey || '').trim() },
      { scope: 'chat', hotkey: settings.chatHotkey || '', enabled: !!(settings.chatHotkey || '').trim() },
      {
        scope: 'screenshotTranslation',
        hotkey: settings.screenshotTranslation?.hotkey || '',
        enabled: settings.screenshotTranslation?.enabled !== false,
      },
      {
        scope: 'screenshotTranslationText',
        hotkey: settings.screenshotTranslation?.textHotkey || '',
        enabled: settings.screenshotTranslation?.enabled !== false,
      },
      {
        scope: 'screenshotTranslationReplace',
        hotkey: settings.screenshotTranslation?.replaceHotkey || '',
        enabled: settings.screenshotTranslation?.enabled !== false
          && settings.screenshotTranslation?.replaceEnabled !== false,
      },
      { scope: 'screenshotAnnotate', hotkey: settings.screenshotAnnotate?.hotkey || '', enabled: settings.screenshotAnnotate?.enabled !== false },
      { scope: 'lens', hotkey: settings.lens?.hotkey || '', enabled: settings.lens?.enabled !== false },
    ]
    const groups = new Map<string, HotkeyScopeKey[]>()
    for (const s of slots) {
      const key = s.hotkey.trim().toLowerCase()
      if (!key || !s.enabled) continue
      const list = groups.get(key) ?? []
      list.push(s.scope)
      groups.set(key, list)
    }
    const out: Partial<Record<HotkeyScopeKey, HotkeyScopeKey>> = {}
    for (const list of groups.values()) {
      if (list.length < 2) continue
      for (const scope of list) {
        const partner = list.find(other => other !== scope)
        if (partner) out[scope] = partner
      }
    }
    return out
  }, [settings])

  const SCOPE_I18N_KEY: Record<HotkeyScopeKey, 'hotkeyScopeTranslator' | 'hotkeyScopeChat' | 'hotkeyScopeScreenshot' | 'hotkeyScopeScreenshotText' | 'hotkeyScopeScreenshotReplace' | 'annotateHotkeyLabel' | 'hotkeyScopeLens'> = {
    main: 'hotkeyScopeTranslator',
    chat: 'hotkeyScopeChat',
    screenshotTranslation: 'hotkeyScopeScreenshot',
    screenshotTranslationText: 'hotkeyScopeScreenshotText',
    screenshotTranslationReplace: 'hotkeyScopeScreenshotReplace',
    screenshotAnnotate: 'annotateHotkeyLabel',
    lens: 'hotkeyScopeLens',
  }
  const conflictMessageFor = (scope: HotkeyScopeKey): string | undefined => {
    const partner = hotkeyConflicts[scope]
    if (!partner) return undefined
    return t.hotkeyConflictWith.replace('{partner}', t[SCOPE_I18N_KEY[partner]])
  }

  // 初始化：加载设置、版本号、默认提示词
  // 重试通过递增 reloadKey 触发本 effect 重跑
  useEffect(() => {
    let active = true
    readyEmittedRef.current = false
    setLoadError('')

    // stale-while-revalidate：有缓存则首帧直接渲染缓存数据（不显示 loading 转圈），
    // 再后台校准；仅当用户尚未改动草稿时才应用校准后的新值，避免覆盖正在编辑的内容。
    const cached = peekSettings()
    if (cached) {
      const cachedSnapshot = stableStringify(cached)
      // 立即种入草稿快照 ref，避免后台校准回调在 sync effect 提交前读到初始空值而误判“已改动”。
      currentSettingsSnapshotRef.current = cachedSnapshot
      setSettings(cached)
      setInitialSettingsSnapshot(cachedSnapshot)
      setChatSystemPromptInteracted(false)
      setLoading(false)
      void refreshSettings()
        .then((fresh) => {
          if (!active) return
          const freshSnapshot = stableStringify(fresh)
          if (freshSnapshot === cachedSnapshot) return // 磁盘无变化
          // 用户已改动草稿（当前快照 ≠ 加载时的缓存快照）→ 保留草稿，不覆盖
          if (currentSettingsSnapshotRef.current !== cachedSnapshot) return
          setSettings(fresh)
          setInitialSettingsSnapshot(freshSnapshot)
        })
        .catch(() => {
          // 校准失败静默：保留已渲染的缓存数据
        })
    } else {
      setLoading(true)
      getSettingsCached()
        .then((data: SettingsData) => {
          if (!active) return
          setSettings(data)
          setInitialSettingsSnapshot(stableStringify(data))
          setChatSystemPromptInteracted(false)
          setLoading(false)
        })
        .catch((err) => {
          if (!active) return
          console.error('Failed to load settings:', err)
          // 不合成默认值：避免用户在错误状态下 Save 把磁盘真实数据覆盖掉
          // 渲染分支会根据 loadError 显示重试 UI
          const message = err instanceof Error ? err.message : String(err)
          setLoadError(message || 'Unknown error')
          setLoading(false)
        })
    }
    api.getAppVersion()
      .then((ver: string) => {
        if (active) setAppVersion(ver)
      })
      .catch(() => {
        if (active) setAppVersion('unknown')
      })
    api.getDefaultPromptTemplates()
      .then((templates) => {
        if (active) setDefaultPrompts(templates)
      })
      .catch((err) => {
        console.error('Failed to load default prompt templates:', err)
      })
    // resizeWindow 已在 App.tsx 中处理，此处不再重复调用
    return () => {
      active = false
    }
  }, [reloadKey])

  // 首屏内容就绪信号：settings 数据就绪或错误态就绪时触发一次。用于让宿主（Chat→App）
  // 把窗口 show 推迟到设置页可渲染之后，避免“窗口已弹出但在转圈”。传了 onReady 就触发，
  // 不再限定 standalone（旧独立设置窗时代的遗留条件）。
  useEffect(() => {
    if (!onReady) return
    if (!loading && !readyEmittedRef.current && (settings || loadError)) {
      readyEmittedRef.current = true
      onReady()
    }
  }, [loadError, loading, onReady, settings])

  /**
   * 刷新权限状态（macOS）
   */
  const refreshPermissions = useCallback(async () => {
    setPermissionsLoading(true)
    try {
      const status = await api.getPermissionStatus()
      setPermissionStatus(status)
    } catch (err) {
      console.error('Failed to get permission status:', err)
    } finally {
      setPermissionsLoading(false)
    }
  }, [])

  useEffect(() => {
    refreshPermissions()
  }, [refreshPermissions])

  // 监听后端启动时的 update-available 事件，发现新版立即在 About 区块展开提示
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined
    api.onUpdateAvailable((info) => {
      if (cancelled) return
      setUpdateInfo(info)
      setUpdateStatus('available')
    }).then((u) => {
      if (cancelled) u()
      else unlisten = u
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  // Settings 打开时静默 check 一次（覆盖启动事件用户当时没开 Settings 的场景）
  useEffect(() => {
    if (!settings) return
    if (settings.autoCheckUpdate === false) return
    if (updateStatus === 'available' || updateStatus === 'checking') return
    let cancelled = false
    api.checkUpdate().then((info) => {
      if (cancelled) return
      if (info.available) {
        setUpdateInfo(info)
        setUpdateStatus('available')
      }
    }).catch(() => {})
    return () => { cancelled = true }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings?.autoCheckUpdate, !!settings])

  /** 用户点 "检查更新" 按钮 */
  const handleCheckUpdate = useCallback(async () => {
    setUpdateStatus('checking')
    try {
      const info = await api.checkUpdate()
      if (info.checkFailed) {
        // 检查本身失败（网络 / api.github.com 受限）——明确提示，不伪装成"已是最新"。
        setUpdateStatus('check-failed')
      } else if (info.available) {
        setUpdateInfo(info)
        setUpdateStatus('available')
      } else {
        setUpdateStatus('up-to-date')
        // 5s 后自动复位回 idle，避免"已是最新"标签长期占位
        setTimeout(() => setUpdateStatus((s) => (s === 'up-to-date' ? 'idle' : s)), 5000)
      }
    } catch (err) {
      console.error('Check update failed:', err)
      setUpdateStatus('check-failed')
    }
  }, [])

  /** 检查失败兜底：打开 GitHub releases 页（github.com 主体，通常可访问） */
  const handleOpenGithubReleases = useCallback(async () => {
    try {
      await api.openExternal('https://github.com/ZMGID/kivio/releases')
    } catch (err) {
      console.error('Open GitHub releases failed:', err)
    }
  }, [])

  const handleOpenReleasePage = useCallback(async () => {
    if (!updateInfo?.htmlUrl) return
    try {
      await api.openExternal(updateInfo.htmlUrl)
    } catch (err) {
      console.error('Open release page failed:', err)
    }
  }, [updateInfo])

  /** 下载新版安装包到 temp dir,期间监听 update-download-progress 事件刷新进度条 */
  const handleDownloadAndInstall = useCallback(async () => {
    if (!updateInfo?.version) return
    setDownloadState('downloading')
    setDownloadPercent(0)
    setDownloadError('')
    let unlisten: (() => void) | undefined
    try {
      unlisten = await api.onUpdateDownloadProgress((p) => {
        setDownloadPercent(Math.max(0, Math.min(100, Math.round(p.percent))))
      })
      const path = await api.downloadUpdate(updateInfo.version)
      setDownloadedPath(path)
      setDownloadState('downloaded')
    } catch (err) {
      console.error('Download update failed:', err)
      setDownloadError(typeof err === 'string' ? err : (err instanceof Error ? err.message : String(err)))
      setDownloadState('failed')
    } finally {
      unlisten?.()
    }
  }, [updateInfo])

  /** 启动 installer 并退出当前应用。Rust 端会在 macOS 上 cp 新 .app + open,在 Windows spawn NSIS exe */
  const handleInstall = useCallback(async () => {
    if (!downloadedPath) return
    try {
      await api.installUpdate(downloadedPath)
    } catch (err) {
      console.error('Install update failed:', err)
      setDownloadError(typeof err === 'string' ? err : (err instanceof Error ? err.message : String(err)))
      setDownloadState('failed')
    }
  }, [downloadedPath])

  /** 拉一次 RapidOCR 状态(app data 里 dylib + 模型 4 个文件齐不齐)。
   *  挂载时 + 切换到 RapidOCR 引擎时调一下。 */
  const refreshRapidOcrStatus = useCallback(async () => {
    if (!hasSystemOcr) return
    try {
      const status = await api.rapidOcrStatus()
      setRapidOcrStatus(status)
    } catch (err) {
      console.error('rapidOcrStatus failed:', err)
    }
  }, [hasSystemOcr])

  /** 下载指定档位的 RapidOCR 包(dylib 共享 + 该档模型):阻塞若干秒,完成后 refresh status。 */
  const handleDownloadRapidOcr = useCallback(async (tier: import('../api/tauri').RapidOcrTier) => {
    setRapidOcrDownloadState('downloading')
    setRapidOcrDownloadError('')
    try {
      const result = await api.rapidOcrInstall(tier)
      if (result.success) {
        setRapidOcrDownloadState('idle')
        await refreshRapidOcrStatus()
      } else {
        setRapidOcrDownloadError(result.message)
        setRapidOcrDownloadState('failed')
      }
    } catch (err) {
      const msg = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err)
      setRapidOcrDownloadError(msg)
      setRapidOcrDownloadState('failed')
    }
  }, [refreshRapidOcrStatus])

  const refreshReplacePackStatus = useCallback(async (tier: RapidOcrTier) => {
    if (!hasSystemOcr) return
    try {
      setReplacePackStatus(await api.replaceTranslationPackStatus(tier))
    } catch (err) {
      console.error('replaceTranslationPackStatus failed:', err)
    }
  }, [hasSystemOcr])

  const handleDownloadReplacePack = useCallback(async (tier: RapidOcrTier) => {
    dispatchReplacePackDownload({ type: 'start' })
    try {
      const result = await api.replaceTranslationPackInstall(tier)
      if (result.success) {
        dispatchReplacePackDownload({ type: 'success' })
        await Promise.all([refreshReplacePackStatus(tier), refreshRapidOcrStatus()])
      } else {
        dispatchReplacePackDownload({ type: 'failure', error: result.message })
      }
    } catch (err) {
      const message = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err)
      dispatchReplacePackDownload({ type: 'failure', error: message })
    }
  }, [refreshRapidOcrStatus, refreshReplacePackStatus])

  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined
    const tier = settings?.screenshotTranslation?.rapidOcrTier ?? 'standard'
    api.onReplaceTranslationPackProgress(progress => {
      if (cancelled) return
      // 两个安装包共用同一事件名；只消费替换翻译离线包事件，
      // 避免知识库 RapidOCR 安装把本面板驱动进“下载中”。
      if (progress.pack !== 'replace_translation') return
      dispatchReplacePackDownload({ type: 'progress', progress })
      // 终态（最后一个文件 completed）时刷新就绪状态，
      // 与 handleDownloadReplacePack 成功路径一致；即使 install promise 丢失也不会卡住。
      if (progress.state === 'completed' && progress.overallDownloadedBytes >= progress.overallTotalBytes) {
        void refreshReplacePackStatus(tier)
      }
    }).then(dispose => {
      if (cancelled) dispose()
      else unlisten = dispose
    }).catch(err => console.error('replace translation pack progress listener failed:', err))
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [refreshReplacePackStatus, settings?.screenshotTranslation?.rapidOcrTier])

  // 挂载时拉一次状态
  useEffect(() => {
    refreshRapidOcrStatus()
  }, [refreshRapidOcrStatus])

  useEffect(() => {
    const tier = settings?.screenshotTranslation?.rapidOcrTier ?? 'standard'
    void refreshReplacePackStatus(tier)
  }, [refreshReplacePackStatus, settings?.screenshotTranslation?.rapidOcrTier])

  const retryAttempts = settings?.retryAttempts

  useEffect(() => {
    if (retryAttempts === undefined) return
    setRetryAttemptsInput(String(retryAttempts ?? 3))
  }, [retryAttempts])

  const uiFontScale = settings?.uiFontScale
  useEffect(() => {
    if (uiFontScale === undefined) return
    setUiFontPxInput(String(Math.round((uiFontScale ?? 1) * UI_FONT_BASE_PX)))
  }, [uiFontScale])

  const commitUiFontPx = (raw: string, commit: boolean) => {
    if (!settings) return
    const fallback = String(Math.round((settings.uiFontScale ?? 1) * UI_FONT_BASE_PX))
    if (raw.trim() === '') {
      if (commit) setUiFontPxInput(fallback)
      return
    }
    const parsed = Number.parseInt(raw, 10)
    if (Number.isNaN(parsed)) {
      if (commit) setUiFontPxInput(fallback)
      return
    }
    const clamped = Math.min(UI_FONT_PX_MAX, Math.max(UI_FONT_PX_MIN, parsed))
    if (commit) setUiFontPxInput(String(clamped))
    updateSettings({ uiFontScale: clamped / UI_FONT_BASE_PX })
  }

  // 系统已装字体列表，仅拉取一次（前端无法枚举，走后端 CoreText/GDI）。
  useEffect(() => {
    let alive = true
    api.listSystemFonts().then((fonts) => {
      if (alive) setSystemFonts(fonts)
    })
    return () => {
      alive = false
    }
  }, [])

  useEffect(() => {
    if (!settings?.providers.length) {
      setSelectedProviderId('')
      return
    }
    if (!selectedProviderId || !settings.providers.some((provider) => provider.id === selectedProviderId)) {
      setSelectedProviderId(settings.providers[0].id)
    }
  }, [selectedProviderId, settings?.providers])

  /**
   * 保存设置
   */
  const handleSave = useCallback(async () => {
    if (!settings) return false
    try {
      setSaving(true)
      setSaveError('')
      setSaveWarning('')
      setSaveSuccess(false)
      if (saveSuccessTimerRef.current) {
        clearTimeout(saveSuccessTimerRef.current)
        saveSuccessTimerRef.current = null
      }
      const savedSettings = await saveSettingsCached(settings)
      setSettings(savedSettings)
      setInitialSettingsSnapshot(stableStringify(savedSettings))
      onSettingsChange()
      setSaveSuccess(true)
      saveSuccessTimerRef.current = setTimeout(() => {
        setSaveSuccess(false)
        saveSuccessTimerRef.current = null
      }, 2200)
      return true
    } catch (err) {
      console.error('Failed to save settings:', err)
      const message = err instanceof Error ? err.message : String(err)
      const translated = formatHotkeyError(message, lang)
      const prefix = lang === 'zh' ? '保存失败:' : 'Save failed: '
      setSaveError(`${prefix}${translated.replace(/\n/g, ' / ')}`)
      setSaveSuccess(false)
      return false
    } finally {
      setSaving(false)
    }
  }, [lang, onSettingsChange, settings])

  useEffect(() => {
    return () => {
      if (saveSuccessTimerRef.current) {
        clearTimeout(saveSuccessTimerRef.current)
      }
    }
  }, [])

  /**
   * 请求关闭设置页（检查未保存更改）
   */
  const handleCloseRequest = useCallback(() => {
    if (recordingTarget) return
    if (hasUnsavedChanges) {
      setCloseConfirmOpen(true)
      return
    }
    onClose()
  }, [hasUnsavedChanges, onClose, recordingTarget])

  useImperativeHandle(ref, () => ({ requestClose: handleCloseRequest }), [handleCloseRequest])

  // 放弃更改并关闭
  const handleDiscardAndClose = () => {
    setCloseConfirmOpen(false)
    onClose()
  }

  // 保存并关闭
  const handleSaveAndClose = useCallback(async () => {
    const saved = await handleSave()
    if (saved) {
      setCloseConfirmOpen(false)
      onClose()
    }
  }, [handleSave, onClose])

  const handleSettingsDragMouseDown = useCallback((event: React.MouseEvent<HTMLElement>) => {
    if (event.button !== 0) return
    const target = event.target as HTMLElement | null
    if (target?.closest('button, input, textarea, select, [data-tauri-drag-region="false"]')) return
    event.preventDefault()
    void api.startDragging().catch((err) => {
      console.error('[settings-drag] startDragging failed:', err)
    })
  }, [])

  // 全局键盘：Esc 关闭、Cmd/Ctrl+S 保存；弹窗打开时优先处理弹窗内的 Esc/Enter
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (recordingTarget) return

      if (modelPickerProviderId) {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          setModelPickerProviderId(null)
        }
        return
      }

      // 删除供应商弹窗：Esc 取消；不绑定 Enter，避免误触发破坏性删除
      if (confirmDeleteProviderId) {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          setConfirmDeleteProviderId(null)
        }
        return
      }

      // 未保存确认弹窗：Esc = 继续编辑（关弹窗）；Enter = 保存并关闭
      if (closeConfirmOpen) {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          setCloseConfirmOpen(false)
        } else if (e.key === 'Enter') {
          e.preventDefault()
          e.stopPropagation()
          if (!saving) void handleSaveAndClose()
        }
        return
      }

      if (e.key === 'Escape') {
        handleCloseRequest()
      } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's') {
        e.preventDefault()
        if (hasUnsavedChanges && !saving) void handleSave()
      }
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [
    handleCloseRequest,
    recordingTarget,
    closeConfirmOpen,
    confirmDeleteProviderId,
    modelPickerProviderId,
    saving,
    hasUnsavedChanges,
    handleSave,
    handleSaveAndClose,
  ])

  /**
   * 打开 macOS 系统权限设置
   */
  const handleOpenPermissionSettings = async (kind: 'accessibility' | 'screen-recording') => {
    try {
      await api.openPermissionSettings(kind)
    } catch (err) {
      console.error('Failed to open permission settings:', err)
    }
  }

  // 重试次数输入处理
  const handleRetryAttemptsChange = (value: string) => {
    if (!settings) return
    setRetryAttemptsInput(value)
    if (value.trim() === '') return
    const parsed = Number.parseInt(value, 10)
    if (Number.isNaN(parsed)) return
    const clamped = Math.min(5, Math.max(1, parsed))
    updateSettings({ retryAttempts: clamped })
  }

  const handleRetryAttemptsBlur = () => {
    if (!settings) return
    if (retryAttemptsInput.trim() === '') {
      setRetryAttemptsInput(String(settings.retryAttempts ?? 3))
      return
    }
    const parsed = Number.parseInt(retryAttemptsInput, 10)
    if (Number.isNaN(parsed)) {
      setRetryAttemptsInput(String(settings.retryAttempts ?? 3))
      return
    }
    const clamped = Math.min(5, Math.max(1, parsed))
    setRetryAttemptsInput(String(clamped))
    if (clamped !== settings.retryAttempts) {
      updateSettings({ retryAttempts: clamped })
    }
  }

  /**
   * 更新设置字段
   */
  const updateSettings = useCallback((updates: Partial<SettingsData>) => {
    setSettings((prev) => {
      if (!prev) return prev
      return { ...prev, ...updates }
    })
  }, [])

  // 设置备份：导出/导入 JSON。导入会覆盖全部设置并立即生效。
  const [backupStatus, setBackupStatus] = useState<{ kind: 'ok' | 'err'; msg: string } | null>(null)

  // 哪些 API Key 输入框处于明文显示（按 `${providerId}-${idx}` 记），默认全部隐藏。
  const [revealedKeys, setRevealedKeys] = useState<Set<string>>(new Set())
  const [gzipInfoOpen, setGzipInfoOpen] = useState<Set<string>>(new Set())
  const toggleKeyReveal = useCallback((keyId: string) => {
    setRevealedKeys((prev) => {
      const next = new Set(prev)
      if (next.has(keyId)) next.delete(keyId)
      else next.add(keyId)
      return next
    })
  }, [])

  const handleExportSettings = useCallback(async () => {
    try {
      const path = await save({
        defaultPath: 'kivio-settings-backup.json',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      })
      if (!path) return
      await api.exportSettings(path)
      setBackupStatus({ kind: 'ok', msg: lang === 'zh' ? '设置已导出。' : 'Settings exported.' })
    } catch (err) {
      setBackupStatus({ kind: 'err', msg: `${lang === 'zh' ? '导出失败：' : 'Export failed: '}${err}` })
    }
  }, [lang])

  const handleImportSettings = useCallback(async () => {
    try {
      const selected = await open({ multiple: false, filters: [{ name: 'JSON', extensions: ['json'] }] })
      if (!selected || typeof selected !== 'string') return
      const imported = await importSettingsCached(selected)
      setSettings(imported)
      setBackupStatus({ kind: 'ok', msg: lang === 'zh' ? '设置已导入并生效。' : 'Settings imported and applied.' })
    } catch (err) {
      setBackupStatus({ kind: 'err', msg: `${lang === 'zh' ? '导入失败：' : 'Import failed: '}${err}` })
    }
  }, [lang])

  const handleRestartOnboarding = useCallback(async () => {
    if (!settings) return
    try {
      const saved = await saveSettingsCached({
        ...settings,
        onboardingStatus: 'pending',
      })
      setSettings(saved)
      setInitialSettingsSnapshot(stableStringify(saved))
      onSettingsChange()
      window.location.hash = '#chat/onboarding'
    } catch (err) {
      console.error('Failed to restart onboarding:', err)
    }
  }, [onSettingsChange, settings])

  const updateDefaultModel = useCallback((
    key: keyof SettingsData['defaultModels'],
    providerId: string,
    model: string,
  ) => {
    setSettings((prev) => {
      if (!prev) return prev
      const current = prev.defaultModels || defaultDefaultModels(prev.chatProviderId, prev.chatModel)
      const defaultModels = {
        ...current,
        [key]: { providerId, model },
      }
      return {
        ...prev,
        defaultModels,
        ...(key === 'chat' ? { chatProviderId: providerId, chatModel: model } : {}),
      }
    })
  }, [])

  const updateChatTools = useCallback((updates: Partial<ChatToolsConfig>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const current = prev.chatTools || defaultChatTools()
      return { ...prev, chatTools: { ...current, ...updates } }
    })
  }, [])

  const updateNativeTools = useCallback((updates: Partial<ChatNativeToolsConfig>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const chatTools = prev.chatTools || defaultChatTools()
      return {
        ...prev,
        chatTools: {
          ...chatTools,
          nativeTools: {
            ...defaultNativeTools(),
            ...chatTools.nativeTools,
            ...updates,
          },
        },
      }
    })
  }, [])

  // 保存后若有热键被系统/其他应用占用未注册，后端推 hotkey-warning——提示但不视为保存失败。
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | null = null
    void api.onHotkeyWarning((raw) => {
      if (cancelled) return
      const detail = formatHotkeyError(raw, lang).replace(/\n/g, ' / ')
      setSaveWarning(lang === 'zh' ? `已保存,但部分热键未注册:${detail}` : `Saved, but some hotkeys were not registered: ${detail}`)
    }).then((fn) => {
      if (cancelled) fn()
      else unlisten = fn
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [lang])

  /**
   * 更新指定提供商配置
   */
  const updateProvider = useCallback((id: string, updates: Partial<ModelProvider>) => {
    setSettings((prev) => {
      if (!prev) return prev
      return {
        ...prev,
        providers: prev.providers.map(p => p.id === id ? { ...p, ...updates } : p)
      }
    })
  }, [])

  const reorderProviders = useCallback((fromId: string, toId: string) => {
    if (fromId === toId) return
    setSettings((prev) => {
      if (!prev) return prev
      const fromIndex = prev.providers.findIndex((p) => p.id === fromId)
      const toIndex = prev.providers.findIndex((p) => p.id === toId)
      if (fromIndex < 0 || toIndex < 0) return prev
      const nextProviders = [...prev.providers]
      const [moved] = nextProviders.splice(fromIndex, 1)
      nextProviders.splice(toIndex, 0, moved)
      return { ...prev, providers: nextProviders }
    })
  }, [])

  /**
   * 添加新提供商
   */
  const addProvider = () => {
    if (!settings) return
    const newId = `provider-${Date.now()}`
    const newProvider: ModelProvider = {
      id: newId,
      name: 'New Provider',
      apiKeys: [],
      baseUrl: 'https://api.openai.com/v1',
      availableModels: [],
      enabledModels: [],
      enabled: true,
      apiFormat: 'openai_chat',
    }
    setSettings({
      ...settings,
      providers: [...settings.providers, newProvider]
    })
    setSelectedProviderId(newId)
  }

  /** 用预设一键添加 provider —— baseUrl 和默认模型已填好，用户只需填 API key */
  const addProviderFromPreset = (preset: ProviderPreset) => {
    if (!settings) return
    const newId = `provider-${Date.now()}`
    const newProvider: ModelProvider = {
      id: newId,
      name: preset.name,
      apiKeys: [],
      baseUrl: preset.baseUrl,
      availableModels: [],
      enabledModels: [],
      enabled: true,
      apiFormat: 'openai_chat',
    }
    setSettings({
      ...settings,
      providers: [...settings.providers, newProvider]
    })
    setSelectedProviderId(newId)
  }

  /**
   * 根据 ID 查找已启用的提供商（找不到或已禁用时返回第一个已启用的）
   */
  const resolveProvider = (providers: ModelProvider[], providerId: string) => {
    const matched = providers.find(p => p.id === providerId)
    if (matched && isProviderEnabled(matched)) return matched
    return providers.find(p => isProviderEnabled(p)) ?? providers[0]
  }

  /**
   * 确保当前模型在已启用模型列表中
   */
  const resolveModel = (provider: ModelProvider | undefined, currentModel: string) => {
    if (!provider) return currentModel
    if (provider.enabledModels.includes(currentModel)) return currentModel
    return provider.enabledModels[0] || currentModel
  }

  /**
   * 删除提供商
   * 删除后会自动将使用该提供商的功能切换到第一个可用提供商
   */
  const deleteProvider = (id: string) => {
    if (!settings) return
    if (modelPickerProviderId === id) setModelPickerProviderId(null)
    const nextProviders = settings.providers.filter(p => p.id !== id)
    const translatorProvider = resolveProvider(nextProviders, settings.translatorProviderId)
    const screenshotProvider = resolveProvider(nextProviders, settings.screenshotTranslation?.providerId || '')
    // lens providerId 为空表示 fallback 到 translator，删除时若已设置自身 provider 才需要级联
    const lensHadOwnProvider = !!settings.lens?.providerId
    const lensProvider = lensHadOwnProvider
      ? resolveProvider(nextProviders, settings.lens?.providerId || '')
      : undefined
    const deletedProviderWasChatModel =
      settings.defaultModels.chat.providerId === id || settings.chatProviderId === id

    const defaultModels = clearDefaultModelProvider(settings.defaultModels, id)
    const nextSettings: SettingsData = {
      ...settings,
      providers: nextProviders,
      translatorProviderId: translatorProvider ? translatorProvider.id : '',
      translatorModel: resolveModel(translatorProvider, settings.translatorModel),
      defaultModels,
      screenshotTranslation: {
        ...settings.screenshotTranslation,
        providerId: screenshotProvider ? screenshotProvider.id : '',
        model: resolveModel(screenshotProvider, settings.screenshotTranslation?.model || '')
      },
      ...(lensHadOwnProvider ? {
        lens: {
          ...settings.lens,
          providerId: lensProvider ? lensProvider.id : '',
          model: resolveModel(lensProvider, settings.lens?.model || '')
        }
      } : {})
    }
    setSettings({
      ...nextSettings,
      chatProviderId: deletedProviderWasChatModel ? '' : settings.chatProviderId,
      chatModel: deletedProviderWasChatModel ? '' : settings.chatModel,
    })
  }

  /**
   * 添加已启用模型
   */
  const addEnabledModel = (providerId: string, model: string) => {
    if (!settings || !model.trim()) return
    const provider = settings.providers.find(p => p.id === providerId)
    if (!provider || provider.enabledModels.includes(model)) return
    updateProvider(providerId, {
      enabledModels: [...provider.enabledModels, model.trim()]
    })
  }

  const addAllEnabledModels = (providerId: string, models: string[]) => {
    if (!settings || models.length === 0) return
    const provider = settings.providers.find((p) => p.id === providerId)
    if (!provider) return

    const enabledKeys = new Set(provider.enabledModels.map((model) => model.toLowerCase()))
    const nextModels: string[] = []
    const seen = new Set<string>()
    for (const model of models) {
      const trimmed = model.trim()
      if (!trimmed) continue
      const key = trimmed.toLowerCase()
      if (enabledKeys.has(key) || seen.has(key)) continue
      seen.add(key)
      nextModels.push(trimmed)
    }
    if (nextModels.length === 0) return

    updateProvider(providerId, {
      enabledModels: [...provider.enabledModels, ...nextModels],
    })
  }

  /**
   * 移除已启用模型
   * 移除后会自动更新使用该模型的功能到新的默认模型
   */
  const removeEnabledModel = (providerId: string, model: string) => {
    if (!settings) return
    const provider = settings.providers.find((p) => p.id === providerId)
    if (!provider) return

    const nextEnabledModels = provider.enabledModels.filter((m) => m !== model)
    const resolveAfterRemoval = (currentModel: string) => {
      if (currentModel !== model) return currentModel
      return nextEnabledModels[0] || ''
    }

    setSettings((prev) => {
      if (!prev) return prev

      const nextProviders = prev.providers.map((p) =>
        p.id === providerId ? { ...p, enabledModels: nextEnabledModels } : p,
      )

      const next = {
        ...prev,
        providers: nextProviders,
      }
      const defaultModels = resolveDefaultModelsAfterModelRemoval(
        prev.defaultModels,
        providerId,
        resolveAfterRemoval,
      )

      if (prev.translatorProviderId === providerId) {
        next.translatorModel = resolveAfterRemoval(prev.translatorModel)
      }
      next.defaultModels = defaultModels
      next.chatProviderId = defaultModels.chat.providerId
      next.chatModel = defaultModels.chat.model

      if (prev.screenshotTranslation.providerId === providerId) {
        next.screenshotTranslation = {
          ...prev.screenshotTranslation,
          model: resolveAfterRemoval(prev.screenshotTranslation.model),
        }
      }

      if (prev.lens?.providerId === providerId) {
        next.lens = {
          ...prev.lens,
          model: resolveAfterRemoval(prev.lens.model || ''),
        }
      }

      return next
    })
  }

  /**
   * 保存模型自定义参数
   */
  const saveModelOverride = useCallback((providerId: string, modelName: string, info: ModelInfo) => {
    if (!settings) return
    const provider = settings.providers.find(p => p.id === providerId)
    if (!provider) return
    updateProvider(providerId, {
      modelOverrides: {
        ...provider.modelOverrides,
        [modelName]: info,
      },
    })
  }, [settings, updateProvider])

  /**
   * 重置模型参数为数据库默认值
   */
  const resetModelOverride = useCallback((providerId: string, modelName: string) => {
    if (!settings) return
    const provider = settings.providers.find(p => p.id === providerId)
    if (!provider?.modelOverrides?.[modelName]) return
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    const { [modelName]: _removed, ...rest } = provider.modelOverrides
    updateProvider(providerId, { modelOverrides: rest })
  }, [settings, updateProvider])

  /**
   * 从提供商 API 获取可用模型列表
   */
  const fetchModels = async (providerId: string) => {
    if (!settings || fetchingProviderId) return
    setFetchingProviderId(providerId)
    try {
      const currentProvider = settings.providers.find(p => p.id === providerId)
      const models = await api.fetchModels(providerId, currentProvider
        ? {
          id: currentProvider.id,
          baseUrl: currentProvider.baseUrl,
          apiKeys: currentProvider.apiKeys,
        }
        : undefined)
      if (currentProvider) {
        updateProvider(providerId, { availableModels: models })
      }
    } catch (err) {
      console.error('Failed to fetch models:', err)
    } finally {
      setFetchingProviderId(null)
    }
  }

  const openModelPicker = (providerId: string) => {
    if (!settings) return
    const provider = settings.providers.find((p) => p.id === providerId)
    if (!provider) return
    setModelPickerProviderId(providerId)
    if (provider.availableModels.length === 0 && fetchingProviderId !== providerId) {
      void fetchModels(providerId)
    }
  }

  /**
   * 更新截图翻译配置
   */
  const updateScreenshotTranslation = useCallback((updates: Partial<SettingsData['screenshotTranslation']>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const current = prev.screenshotTranslation || {
        enabled: true,
        hotkey: 'CommandOrControl+Shift+A',
        textHotkey: 'CommandOrControl+Shift+T',
        providerId: 'default-ocr',
        model: '',
        directTranslate: false,
        thinkingEnabled: false,
        streamEnabled: true,
        ocrMode: 'cloud_vision',
        prompt: ''
      }
      return { ...prev, screenshotTranslation: { ...current, ...updates } }
    })
  }, [])

  /**
   * 更新截图标注配置
   */
  const updateScreenshotAnnotate = useCallback((updates: Partial<NonNullable<SettingsData['screenshotAnnotate']>>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const current = prev.screenshotAnnotate || {
        enabled: true,
        hotkey: 'CommandOrControl+Shift+S',
      }
      return { ...prev, screenshotAnnotate: { ...current, ...updates } }
    })
  }, [])

  /**
   * 更新 Lens 配置
   */
  const updateLens = useCallback((updates: Partial<SettingsData['lens']>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const current = prev.lens || {
        enabled: true,
        hotkey: 'CommandOrControl+Shift+G',
        providerId: '',
        model: '',
        defaultLanguage: '',
        streamEnabled: true,
        thinkingEnabled: true,
        systemPrompt: '',
        questionPrompt: '',
        sendToChat: true,
        messageOrder: 'asc' as const,
        showCaptureHint: true,
        webSearch: {
          enabled: false,
          provider: 'tavily' as const,
          tavilyApiKey: '',
          exaApiKey: '',
          maxResults: 5,
          searchDepth: 'basic' as const,
        },
      }
      return { ...prev, lens: { ...current, ...updates } }
    })
  }, [])

  const updateChat = useCallback((updates: Partial<NonNullable<SettingsData['chat']>>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const current = prev.chat || defaultChatConfig()
      return { ...prev, chat: { ...current, ...updates } }
    })
  }, [])

  const updateChatMemory = useCallback((updates: Partial<ChatMemoryConfig>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const current = prev.chatMemory || defaultChatMemory()
      return { ...prev, chatMemory: { ...current, ...updates } }
    })
  }, [])

  const updateLensWebSearch = useCallback((updates: Partial<NonNullable<SettingsData['lens']['webSearch']>>) => {
    setSettings((prev) => {
      if (!prev) return prev
      const currentLens = prev.lens || {
        enabled: true,
        hotkey: 'CommandOrControl+Shift+G',
      }
      const currentWebSearch = currentLens.webSearch || {
        enabled: false,
        provider: 'tavily' as const,
        tavilyApiKey: '',
        exaApiKey: '',
        maxResults: 5,
        searchDepth: 'basic' as const,
      }
      return {
        ...prev,
        lens: {
          ...currentLens,
          webSearch: {
            ...currentWebSearch,
            ...updates,
          },
        },
      }
    })
  }, [])

  const refreshChatMemory = useCallback(async () => {
    setMemoryLoading(true)
    setMemoryError('')
    try {
      const result = await api.chatMemoryGet()
      const next = {
        l1: result.l1.content,
        l2: result.l2.content,
      }
      setMemoryDrafts(next)
      setMemorySnapshots(next)
      setMemoryDir(result.dir)
      setMemorySuccess('')
    } catch (err) {
      setMemoryError(err instanceof Error ? err.message : String(err))
    } finally {
      setMemoryLoading(false)
    }
  }, [])

  const handleSaveMemoryLayer = useCallback(async (layer: MemoryLayerKey) => {
    const content = memoryDrafts[layer]
    if (layer === 'l1' && utf8ByteLength(content) > MEMORY_L1_MAX_BYTES) {
      setMemoryError(lang === 'zh'
        ? `L1 超过 ${MEMORY_L1_MAX_BYTES} 字节，请先精简或归档到 L2。`
        : `L1 exceeds ${MEMORY_L1_MAX_BYTES} bytes. Shorten it or archive details into L2.`)
      return
    }
    setMemorySavingLayer(layer)
    setMemoryError('')
    setMemorySuccess('')
    try {
      const saved = await api.chatMemorySave(layer, content)
      setMemoryDrafts((prev) => ({ ...prev, [layer]: saved.content }))
      setMemorySnapshots((prev) => ({ ...prev, [layer]: saved.content }))
      setMemorySuccess(lang === 'zh'
        ? `${layer.toUpperCase()} 已保存`
        : `${layer.toUpperCase()} saved`)
    } catch (err) {
      setMemoryError(err instanceof Error ? err.message : String(err))
    } finally {
      setMemorySavingLayer(null)
    }
  }, [lang, memoryDrafts])

  const handleOpenMemoryFolder = useCallback(async () => {
    setMemoryError('')
    try {
      const result = await api.chatMemoryOpenFolder()
      if (!result.success) {
        setMemoryError(result.error || (lang === 'zh' ? '打开记忆文件夹失败' : 'Failed to open memory folder'))
      } else if (result.path) {
        setMemoryDir(result.path)
      }
    } catch (err) {
      setMemoryError(err instanceof Error ? err.message : String(err))
    }
  }, [lang])

  useEffect(() => {
    if (activeTab === 'memory') {
      void refreshChatMemory()
    }
  }, [activeTab, refreshChatMemory])

  /**
   * 切换快捷键录制状态
   */
  const toggleRecording = (target: 'main' | 'chat' | 'screenshotTranslation' | 'screenshotTranslationText' | 'screenshotTranslationReplace' | 'screenshotAnnotate' | 'lens') => {
    setRecordingTarget((current) => (current === target ? null : target))
  }

  // 当前语言对应的默认 lens 提示词
  const lensDefaults = defaultPrompts?.lensPrompts?.[settings?.lens?.defaultLanguage === 'en' ? 'en' : 'zh']
  const chatLangKey = settings?.chat?.defaultLanguage === 'en' ? 'en' : 'zh'
  const chatDefaults = defaultPrompts?.chatPrompts?.[chatLangKey]
  const chatConfig = settings?.chat || defaultChatConfig()
  const chatMemory = settings?.chatMemory || defaultChatMemory()
  const chatSystemPromptValue = chatSystemPromptInteracted
    ? (chatConfig.systemPrompt || '')
    : (chatConfig.systemPrompt || chatDefaults || '')
  const chatFallbackMaxOutputTokens = chatConfig.maxOutputTokens ?? 8192
  const effectiveChatMaxOutput = settings
    ? resolveEffectiveChatMaxOutput(settings, chatFallbackMaxOutputTokens)
    : { maxOutput: chatFallbackMaxOutputTokens, source: 'fallback' as const, model: '', provider: undefined }
  const chatMaxOutputSourceLabel = effectiveChatMaxOutput.source === 'override'
    ? (lang === 'zh' ? '模型参数' : 'Model override')
    : effectiveChatMaxOutput.source === 'database'
      ? (lang === 'zh' ? '内置模型库' : 'Model database')
      : (lang === 'zh' ? '兜底设置' : 'Fallback setting')
  const chatMaxOutputModelLabel = effectiveChatMaxOutput.model
    ? (effectiveChatMaxOutput.provider?.name
      ? `${effectiveChatMaxOutput.provider.name} / ${effectiveChatMaxOutput.model}`
      : effectiveChatMaxOutput.model)
    : (lang === 'zh' ? '未配置聊天模型' : 'No chat model configured')

  // 快捷键录制监听
  useEffect(() => {
    if (!recordingTarget) return
    const handler = (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()
      if (e.key === 'Escape') {
        setRecordingTarget(null)
        return
      }
      const hotkey = buildHotkey(e)
      if (!hotkey) return
      if (recordingTarget === 'main') {
        updateSettings({ hotkey })
      } else if (recordingTarget === 'chat') {
        updateSettings({ chatHotkey: hotkey })
      } else if (recordingTarget === 'screenshotTranslation') {
        updateScreenshotTranslation({ hotkey })
      } else if (recordingTarget === 'screenshotTranslationText') {
        updateScreenshotTranslation({ textHotkey: hotkey })
      } else if (recordingTarget === 'screenshotTranslationReplace') {
        updateScreenshotTranslation({ replaceHotkey: hotkey })
      } else if (recordingTarget === 'screenshotAnnotate') {
        updateScreenshotAnnotate({ hotkey })
      } else if (recordingTarget === 'lens') {
        updateLens({ hotkey })
      }
      setRecordingTarget(null)
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [recordingTarget, updateLens, updateScreenshotAnnotate, updateScreenshotTranslation, updateSettings])

  const loadingShellClass =
    variant === 'embedded'
      ? 'flex min-h-0 min-w-0 flex-1 items-center justify-center bg-white dark:bg-[#212121]'
      : 'flex items-center justify-center h-full bg-neutral-200 dark:bg-black'

  if (loading) {
    return (
      <div className={loadingShellClass}>
        <div className="w-6 h-6 border-2 border-neutral-300 dark:border-neutral-700 border-t-neutral-800 dark:border-t-neutral-200 rounded-full animate-spin" />
      </div>
    )
  }

  if (loadError || !settings) {
    // 加载失败：显示错误 + 重试按钮，禁止用户在不知情的情况下用合成默认值 Save 覆盖磁盘
    return (
      <div className={`${loadingShellClass} p-6`}>
        <div className="max-w-sm w-full bg-white dark:bg-[#1C1C1E] rounded-xl shadow-sm border border-black/5 dark:border-white/5 p-5 text-center">
          <div className="text-[14px] font-semibold text-neutral-900 dark:text-neutral-100 mb-1">
            {lang === 'zh' ? '加载设置失败' : 'Failed to load settings'}
          </div>
          <div className="text-[11px] text-rose-600 dark:text-rose-400 mb-4 break-all" title={loadError}>
            {loadError}
          </div>
          <div className="flex gap-2 justify-center">
            <button
              type="button"
              onClick={() => setReloadKey((k) => k + 1)}
              className="flex items-center gap-1.5 text-[12px] font-medium px-3 py-1.5 rounded-md bg-neutral-900 dark:bg-white text-white dark:text-neutral-900 hover:bg-neutral-800 dark:hover:bg-neutral-100 transition-colors duration-[var(--kv-dur-fast)]"
              data-tauri-drag-region="false"
            >
              <RefreshCw size={12} />
              {lang === 'zh' ? '重试' : 'Retry'}
            </button>
            <button
              type="button"
              onClick={onClose}
              className="text-[12px] font-medium px-3 py-1.5 rounded-md text-neutral-600 dark:text-neutral-400 hover:bg-black/5 dark:hover:bg-white/5 transition-colors duration-[var(--kv-dur-fast)]"
              data-tauri-drag-region="false"
            >
              {t.cancel}
            </button>
          </div>
        </div>
      </div>
    )
  }

  const navItems = [
    { id: 'general' as const, label: t.tabGeneral, icon: GeneralIcon },
    { id: 'hotkeys' as const, label: t.tabHotkeys, icon: HotkeysIcon },
    { id: 'translate' as const, label: t.tabTranslation, icon: TranslateIcon },
    { id: 'lens' as const, label: t.lensTabLabel, icon: LensIcon },
    { id: 'chat' as const, label: t.tabChatClient, icon: ChatIcon },
    { id: 'memory' as const, label: t.tabMemory, icon: MemoryIcon },
    { id: 'mixer' as const, label: t.tabMixer, icon: MixerIcon },
    { id: 'externalAgents' as const, label: t.tabExternalAgents, icon: AgentIcon },
    { id: 'connectors' as const, label: t.tabConnectors, icon: ConnectorsIcon },
    { id: 'webSearch' as const, label: t.tabWebSearch, icon: WebSearchIcon },
    { id: 'usage' as const, label: lang === 'zh' ? '用量统计' : 'Usage', icon: UsageIcon },
    { id: 'providers' as const, label: t.tabModels, icon: ProvidersIcon },
  ]
  const pageMeta: Record<typeof activeTab, { title: string; subtitle: string; right?: string }> = {
    general: {
      title: t.tabGeneral,
      subtitle: lang === 'zh' ? '外观、行为、归档和权限。' : 'Appearance, behavior, archive, and permissions.',
    },
    translate: {
      title: t.tabTranslation,
      subtitle: lang === 'zh' ? '输入翻译与快速翻译的语言、模型、OCR 和提示词。' : 'Language, model, OCR, and prompts for input and quick translation.',
    },
    hotkeys: {
      title: t.tabHotkeys,
      subtitle: lang === 'zh' ? '集中管理所有全局快捷键。' : 'Manage all global hotkeys in one place.',
    },
    lens: {
      title: t.lensTabLabel,
      subtitle: lang === 'zh' ? '视觉问答的快捷键、响应方式和提示词。' : 'Shortcut, response behavior, and prompts for visual Q&A.',
    },
    chat: {
      title: t.tabChatClient,
      subtitle: lang === 'zh'
        ? '主对话模型、流式/思考、系统提示词。'
        : 'Main chat model, streaming/thinking, and system prompt.',
    },
    memory: {
      title: t.tabMemory,
      subtitle: lang === 'zh'
        ? 'L1 注入；L2 按需读取。'
        : 'L1 injected; L2 read on demand.',
    },
    mixer: {
      title: t.tabMixer,
      subtitle: lang === 'zh'
        ? '按副任务路由模型：视觉、标题总结、上下文压缩、生图。'
        : 'Route models by side task: vision, title summaries, context compression, and image generation.',
    },
    externalAgents: {
      title: t.tabExternalAgents,
      subtitle: lang === 'zh'
        ? '检测并启用外部 CLI 编码代理。'
        : 'Detect and enable external CLI coding agents.',
    },
    connectors: {
      title: t.tabConnectors,
      subtitle: lang === 'zh'
        ? '连接外部数据源；凭据存本机。'
        : 'Connect external data sources; credentials stay local.',
    },
    webSearch: {
      title: t.tabWebSearch,
      subtitle: lang === 'zh'
        ? 'Tavily/Exa 密钥；分别开启 Lens 与 Chat 联网。'
        : 'Tavily/Exa keys; enable web search for Lens and Chat.',
    },
    usage: {
      title: lang === 'zh' ? '用量统计' : 'Usage',
      subtitle: lang === 'zh'
        ? '查看本地模型请求、Token、成本估算和来源分布；请求调试并入此页。'
        : 'Inspect local model requests, tokens, cost, and usage distribution; request debug lives here too.',
    },
    providers: {
      title: t.tabModels,
      subtitle: lang === 'zh' ? '管理 OpenAI 兼容供应商、密钥和启用模型。' : 'Manage OpenAI-compatible providers, keys, and enabled models.',
    },
    about: {
      title: lang === 'zh' ? '关于' : 'About',
      subtitle: lang === 'zh' ? '版本、更新和应用信息。' : 'Version, updates, and application details.',
    },
  }
  const selectedProvider = settings.providers.find((provider) => provider.id === selectedProviderId) ?? settings.providers[0]
  const chatProvider = settings.providers.find((provider) => provider.id === settings.chatProviderId)
    ?? settings.providers.find((provider) => provider.id === settings.lens?.providerId)
    ?? settings.providers.find((provider) => provider.id === settings.translatorProviderId)

  const categoryNav =
    variant === 'embedded' ? (
      <>
        <nav className="settings-embedded-nav-list">
          {navItems.map((item) => {
            const Icon = item.icon
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => setActiveTab(item.id)}
                className={`settings-embedded-nav-item ${activeTab === item.id ? 'active' : ''}`}
                data-tauri-drag-region="false"
              >
                <span className="settings-embedded-nav-icon">
                  <Icon size={17} strokeWidth={1.75} />
                </span>
                <span>{item.label}</span>
              </button>
            )
          })}
        </nav>
        <div className="min-h-0 flex-1" />
        <nav className="settings-embedded-nav-list settings-embedded-nav-list--footer">
          <button
            type="button"
            onClick={() => setActiveTab('about')}
            className={`settings-embedded-nav-item ${activeTab === 'about' ? 'active' : ''}`}
            data-tauri-drag-region="false"
          >
            <span className="settings-embedded-nav-icon">
              <AboutIcon size={17} strokeWidth={1.75} />
            </span>
            <span>{lang === 'zh' ? '关于' : 'About'}</span>
          </button>
        </nav>
      </>
    ) : (
      <>
        <nav className="kv-nav">
          {navItems.map((item) => {
            const Icon = item.icon
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => setActiveTab(item.id)}
                className={`kv-nav-item ${activeTab === item.id ? 'active' : ''}`}
                data-tauri-drag-region="false"
              >
                <Icon strokeWidth={1.7} />
                <span>{item.label}</span>
              </button>
            )
          })}
        </nav>

        <div className="kv-nav-spacer" />

        <nav className="kv-nav">
          <button
            type="button"
            onClick={() => setActiveTab('about')}
            className={`kv-nav-item ${activeTab === 'about' ? 'active' : ''}`}
            data-tauri-drag-region="false"
          >
            <AboutIcon strokeWidth={1.7} />
            <span>{lang === 'zh' ? '关于' : 'About'}</span>
          </button>
        </nav>
      </>
    )

  const settingsMain = (
        <main className={`kv-content ${variant === 'embedded' ? 'settings-embedded-main' : ''}`}>
          <header
            className={`kv-page-header ${variant === 'embedded' ? 'settings-embedded-header' : ''}`}
            onMouseDown={handleSettingsDragMouseDown}
          >
            <div key={activeTab} className="settings-section-title-enter">
              <div className="kv-page-title">{pageMeta[activeTab].title}</div>
              <div className="kv-page-sub">{pageMeta[activeTab].subtitle}</div>
            </div>
            <div className="kv-page-header-right">{pageMeta[activeTab].right}</div>
          </header>

          <div
            key={activeTab}
            className={`kv-scroll settings-section-enter ${variant === 'embedded' ? 'settings-embedded-scroll' : ''}`}
          >
            {/* ===== 基础设置标签页 ===== */}
            {activeTab === 'general' && (
              <>
                <SettingsGroup title={lang === 'zh' ? '外观' : 'Appearance'}>
                  <SettingRow label={t.language}>
                    <Select
                      className="w-36"
                      value={settings.settingsLanguage || 'zh'}
                      onChange={(v) => updateSettings({ settingsLanguage: v as 'zh' | 'en' })}
                      options={[
                        { value: 'zh', label: '中文' },
                        { value: 'en', label: 'English' },
                      ]}
                    />
                  </SettingRow>
                  <SettingRow label={t.theme}>
                    <div className="kv-seg">
                      {[
                        { value: 'system', label: t.themeSystem },
                        { value: 'light', label: t.themeLight },
                        { value: 'dark', label: t.themeDark },
                      ].map((option) => (
                        <button
                          key={option.value}
                          type="button"
                          className={(settings.theme || 'system') === option.value ? 'active' : ''}
                          onClick={() => updateSettings({ theme: option.value as SettingsData['theme'] })}
                          data-tauri-drag-region="false"
                        >
                          {option.label}
                        </button>
                      ))}
                    </div>
                  </SettingRow>
                  <SettingRow label={t.themeColor}>
                    <div className="kv-seg" role="radiogroup" aria-label={t.themeColor}>
                      {THEME_COLOR_PRESETS.map((preset) => {
                        const active = themeColor === preset.id
                        return (
                          <button
                            key={preset.id}
                            type="button"
                            className={active ? 'active' : ''}
                            onClick={() => updateSettings({ themeColor: preset.id })}
                            role="radio"
                            aria-checked={active}
                            data-tauri-drag-region="false"
                          >
                            {preset.labels[lang]}
                          </button>
                        )
                      })}
                    </div>
                  </SettingRow>
                  <SettingRow
                    label={lang === 'zh' ? '界面字号' : 'UI size'}
                    description={lang === 'zh' ? '调整界面使用的基准字号' : 'Adjust the base UI font size'}
                  >
                    <div className="flex items-center gap-2">
                      <Input
                        type="number"
                        value={uiFontPxInput}
                        onChange={(v) => { setUiFontPxInput(v); commitUiFontPx(v, false) }}
                        onBlur={() => commitUiFontPx(uiFontPxInput, true)}
                        min={UI_FONT_PX_MIN}
                        max={UI_FONT_PX_MAX}
                        className="!w-16 text-center"
                      />
                      <span className="text-[13px] text-neutral-400 dark:text-neutral-500">px</span>
                    </div>
                  </SettingRow>
                  <SettingRow
                    label={lang === 'zh' ? '界面字体' : 'UI font'}
                    description={lang === 'zh' ? '搜索并选择系统已安装的字体，不影响性能' : 'Search and pick an installed system font; no performance cost'}
                  >
                    <FontPicker
                      value={settings.uiFontFamily ?? ''}
                      systemFonts={systemFonts}
                      placeholder={lang === 'zh' ? '搜索字体…' : 'Search fonts…'}
                      defaultLabel={lang === 'zh' ? '系统默认' : 'System default'}
                      emptyText={lang === 'zh' ? '无匹配字体' : 'No matching fonts'}
                      onChange={(v) => updateSettings({ uiFontFamily: v })}
                    />
                  </SettingRow>
                  <SettingRow
                    label={lang === 'zh' ? '代码字体' : 'Code font'}
                    description={lang === 'zh' ? '代码块与等宽文本使用的字体' : 'Font for code blocks and monospace text'}
                  >
                    <FontPicker
                      value={settings.uiFontMono ?? ''}
                      systemFonts={systemFonts}
                      placeholder={lang === 'zh' ? '搜索字体…' : 'Search fonts…'}
                      defaultLabel={lang === 'zh' ? '系统默认' : 'System default'}
                      emptyText={lang === 'zh' ? '无匹配字体' : 'No matching fonts'}
                      onChange={(v) => updateSettings({ uiFontMono: v })}
                    />
                  </SettingRow>
                </SettingsGroup>

                <SettingsGroup title={lang === 'zh' ? '行为' : 'Behavior'}>
                  <SettingRow label={t.launchAtStartup}>
                    <Toggle
                      checked={settings.launchAtStartup ?? false}
                      onChange={(v) => updateSettings({ launchAtStartup: v })}
                    />
                  </SettingRow>
                  <SettingRow label={t.retryEnabled}>
                    <Toggle
                      checked={settings.retryEnabled ?? true}
                      onChange={(v) => updateSettings({ retryEnabled: v })}
                    />
                  </SettingRow>
                  {settings.retryEnabled !== false && (
                    <SettingRow label={t.retryAttempts}>
                      <Input
                        type="number"
                        value={retryAttemptsInput}
                        onChange={handleRetryAttemptsChange}
                        onBlur={handleRetryAttemptsBlur}
                        placeholder="3"
                        min={1}
                        max={5}
                        className="!w-20 text-center"
                      />
                    </SettingRow>
                  )}
                </SettingsGroup>

                <SettingsGroup title={lang === 'zh' ? '首次使用' : 'First-time setup'}>
                  <SettingRow
                    label={lang === 'zh' ? '首次使用引导' : 'Setup wizard'}
                    description={t.onboardingRestartDesc}
                  >
                    <Button
                      size="sm"
                      onClick={() => void handleRestartOnboarding()}
                      data-tauri-drag-region="false"
                    >
                      {t.onboardingRestart}
                    </Button>
                  </SettingRow>
                </SettingsGroup>

                <SettingsGroup title={lang === 'zh' ? '备份与恢复' : 'Backup & Restore'}>
                  <FieldBlock
                    label={lang === 'zh' ? '设置备份' : 'Settings backup'}
                    description={lang === 'zh'
                      ? '导出/导入全部设置（含 API Key）。'
                      : 'Export/import all settings (incl. API keys).'}
                  >
                    <div className="flex flex-wrap items-center gap-2">
                      <Button
                        size="sm"
                        onClick={handleExportSettings}
                        data-tauri-drag-region="false"
                      >
                        <Download size={11} />
                        {lang === 'zh' ? '导出设置' : 'Export'}
                      </Button>
                      <Button
                        size="sm"
                        onClick={handleImportSettings}
                        data-tauri-drag-region="false"
                      >
                        <Upload size={11} />
                        {lang === 'zh' ? '导入设置' : 'Import'}
                      </Button>
                      {backupStatus && (
                        <span className={`text-[12px] ${backupStatus.kind === 'ok' ? 'text-emerald-600 dark:text-emerald-400' : 'text-red-500'}`}>
                          {backupStatus.msg}
                        </span>
                      )}
                    </div>
                  </FieldBlock>
                </SettingsGroup>

                {permissionStatus?.platform === 'macos' && (
                  <SettingsGroup title={t.permissions}>
                    <PermissionItem
                      label={t.accessibilityPermission}
                      granted={permissionStatus.accessibility}
                      grantedText={t.permissionGranted}
                      missingText={t.permissionMissing}
                      actionLabel={t.openSystemSettings}
                      onOpen={() => handleOpenPermissionSettings('accessibility')}
                    />
                    <PermissionItem
                      label={t.screenRecordingPermission}
                      granted={permissionStatus.screenRecording}
                      grantedText={t.permissionGranted}
                      missingText={t.permissionMissing}
                      actionLabel={t.openSystemSettings}
                      onOpen={() => handleOpenPermissionSettings('screen-recording')}
                    />
                    <div className="flex justify-end py-2">
                      <Button
                        size="sm"
                        onClick={refreshPermissions}
                        disabled={permissionsLoading}
                        data-tauri-drag-region="false"
                      >
                        <RefreshCw size={10} className={permissionsLoading ? 'animate-spin' : ''} />
                        {t.refreshPermissions}
                      </Button>
                    </div>
                  </SettingsGroup>
                )}
              </>
            )}

            {/* ===== 翻译设置标签页 ===== */}
            {activeTab === 'translate' && (
              <>
                <TranslateTab
                  settings={settings}
                  t={t}
                  lang={lang}
                  defaultPrompts={defaultPrompts}
                  onUpdateSettings={updateSettings}
                />

                <div className="kv-section-title">{t.tabScreenshot}</div>
                <ScreenshotTranslationSettings
                  settings={settings}
                  isMac={isMac}
                  hasSystemOcr={hasSystemOcr}
                  defaultPrompts={defaultPrompts}
                  rapidOcrStatus={rapidOcrStatus}
                  rapidOcrDownloadState={rapidOcrDownloadState}
                  rapidOcrDownloadError={rapidOcrDownloadError}
                  replacePackStatus={replacePackStatus}
                  replacePackDownloadState={replacePackDownload.downloadState}
                  replacePackDownloadError={replacePackDownload.error}
                  replacePackProgress={replacePackDownload.progress}
                  t={t}
                  onUpdate={updateScreenshotTranslation}
                  onRefreshRapidOcrStatus={refreshRapidOcrStatus}
                  onDownloadRapidOcr={handleDownloadRapidOcr}
                  onRefreshReplacePack={refreshReplacePackStatus}
                  onDownloadReplacePack={handleDownloadReplacePack}
                />
              </>
            )}

            {/* ===== 快捷键标签页：集中所有全局热键 ===== */}
            {activeTab === 'hotkeys' && (
              <HotkeysTab
                settings={settings}
                t={t}
                recordingTarget={recordingTarget}
                onToggleRecording={toggleRecording}
                conflictMessageFor={conflictMessageFor}
                onUpdateSettings={updateSettings}
                onUpdateScreenshotTranslation={updateScreenshotTranslation}
                onUpdateScreenshotAnnotate={updateScreenshotAnnotate}
                onUpdateLens={updateLens}
              />
            )}

            {/* ===== Lens 标签页 ===== */}
            {activeTab === 'lens' && (
              <LensTab
                settings={settings}
                t={t}
                lang={lang}
                lensDefaults={lensDefaults}
                onUpdateSettings={updateSettings}
                onUpdateLens={updateLens}
              />
            )}

            {/* ===== AI 客户端标签页 ===== */}
            {activeTab === 'chat' && (
              <>
                <SettingsGroup title={lang === 'zh' ? '个人资料' : 'Profile'}>
                  <div className="flex items-center gap-3 py-1">
                    <AvatarField
                      value={chatConfig.userAvatar || ''}
                      onChange={(userAvatar) => updateChat({ userAvatar })}
                      zh={lang === 'zh'}
                    />
                    <Input
                      className="min-w-0 flex-1"
                      value={chatConfig.userDisplayName || ''}
                      onChange={(userDisplayName) => updateChat({ userDisplayName })}
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
                          onChange={(workingDirectory) => updateNativeTools({ workingDirectory })}
                        />
                        <Button
                          size="sm"
                          className="shrink-0"
                          onClick={async () => {
                            const selected = await open({ directory: true, multiple: false })
                            if (!selected || typeof selected !== 'string') return
                            updateNativeTools({ workingDirectory: selected })
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
                            updateNativeTools({ workingDirectory: defaultPath })
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
                      onChange={(streamEnabled) => updateChat({ streamEnabled })}
                    />
                  </SettingRow>
                  <SettingRow label={t.chatThinkingEnabled} description={t.chatThinkingHint}>
                    <Toggle
                      checked={chatConfig.thinkingEnabled !== false}
                      onChange={(thinkingEnabled) => updateChat({ thinkingEnabled })}
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
                          onChange={(maxOutputTokens) => updateChat({ maxOutputTokens: Number(maxOutputTokens) })}
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
                      onChange={(defaultLanguage) => updateChat({ defaultLanguage })}
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
                          setChatSystemPromptInteracted(false)
                          updateChat({ systemPrompt: '' })
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
                        setChatSystemPromptInteracted(true)
                        updateChat({ systemPrompt })
                      }}
                      rows={4}
                    />
                  </FieldBlock>
                </SettingsGroup>

                <SettingsGroup title={t.chatToolsSection}>
                  <div className="flex flex-wrap gap-2 pb-2">
                    <Button
                      size="sm"
                      onClick={() => setActiveTab('memory')}
                      data-tauri-drag-region="false"
                    >
                      <MemoryIcon size={11} />
                      {t.tabMemory}
                    </Button>
                    <Button
                      size="sm"
                      onClick={() => setActiveTab('externalAgents')}
                      data-tauri-drag-region="false"
                    >
                      <AgentIcon size={11} />
                      {t.chatOpenExternalAgents}
                    </Button>
                    <Button
                      size="sm"
                      onClick={() => setActiveTab('providers')}
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
                      {chatTools.enabled
                        ? (lang === 'zh' ? '已启用' : 'On')
                        : (lang === 'zh' ? '未启用' : 'Off')}
                    </span>
                  </SettingRow>
                  <SettingRow
                    label={lang === 'zh' ? 'Skill 运行时' : 'Skill runtime'}
                  >
                    <span className={`kv-tag ${skillRuntimeEnabled ? 'ok' : ''}`}>
                      {skillRuntimeEnabled
                        ? (lang === 'zh' ? '已启用' : 'On')
                        : (lang === 'zh' ? '未启用' : 'Off')}
                    </span>
                  </SettingRow>
                  <SettingRow
                    label={lang === 'zh' ? '内置工具' : 'Native tools'}
                  >
                    <span className={`kv-tag ${nativeBuiltinToolsEnabled ? 'ok' : ''}`}>
                      {nativeBuiltinToolsEnabled
                        ? (lang === 'zh' ? '已启用' : 'On')
                        : (lang === 'zh' ? '未启用' : 'Off')}
                    </span>
                  </SettingRow>
                  <SettingRow
                    label={t.tabMemory}
                  >
                    <span className={`kv-tag ${chatMemory.enabled ? 'ok' : ''}`}>
                      {chatMemory.enabled
                        ? (lang === 'zh' ? '已启用' : 'On')
                        : (lang === 'zh' ? '未启用' : 'Off')}
                    </span>
                  </SettingRow>
                  <SettingRow
                    label={lang === 'zh' ? '联网搜索' : 'Web search'}
                  >
                    <span className={`kv-tag ${(settings.lens?.webSearch?.enabled || chatTools.nativeTools?.webSearch) ? 'ok' : ''}`}>
                      {(settings.lens?.webSearch?.enabled || chatTools.nativeTools?.webSearch)
                        ? (lang === 'zh' ? '部分启用' : 'Partially on')
                        : (lang === 'zh' ? '未启用' : 'Off')}
                    </span>
                  </SettingRow>
                </SettingsGroup>
              </>
            )}

            {/* ===== 记忆标签页 ===== */}
            {activeTab === 'memory' && (
              <>
                <SettingsGroup title={lang === 'zh' ? '记忆运行' : 'Memory runtime'}>
                  <SettingRow
                    label={lang === 'zh' ? '启用记忆' : 'Enable memory'}
                    description={lang === 'zh'
                      ? '开启后注入 L1，并暴露 memory 工具。'
                      : 'Injects L1 and exposes memory tools.'}
                  >
                    <Toggle
                      checked={chatMemory.enabled}
                      onChange={(enabled) => updateChatMemory({ enabled })}
                    />
                  </SettingRow>
                  <SettingRow label={lang === 'zh' ? '记忆文件夹' : 'Memory folder'} stack>
                    <div className="flex min-w-0 flex-wrap items-center gap-2">
                      <Button
                        size="sm"
                        onClick={() => void refreshChatMemory()}
                        disabled={memoryLoading}
                        data-tauri-drag-region="false"
                      >
                        <RefreshCw size={10} className={memoryLoading ? 'animate-spin' : ''} />
                        {lang === 'zh' ? '刷新' : 'Refresh'}
                      </Button>
                      <Button
                        size="sm"
                        onClick={() => void handleOpenMemoryFolder()}
                        data-tauri-drag-region="false"
                      >
                        <FolderOpen size={11} />
                        {lang === 'zh' ? '打开文件夹' : 'Open folder'}
                      </Button>
                      {memoryDir && <span className="kv-row-desc min-w-0 break-all">{memoryDir}</span>}
                    </div>
                  </SettingRow>
                  {memoryError && <div className="kv-inline-error">{memoryError}</div>}
                  {memorySuccess && (
                    <div className="kv-panel info">
                      <div className="kv-panel-body">{memorySuccess}</div>
                    </div>
                  )}
                </SettingsGroup>

                <SettingsGroup title="L1">
                  <MemoryEditor
                    layer="l1"
                    title={lang === 'zh' ? 'L1 在线记忆' : 'L1 Online Memory'}
                    description={lang === 'zh'
                      ? '每次回答都会参考的偏好与约束。'
                      : 'Preferences and constraints applied to every reply.'}
                    value={memoryDrafts.l1}
                    savedValue={memorySnapshots.l1}
                    maxBytes={MEMORY_L1_MAX_BYTES}
                    rows={9}
                    loading={memoryLoading}
                    saving={memorySavingLayer === 'l1'}
                    lang={lang}
                    onChange={(value) => {
                      setMemoryDrafts((prev) => ({ ...prev, l1: value }))
                      setMemorySuccess('')
                    }}
                    onSave={() => void handleSaveMemoryLayer('l1')}
                    onReload={() => void refreshChatMemory()}
                  />
                </SettingsGroup>

                <SettingsGroup title="L2">
                  <MemoryEditor
                    layer="l2"
                    title={lang === 'zh' ? 'L2 长期记忆' : 'L2 Long-Term Memory'}
                    description={lang === 'zh'
                      ? '长期记录，按需通过 memory 工具读取。'
                      : 'Long-term notes, read on demand via memory tools.'}
                    value={memoryDrafts.l2}
                    savedValue={memorySnapshots.l2}
                    rows={13}
                    loading={memoryLoading}
                    saving={memorySavingLayer === 'l2'}
                    lang={lang}
                    onChange={(value) => {
                      setMemoryDrafts((prev) => ({ ...prev, l2: value }))
                      setMemorySuccess('')
                    }}
                    onSave={() => void handleSaveMemoryLayer('l2')}
                    onReload={() => void refreshChatMemory()}
                  />
                </SettingsGroup>
              </>
            )}

            {/* ===== 混音器标签页 ===== */}
            {activeTab === 'mixer' && (
              <MixerTab
                settings={settings}
                t={t}
                lang={lang}
                chatTools={chatTools}
                hasChatProvider={Boolean(chatProvider)}
                onUpdateDefaultModel={updateDefaultModel}
                onUpdateChatTools={updateChatTools}
              />
            )}

            {activeTab === 'externalAgents' && (
              <ExternalAgentsSettings
                lang={lang}
              />
            )}

            {/* ===== 连接器标签页 ===== */}
            {activeTab === 'connectors' && (
              <ConnectorsPanel
                servers={chatTools.servers}
                updateChatTools={updateChatTools}
                obsidianVaultPath={settings?.obsidianVaultPath ?? ''}
                onObsidianVaultPathChange={(path) => updateSettings({ obsidianVaultPath: path })}
                emailAccounts={settings?.emailAccounts ?? []}
                onEmailAccountsChange={(accounts) => updateSettings({ emailAccounts: accounts })}
                lang={lang}
                testServer={async (server) => {
                  try {
                    const result = await api.chatMcpTestServer(server, settings?.chatTools?.toolTimeoutMs)
                    return {
                      ok: result.success,
                      message: result.error || '',
                      tools: result.tools,
                    }
                  } catch {
                    return null
                  }
                }}
              />
            )}

            {/* ===== 网络搜索标签页 ===== */}
            {activeTab === 'webSearch' && (
              <WebSearchPanel
                t={t}
                lang={lang}
                webSearch={settings.lens?.webSearch}
                onChange={updateLensWebSearch}
              />
            )}

            {/* ===== 用量统计标签页（内含请求调试二级视图） ===== */}
            {activeTab === 'usage' && (
              <div className="space-y-3">
                <div className="kv-seg w-fit">
                  <button
                    type="button"
                    className={usageView === 'stats' ? 'active' : ''}
                    onClick={() => setUsageView('stats')}
                    data-tauri-drag-region="false"
                  >
                    {lang === 'zh' ? '用量统计' : 'Usage'}
                  </button>
                  <button
                    type="button"
                    className={usageView === 'debug' ? 'active' : ''}
                    onClick={() => setUsageView('debug')}
                    data-tauri-drag-region="false"
                  >
                    {lang === 'zh' ? '请求调试' : 'Request debug'}
                  </button>
                </div>
                {usageView === 'stats' ? (
                  <UsageStatsPanel lang={lang} />
                ) : (
                  <RequestDebugPanel
                    lang={lang}
                    enabled={chatTools.requestDebugEnabled ?? false}
                    onToggleEnabled={(v) => updateChatTools({ requestDebugEnabled: v })}
                  />
                )}
              </div>
            )}

            {/* ===== 模型管理标签页 ===== */}
            {activeTab === 'providers' && (
              <div className="kv-providers-root">
                <div className="kv-providers">
                <div className="kv-provider-list">
                  <button
                    type="button"
                    onClick={addProvider}
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
                    onSelect={setSelectedProviderId}
                    onReorder={reorderProviders}
                    trailing={PROVIDER_PRESETS
                      .filter((preset) => !settings.providers.some((p) => p.baseUrl === preset.baseUrl))
                      .map((preset) => (
                        <button
                          key={preset.name}
                          type="button"
                          onClick={() => addProviderFromPreset(preset)}
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

                <div className="kv-provider-detail">
                  <SettingsGroup title={lang === 'zh' ? '供应商' : 'Provider'} className="!pt-0 kv-provider-section">
                    {selectedProvider ? (() => {
                      const provider = selectedProvider
                      const configured = provider.apiKeys.some((key) => key.trim())
                      return (
                        <div className="kv-provider-header">
                          <div className="kv-provider-header-toolbar">
                            <span className="kv-row-label">{lang === 'zh' ? '启用供应商' : 'Enable provider'}</span>
                            <Toggle
                              checked={isProviderEnabled(provider)}
                              onChange={(enabled) => updateProvider(provider.id, { enabled })}
                            />
                          </div>
                          <div className="kv-provider-header-toolbar">
                            <span className="kv-row-label">{t.providerName}</span>
                            <div className="kv-provider-header-actions">
                              <span className={`kv-tag ${!isProviderEnabled(provider) ? 'warn' : configured ? 'ok' : 'warn'}`}>
                                {!isProviderEnabled(provider)
                                  ? (lang === 'zh' ? '已禁用' : 'Disabled')
                                  : configured ? t.connectionOk : t.permissionMissing}
                              </span>
                              <IconButton
                                variant="danger"
                                size="xs"
                                onClick={() => setConfirmDeleteProviderId(provider.id)}
                                data-tauri-drag-region="false"
                                title={t.deleteProvider}
                                label={t.deleteProvider}
                              >
                                <Trash2 size={12} />
                              </IconButton>
                            </div>
                          </div>
                          <Input
                            value={provider.name}
                            onChange={(v) => updateProvider(provider.id, { name: v })}
                            placeholder="Provider name"
                          />
                        </div>
                      )
                    })() : (
                      <p className="kv-provider-empty-hint">
                        {lang === 'zh' ? '在左侧选择供应商，或点上方「添加」新建。' : 'Select a provider on the left, or click “Add” above.'}
                      </p>
                    )}
                  </SettingsGroup>

                  {selectedProvider ? (() => {
                    const provider = selectedProvider
                    return (
                        <SettingsGroup title={lang === 'zh' ? '配置' : 'Configuration'}>
                          <FieldBlock label={t.baseUrl}>
                            <div className="kv-provider-endpoint-row">
                              <Input
                                className="min-w-0 flex-1"
                                value={provider.baseUrl}
                                onChange={(v) => updateProvider(provider.id, { baseUrl: v })}
                                placeholder="https://api.openai.com/v1"
                                mono
                              />
                              <Select
                                className="w-[11.5rem] shrink-0"
                                value={normalizeProviderApiFormat(provider.apiFormat)}
                                onChange={(apiFormat) => updateProvider(provider.id, { apiFormat })}
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
                                    onClick={() => setGzipInfoOpen((prev) => {
                                      const next = new Set(prev)
                                      if (next.has(provider.id)) next.delete(provider.id)
                                      else next.add(provider.id)
                                      return next
                                    })}
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
                              onChange={(v) => updateProvider(provider.id, { compressRequestBody: v })}
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
                                        updateProvider(provider.id, { apiKeys: base })
                                      }}
                                      placeholder={idx === 0 ? `sk-... (${t.apiKeyPrimary})` : `sk-... (${t.apiKeyBackup})`}
                                    />
                                    <IconButton
                                      size="xs"
                                      onClick={() => toggleKeyReveal(keyId)}
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
                                          updateProvider(provider.id, { apiKeys: next })
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
                                updateProvider(provider.id, { apiKeys: [...base, ''] })
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
                                onClick={() => openModelPicker(provider.id)}
                                data-tauri-drag-region="false"
                              >
                                <RefreshCw size={10} className={fetchingProviderId === provider.id ? 'animate-spin' : ''} />
                                {provider.availableModels.length > 0
                                  ? (lang === 'zh' ? '管理模型' : 'Models')
                                  : t.fetchModels}
                              </Button>
                              <Button
                                size="sm"
                                onClick={() => setModelTestProviderId(provider.id)}
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
                                  <li key={model} className="kv-enabled-model-row" onClick={() => setDrawerModel({ providerId: provider.id, model })}>
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
                                      onClick={(e) => { e.stopPropagation(); removeEnabledModel(provider.id, model) }}
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
                  })() : null}
                </div>
                </div>
              </div>
            )}

            {/* ===== 关于标签页 ===== */}
            {activeTab === 'about' && (
              <>
                <SettingsGroup title={lang === 'zh' ? '应用' : 'Application'}>
                  <div className="kv-panel mb-2">
                    <div className="flex items-center gap-3">
                      <div className="w-12 h-12 rounded-[10px] overflow-hidden shrink-0">
                        <img src="/icon.png" alt="Kivio Desktop" className="w-full h-full object-contain" />
                      </div>
                      <div className="min-w-0">
                        <div className="kv-page-title">Kivio Desktop</div>
                        <div className="kv-panel-body">{lang === 'zh' ? '屏幕级 AI 助手' : 'Screen-level AI Assistant'}</div>
                      </div>
                    </div>
                  </div>
                  <SettingRow label={t.currentVersion}>
                    <span className="kv-tag">v{appVersion}</span>
                  </SettingRow>
                  <SettingRow label={lang === 'zh' ? '开发者' : 'Developer'}>
                    <span className="kv-row-desc">ZM</span>
                  </SettingRow>
                </SettingsGroup>

                <SettingsGroup title={t.checkUpdate}>
                  <SettingRow label={t.autoCheckUpdate}>
                    <Toggle
                      checked={settings?.autoCheckUpdate ?? true}
                      onChange={(v) => updateSettings({ autoCheckUpdate: v })}
                    />
                  </SettingRow>
                  <SettingRow
                    label={t.checkUpdate}
                    description={updateStatus === 'up-to-date' ? t.upToDate : undefined}
                  >
                    <Button
                      size="sm"
                      onClick={handleCheckUpdate}
                      disabled={updateStatus === 'checking'}
                      data-tauri-drag-region="false"
                    >
                      <RefreshCw size={11} className={updateStatus === 'checking' ? 'animate-spin' : ''} />
                      {updateStatus === 'checking' ? t.checkingUpdate : t.checkUpdate}
                    </Button>
                  </SettingRow>

                  {updateStatus === 'check-failed' && (
                    <div className="kv-panel mt-2">
                      <div className="kv-panel-body mb-2 text-amber-700 dark:text-amber-400">
                        {t.updateCheckFailed}
                      </div>
                      <Button
                        size="sm"
                        onClick={handleOpenGithubReleases}
                        data-tauri-drag-region="false"
                      >
                        {t.downloadFromGithub}
                      </Button>
                    </div>
                  )}

                  {updateStatus === 'available' && updateInfo && (
                    <div className="kv-panel info mt-2">
                      <div className="kv-panel-title">
                        {t.updateAvailable}
                        <span className="kv-tag accent ml-auto">v{updateInfo.version}</span>
                      </div>
                      {updateInfo.body && (
                        <div className="custom-scrollbar mb-3 max-h-40 overflow-y-auto text-[12px] leading-relaxed">
                          <ChatMarkdown content={updateInfo.body} />
                        </div>
                      )}

                      {downloadState === 'downloading' && (
                        <div className="mb-3">
                          <div className="flex items-center justify-between kv-panel-body mb-1">
                            <span>{t.downloading}</span>
                            <span className="font-mono tabular-nums">{downloadPercent}%</span>
                          </div>
                          <div className="kv-progress">
                            <div style={{ width: `${downloadPercent}%` }} />
                          </div>
                        </div>
                      )}

                      {downloadState === 'failed' && downloadError && (
                        <div className="kv-inline-error mb-3">
                          {t.downloadFailed}: {downloadError}
                        </div>
                      )}

                      <div className="flex gap-2 flex-wrap">
                        {downloadState === 'idle' && (
                          <>
                            <Button
                              variant="primary"
                              onClick={handleDownloadAndInstall}
                              data-tauri-drag-region="false"
                            >
                              <Download size={12} />
                              {t.downloadAndInstall}
                            </Button>
                            <Button
                              onClick={handleOpenReleasePage}
                              data-tauri-drag-region="false"
                            >
                              <ExternalLink size={12} />
                              {t.downloadFromGithub}
                            </Button>
                          </>
                        )}
                        {downloadState === 'downloading' && (
                          <Button disabled>
                            <RefreshCw size={12} className="animate-spin" />
                            {t.downloading}
                          </Button>
                        )}
                        {downloadState === 'downloaded' && (
                          <Button
                            variant="primary"
                            onClick={handleInstall}
                            data-tauri-drag-region="false"
                          >
                            <Download size={12} />
                            {t.installAndRestart}
                          </Button>
                        )}
                        {downloadState === 'failed' && (
                          <>
                            <Button
                              variant="primary"
                              onClick={handleDownloadAndInstall}
                              data-tauri-drag-region="false"
                            >
                              <RefreshCw size={12} />
                              {t.retryDownload}
                            </Button>
                            <Button
                              onClick={handleOpenReleasePage}
                              data-tauri-drag-region="false"
                            >
                              <ExternalLink size={12} />
                              {t.downloadFromGithub}
                            </Button>
                          </>
                        )}
                        <Button
                          variant="ghost"
                          onClick={() => {
                            setUpdateStatus('idle')
                            setDownloadState('idle')
                            setDownloadPercent(0)
                            setDownloadError('')
                          }}
                          data-tauri-drag-region="false"
                        >
                          {t.updateLater}
                        </Button>
                      </div>
                    </div>
                  )}
                </SettingsGroup>
              </>
            )}
          </div>

          <div className={`kv-savebar ${variant === 'embedded' ? 'settings-embedded-savebar' : ''}`}>
            <div className={`kv-savebar-hint ${saveError ? 'error' : saveWarning ? 'warn' : hasUnsavedChanges ? 'dirty' : ''}`}>
              {saveError ? (
                <>
                  <span className="dot" />
                  <span title={saveError}>{saveError}</span>
                </>
              ) : saveWarning ? (
                <>
                  <span className="dot" />
                  <span title={saveWarning}>{saveWarning}</span>
                </>
              ) : saveSuccess ? (
                <>
                  <span className="clean-icon"><Check size={13} strokeWidth={2.4} /></span>
                  <span>{t.saved}</span>
                </>
              ) : hasUnsavedChanges ? (
                <>
                  <span className="dot" />
                  <span>{lang === 'zh' ? '有未保存更改。' : 'You have unsaved changes.'}</span>
                </>
              ) : (
                <>
                  <span className="clean-icon"><Check size={13} strokeWidth={2.4} /></span>
                  <span>{lang === 'zh' ? '所有更改已保存。' : 'All changes saved.'}</span>
                </>
              )}
            </div>
            <Button
              onClick={handleCloseRequest}
              data-tauri-drag-region="false"
            >
              {t.cancel}
            </Button>
            <Button
              variant="primary"
              onClick={handleSave}
              disabled={saving || !hasUnsavedChanges}
              data-tauri-drag-region="false"
            >
              {saving ? t.saving : t.save}
            </Button>
          </div>
        </main>
  )

  const modelPickerProvider =
    modelPickerProviderId && settings
      ? settings.providers.find((p) => p.id === modelPickerProviderId)
      : undefined

  const settingsModals = (
    <>
      {modelPickerProvider && (
        <ProviderModelsPicker
          provider={modelPickerProvider}
          lang={lang}
          labels={{
            title: lang === 'zh' ? '模型' : 'Models',
            searchPlaceholder: lang === 'zh' ? '搜索模型 ID 或名称' : 'Search model ID or name',
            fetchModels: t.fetchModels,
            fetching: t.fetching,
            addModel: t.addModel,
            manualAddModel: t.manualAddModel,
            noModels: lang === 'zh' ? '尚未获取模型，请点击上方按钮拉取。' : 'No models yet. Click the button above to fetch.',
            noSearchResults: lang === 'zh' ? '没有匹配的模型' : 'No matching models',
            enabled: lang === 'zh' ? '已启用' : 'On',
            addAllModels: lang === 'zh' ? '添加当前列表中的全部模型' : 'Add all models in the current list',
            close: lang === 'zh' ? '关闭' : 'Close',
          }}
          fetching={fetchingProviderId === modelPickerProvider.id}
          onClose={() => setModelPickerProviderId(null)}
          onFetch={() => void fetchModels(modelPickerProvider.id)}
          onAdd={(model) => addEnabledModel(modelPickerProvider.id, model)}
          onAddAll={(models) => addAllEnabledModels(modelPickerProvider.id, models)}
          onRemove={(model) => removeEnabledModel(modelPickerProvider.id, model)}
        />
      )}
      {/* 模型详情抽屉 */}
      {drawerModel && settings && (
        <ModelDetailDrawer
          modelName={drawerModel.model}
          overrides={settings.providers.find(p => p.id === drawerModel.providerId)?.modelOverrides}
          lang={lang}
          onClose={() => setDrawerModel(null)}
          onSave={(modelName, info) => {
            saveModelOverride(drawerModel.providerId, modelName, info)
            setDrawerModel(null)
          }}
          onReset={(modelName) => resetModelOverride(drawerModel.providerId, modelName)}
        />
      )}
      {modelTestProviderId && settings && (() => {
        const p = settings.providers.find(pv => pv.id === modelTestProviderId)
        if (!p) return null
        return (
          <ProviderModelTestModal
            providerId={p.id}
            baseUrl={p.baseUrl}
            apiKeys={p.apiKeys}
            apiFormat={p.apiFormat}
            models={p.enabledModels}
            lang={lang}
            onClose={() => setModelTestProviderId(null)}
          />
        )
      })()}
      {/* 未保存更改确认弹窗 */}
      {closeConfirmOpen && (
        <div className="kv-modal-backdrop" data-tauri-drag-region="false">
          <div className="kv-modal space-y-3">
            <h3 className="text-[14px] font-semibold">{t.unsavedChanges}</h3>
            <p className="kv-panel-body">{t.unsavedChangesDesc}</p>
            <div className="flex justify-end gap-2 pt-1">
              <Button
                variant="ghost"
                onClick={() => setCloseConfirmOpen(false)}
              >
                {t.continueEditing}
              </Button>
              <Button
                onClick={handleDiscardAndClose}
              >
                {t.discardAndClose}
              </Button>
              <Button
                variant="primary"
                onClick={handleSaveAndClose}
                disabled={saving}
                autoFocus
              >
                {saving ? t.saving : t.saveAndClose}
              </Button>
            </div>
          </div>
        </div>
      )}
      {/* 删除提供商确认弹窗 */}
      {confirmDeleteProviderId && (
        <div className="kv-modal-backdrop" data-tauri-drag-region="false">
          <div className="kv-modal space-y-3">
            <h3 className="text-[14px] font-semibold">{t.confirmDeleteProvider}</h3>
            <p className="kv-panel-body">{t.confirmDeleteProviderDesc}</p>
            <div className="flex justify-end gap-2 pt-1">
              <Button
                onClick={() => setConfirmDeleteProviderId(null)}
                data-tauri-drag-region="false"
              >
                {t.cancel}
              </Button>
              <Button
                variant="danger"
                onClick={() => {
                  if (confirmDeleteProviderId) deleteProvider(confirmDeleteProviderId)
                  setConfirmDeleteProviderId(null)
                }}
                data-tauri-drag-region="false"
              >
                {t.deleteProvider}
              </Button>
            </div>
          </div>
        </div>
      )}
    </>
  )

  const focusHandlers = {
    onPointerEnter: requestWindowFocus,
    onPointerMove: requestWindowFocus,
    onPointerDownCapture: requestWindowFocus,
  }

  if (variant === 'embedded') {
    return (
      <div
        className={`settings-embedded kv flex min-h-0 min-w-0 flex-1 ${
          reserveTrafficLightSpace ? 'settings-embedded--traffic-safe' : ''
        }`}
        data-theme-color={themeColor}
      >
        {!hideNav && (
          <aside className="settings-embedded-nav">
            <button
              type="button"
              onClick={handleCloseRequest}
              className="settings-embedded-back"
              title={lang === 'zh' ? '返回对话' : 'Back to chat'}
              data-tauri-drag-region="false"
            >
              <ArrowLeft size={16} strokeWidth={1.9} />
              <span>{lang === 'zh' ? '返回对话' : 'Back to chat'}</span>
            </button>
            <h2 className="settings-embedded-nav-title">{t.settings}</h2>
            {categoryNav}
          </aside>
        )}
        {settingsMain}
        {settingsModals}
      </div>
    )
  }

  return (
    <div className="kv kv-window" data-theme-color={themeColor} {...focusHandlers}>
      <div className="kv-titlebar" onMouseDown={handleSettingsDragMouseDown}>
        <div className="kv-titlebar-spacer" aria-hidden="true" />
        <div className="kv-title">{t.settings}</div>
        <button
          type="button"
          onClick={handleCloseRequest}
          className="kv-titlebar-close"
          data-tauri-drag-region="false"
          aria-label={t.cancel}
        >
          <X size={13} strokeWidth={2.2} />
        </button>
      </div>

      <div className="kv-body">
        <aside className="kv-sidebar">
          <div className="kv-sidebar-brand" onMouseDown={handleSettingsDragMouseDown}>
            <div className="kv-sidebar-brand-mark">
              <img src="/icon.png" alt="" aria-hidden="true" />
            </div>
            <div className="kv-sidebar-brand-name">Kivio Desktop</div>
            <div className="kv-sidebar-brand-ver">v{appVersion}</div>
          </div>
          {categoryNav}
        </aside>
        {settingsMain}
      </div>
      {settingsModals}
    </div>
  )
})
