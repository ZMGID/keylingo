// 知识库中心页（扩展 → 知识库）：与 Skill/MCP 中心页同级同语汇的独立页面，
// 取代原「设置 → 知识库」标签页。库列表卡片网格 + 库详情（导入/进度/重建）+ RAG 设置 tab。
// 库/文档逻辑移植自原 settings/KnowledgeBasePanel（上传、拖拽、进度、重命名等保持不变）。
import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  AlertCircle,
  ArrowLeft,
  CheckCircle2,
  FileText,
  Library,
  Link2,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
  Upload,
} from 'lucide-react'
import {
  isTauriRuntime,
  type DocumentProcessingConfig,
  type KnowledgeBaseConfig,
  type ModelProvider,
  type Settings,
} from '../api/tauri'
import { getSettingsCached, refreshSettings, saveSettingsCached } from '../api/settingsCache'
import { Button, IconButton } from '../components/Button'
import { Input, Select } from '../settings/components'
import { KnowledgeIcon } from '../settings/NavIcons'
import { resolveModelInfo } from '../data/modelMatching'
import { KnowledgeRagPanel } from '../settings/KnowledgeRagPanel'
import { RetrievalTestPanel } from './RetrievalTestPanel'
import {
  kbCreateLibrary,
  kbDeleteDocument,
  kbDeleteLibrary,
  kbImportUrl,
  kbListDocuments,
  kbListLibraries,
  kbRenameLibrary,
  kbReindexLibrary,
  kbSetEmbedBatchSize,
  kbUpdateEmbedding,
  kbUploadDocument,
  onKbIndex,
  type KnowledgeDocument,
  type KnowledgeLibrary,
} from './knowledgeBase'

const UPLOAD_EXTS = ['txt', 'text', 'log', 'csv', 'tsv', 'md', 'markdown', 'mdown', 'mkd', 'pdf', 'docx', 'xlsx', 'html', 'htm', 'png', 'jpg', 'jpeg', 'webp', 'bmp', 'tif', 'tiff', 'gif']

// embedding 请求批次：默认值 + 滑块范围，与后端 ingest::DEFAULT_EMBED_BATCH 及 1..=128 夹取一致。
const EMBED_BATCH_DEFAULT = 64
const EMBED_BATCH_MIN = 1
const EMBED_BATCH_MAX = 128

// Embedding 模型选择器：从 provider 的 enabledModels 取候选，并保留悬空绑定可见。
// （移植自 KnowledgeBasePanel，中文固定 —— 中心页无 lang prop，与其余中心页一致。）
function EmbeddingModelPicker({
  providers,
  providerId,
  model,
  onChange,
  showBadges = true,
}: {
  providers: ModelProvider[]
  providerId: string
  model: string
  onChange: (providerId: string, model: string) => void
  showBadges?: boolean
}) {
  const enabled = providers.filter((p) => p.enabled !== false)
  const selected = enabled.find((p) => p.id === providerId)
  const options = [
    { value: '', label: '选择提供商…' },
    ...enabled.map((p) => ({ value: p.id, label: p.name || p.id })),
  ]
  if (providerId && !options.some((o) => o.value === providerId)) {
    const known = providers.find((p) => p.id === providerId)
    const label = known
      ? `${known.name || known.id}${known.enabled === false ? '（已停用）' : ''}`
      : '⚠ 供应商已删除，请重新选择'
    options.unshift({ value: providerId, label })
  }

  // 只列具备 embedding 能力的模型（capabilities.embedding 或有 dimensions，与下方 isEmbedding 同源）；
  // 当前已选模型即使不符也由下方回退保留可见。
  const configuredModels = (selected?.enabledModels ?? []).filter((m) => {
    const mi = resolveModelInfo(m.trim(), selected?.modelOverrides)
    return Boolean(mi.capabilities?.embedding || mi.dimensions)
  })
  const modelOptions = [
    { value: '', label: '选择 embedding 模型…' },
    ...configuredModels.map((m) => ({ value: m, label: m })),
  ]
  if (model && !modelOptions.some((o) => o.value === model)) {
    modelOptions.push({ value: model, label: model })
  }

  const info = model.trim() ? resolveModelInfo(model.trim(), selected?.modelOverrides) : null
  const isEmbedding = Boolean(info?.capabilities?.embedding || info?.dimensions)
  const ctxLabel = (n?: number) => (!n ? null : n >= 1000 ? `${Math.round(n / 1000)}K` : `${n}`)
  return (
    <div className="space-y-1.5">
      <div className="flex flex-wrap items-center gap-2">
        <Select className="w-44" value={providerId} onChange={(pid) => onChange(pid, '')} options={options} />
        <Select className="w-64" value={model} onChange={(m) => onChange(providerId, m)} options={modelOptions} />
      </div>
      {showBadges && isEmbedding && (
        <div className="flex flex-wrap items-center gap-1">
          <span className="rounded-md border border-indigo-300 bg-indigo-50 px-1.5 py-0.5 text-[11px] font-medium text-indigo-600 dark:border-indigo-800 dark:bg-indigo-950/40 dark:text-indigo-300">
            嵌入
          </span>
          {info?.multilingual && <InfoPill>多语言</InfoPill>}
          {info?.dimensions ? <InfoPill>{info.dimensions} 维</InfoPill> : null}
          {ctxLabel(info?.contextWindow) ? <InfoPill>{ctxLabel(info?.contextWindow)}</InfoPill> : null}
          <InfoPill>RAG</InfoPill>
        </div>
      )}
    </div>
  )
}

