import { useRef } from 'react'
import { Upload, X } from 'lucide-react'

/**
 * 头像上传字段。原本在 SettingsShell 模块作用域，只被聊天标签页使用，
 * 随 ChatTab 一起搬过来。
 */
export function AvatarField({
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
