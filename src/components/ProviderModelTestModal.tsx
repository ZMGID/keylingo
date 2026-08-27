import { useState } from 'react'
import { X, RefreshCw, Check } from 'lucide-react'
import { api } from '../api/tauri'
import type { ProviderRequestConfig } from '../api/tauri'
import { ModelIcon } from '../chat/ModelIcon'
import { Button, IconButton } from './Button'

type Lang = 'zh' | 'en'
type Result = { status: 'testing' | 'ok' | 'fail'; error?: string }

/**
 * 供应商「测试连接」弹窗：勾选已添加的模型批量测试。
 * 每个模型发一条极小对话请求（后端 test_provider_connection 带 model），
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

  const runTests = async () => {
    const targets = models.filter((m) => selected.has(m))
    if (targets.length === 0 || running) return
    setRunning(true)
    setResults((prev) => {
      const next = { ...prev }
      targets.forEach((m) => { next[m] = { status: 'testing' } })
      return next
    })
    await Promise.all(
      targets.map(async (model) => {
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
          setResults((prev) => ({
            ...prev,
            [model]: r.success ? { status: 'ok' } : { status: 'fail', error: r.error },
          }))
        } catch (e) {
          setResults((prev) => ({
            ...prev,
            [model]: { status: 'fail', error: e instanceof Error ? e.message : String(e) },
          }))
        }
      }),
    )
    setRunning(false)
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
              <span className="kv-row-desc">{t.selectedCount(selected.size)}</span>
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