function InfoPill({ children }: { children: React.ReactNode }) {
  return (
    <span className="rounded-md bg-neutral-100 px-1.5 py-0.5 text-[11px] text-neutral-600 dark:bg-neutral-800 dark:text-neutral-300">
      {children}
    </span>
  )
}

type Progress = { indexed: number; total: number }

function DocRow({
  doc,
  progress,
  onDelete,
}: {
  doc: KnowledgeDocument
  progress?: Progress
  onDelete: () => void
}) {
  const indexing = doc.status === 'indexing'
  // 有 total 才是「向量化」阶段（可确定进度）；total 未知＝还在解析/OCR（不确定进度）。
  const determinate = indexing && !!progress && progress.total > 0
  const pct = determinate ? Math.round((progress!.indexed / progress!.total) * 100) : 0
  return (
    <div className="px-3 py-2 text-sm">
      <div className="flex items-center gap-2">
        <FileText size={14} className="shrink-0 text-neutral-400" />
        <span className="flex-1 truncate" title={doc.name}>
          {doc.name}
        </span>
        {indexing && (
          <span className="flex items-center gap-1 text-xs text-indigo-500">
            <Loader2 size={12} className="animate-spin" />
            {determinate ? `${progress!.indexed}/${progress!.total}` : '处理中'}
          </span>
        )}
        {doc.status === 'ready' && (
          <span className="flex items-center gap-1 text-xs text-emerald-500">
            <CheckCircle2 size={12} /> {doc.chunkCount} 块
          </span>
        )}
        {doc.status === 'error' && (
          <span className="flex items-center gap-1 text-xs text-red-500" title={doc.error ?? ''}>
            <AlertCircle size={12} /> 失败
          </span>
        )}
        <IconButton size="sm" className="danger shrink-0" onClick={onDelete} label={`删除 ${doc.name}`} title="删除文档">
          <Trash2 size={13} />
        </IconButton>
      </div>
      {indexing && (
        <div className="mt-1.5 h-1 w-full overflow-hidden rounded-full bg-neutral-100 dark:bg-neutral-800">
          {determinate ? (
            <div
              className="h-full rounded-full bg-indigo-500 transition-[width] duration-[var(--kv-dur-normal)] ease-[var(--kv-ease-out)]"
              style={{ width: `${pct}%` }}
            />
          ) : (
            // 解析/OCR 阶段无逐步进度：用脉动条表示「进行中但无确定百分比」。
            <div className="h-full w-1/3 animate-pulse rounded-full bg-indigo-400/70" />
          )}
        </div>
      )}
    </div>
  )
}

