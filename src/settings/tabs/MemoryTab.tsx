import { RefreshCw, FolderOpen } from 'lucide-react'
import { SettingRow, SettingsGroup, Toggle } from '../components'
import { Button } from '../../components/Button'
import {
  MEMORY_L1_MAX_BYTES,
  utf8ByteLength,
  type MemoryLayerKey,
} from '../memoryLayers'
import type { Lang } from '../i18n'
import type { ChatMemoryConfig } from '../../api/tauri'

/**
 * 单层记忆编辑器。原本在 SettingsShell 模块作用域，只被记忆标签页使用，
 * 随 MemoryTab 一起搬过来。
 */
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
    <div className="kv-panel mb-2">
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

interface MemoryTabProps {
  lang: Lang
  chatMemory: ChatMemoryConfig
  memoryDir: string
  memoryError: string
  memorySuccess: string
  memoryLoading: boolean
  memorySavingLayer: MemoryLayerKey | null
  memoryDrafts: Record<MemoryLayerKey, string>
  memorySnapshots: Record<MemoryLayerKey, string>
  onUpdateChatMemory: (updates: Partial<ChatMemoryConfig>) => void
  onRefresh: () => void
  onOpenFolder: () => void
  onDraftChange: (layer: MemoryLayerKey, value: string) => void
  onSaveLayer: (layer: MemoryLayerKey) => void
}

/** 记忆标签页。纯展示：加载 / 保存 / 草稿状态都留在 SettingsShell。 */
export function MemoryTab({
  lang,
  chatMemory,
  memoryDir,
  memoryError,
  memorySuccess,
  memoryLoading,
  memorySavingLayer,
  memoryDrafts,
  memorySnapshots,
  onUpdateChatMemory,
  onRefresh,
  onOpenFolder,
  onDraftChange,
  onSaveLayer,
}: MemoryTabProps) {
  return (
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
            onChange={(enabled) => onUpdateChatMemory({ enabled })}
          />
        </SettingRow>
        <SettingRow label={lang === 'zh' ? '记忆文件夹' : 'Memory folder'} stack>
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <Button
              size="sm"
              onClick={onRefresh}
              disabled={memoryLoading}
              data-tauri-drag-region="false"
            >
              <RefreshCw size={10} className={memoryLoading ? 'animate-spin' : ''} />
              {lang === 'zh' ? '刷新' : 'Refresh'}
            </Button>
            <Button
              size="sm"
              onClick={onOpenFolder}
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

      {/* L1 / L2 编辑器自带标题+描述+按钮，面板间用 mb-2 直接留间距（无额外容器层） */}
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
        onChange={(value) => onDraftChange('l1', value)}
        onSave={() => onSaveLayer('l1')}
        onReload={onRefresh}
      />

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
        onChange={(value) => onDraftChange('l2', value)}
        onSave={() => onSaveLayer('l2')}
        onReload={onRefresh}
      />
    </>
  )
}
