// 消息区工具卡片「点文件名 / 点 +N -N」→ 右侧 dock 查看器 的单监听信道。
// 模式同 composerInsert：同一时刻只有一个活跃 Chat 实例，单监听足够。
type Listener = (path: string) => void

let listener: Listener | null = null

export function onDockPreviewRequest(cb: Listener): () => void {
  listener = cb
  return () => {
    if (listener === cb) listener = null
  }
}

/** path 可以是会话 workdir 内的相对路径，也可以是任意绝对路径（Chat.tsx 负责解析）。 */
export function requestDockPreview(path: string): void {
  listener?.(path)
}

// diff 预览信道：点工具卡片的 +N -N 徽标 → 右侧栏渲染整份带色 diff。
export type DockDiffPayload = { title: string; patch: string }
type DiffListener = (payload: DockDiffPayload) => void

let diffListener: DiffListener | null = null

export function onDockDiffPreviewRequest(cb: DiffListener): () => void {
  diffListener = cb
  return () => {
    if (diffListener === cb) diffListener = null
  }
}

export function requestDockDiffPreview(payload: DockDiffPayload): void {
  diffListener?.(payload)
}

// markdown 预览信道：claude 提交计划（ExitPlanMode）→ 右侧栏渲染整份计划。
// 与 diff 分开而不是复用：载荷不同（正文 vs 补丁），渲染器也不同（ChatMarkdown vs DiffView）。
export type DockMarkdownPayload = { title: string; text: string }
type MarkdownListener = (payload: DockMarkdownPayload) => void

let markdownListener: MarkdownListener | null = null

export function onDockMarkdownPreviewRequest(cb: MarkdownListener): () => void {
  markdownListener = cb
  return () => {
    if (markdownListener === cb) markdownListener = null
  }
}

export function requestDockMarkdownPreview(payload: DockMarkdownPayload): void {
  markdownListener?.(payload)
}