/** 库卡片：与 Skill/MCP 卡片同语汇（rounded-xl、hover 上浮、stagger 入场）。 */
function LibraryCard({
  lib,
  index,
  onOpen,
}: {
  lib: KnowledgeLibrary
  index: number
  onOpen: () => void
}) {
  return (
    <button
      type="button"
      onClick={onOpen}
      data-tauri-drag-region="false"
      style={{ '--chat-motion-delay': `${Math.min(index, 8) * 24}ms` } as CSSProperties}
      className="chat-motion-fade-up group flex h-full min-w-0 cursor-pointer flex-col rounded-xl border border-neutral-200 bg-white p-3.5 text-left shadow-sm transition-[border-color,box-shadow,transform] duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-standard)] hover:-translate-y-0.5 hover:border-neutral-300 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-neutral-900/15 dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700 dark:focus-visible:ring-white/20"
    >
      <div className="flex items-start justify-between gap-2">
        <span className="grid size-10 shrink-0 place-items-center rounded-lg border border-neutral-200 bg-white text-neutral-600 dark:border-neutral-700 dark:bg-neutral-950 dark:text-neutral-300">
          <Library size={18} />
        </span>
        <span className="shrink-0 rounded-full bg-neutral-100 px-2 py-0.5 text-[10.5px] tabular-nums text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400">
          {lib.docCount} 文档
        </span>
      </div>
      <div className="mt-2.5 min-w-0 flex-1">
        <div className="truncate text-[13.5px] font-semibold leading-tight text-neutral-950 dark:text-neutral-50">
          {lib.name}
        </div>
        <p className="mt-1 truncate text-[12px] leading-[1.45] text-neutral-500 dark:text-neutral-400">
          {lib.embeddingModel || '未设置 embedding 模型'}
        </p>
      </div>
      <div className="mt-2.5 flex min-h-6 items-center gap-2 border-t border-neutral-100 pt-2 text-[11px] tabular-nums text-neutral-400 dark:border-neutral-800/70 dark:text-neutral-500">
        <span>{lib.chunkCount} 块</span>
        {lib.embeddingDim > 0 && <span>{lib.embeddingDim} 维</span>}
      </div>
    </button>
  )
}

