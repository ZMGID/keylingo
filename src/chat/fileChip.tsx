import {
  File,
  FileArchive,
  FileCode,
  FileJson,
  FileMusic,
  FilePlay,
  FileSpreadsheet,
  FileText,
  Presentation,
} from 'lucide-react'

type FileKindVisual = {
  Icon: typeof File
  label: string
  iconClass: string
  wellClass: string
}

const CODE_EXTS = new Set([
  'ts', 'tsx', 'mts', 'cts', 'js', 'jsx', 'mjs', 'cjs',
  'py', 'rs', 'go', 'java', 'kt', 'kts', 'c', 'h', 'cc', 'cpp', 'cxx', 'hpp',
  'rb', 'php', 'swift', 'html', 'htm', 'css', 'scss', 'less', 'vue', 'svelte',
  'sh', 'bash', 'zsh', 'ps1', 'bat', 'sql',
])

function extensionOf(name: string): string {
  const slash = Math.max(name.lastIndexOf('/'), name.lastIndexOf('\\'))
  const base = slash >= 0 ? name.slice(slash + 1) : name
  const dot = base.lastIndexOf('.')
  if (dot <= 0 || dot === base.length - 1) return ''
  return base.slice(dot + 1).toLowerCase()
}

function fileKindVisual(name: string): FileKindVisual {
  const ext = extensionOf(name)
  const label = ext ? ext.toUpperCase() : 'FILE'
  if (ext === 'pdf') {
    return { Icon: FileText, label, iconClass: 'text-red-500 dark:text-red-400', wellClass: 'bg-red-500/10 dark:bg-red-400/15' }
  }
  if (ext === 'doc' || ext === 'docx') {
    return { Icon: FileText, label, iconClass: 'text-blue-600 dark:text-blue-400', wellClass: 'bg-blue-500/10 dark:bg-blue-400/15' }
  }
  if (ext === 'xls' || ext === 'xlsx' || ext === 'xlsm' || ext === 'csv' || ext === 'tsv') {
    return { Icon: FileSpreadsheet, label, iconClass: 'text-emerald-600 dark:text-emerald-400', wellClass: 'bg-emerald-500/10 dark:bg-emerald-400/15' }
  }
  if (ext === 'ppt' || ext === 'pptx') {
    return { Icon: Presentation, label, iconClass: 'text-orange-500 dark:text-orange-400', wellClass: 'bg-orange-500/10 dark:bg-orange-400/15' }
  }
  if (ext === 'zip' || ext === 'tar' || ext === 'gz' || ext === '7z' || ext === 'rar') {
    return { Icon: FileArchive, label, iconClass: 'text-amber-600 dark:text-amber-400', wellClass: 'bg-amber-500/10 dark:bg-amber-400/15' }
  }
  if (ext === 'json' || ext === 'jsonc' || ext === 'yaml' || ext === 'yml' || ext === 'toml') {
    return { Icon: FileJson, label, iconClass: 'text-amber-500 dark:text-amber-400', wellClass: 'bg-amber-500/10 dark:bg-amber-400/15' }
  }
  if (ext === 'mp3' || ext === 'wav' || ext === 'm4a' || ext === 'flac' || ext === 'ogg' || ext === 'aac') {
    return { Icon: FileMusic, label, iconClass: 'text-violet-500 dark:text-violet-400', wellClass: 'bg-violet-500/10 dark:bg-violet-400/15' }
  }
  if (ext === 'mp4' || ext === 'mov' || ext === 'webm' || ext === 'mkv' || ext === 'avi') {
    return { Icon: FilePlay, label, iconClass: 'text-fuchsia-500 dark:text-fuchsia-400', wellClass: 'bg-fuchsia-500/10 dark:bg-fuchsia-400/15' }
  }
  if (CODE_EXTS.has(ext)) {
    return { Icon: FileCode, label, iconClass: 'text-sky-500 dark:text-sky-400', wellClass: 'bg-sky-500/10 dark:bg-sky-400/15' }
  }
  if (ext === 'md' || ext === 'markdown' || ext === 'txt' || ext === 'log') {
    return { Icon: FileText, label, iconClass: 'text-neutral-500 dark:text-neutral-300', wellClass: 'bg-neutral-500/10 dark:bg-white/10' }
  }
  return { Icon: File, label, iconClass: 'text-neutral-500 dark:text-neutral-300', wellClass: 'bg-neutral-500/10 dark:bg-white/10' }
}

/** 输入框 / 用户气泡 / 助手产物共用的 64px 文件芯片。 */
export function FileChip({
  name,
  onClick,
  ariaLabel,
}: {
  name: string
  onClick: () => void
  ariaLabel?: string
}) {
  const visual = fileKindVisual(name)
  const Icon = visual.Icon
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex h-16 w-[9.5rem] shrink-0 items-center gap-2 rounded-lg border border-neutral-200/90 bg-neutral-50 px-1.5 text-left hover:bg-neutral-100 dark:border-neutral-700 dark:bg-neutral-800 dark:hover:bg-neutral-700/80"
      title={name}
      aria-label={ariaLabel ?? name}
    >
      <span className={`flex h-11 w-11 shrink-0 items-center justify-center rounded-md ${visual.wellClass}`}>
        <Icon size={18} strokeWidth={1.8} className={visual.iconClass} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[12px] font-medium leading-tight text-neutral-800 dark:text-neutral-100">
          {name}
        </span>
        <span className="mt-0.5 block truncate text-[10px] font-medium uppercase tracking-wide text-neutral-400 dark:text-neutral-500">
          {visual.label}
        </span>
      </span>
    </button>
  )
}
