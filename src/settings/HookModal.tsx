import { useState } from 'react'
import { createPortal } from 'react-dom'
import { X } from 'lucide-react'
import { HOOK_EVENTS, type HookDef, type HookEvent } from '../api/tauri'
import { i18n, type Lang } from './i18n'
import { Input, Label, Select, TextArea } from './components'
import { Button, IconButton } from '../components/Button'
import { getPlatform } from './utils'

const TIMEOUT_MIN_MS = 1_000
const TIMEOUT_MAX_MS = 600_000
const TIMEOUT_DEFAULT_MS = 60_000
const METHODS = ['POST', 'GET', 'PUT', 'PATCH', 'DELETE']
const SCRIPT_PLACEHOLDER = getPlatform() === 'windows'
  ? 'echo done >> %TEMP%\\kivio-hook.log'
  : 'echo done >> /tmp/kivio-hook.log'

function headersToText(headers: Record<string, string>): string {
  return Object.entries(headers)
    .map(([key, value]) => `${key}: ${value}`)
    .join('\n')
}

function textToHeaders(text: string): Record<string, string> {
  const headers: Record<string, string> = {}
  for (const line of text.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed) continue
    const separator = trimmed.indexOf(':')
    const key = (separator >= 0 ? trimmed.slice(0, separator) : trimmed).trim()
    if (!key) continue
    headers[key] = separator >= 0 ? trimmed.slice(separator + 1).trim() : ''
  }
  return headers
}

type Props = {
  lang: Lang
  /** 新建时绑定的事件；编辑时沿用 hook 自己的事件。 */
  event: string
  eventLabel: string
  eventOptions: { value: HookEvent; label: string }[]
  initial?: HookDef | null
  onSave: (hook: HookDef) => void
  onClose: () => void
}

/**
 * Hook 编辑弹窗。command 分支只有脚本 + 超时，http 分支是 URL + 方法 + 头 + 超时。
 * ponytail: 后端 `HookDef` 没有自定义 body 字段，所以这里也不提供 body 编辑器。
 */