export function KnowledgeCenter() {
  const [settings, setSettings] = useState<Settings | null>(null)
  const [view, setView] = useState<'libraries' | 'rag' | 'test'>('libraries')
  const [libraries, setLibraries] = useState<KnowledgeLibrary[]>([])
  // null = 库列表网格；有值 = 库详情
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [docs, setDocs] = useState<KnowledgeDocument[]>([])
  const [progress, setProgress] = useState<Record<string, Progress>>({})
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  // 建库表单
  const [newName, setNewName] = useState('')
  const [newProviderId, setNewProviderId] = useState('')
  const [newModel, setNewModel] = useState('')

  // 选中库的 embedding 编辑草稿（改完点「应用并重建」才生效）
  const [editProviderId, setEditProviderId] = useState('')
  const [editModel, setEditModel] = useState('')
  const [batchDraft, setBatchDraft] = useState(EMBED_BATCH_DEFAULT)

  const [urlInput, setUrlInput] = useState('')
  const [renaming, setRenaming] = useState(false)
  const [renameDraft, setRenameDraft] = useState('')
  const [dragActive, setDragActive] = useState(false)

  const providers = settings?.providers ?? []
  const selected = libraries.find((l) => l.id === selectedId) ?? null

  const loadSettings = useCallback(async () => {
    try {
      setSettings(await getSettingsCached())
    } catch (e) {
      setError(String(e))
    }
  }, [])

  // RAG 配置持久化：读 fresh 再合并保存（同 McpCenter，避免覆盖后端 OAuth 刷新等旁路写）。
  const persistSettingsPatch = useCallback((patch: Partial<Settings>) => {
    setSettings((prev) => (prev ? { ...prev, ...patch } : prev))
    void (async () => {
      try {
        const fresh = await refreshSettings()
        const saved = await saveSettingsCached({ ...fresh, ...patch })
        setSettings(saved)
      } catch (e) {
        setError(String(e))
      }
    })()
  }, [])

  const ragEnabled = settings?.chatTools?.nativeTools?.knowledgeSearch !== false
  const toggleRag = useCallback((v: boolean) => {
    void (async () => {
      try {
        const fresh = await refreshSettings()
        const chatTools = fresh.chatTools
        if (!chatTools) return
        const saved = await saveSettingsCached({
          ...fresh,
          chatTools: { ...chatTools, nativeTools: { ...chatTools.nativeTools, knowledgeSearch: v } },
        })
        setSettings(saved)
      } catch (e) {
        setError(String(e))
      }
    })()
  }, [])

  const refreshLibraries = useCallback(async () => {
    try {
      setLibraries(await kbListLibraries())
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  const refreshDocs = useCallback(async (kbId: string) => {
    try {
      setDocs(await kbListDocuments(kbId))
    } catch (e) {
      setError(String(e))
    }
  }, [])

  useEffect(() => {
    void loadSettings()
    void refreshLibraries()
  }, [loadSettings, refreshLibraries])

  useEffect(() => {
    if (selectedId) void refreshDocs(selectedId)
    else setDocs([])
  }, [selectedId, refreshDocs])

  // 选中库变化时，把 embedding 草稿重置为该库当前配置。
  useEffect(() => {
    setEditProviderId(selected?.embeddingProviderId ?? '')
    setEditModel(selected?.embeddingModel ?? '')
    setBatchDraft(selected?.embedBatchSize ? selected.embedBatchSize : EMBED_BATCH_DEFAULT)
    setRenaming(false)
  }, [selected?.id, selected?.embeddingProviderId, selected?.embeddingModel, selected?.embedBatchSize])

  const commitBatchSize = useCallback(
    async (kbId: string, size: number) => {
      try {
        await kbSetEmbedBatchSize(kbId, size)
        await refreshLibraries()
      } catch (e) {
        setError(String(e))
      }
    },
    [refreshLibraries],
  )

  // 实时索引进度：更新进度条；终态时刷新文档+库计数。
  const selectedRef = useRef<string | null>(null)
  selectedRef.current = selectedId
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined
    void onKbIndex((ev) => {
      setProgress((p) => ({ ...p, [ev.docId]: { indexed: ev.indexed, total: ev.total } }))
      if (ev.status !== 'indexing') {
        if (ev.kbId === selectedRef.current) void refreshDocs(ev.kbId)
        void refreshLibraries()
      }
    }).then((fn) => {
      if (cancelled) fn()
      else unlisten = fn
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [refreshDocs, refreshLibraries])

  const handleCreate = async () => {
    if (!newName.trim() || !newProviderId || !newModel) {
      setError('请填写名称并选择 embedding 模型')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const lib = await kbCreateLibrary(newName.trim(), newProviderId, newModel)
      setNewName('')
      setCreating(false)
      await refreshLibraries()
      setSelectedId(lib.id)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  // 导入一批文件路径：每个独立 try，一个失败不拖累其余，最后统一报告。
  const uploadPaths = useCallback(
    async (kbId: string, paths: string[]) => {
      if (paths.length === 0) return
      setBusy(true)
      setError(null)
      const failures: string[] = []
      for (const path of paths) {
        try {
          await kbUploadDocument(kbId, path)
        } catch (e) {
          const name = path.split(/[\\/]/).pop() || path
          failures.push(`${name}: ${e}`)
        }
      }
      await refreshDocs(kbId).catch(() => {})
      await refreshLibraries()
      setBusy(false)
      if (failures.length > 0) {
        setError(`${failures.length} 个文件导入失败：` + failures.join('; '))
      }
    },
    [refreshDocs, refreshLibraries],
  )

  const handleUpload = async () => {
    if (!selectedId) return
    let picked: string | string[] | null
    try {
      picked = await open({ multiple: true, filters: [{ name: 'Documents', extensions: UPLOAD_EXTS }] })
    } catch (e) {
      setError(String(e))
      return
    }
    if (!picked) return
    await uploadPaths(selectedId, Array.isArray(picked) ? picked : [picked])
  }

  // 拖拽导入：Tauri drag-drop 事件才带真实文件路径；窗口级事件，本页挂载期间全窗口接收
  // （不做坐标命中测试 —— macOS/Windows 坐标基准不一致，详见原 KnowledgeBasePanel 注释）。
  useEffect(() => {
    if (!isTauriRuntime()) return
    let cancelled = false
    let unlisten: (() => void) | undefined

    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (cancelled) return
        const p = event.payload
        if (p.type === 'enter' || p.type === 'over') {
          setDragActive(Boolean(selectedRef.current))
          return
        }
        if (p.type === 'leave') {
          setDragActive(false)
          return
        }
        if (p.type === 'drop') {
          setDragActive(false)
          const kbId = selectedRef.current
          if (!kbId) return
          const paths = p.paths.filter((path) => {
            const ext = path.split('.').pop()?.toLowerCase() ?? ''
            return UPLOAD_EXTS.includes(ext)
          })
          if (paths.length === 0) {
            setError('拖入的文件类型不受支持')
            return
          }
          void uploadPaths(kbId, paths)
        }
      })
      .then((fn) => {
        if (cancelled) fn()
        else unlisten = fn
      })
      .catch((err) => console.error('KB drag-drop listen failed:', err))

    return () => {
      cancelled = true
      setDragActive(false)
      unlisten?.()
    }
  }, [uploadPaths])

  const handleImportUrl = async () => {
    if (!selectedId) return
    const url = urlInput.trim()
    if (!url) return
    setBusy(true)
    setError(null)
    try {
      await kbImportUrl(selectedId, url)
      setUrlInput('')
      await refreshDocs(selectedId).catch(() => {})
      await refreshLibraries()
    } catch (e) {
      setError('网址导入失败：' + String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleDeleteLibrary = async (kbId: string) => {
    if (!confirm('删除该知识库及其所有文档？')) return
    try {
      await kbDeleteLibrary(kbId)
      if (selectedId === kbId) setSelectedId(null)
      await refreshLibraries()
    } catch (e) {
      setError(String(e))
    }
  }

  const handleDeleteDoc = async (docId: string) => {
    if (!selectedId) return
    try {
      await kbDeleteDocument(selectedId, docId)
      await refreshDocs(selectedId)
      await refreshLibraries()
    } catch (e) {
      setError(String(e))
    }
  }

  const handleRename = async (kbId: string, name: string) => {
    const trimmed = name.trim()
    const current = libraries.find((l) => l.id === kbId)?.name
    if (!trimmed || trimmed === current) {
      setRenaming(false)
      return
    }
    try {
      await kbRenameLibrary(kbId, trimmed)
      setRenaming(false)
      await refreshLibraries()
    } catch (e) {
      setError(String(e))
    }
  }

  const handleChangeEmbedding = async (providerId: string, model: string) => {
    if (!selected || !providerId || !model) return
    if (providerId === selected.embeddingProviderId && model === selected.embeddingModel) return
    if (!confirm('更换 embedding 模型会重建整个知识库索引（重新调用 embedding，可能耗时与产生费用）。继续？')) return
    setBusy(true)
    try {
      await kbUpdateEmbedding(selected.id, providerId, model)
      await refreshLibraries()
      await refreshDocs(selected.id)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleReindex = async () => {
    if (!selected) return
    if (!confirm('重建该知识库的全部索引？')) return
    setBusy(true)
    try {
      await kbReindexLibrary(selected.id)
      await refreshDocs(selected.id)
      await refreshLibraries()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="assistant-center-root flex h-full min-h-0 flex-col text-neutral-900 dark:text-neutral-100">
      <main className="custom-scrollbar min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[1040px] px-9 pb-10 pt-7">
          {/* 头部：标题 + 副标题 + 刷新 */}
          <div className="border-b border-neutral-200 pb-5 dark:border-neutral-800">
            <h1 className="flex items-center gap-2.5 text-[28px] font-semibold tracking-normal text-neutral-950 dark:text-neutral-50">
              <KnowledgeIcon size={24} className="text-neutral-500" />
              知识库
            </h1>
            <div className="mt-3.5 flex min-w-0 items-center gap-4">
              <p className="min-w-0 flex-1 text-[14px] leading-relaxed text-neutral-500 dark:text-neutral-400">
                导入文档构建本地 RAG，对话中由 agent 检索引用。数据不出本机。
              </p>
              <IconButton size="lg" label="刷新" onClick={() => void refreshLibraries()} data-tauri-drag-region="false">
                <RefreshCw size={17} />
              </IconButton>
            </div>
          </div>

          {/* Tab 行 */}
          <div className="mt-5 flex items-center gap-1 border-b border-neutral-200 dark:border-neutral-800">
            {([['libraries', '知识库'], ['test', '检索'], ['rag', 'RAG 设置']] as const).map(([id, label]) => (
              <button
                key={id}
                type="button"
                onClick={() => setView(id)}
                data-tauri-drag-region="false"
                className={`relative px-3 py-2 text-[13px] font-medium transition-colors ${
                  view === id
                    ? 'text-neutral-900 dark:text-neutral-100'
                    : 'text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200'
                }`}
              >
                {label}
                {id === 'libraries' && libraries.length > 0 && (
                  <span className="ml-1.5 text-[11px] tabular-nums text-neutral-400">{libraries.length}</span>
                )}
                {view === id && <span className="chat-motion-tab-underline absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-[#C56646] dark:bg-[#E39A78]" />}
              </button>
            ))}
          </div>

          {error && (
            <div className="mt-4 flex items-center gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-[12px] text-red-700 dark:border-red-900/60 dark:bg-red-950/30 dark:text-red-300">
              <AlertCircle size={14} className="shrink-0" />
              <span className="min-w-0 flex-1 break-words">{error}</span>
              <button type="button" onClick={() => setError(null)} className="shrink-0 text-xs underline" data-tauri-drag-region="false">
                关闭
              </button>
            </div>
          )}

          {view === 'rag' ? (
            /* RAG 设置：复用 settings 的合并面板。其子组件（kv-seg 等）依赖 .kv 作用域变量，
               包一层 .kv 提供变量；背景压透明与中心页底色无缝。 */
            <div key="rag" className="kv chat-motion-tab-in mt-5 !bg-transparent">
              <KnowledgeRagPanel
                providers={providers}
                lang="zh"
                docProcessing={settings?.documentProcessing}
                onChangeDocProcessing={(dp: DocumentProcessingConfig) => persistSettingsPatch({ documentProcessing: dp })}
                kbConfig={settings?.knowledgeBase}
                onChangeKbConfig={(kb: KnowledgeBaseConfig) => persistSettingsPatch({ knowledgeBase: kb })}
                ragEnabled={ragEnabled}
                onToggleRag={toggleRag}
              />
            </div>
          ) : view === 'test' ? (
            <RetrievalTestPanel libraries={libraries} />
          ) : selected ? (
            /* ===== 库详情 ===== */
            <div key={selected.id} className="chat-motion-tab-in mt-5 space-y-5">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <Button variant="ghost" size="sm" onClick={() => setSelectedId(null)} data-tauri-drag-region="false">
                    <ArrowLeft size={14} />
                    全部知识库
                  </Button>
                  {renaming ? (
                    <div className="mt-2 flex flex-wrap items-center gap-2">
                      <Input value={renameDraft} onChange={setRenameDraft} className="w-56" placeholder="知识库名称" />
                      <Button size="sm" disabled={busy || !renameDraft.trim()} onClick={() => void handleRename(selected.id, renameDraft)} data-tauri-drag-region="false">
                        保存
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => setRenaming(false)} data-tauri-drag-region="false">
                        取消
                      </Button>
                    </div>
                  ) : (
                    <h2 className="mt-2 truncate text-[20px] font-semibold text-neutral-950 dark:text-neutral-50">{selected.name}</h2>
                  )}
                  <p className="mt-1 text-[12.5px] text-neutral-500 dark:text-neutral-400">
                    {selected.docCount} 个文档 · {selected.chunkCount} 块
                    {selected.embeddingDim > 0 ? ` · ${selected.embeddingDim} 维` : ''}
                  </p>
                </div>
                {!renaming && (
                  <div className="flex shrink-0 items-center gap-1 pt-9">
                    <IconButton
                      size="sm"
                      label="重命名"
                      onClick={() => {
                        setRenaming(true)
                        setRenameDraft(selected.name)
                      }}
                      data-tauri-drag-region="false"
                    >
                      <Pencil size={14} />
                    </IconButton>
                    <IconButton size="sm" className="danger" label="删除库" onClick={() => void handleDeleteLibrary(selected.id)} data-tauri-drag-region="false">
                      <Trash2 size={14} />
                    </IconButton>
                  </div>
                )}
              </div>

              {/* Embedding 模型 */}
              <section className="rounded-xl border border-neutral-200 p-4 dark:border-neutral-800">
                <div className="mb-3 text-[13px] font-semibold text-neutral-800 dark:text-neutral-100">Embedding 模型</div>
                <div className="space-y-3">
                  <EmbeddingModelPicker
                    providers={providers}
                    providerId={editProviderId}
                    model={editModel}
                    onChange={(p, m) => {
                      setEditProviderId(p)
                      setEditModel(m)
                    }}
                  />
                  {(editProviderId !== selected.embeddingProviderId || editModel !== selected.embeddingModel) && (
                    <button
                      type="button"
                      disabled={busy || !editProviderId || !editModel}
                      onClick={() => void handleChangeEmbedding(editProviderId, editModel)}
                      className="inline-flex items-center gap-1 rounded-lg bg-amber-600 px-2.5 py-1.5 text-xs font-medium text-white transition-colors duration-[var(--kv-dur-fast)] hover:bg-amber-700 disabled:opacity-50"
                      data-tauri-drag-region="false"
                    >
                      应用并重建索引
                    </button>
                  )}

                  <div className="border-t border-neutral-100 pt-3 dark:border-neutral-800">
                    <div className="flex items-center justify-between gap-3">
                      <span className="text-[13px] text-neutral-700 dark:text-neutral-200">请求文档片段数量</span>
                      <span className="rounded-md border border-neutral-200 bg-white px-2 py-0.5 font-mono text-xs text-neutral-700 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
                        {batchDraft}
                      </span>
                    </div>
                    <input
                      type="range"
                      min={EMBED_BATCH_MIN}
                      max={EMBED_BATCH_MAX}
                      step={1}
                      value={batchDraft}
                      onChange={(e) => setBatchDraft(Number(e.target.value))}
                      onPointerUp={() => void commitBatchSize(selected.id, batchDraft)}
                      onBlur={() => void commitBatchSize(selected.id, batchDraft)}
                      className="mt-2 w-full"
                      style={{ accentColor: 'var(--accent)' }}
                      data-tauri-drag-region="false"
                    />
                    <div className="flex justify-between text-[11px] text-neutral-400">
                      <span>{EMBED_BATCH_MIN}</span>
                      <span>{EMBED_BATCH_MAX}</span>
                    </div>
                    <p className="mt-1.5 text-xs leading-relaxed text-neutral-500 dark:text-neutral-400">
                      每次向量化请求打包多少个文档片段（默认 {EMBED_BATCH_DEFAULT}）。若 embedding 服务报“批量过大/条数超限”可调小，只影响后续索引、无需重建。
                    </p>
                  </div>
                </div>
              </section>

              {/* 文档 */}
              <section className="rounded-xl border border-neutral-200 p-4 dark:border-neutral-800">
                <div className="mb-3 text-[13px] font-semibold text-neutral-800 dark:text-neutral-100">文档</div>
                <div className="space-y-3">
                  <div className="flex flex-wrap items-center gap-2">
                    <Button disabled={busy} onClick={() => void handleUpload()} data-tauri-drag-region="false">
                      <Upload size={14} /> 导入文档
                    </Button>
                    <Button variant="ghost" disabled={busy || docs.length === 0} onClick={() => void handleReindex()} data-tauri-drag-region="false">
                      <RefreshCw size={14} /> 重建索引
                    </Button>
                  </div>

                  <div className="flex flex-wrap items-center gap-2">
                    <div className="relative min-w-[220px] flex-1">
                      <Link2 size={14} className="pointer-events-none absolute left-3 top-1/2 z-10 -translate-y-1/2 text-neutral-400" />
                      <Input className="!pl-10" value={urlInput} onChange={setUrlInput} placeholder="粘贴网址导入（https://…）" mono />
                    </div>
                    <Button variant="ghost" disabled={busy || !urlInput.trim()} onClick={() => void handleImportUrl()} data-tauri-drag-region="false">
                      <Plus size={14} /> 导入网址
                    </Button>
                  </div>

                  {/* 大虚线框常驻：点击选文件 / 拖拽到此；拖拽悬停时高亮框本身。 */}
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void handleUpload()}
                    data-tauri-drag-region="false"
                    className={`flex w-full flex-col items-center gap-2 rounded-xl border border-dashed px-4 py-8 text-center transition-colors duration-[var(--kv-dur-fast)] disabled:opacity-50 ${
                      dragActive
                        ? 'border-indigo-400 bg-indigo-50/70 dark:border-indigo-500 dark:bg-indigo-950/40'
                        : 'border-neutral-300 bg-neutral-50/50 hover:border-indigo-300 hover:bg-indigo-50/40 dark:border-neutral-700 dark:bg-neutral-900/30 dark:hover:border-indigo-800 dark:hover:bg-indigo-950/20'
                    }`}
                  >
                    <Upload size={20} className={dragActive ? 'text-indigo-500' : 'text-neutral-400'} />
                    <span className={`text-sm font-medium ${dragActive ? 'text-indigo-600 dark:text-indigo-300' : 'text-neutral-600 dark:text-neutral-300'}`}>
                      {dragActive ? '松开以导入' : '点击导入文档'}
                    </span>
                    <span className="max-w-md text-xs leading-relaxed text-neutral-400">
                      点击或拖拽文件到此处；支持 txt / md / pdf / docx / xlsx / html、图片（需开启 OCR），或使用上方网址导入
                    </span>
                  </button>

                  {docs.length > 0 && (
                    <div className="divide-y divide-neutral-100 overflow-hidden rounded-lg border border-neutral-200 dark:divide-neutral-800 dark:border-neutral-700">
                      {docs.map((doc) => (
                        <DocRow key={doc.id} doc={doc} progress={progress[doc.id]} onDelete={() => void handleDeleteDoc(doc.id)} />
                      ))}
                    </div>
                  )}
                </div>
              </section>
            </div>
          ) : (
            /* ===== 库列表 ===== */
            <div key="libraries" className="chat-motion-tab-in mt-5 space-y-4">
              {creating && (
                <div className="chat-motion-search-reveal rounded-xl border border-neutral-200 p-4 dark:border-neutral-800">
                  <div className="mb-3 text-[13px] font-semibold text-neutral-800 dark:text-neutral-100">新建知识库</div>
                  <div className="flex flex-wrap items-center gap-2">
                    <Input value={newName} onChange={setNewName} placeholder="知识库名称" className="w-44" />
                    <EmbeddingModelPicker
                      providers={providers}
                      providerId={newProviderId}
                      model={newModel}
                      onChange={(p, m) => {
                        setNewProviderId(p)
                        setNewModel(m)
                      }}
                      showBadges={false}
                    />
                    <Button disabled={busy} onClick={() => void handleCreate()} data-tauri-drag-region="false">
                      <Plus size={14} /> 创建
                    </Button>
                    <Button variant="ghost" onClick={() => setCreating(false)} data-tauri-drag-region="false">
                      取消
                    </Button>
                  </div>
                  <p className="mt-2 text-xs text-neutral-500 dark:text-neutral-400">
                    embedding 模型决定向量维度，建库后更换需重建索引。需选用支持 /embeddings 接口的模型。
                  </p>
                </div>
              )}

              {loading ? (
                <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
                  {Array.from({ length: 3 }, (_, i) => (
                    <div key={i} className="rounded-xl border border-neutral-200/80 p-3.5 dark:border-neutral-800/70">
                      <div className="kv-skeleton size-10 rounded-lg" />
                      <div className="kv-skeleton mt-2.5 h-4 w-2/5 rounded" />
                      <div className="kv-skeleton mt-2 h-3 w-3/4 rounded" />
                    </div>
                  ))}
                </div>
              ) : libraries.length === 0 && !creating ? (
                <button
                  type="button"
                  onClick={() => setCreating(true)}
                  data-tauri-drag-region="false"
                  className="flex w-full flex-col items-center gap-3 rounded-xl border border-dashed border-neutral-200 px-6 py-14 text-center transition-colors duration-[var(--kv-dur-fast)] hover:border-neutral-300 hover:bg-neutral-50/60 dark:border-neutral-800 dark:hover:border-neutral-700 dark:hover:bg-neutral-900/30"
                >
                  <span className="grid size-12 place-items-center rounded-full bg-neutral-100 text-neutral-400 dark:bg-neutral-800 dark:text-neutral-500">
                    <Library size={22} />
                  </span>
                  <span className="text-[14px] font-medium text-neutral-700 dark:text-neutral-200">还没有知识库</span>
                  <span className="text-[12.5px] text-neutral-400">点击新建知识库，导入文档后即可在对话中检索引用</span>
                </button>
              ) : (
                <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
                  {libraries.map((lib, index) => (
                    <LibraryCard key={lib.id} lib={lib} index={index} onOpen={() => setSelectedId(lib.id)} />
                  ))}
                  {!creating && (
                    <button
                      type="button"
                      onClick={() => setCreating(true)}
                      data-tauri-drag-region="false"
                      style={{ '--chat-motion-delay': `${Math.min(libraries.length, 8) * 24}ms` } as CSSProperties}
                      className="chat-motion-fade-up flex min-h-[132px] flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-neutral-200 text-neutral-400 transition-colors duration-[var(--kv-dur-fast)] hover:border-neutral-300 hover:bg-neutral-50/60 hover:text-neutral-600 dark:border-neutral-800 dark:hover:border-neutral-700 dark:hover:bg-neutral-900/30 dark:hover:text-neutral-300"
                    >
                      <Plus size={20} />
                      <span className="text-[13px] font-medium">新建知识库</span>
                    </button>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      </main>
    </div>
  )
}
