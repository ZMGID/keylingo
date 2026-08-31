import { invoke } from '@tauri-apps/api/core'
import { artifactDataUrl, isFileArtifact } from './artifacts'
import { FileChip } from './fileChip'
import type { ChatToolArtifact } from './types'

function artifactPath(artifact: ChatToolArtifact): string {
  return artifact.path ?? artifact.filePath ?? artifact.localPath ?? ''
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

export function GeneratedFileArtifacts({ artifacts }: { artifacts: ChatToolArtifact[] }) {
  const fileArtifacts = artifacts.filter(isFileArtifact)
  if (fileArtifacts.length === 0) return null

  return (
    <div className="not-prose mt-3 flex min-w-0 max-w-full flex-wrap gap-2">
      {fileArtifacts.map((artifact, index) => (
        <FileChip
          key={`${artifact.path || artifact.name}-${index}`}
          name={artifact.name}
          ariaLabel={`打开文件 ${artifact.name}`}
          onClick={() => void openGeneratedArtifact(artifact)}
        />
      ))}
    </div>
  )
}
