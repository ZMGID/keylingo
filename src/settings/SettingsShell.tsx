import { forwardRef, useImperativeHandle, useState, useEffect, useCallback, useMemo, useRef, useReducer } from 'react'
import {
  X, RefreshCw,
  Download, Upload, ArrowLeft,
} from 'lucide-react'
import { open, save } from '@tauri-apps/plugin-dialog'
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
  AgentIcon, WebSearchIcon, ConnectorsIcon, PluginsIcon, UsageIcon, ProvidersIcon, AboutIcon, HooksIcon,
} from './NavIcons'
import { PluginCenter } from '../chat/PluginCenter'
import { buildHotkey, formatHotkeyError, getPlatform, isProviderEnabled, stableStringify } from './utils'
import { type ProviderPreset } from './providerPresets'
import { ProviderModelsPicker } from './ProviderModelsPicker'
import { ScreenshotTranslationSettings } from './ScreenshotTranslationSettings'
import { initialReplacePackProgressState, reduceReplacePackProgress } from './replacePackProgress'
import { UsageStatsPanel } from './UsageStatsPanel'
import { RequestDebugPanel } from './RequestDebugPanel'
import { ExternalAgentsSettings } from './ExternalAgentsSettings'
import { HotkeysTab } from './tabs/HotkeysTab'
import { LensTab } from './tabs/LensTab'
import { MixerTab } from './tabs/MixerTab'
import { TranslateTab } from './tabs/TranslateTab'
import { MemoryTab } from './tabs/MemoryTab'
import { ChatTab } from './tabs/ChatTab'
import { ProvidersTab } from './tabs/ProvidersTab'
import { HooksTab } from './tabs/HooksTab'
import { AppearanceGroup, BehaviorGroup, PermissionsGroup } from './tabs/GeneralTab'
import { AppInfoGroup, UpdateGroup } from './tabs/AboutTab'
import { MEMORY_L1_MAX_BYTES, utf8ByteLength, type MemoryLayerKey } from './memoryLayers'
import { ModelDetailDrawer } from '../components/ModelDetailDrawer'
import { ProviderModelTestModal } from '../components/ProviderModelTestModal'
import { Button } from '../components/Button'
import { resolveModelInfo } from '../data/modelMatching'
import { useWindowInteractionFocus } from '../utils/windowFocus'
import { hasEnabledNativeBuiltinTool, hasEnabledSkillRuntime } from '../utils/chatTools'
import { normalizeThemeColorId } from '../themeColors'
import { UI_FONT_PX_MIN, UI_FONT_PX_MAX } from './uiFont'
import {
  SettingRow,
  SettingsGroup, FieldBlock,
} from './components'
import { ConnectorsPanel } from './ConnectorsPanel'
import { WebSearchPanel } from './WebSearchPanel'
import { defaultChatTools } from './chatToolsShared'

export type SettingsTab = 'general' | 'hotkeys' | 'translate' | 'lens' | 'chat' | 'memory' | 'mixer' | 'externalAgents' | 'hooks' | 'webSearch' | 'connectors' | 'plugins' | 'usage' | 'providers' | 'about'

type SettingsData = SettingsType
// UI 字号：以 px 展示、以整体缩放（zoom）实现。CSS 全是 px 硬编码，做不了真正的 rem 基准字号，
// 故 14px 锚定为 100%，输入 px → scale = px/14。ponytail: zoom 代理，若将来全量 rem 化可换真基准。
const UI_FONT_BASE_PX = 14

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
  /** 插件页「让 AI 代装」：由 Chat 宿主开新对话并发送 install brief */
  onRequestPluginAiInstall?: (pluginId: string) => void | Promise<void>
}

export interface SettingsShellHandle {
  requestClose: () => void
}

/** 快捷键作用域。原本是组件体内的局部 type，抽 HotkeysTab 后需要跨模块共享，提到模块作用域。 */
export type HotkeyScopeKey =
  | 'main'
  | 'chat'
  | 'closeChat'
  | 'screenshotTranslation'
  | 'screenshotTranslationText'
  | 'screenshotTranslationReplace'
  | 'screenshotAnnotate'
  | 'lens'

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
    chatMode: {
      systemPrompt: '',
      webSearch: true,
      webFetch: true,
      knowledgeSearch: true,
      memoryTools: true,
      mcpReadOnly: true,
    },
  }
}

