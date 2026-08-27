import { useEffect, useRef, useState } from 'react'
import { X, RefreshCw, Check } from 'lucide-react'
import { api } from '../api/tauri'
import type { ProviderRequestConfig } from '../api/tauri'
import { ModelIcon } from '../chat/ModelIcon'
import { Button, IconButton } from './Button'

type Lang = 'zh' | 'en'
type Result = { status: 'queued' | 'testing' | 'ok' | 'fail'; error?: string }

/** 同时打太多模型会卡住 WebView（IPC + 旋转图标 + 网关限流）。 */
export const MODEL_TEST_CONCURRENCY = 10

export async function runPool<T>(
  items: T[],
  limit: number,
  worker: (item: T) => Promise<void>,
): Promise<void> {
  let next = 0
  const n = Math.max(1, Math.min(limit, items.length))
  await Promise.all(
    Array.from({ length: n }, async () => {
      while (true) {
        const i = next++
        if (i >= items.length) return
        await worker(items[i])
      }
    }),
  )
}

/**
 * 供应商「测试连接」弹窗：勾选已添加的模型批量测试。
 * 每个模型发一条短对话请求（后端 test_provider_connection 带 model），
 * 失败时展示后端原始报错（含 404/503 等状态码）。
 */
export function ProviderModelTestModal({
  providerId,
  baseUrl,
  apiKeys,
  activeKeyIndex,
  apiFormat,
  request,
  models,
  lang,
  onClose,
}: {
  providerId: string
  baseUrl: string
  apiKeys: string[]
  activeKeyIndex?: number
  apiFormat: string
  /** 编辑中的请求配置：测试必须和真实聊天带一样的头，否则「测试通过、聊天 403」。 */
  request?: ProviderRequestConfig
  models: string[]
  lang: Lang
  onClose: () => void
}) {
  const [selected, setSelected] = useState<Set<string>>(() => new Set(models))
  const [results, setResults] = useState<Record<string, Result>>({})
  const [running, setRunning] = useState(false)
  const runIdRef = useRef(0)

  useEffect(() => () => {
    runIdRef.current += 1
  }, [])

  const t = {
    title: lang === 'zh' ? '测试模型' : 'Test Models',
    selectAll: lang === 'zh' ? '全选' : 'Select all',
    empty: lang === 'zh' ? '尚未添加模型，请先「获取模型列表」。' : 'No models yet — use "Fetch Models" first.',
    run: lang === 'zh' ? '开始测试' : 'Run test',
    running: lang === 'zh' ? '测试中…' : 'Testing…',
    ok: lang === 'zh' ? '正常' : 'OK',
    fail: lang === 'zh' ? '失败' : 'Failed',
    close: lang === 'zh' ? '关闭' : 'Close',
    selectedCount: (n: number) => (lang === 'zh' ? `已选 ${n}` : `${n} selected`),
    progress: (done: number, total: number) =>
      lang === 'zh' ? `${done}/${total}` : `${done}/${total}`,
  }

  const allChecked = models.length > 0 && selected.size === models.length
  const toggle = (m: string) =>
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(m)) next.delete(m)
      else next.add(m)
      return next
    })
  const toggleAll = () => setSelected(allChecked ? new Set() : new Set(models))

  const finishedCount = Object.values(results).filter(
    (r) => r.status === 'ok' || r.status === 'fail',
  ).length

  const runTests = async () => {
    const targets = models.filter((m) => selected.has(m))
    if (targets.length === 0 || running) return
    const runId = ++runIdRef.current
    setRunning(true)
    setResults(
      Object.fromEntries(targets.map((m) => [m, { status: 'queued' as const }])),
    )
    await runPool(targets, MODEL_TEST_CONCURRENCY, async (model) => {
      if (runIdRef.current !== runId) return
      setResults((prev) => ({ ...prev, [model]: { status: 'testing' } }))
      try {
        const r = await api.testProviderConnection(providerId, {
          id: providerId,
          baseUrl,
          apiKeys,
          activeKeyIndex,
          apiFormat,
          model,
          request,
        })
        if (runIdRef.current !== runId) return
        setResults((prev) => ({
          ...prev,
          [model]: r.success ? { status: 'ok' } : { status: 'fail', error: r.error },
        }))
      } catch (e) {
        if (runIdRef.current !== runId) return
        setResults((prev) => ({
          ...prev,
          [model]: { status: 'fail', error: e instanceof Error ? e.message : String(e) },
        }))
      }
    })
    if (runIdRef.current === runId) setRunning(false)
  }

  return (
    <div
      className="kv-modal-backdrop"
      data-tauri-drag-region="false"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose() }}
    >
      <div className="kv-modal kv-model-test-modal" data-tauri-drag-region="false" onMouseDown={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-2">
          <h3 className="text-[14px] font-semibold">{t.title}</h3>
          <IconButton size="xs" onClick={onClose} label={t.close} data-tauri-drag-region="false">
            <X size={14} />
          </IconButton>
        </div>

        {models.length === 0 ? (
          <p className="kv-panel-body py-4">{t.empty}</p>
        ) : (
          <>
            <div className="kv-mtest-toolbar">
              <label className="inline-flex items-center gap-2 cursor-pointer select-none">
                <input type="checkbox" className="kv-mtest-check" checked={allChecked} onChange={toggleAll} />
                <span className="text-[12px]">{t.selectAll}</span>
              </label>
              <span className="kv-row-desc">
                {running
                  ? t.progress(finishedCount, selected.size)
                  : t.selectedCount(selected.size)}
              </span>
            </div>

            <ul className="kv-mtest-list custom-scrollbar">
              {models.map((model) => {
                const res = results[model]
                return (
                  <li key={model}>
                    <div className="kv-mtest-row">
                      <input
                        type="checkbox"
                        className="kv-mtest-check"
                        checked={selected.has(model)}
                        onChange={() => toggle(model)}
                        disabled={running}
                      />
                      <ModelIcon model={model} size={16} />
                      <span className="kv-mtest-name" title={model}>{model}</span>
                      {res?.status === 'testing' && (
                        <span className="kv-mtest-status"><RefreshCw size={12} className="animate-spin" /></span>
                      )}
                      {res?.status === 'ok' && (
                        <span className="kv-mtest-status kv-mtest-status--ok"><Check size={13} strokeWidth={2.5} />{t.ok}</span>
                      )}
                      {res?.status === 'fail' && (
                        <span className="kv-mtest-status kv-mtest-status--fail"><X size={13} strokeWidth={2.5} />{t.fail}</span>
                      )}
                    </div>
                    {res?.status === 'fail' && res.error && (
                      <pre className="kv-mtest-error">{res.error}</pre>
                    )}
                  </li>
                )
              })}
            </ul>

            <div className="flex justify-end gap-2 pt-3">
              <Button variant="ghost" onClick={onClose} data-tauri-drag-region="false">{t.close}</Button>
              <Button
                variant="primary"
                onClick={runTests}
                disabled={running || selected.size === 0}
                data-tauri-drag-region="false"
              >
                <RefreshCw size={12} className={running ? 'animate-spin' : ''} />
                {running ? t.running : t.run}
              </Button>
            </div>
          </>
        )}
      </div>
    </div>
  )
}
