import { invoke } from '@tauri-apps/api/core'
import { useEffect, useRef, useState } from 'react'
import { Check, Clipboard, FileJson, FileSpreadsheet, FileText, FolderOpen, MoreHorizontal } from 'lucide-react'
import { copyToClipboard } from '../utils/clipboard'
import { formatBytes } from '../utils/formatBytes'
import { IconButton } from '../components/Button'
import { artifactDataUrl, artifactMimeType, isFileArtifact } from './artifacts'
import type { ChatToolArtifact } from './types'
import { ChatHeavyIsland } from './ChatHeavyIsland'

const TEXT_PREVIEW_LIMIT = 260

function artifactPath(artifact: ChatToolArtifact): string {
  return artifact.path ?? artifact.filePath ?? artifact.localPath ?? ''
}

function artifactSizeBytes(artifact: ChatToolArtifact): number | null {
  const value = artifact.sizeBytes ?? artifact.size_bytes
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function fileExtension(name: string): string {
  const match = /\.([a-z0-9]+)$/i.exec(name)
  return match?.[1]?.toLowerCase() ?? ''
}

function artifactKind(artifact: ChatToolArtifact): string {
  const mime = artifactMimeType(artifact)
  const ext = fileExtension(artifact.name)
  if (mime.includes('markdown') || ext === 'md' || ext === 'markdown') return 'Markdown'
  if (mime.includes('csv') || ext === 'csv') return 'CSV'
  if (mime.includes('json') || ext === 'json') return 'JSON'
  if (mime.includes('html') || ext === 'html' || ext === 'htm') return 'HTML'
  if (mime.includes('spreadsheet') || ext === 'xlsx' || ext === 'xls') return 'Spreadsheet'
  if (mime.startsWith('text/') || ext === 'txt') return 'Text'
  return ext ? ext.toUpperCase() : 'File'
}

function ArtifactIcon({ artifact }: { artifact: ChatToolArtifact }) {
  const kind = artifactKind(artifact)
  const className = 'h-5 w-5 text-neutral-500 dark:text-neutral-400'
  if (kind === 'JSON') return <FileJson className={className} strokeWidth={1.9} />
  if (kind === 'CSV' || kind === 'Spreadsheet') {
    return <FileSpreadsheet className={className} strokeWidth={1.9} />
  }
  return <FileText className={className} strokeWidth={1.9} />
}

function decodeBase64Utf8(payload: string): string {
  const binary = atob(payload.replace(/\s+/g, ''))
  const bytes = Uint8Array.from(binary, (ch) => ch.charCodeAt(0))
  return new TextDecoder().decode(bytes)
}

function previewFromDataUrl(dataUrl: string): string {
  if (!dataUrl.startsWith('data:')) return ''
  const [, payload = ''] = dataUrl.split(',', 2)
  if (!payload) return ''
  try {
    const decoded = dataUrl.slice(0, dataUrl.indexOf(',')).includes(';base64')
      ? decodeBase64Utf8(payload)
      : decodeURIComponent(payload)
    return decoded
      .replace(/\r\n/g, '\n')
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .join('\n')
      .slice(0, TEXT_PREVIEW_LIMIT)
  } catch {
    return ''
  }
}

function artifactPreview(artifact: ChatToolArtifact): string {
  const kind = artifactKind(artifact)
  if (kind === 'Spreadsheet') return '表格文件已生成，可用默认应用打开查看。'
  const preview = previewFromDataUrl(artifactDataUrl(artifact)).trim()
  if (preview) return preview
  return '文件已生成，可打开查看完整内容。'
}

async function openGeneratedArtifact(artifact: ChatToolArtifact) {
  const path = artifactPath(artifact)
  if (path) {
    await invoke('chat_open_generated_artifact', { path })
    return
  }

  const dataUrl = artifactDataUrl(artifact)
  if (!dataUrl) return
  // 没有 path 的旧 artifact：落成临时文件再交给系统默认程序。
  // 不能用 `window.open(dataUrl)` —— 那条在 Tauri 里由 webview 自己处理，不会打开默认程序。
  await invoke('open_data_url_file', { name: artifact.name ?? 'file.bin', dataUrl })
}

async function revealGeneratedArtifact(artifact: ChatToolArtifact) {
  const path = artifactPath(artifact)
  if (!path) return
  await invoke('chat_reveal_generated_artifact', { path })
}

function GeneratedFileCard({
  artifact,
  index,
}: {
  artifact: ChatToolArtifact
  index: number
}) {
  const [menuOpen, setMenuOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const path = artifactPath(artifact)
  const size = formatBytes(artifactSizeBytes(artifact))
  const kind = artifactKind(artifact)
  const meta = [kind, size].filter(Boolean).join(' · ')

  useEffect(() => {
    if (!menuOpen) return
    const handlePointerDown = (event: PointerEvent) => {
      if (menuRef.current?.contains(event.target as Node)) return
      setMenuOpen(false)
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenuOpen(false)
    }
    window.addEventListener('pointerdown', handlePointerDown)
    window.addEventListener('keydown', handleKeyDown)
    return () => {
      window.removeEventListener('pointerdown', handlePointerDown)
      window.removeEventListener('keydown', handleKeyDown)
    }
  }, [menuOpen])

  const menuButtonClass = 'kv-menu-item whitespace-nowrap'
  const menuIconClass = 'h-3.5 w-3.5 shrink-0 text-neutral-600 dark:text-neutral-300'

  const handleCopyPath = async () => {
    if (!path) return
    const ok = await copyToClipboard(path)
    if (!ok) return
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1400)
  }

  return (
    <div
      key={`${artifact.name}-${index}`}
      className="group/file-card relative flex w-full min-w-0 rounded-lg border border-neutral-200/90 bg-white px-3 py-3 text-left shadow-[0_1px_2px_rgba(15,23,42,0.03)] transition-colors hover:border-neutral-300 hover:bg-neutral-50 dark:border-neutral-800 dark:bg-neutral-900/80 dark:hover:border-neutral-700 dark:hover:bg-neutral-900"
    >
      <button
        type="button"
        className="flex min-w-0 flex-1 p-0 text-left"
        onClick={() => void openGeneratedArtifact(artifact)}
        title="打开文件"
        aria-label={`打开文件 ${artifact.name}`}
      >
        <span className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md border border-neutral-200 bg-neutral-50 dark:border-neutral-700 dark:bg-neutral-800">
          <ArtifactIcon artifact={artifact} />
        </span>
        <span className="ml-3 min-w-0 flex-1 pr-8">
          <span className="block min-w-0 truncate text-[15px] font-semibold leading-6 text-neutral-900 dark:text-neutral-100">
            {artifact.name}
          </span>
          {meta && (
            <span className="mt-0.5 block text-[11px] font-medium leading-4 text-neutral-400 dark:text-neutral-500">
              {meta}
            </span>
          )}
          <span className="mt-2 line-clamp-3 whitespace-pre-line border-t border-neutral-200/80 pt-2 text-[13px] leading-6 text-neutral-500 dark:border-neutral-800 dark:text-neutral-400">
            {artifactPreview(artifact)}
          </span>
        </span>
      </button>

      <div ref={menuRef} className="absolute right-1.5 top-1.5">
        <IconButton
          size="sm"
          onClick={(event) => {
            event.stopPropagation()
            setMenuOpen((value) => !value)
          }}
          aria-expanded={menuOpen}
          label="文件操作"
        >
          <MoreHorizontal size={16} strokeWidth={2} />
        </IconButton>
        {menuOpen && (
          <div className="absolute right-0 top-7 z-20 w-44 overflow-hidden kv-menu">
            <button
              type="button"
              className={menuButtonClass}
              onClick={() => {
                setMenuOpen(false)
                void openGeneratedArtifact(artifact)
              }}
            >
              <FileText className={menuIconClass} strokeWidth={1.9} />
              <span>打开</span>
            </button>
            <button
              type="button"
              disabled={!path}
              className={`${menuButtonClass} disabled:cursor-not-allowed disabled:opacity-40`}
              onClick={() => {
                setMenuOpen(false)
                void revealGeneratedArtifact(artifact)
              }}
            >
              <FolderOpen className={menuIconClass} strokeWidth={1.9} />
              <span>在文件系统中打开</span>
            </button>
            <button
              type="button"
              disabled={!path}
              className={`${menuButtonClass} disabled:cursor-not-allowed disabled:opacity-40`}
              onClick={() => void handleCopyPath()}
            >
              {copied ? <Check className={`${menuIconClass} chat-motion-pop`} strokeWidth={2} /> : <Clipboard className={menuIconClass} strokeWidth={1.9} />}
              <span>{copied ? '已复制路径' : '复制路径'}</span>
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

export function GeneratedFileArtifacts({ artifacts }: { artifacts: ChatToolArtifact[] }) {
  const fileArtifacts = artifacts.filter(isFileArtifact)
  if (fileArtifacts.length === 0) return null

  return (
    <ChatHeavyIsland
      minHeight={Math.min(240, 72 + fileArtifacts.length * 76)}
      fallback={(
        <div className="not-prose mt-3 h-16 rounded-lg border border-neutral-200/90 bg-white/70 dark:border-neutral-800 dark:bg-neutral-900/50" />
      )}
      eager
    >
      <div className="not-prose mt-3 space-y-2">
        {fileArtifacts.map((artifact, index) => (
          <GeneratedFileCard
            key={`${artifact.name}-${index}`}
            artifact={artifact}
            index={index}
          />
        ))}
      </div>
    </ChatHeavyIsland>
  )
}