function defaultChatMemory(): ChatMemoryConfig {
  return {
    enabled: false,
    toolWriteConfirm: false,
  }
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
 * 设置面板主组件（standalone / embedded 双宿主）
 */
export const SettingsShell = forwardRef<SettingsShellHandle, SettingsShellProps>(function SettingsShell(
  { variant, onClose, onSettingsChange, onReady, reserveTrafficLightSpace = false, initialTab, hideNav = false, onRequestPluginAiInstall },
  ref,
) {
  const [settings, setSettings] = useState<SettingsData | null>(null)
  const [initialSettingsSnapshot, setInitialSettingsSnapshot] = useState('')
  const [loading, setLoading] = useState(true)
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
  const [confirmDeleteProviderId, setConfirmDeleteProviderId] = useState<string | null>(null)
  const [recordingTarget, setRecordingTarget] = useState<HotkeyScopeKey | null>(null)
  const [defaultPrompts, setDefaultPrompts] = useState<DefaultPromptTemplates | null>(null)
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
  // 下载后立刻静默安装:idle → downloading → (downloaded 一闪) → installer 拉起并退出
  // 安装失败才停在 downloaded / failed，让用户点「安装并重启」或重试。
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
  // （否则用户可能没察觉就自动保存把磁盘真实数据覆盖掉）
  const [loadError, setLoadError] = useState('')
  const [reloadKey, setReloadKey] = useState(0)
  const readyEmittedRef = useRef(false)
  // 镜像当前草稿的序列化快照，供后台 SWR 校准回调判断“用户是否已改动”而无需闭包捕获最新 state。
  const currentSettingsSnapshotRef = useRef('')
  // 自动保存：最新草稿 + 防抖定时器 + 在飞请求（避免并发写互相覆盖）
  const settingsRef = useRef<SettingsData | null>(null)
  const autosaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const saveInFlightRef = useRef(false)
  const saveAgainRef = useRef(false)
  const toastClearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const lang = settings?.settingsLanguage || 'zh'
  const t = i18n[lang]
  const themeColor = normalizeThemeColorId(settings?.themeColor)
  const chatTools = settings?.chatTools || defaultChatTools()
  const nativeBuiltinToolsEnabled = hasEnabledNativeBuiltinTool(chatTools.nativeTools)
  const skillRuntimeEnabled = hasEnabledSkillRuntime(chatTools.nativeTools)
  // 判断是否有未保存的更改（自动保存会在防抖后清掉）
  const hasUnsavedChanges = settings ? stableStringify(settings) !== initialSettingsSnapshot : false
  // 同步当前草稿快照到 ref（SWR 校准回调据此判断草稿是否 pristine）。
  useEffect(() => {
    const snapshot = settings ? stableStringify(settings) : ''
    currentSettingsSnapshotRef.current = snapshot
    settingsRef.current = settings
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
        scope: 'closeChat',
        hotkey: settings.closeChatHotkey || '',
        enabled: !!(settings.closeChatHotkey || '').trim(),
      },
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

  const SCOPE_I18N_KEY: Record<HotkeyScopeKey, 'hotkeyScopeTranslator' | 'hotkeyScopeChat' | 'hotkeyScopeCloseChat' | 'hotkeyScopeScreenshot' | 'hotkeyScopeScreenshotText' | 'hotkeyScopeScreenshotReplace' | 'annotateHotkeyLabel' | 'hotkeyScopeLens'> = {
    main: 'hotkeyScopeTranslator',
    chat: 'hotkeyScopeChat',
    closeChat: 'hotkeyScopeCloseChat',
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

  /** 下载安装包后立刻静默安装并退出。Windows 走 NSIS `/S /UPDATE /R`，装完会自己拉起。 */
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
      await api.installUpdate(path)
    } catch (err) {
      console.error('Download or install update failed:', err)
      setDownloadError(typeof err === 'string' ? err : (err instanceof Error ? err.message : String(err)))
      setDownloadState('failed')
    } finally {
      unlisten?.()
    }
  }, [updateInfo])

  /** 已下载的安装包再装一次（下载成功但拉起 installer 失败时用）。 */
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
   * 立即把当前草稿写盘。自动保存与关闭前 flush 共用。
   * 保存中若草稿又变了，收尾后会再跑一轮，避免丢字。
   */
  const persistSettingsNow = useCallback(async () => {
    const draft = settingsRef.current
    if (!draft) return false
    const draftSnapshot = stableStringify(draft)
    if (draftSnapshot === initialSettingsSnapshot) return true

    if (saveInFlightRef.current) {
      saveAgainRef.current = true
      return false
    }

    saveInFlightRef.current = true
    saveAgainRef.current = false
    try {
      setSaveError('')
      // 不主动清 saveWarning：热键警告来自独立事件，成功保存后可能紧接着到达
      const savedSettings = await saveSettingsCached(draft)
      const savedSnapshot = stableStringify(savedSettings)
      const latestSnapshot = settingsRef.current ? stableStringify(settingsRef.current) : ''
      // 用户在请求飞行中继续改了 → 只推进“已落盘基线”，别用服务端回包盖掉本地草稿
      if (latestSnapshot === draftSnapshot) {
        setSettings(savedSettings)
        setInitialSettingsSnapshot(savedSnapshot)
        settingsRef.current = savedSettings
        currentSettingsSnapshotRef.current = savedSnapshot
      } else {
        setInitialSettingsSnapshot(savedSnapshot)
      }
      onSettingsChange()
      return true
    } catch (err) {
      console.error('Failed to save settings:', err)
      const message = err instanceof Error ? err.message : String(err)
      const translated = formatHotkeyError(message, lang)
      const prefix = lang === 'zh' ? '保存失败:' : 'Save failed: '
      setSaveError(`${prefix}${translated.replace(/\n/g, ' / ')}`)
      return false
    } finally {
      saveInFlightRef.current = false
      if (saveAgainRef.current) {
        saveAgainRef.current = false
        void persistSettingsNow()
      }
    }
  }, [initialSettingsSnapshot, lang, onSettingsChange])

  // 草稿相对已落盘基线有 diff → 防抖自动保存（开关/输入共用，避免每个按键打盘）
  useEffect(() => {
    if (!hasUnsavedChanges) return
    // 自定义请求头的新行会先以空值占位。后端存盘归一化会丢掉这种不完整的行，如果此时发起自动保存，回包会把输入框立刻恢复成消失。等用户填完后再保存。
    const hasIncompleteCustomHeader = settings?.providers.some((provider) =>
      (provider.request?.customHeaders ?? []).some((header) =>
        header.key.trim() === '' || header.value.trim() === '',
      ),
    )
    if (hasIncompleteCustomHeader) return
    // 同理：本地 CLI 的「添加模型」也是先插一行空的再填。sanitize_external_cli_agents 会
    // retain 掉 id 为空的条目，回包盖回草稿 → 那一行刚出现就消失（表现为「点添加冒一下就没了」）。
    const hasIncompleteCliModel = Object.values(settings?.chat?.externalCliAgents ?? {}).some(
      (config) => (config.customModels ?? []).some((model) => model.id.trim() === ''),
    )
    if (hasIncompleteCliModel) return
    if (autosaveTimerRef.current) {
      clearTimeout(autosaveTimerRef.current)
      autosaveTimerRef.current = null
    }
    autosaveTimerRef.current = setTimeout(() => {
      autosaveTimerRef.current = null
      void persistSettingsNow()
    }, 400)
    return () => {
      if (autosaveTimerRef.current) {
        clearTimeout(autosaveTimerRef.current)
        autosaveTimerRef.current = null
      }
    }
  }, [hasUnsavedChanges, settings, persistSettingsNow])

  useEffect(() => {
    return () => {
      if (autosaveTimerRef.current) clearTimeout(autosaveTimerRef.current)
      if (toastClearTimerRef.current) clearTimeout(toastClearTimerRef.current)
    }
  }, [])

  // 错误 / 热键警告：短暂 toast，几秒后自动消失
  useEffect(() => {
    if (!saveError && !saveWarning) return
    if (toastClearTimerRef.current) clearTimeout(toastClearTimerRef.current)
    toastClearTimerRef.current = setTimeout(() => {
      setSaveError('')
      setSaveWarning('')
      toastClearTimerRef.current = null
    }, 5000)
    return () => {
      if (toastClearTimerRef.current) {
        clearTimeout(toastClearTimerRef.current)
        toastClearTimerRef.current = null
      }
    }
  }, [saveError, saveWarning])

  /**
   * 关闭设置页：先 flush 未落盘改动，再关（不阻塞 UI 等回包）
   */
  const handleCloseRequest = useCallback(() => {
    if (recordingTarget) return
    if (autosaveTimerRef.current) {
      clearTimeout(autosaveTimerRef.current)
      autosaveTimerRef.current = null
    }
    if (hasUnsavedChanges) {
      // 先 flush 再关闭：onClose 会切回聊天视图并重挂会话面板，若不等落盘完成，
      // 面板挂载时读到的还是保存回包前的旧缓存（新配的模型"消失"，要重开窗口才出现）。
      // flush 是本地 IPC 写盘（毫秒级），await 不产生可感知延迟；失败也照关（错误已
      // toast + console，与旧行为一致，不把用户困在设置页）。
      void persistSettingsNow().finally(() => onClose())
      return
    }
    onClose()
  }, [hasUnsavedChanges, onClose, persistSettingsNow, recordingTarget])

  useImperativeHandle(ref, () => ({ requestClose: handleCloseRequest }), [handleCloseRequest])

  const handleSettingsDragMouseDown = useCallback((event: React.MouseEvent<HTMLElement>) => {
    if (event.button !== 0) return
    const target = event.target as HTMLElement | null
    if (target?.closest('button, input, textarea, select, [data-tauri-drag-region="false"]')) return
    event.preventDefault()
    void api.startDragging().catch((err) => {
      console.error('[settings-drag] startDragging failed:', err)
    })
  }, [])

  // 全局键盘：Esc 关闭、Cmd/Ctrl+S 立即 flush；弹窗打开时优先处理弹窗内的 Esc
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

      if (e.key === 'Escape') {
        handleCloseRequest()
      } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's') {
        e.preventDefault()
        if (autosaveTimerRef.current) {
          clearTimeout(autosaveTimerRef.current)
          autosaveTimerRef.current = null
        }
        if (hasUnsavedChanges) void persistSettingsNow()
      }
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [
    handleCloseRequest,
    recordingTarget,
    confirmDeleteProviderId,
    modelPickerProviderId,
    hasUnsavedChanges,
    persistSettingsNow,
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
      setInitialSettingsSnapshot(stableStringify(imported))
      onSettingsChange()
      setBackupStatus({ kind: 'ok', msg: lang === 'zh' ? '设置已导入并生效。' : 'Settings imported and applied.' })
    } catch (err) {
      setBackupStatus({ kind: 'err', msg: `${lang === 'zh' ? '导入失败：' : 'Import failed: '}${err}` })
    }
  }, [lang, onSettingsChange])

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

  const setProviderIcon = useCallback((id: string, iconKey: string) => {
    setSettings((prev) => {
      if (!prev) return prev
      const next = { ...(prev.providerIcons ?? {}) }
      // 空串 = 恢复「自动匹配」，删掉这条而不是存空值。
      if (iconKey) next[id] = iconKey
      else delete next[id]
      return { ...prev, providerIcons: next }
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
      apiFormat: preset.apiFormat ?? 'openai_chat',
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
    const providerIcons = { ...(settings.providerIcons ?? {}) }
    delete providerIcons[id]
    const nextSettings: SettingsData = {
      ...settings,
      providers: nextProviders,
      providerIcons,
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
          apiFormat: currentProvider.apiFormat,
          // 草稿可能尚未落盘，这里必须带上编辑中的请求配置，
          // 否则拉列表用的头和真实聊天不一致。
          request: currentProvider.request,
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
  const toggleRecording = (target: HotkeyScopeKey) => {
    setRecordingTarget((current) => (current === target ? null : target))
  }

  // 当前语言对应的默认 lens 提示词
  const lensDefaults = defaultPrompts?.lensPrompts?.[settings?.lens?.defaultLanguage === 'en' ? 'en' : 'zh']
  const chatLangKey = settings?.chat?.defaultLanguage === 'en' ? 'en' : 'zh'
  const chatDefaults = defaultPrompts?.chatPrompts?.[chatLangKey]
  const chatRuntimeDefaults = defaultPrompts?.chatRuntimePrompt
  const chatConfig = settings?.chat || defaultChatConfig()
  const chatMemory = settings?.chatMemory || defaultChatMemory()
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
      } else if (recordingTarget === 'closeChat') {
        updateSettings({ closeChatHotkey: hotkey })
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
    { id: 'providers' as const, label: t.tabModels, icon: ProvidersIcon },
    { id: 'hotkeys' as const, label: t.tabHotkeys, icon: HotkeysIcon },
    { id: 'translate' as const, label: t.tabTranslation, icon: TranslateIcon },
    { id: 'lens' as const, label: t.lensTabLabel, icon: LensIcon },
    { id: 'chat' as const, label: t.tabChatClient, icon: ChatIcon },
    { id: 'memory' as const, label: t.tabMemory, icon: MemoryIcon },
    { id: 'mixer' as const, label: t.tabMixer, icon: MixerIcon },
    { id: 'externalAgents' as const, label: t.tabExternalAgents, icon: AgentIcon },
    { id: 'hooks' as const, label: t.tabHooks, icon: HooksIcon },
    { id: 'connectors' as const, label: t.tabConnectors, icon: ConnectorsIcon },
    { id: 'plugins' as const, label: t.tabPlugins, icon: PluginsIcon },
    { id: 'webSearch' as const, label: t.tabWebSearch, icon: WebSearchIcon },
    { id: 'usage' as const, label: lang === 'zh' ? '用量统计' : 'Usage', icon: UsageIcon },
    // 关于固定在分类列表最末
    { id: 'about' as const, label: lang === 'zh' ? '关于' : 'About', icon: AboutIcon },
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
        ? '检测外部 CLI 编码代理，管理版本、路径、模型与环境变量。'
        : 'Detect external CLI coding agents; manage versions, paths, models, and env vars.',
    },
    hooks: {
      title: t.tabHooks,
      subtitle: t.hooksPageSubtitle,
    },
    connectors: {
      title: t.tabConnectors,
      subtitle: lang === 'zh'
        ? '连接外部数据源；凭据存本机。'
        : 'Connect external data sources; credentials stay local.',
    },
    plugins: {
      title: t.tabPlugins,
      subtitle: lang === 'zh'
        ? '检测本机能力插件（OfficeCLI / Cua Driver / ego lite）；启用后自动注入 Skill / MCP。'
        : 'Detect local capability plugins (OfficeCLI / Cua Driver / ego lite); enable to inject Skills / MCP.',
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
      subtitle: lang === 'zh' ? '应用、版本和更新信息。' : 'Application, version, and update details.',
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
            onClick={handleCloseRequest}
            className="settings-embedded-back"
            title={lang === 'zh' ? '返回对话' : 'Back to chat'}
            data-tauri-drag-region="false"
          >
            <span className="settings-embedded-nav-icon">
              <ArrowLeft size={17} strokeWidth={1.75} />
            </span>
            <span>{lang === 'zh' ? '返回对话' : 'Back to chat'}</span>
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
                <AppearanceGroup
                  settings={settings}
                  t={t}
                  lang={lang}
                  themeColor={themeColor}
                  systemFonts={systemFonts}
                  uiFontPxInput={uiFontPxInput}
                  onUpdateSettings={updateSettings}
                  onUiFontPxInputChange={setUiFontPxInput}
                  onCommitUiFontPx={commitUiFontPx}
                />

                <BehaviorGroup
                  settings={settings}
                  t={t}
                  lang={lang}
                  retryAttemptsInput={retryAttemptsInput}
                  onUpdateSettings={updateSettings}
                  onRetryAttemptsChange={handleRetryAttemptsChange}
                  onRetryAttemptsBlur={handleRetryAttemptsBlur}
                />

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
                  <PermissionsGroup
                    t={t}
                    permissionStatus={permissionStatus}
                    permissionsLoading={permissionsLoading}
                    onOpenPermissionSettings={handleOpenPermissionSettings}
                    onRefreshPermissions={refreshPermissions}
                  />
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
                hotkeyConflicts={hotkeyConflicts}
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
              <ChatTab
                settings={settings}
                t={t}
                lang={lang}
                chatConfig={chatConfig}
                chatTools={chatTools}
                chatMemory={chatMemory}
                chatDefaults={chatDefaults}
                chatRuntimeDefaults={chatRuntimeDefaults}
                chatFallbackMaxOutputTokens={chatFallbackMaxOutputTokens}
                effectiveChatMaxOutput={effectiveChatMaxOutput}
                chatMaxOutputSourceLabel={chatMaxOutputSourceLabel}
                chatMaxOutputModelLabel={chatMaxOutputModelLabel}
                skillRuntimeEnabled={skillRuntimeEnabled}
                nativeBuiltinToolsEnabled={nativeBuiltinToolsEnabled}
                onUpdateChat={updateChat}
                onUpdateNativeTools={updateNativeTools}
                onNavigateTab={setActiveTab}
              />
            )}

            {/* ===== 记忆标签页 ===== */}
            {activeTab === 'memory' && (
              <MemoryTab
                lang={lang}
                chatMemory={chatMemory}
                memoryDir={memoryDir}
                memoryError={memoryError}
                memorySuccess={memorySuccess}
                memoryLoading={memoryLoading}
                memorySavingLayer={memorySavingLayer}
                memoryDrafts={memoryDrafts}
                memorySnapshots={memorySnapshots}
                onUpdateChatMemory={updateChatMemory}
                onRefresh={() => void refreshChatMemory()}
                onOpenFolder={() => void handleOpenMemoryFolder()}
                onDraftChange={(layer, value) => {
                  setMemoryDrafts((prev) => ({ ...prev, [layer]: value }))
                  setMemorySuccess('')
                }}
                onSaveLayer={(layer) => void handleSaveMemoryLayer(layer)}
              />
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
                settings={settings}
                updateChat={updateChat}
              />
            )}

            {/* ===== Hooks 标签页（对话生命周期） ===== */}
            {activeTab === 'hooks' && (
              <HooksTab
                lang={lang}
                hooks={chatTools.hooks ?? []}
                onChange={(hooks) => updateChatTools({ hooks })}
              />
            )}

            {/* ===== 连接器标签页 ===== */}
            {activeTab === 'connectors' && (
              <ConnectorsPanel
                servers={chatTools.servers}
                updateChatTools={updateChatTools}
                obsidianVaultPath={settings?.obsidianVaultPath ?? ''}
                onObsidianVaultPathChange={(path) => updateSettings({ obsidianVaultPath: path })}
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

            {/* ===== 插件标签页（原扩展 → 插件） ===== */}
            {activeTab === 'plugins' && (
              <PluginCenter
                onRequestAiInstall={onRequestPluginAiInstall}
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
              <ProvidersTab
                settings={settings}
                t={t}
                lang={lang}
                selectedProvider={selectedProvider}
                revealedKeys={revealedKeys}
                gzipInfoOpen={gzipInfoOpen}
                fetchingProviderId={fetchingProviderId}
                onSelectProvider={setSelectedProviderId}
                onReorderProviders={reorderProviders}
                onAddProvider={addProvider}
                onAddProviderFromPreset={addProviderFromPreset}
                onUpdateProvider={updateProvider}
                onSetProviderIcon={setProviderIcon}
                onRequestDeleteProvider={setConfirmDeleteProviderId}
                onToggleGzipInfo={(id) => setGzipInfoOpen((prev) => {
                  const next = new Set(prev)
                  if (next.has(id)) next.delete(id)
                  else next.add(id)
                  return next
                })}
                onToggleKeyReveal={toggleKeyReveal}
                onOpenModelPicker={openModelPicker}
                onOpenModelTest={setModelTestProviderId}
                onOpenModelDrawer={setDrawerModel}
                onRemoveEnabledModel={removeEnabledModel}
              />
            )}

            {/* ===== 关于标签页 ===== */}
            {activeTab === 'about' && (
              <>
                <AppInfoGroup t={t} lang={lang} appVersion={appVersion} />

                <UpdateGroup
                  settings={settings}
                  t={t}
                  update={{
                    status: updateStatus,
                    info: updateInfo,
                    downloadState,
                    downloadPercent,
                    downloadError,
                  }}
                  onUpdateSettings={updateSettings}
                  onCheck={handleCheckUpdate}
                  onDownloadAndInstall={handleDownloadAndInstall}
                  onInstall={handleInstall}
                  onOpenReleasePage={handleOpenReleasePage}
                  onOpenGithubReleases={handleOpenGithubReleases}
                  onDismiss={() => {
                    setUpdateStatus('idle')
                    setDownloadState('idle')
                    setDownloadPercent(0)
                    setDownloadError('')
                  }}
                />
              </>
            )}
          </div>

          {(saveError || saveWarning) && (
            <div
              className={`settings-autosave-toast ${saveError ? 'error' : 'warn'}`}
              role="status"
              title={saveError || saveWarning}
              data-tauri-drag-region="false"
            >
              {saveError || saveWarning}
            </div>
          )}
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
            request={p.request}
            models={p.enabledModels}
            lang={lang}
            onClose={() => setModelTestProviderId(null)}
          />
        )
      })()}
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
            <h2 className="settings-embedded-nav-title" onMouseDown={handleSettingsDragMouseDown}>
              {t.settings}
            </h2>
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