export function HookModal({ lang, event, eventLabel, eventOptions, initial, onSave, onClose }: Props) {
  const t = i18n[lang]
  const [name, setName] = useState(initial?.name ?? '')
  const [description, setDescription] = useState(initial?.description ?? '')
  const [boundEvent, setBoundEvent] = useState<string>(initial?.event ?? event)
  const [kind, setKind] = useState<'command' | 'http'>(initial?.type === 'http' ? 'http' : 'command')
  const [script, setScript] = useState(initial?.script ?? '')
  const [url, setUrl] = useState(initial?.url ?? '')
  const [method, setMethod] = useState(initial?.method || 'POST')
  const [headersText, setHeadersText] = useState(headersToText(initial?.headers ?? {}))
  const [timeoutText, setTimeoutText] = useState(String(initial?.timeoutMs ?? TIMEOUT_DEFAULT_MS))
  const [error, setError] = useState('')

  const handleSave = () => {
    const trimmedName = name.trim()
    if (!trimmedName) {
      setError(t.hooksNameRequired)
      return
    }
    if (kind === 'command' && !script.trim()) {
      setError(t.hooksScriptRequired)
      return
    }
    if (kind === 'http' && !url.trim()) {
      setError(t.hooksUrlRequired)
      return
    }
    const timeoutMs = Number(timeoutText)
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs < TIMEOUT_MIN_MS || timeoutMs > TIMEOUT_MAX_MS) {
      setError(t.hooksTimeoutInvalid)
      return
    }
    onSave({
      id: initial?.id || `hook-${Date.now()}`,
      name: trimmedName,
      description: description.trim(),
      event: HOOK_EVENTS.includes(boundEvent as HookEvent) ? boundEvent : event,
      enabled: initial?.enabled ?? true,
      type: kind,
      script: kind === 'command' ? script : '',
      url: kind === 'http' ? url.trim() : '',
      method: kind === 'http' ? method : 'POST',
      headers: kind === 'http' ? textToHeaders(headersText) : {},
      timeoutMs,
    })
  }

  return createPortal(
    <div
      className="kv-modal-backdrop kv-modal-backdrop--portal"
      data-tauri-drag-region="false"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div
        className="kv kv-modal max-w-xl space-y-3"
        role="dialog"
        aria-modal="true"
        aria-label={initial ? t.hooksEdit : t.hooksAdd}
        data-tauri-drag-region="false"
      >
        <div className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <h3 className="text-[14px] font-semibold">{initial ? t.hooksEdit : t.hooksAdd}</h3>
            <p className="kv-row-desc">
              {eventOptions.find((option) => option.value === boundEvent)?.label ?? eventLabel}
            </p>
          </div>
          <IconButton size="xs" label={t.cancel} onClick={onClose} data-tauri-drag-region="false">
            <X size={14} />
          </IconButton>
        </div>

        <div className="custom-scrollbar max-h-[60vh] space-y-3 overflow-y-auto pr-0.5">
          <div className="space-y-1">
            <Label>{t.hooksEvent}</Label>
            <Select
              value={boundEvent}
              onChange={setBoundEvent}
              options={eventOptions}
              className="w-full"
            />
          </div>

          <div className="space-y-1">
            <Label>{t.hooksName}</Label>
            <Input value={name} onChange={setName} placeholder={t.hooksNamePlaceholder} />
          </div>
          <div className="space-y-1">
            <Label>{t.hooksDescription}</Label>
            <Input value={description} onChange={setDescription} placeholder={t.hooksDescriptionPlaceholder} />
          </div>

          <div className="space-y-1">
            <Label>{t.hooksType}</Label>
            <div className="kv-seg w-fit">
              <button
                type="button"
                className={kind === 'command' ? 'active' : ''}
                onClick={() => setKind('command')}
                data-tauri-drag-region="false"
              >
                {t.hooksTypeCommand}
              </button>
              <button
                type="button"
                className={kind === 'http' ? 'active' : ''}
                onClick={() => setKind('http')}
                data-tauri-drag-region="false"
              >
                {t.hooksTypeHttp}
              </button>
            </div>
          </div>

          {kind === 'command' ? (
            <div className="space-y-1">
              <Label>{t.hooksCommandLabel}</Label>
              <TextArea value={script} onChange={setScript} rows={6} mono placeholder={SCRIPT_PLACEHOLDER} />
              <p className="kv-row-desc">{t.hooksCommandHint}</p>
            </div>
          ) : (
            <>
              <div className="space-y-1">
                <Label>{t.hooksUrl}</Label>
                <Input value={url} onChange={setUrl} mono placeholder="https://example.com/hook" />
                <p className="kv-row-desc">{t.hooksHttpHint}</p>
              </div>
              <div className="space-y-1">
                <Label>{t.hooksMethod}</Label>
                <Select
                  value={method}
                  onChange={setMethod}
                  options={METHODS.map((value) => ({ value, label: value }))}
                  className="w-40"
                />
              </div>
              <div className="space-y-1">
                <Label>{t.hooksHeaders}</Label>
                <TextArea value={headersText} onChange={setHeadersText} rows={3} mono placeholder="Authorization: Bearer …" />
                <p className="kv-row-desc">{t.hooksHeadersHint}</p>
              </div>
            </>
          )}

          <div className="space-y-1">
            <Label>{t.hooksTimeout}</Label>
            <Input
              value={timeoutText}
              onChange={(value) => setTimeoutText(value.replace(/[^\d]/g, ''))}
              inputMode="numeric"
              className="w-40"
            />
            <p className="kv-row-desc">{t.hooksTimeoutHint}</p>
          </div>
        </div>

        {error && <p className="text-[12px] text-red-500 dark:text-red-400">{error}</p>}

        <div className="flex justify-end gap-2 pt-1">
          <Button onClick={onClose} data-tauri-drag-region="false">{t.cancel}</Button>
          <Button variant="primary" onClick={handleSave} data-tauri-drag-region="false">{t.save}</Button>
        </div>
      </div>
    </div>,
    document.body,
  )
}
