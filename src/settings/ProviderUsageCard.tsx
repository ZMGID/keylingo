import { useEffect, useRef, useState } from 'react'
import { RefreshCw } from 'lucide-react'
import { api, type ModelProvider, type ProviderOAuthUsage } from '../api/tauri'
import { Button } from '../components/Button'
import type { Lang } from './i18n'

export function ProviderUsageCard({ provider, lang }: { provider: ModelProvider; lang: Lang }) {
  const auth = provider.request?.oauth
  const supported = auth?.provider === 'kimi' || auth?.provider === 'codex' || auth?.provider === 'antigravity'
  const [data, setData] = useState<ProviderOAuthUsage | null>(null)
  const [error, setError] = useState(false)
  const [busy, setBusy] = useState(false)
  const [refresh, setRefresh] = useState(0)
  const current = useRef(provider)
  current.current = provider
  const zh = lang === 'zh'
  useEffect(() => {
    setData(null); setError(false)
    if (!supported || !auth?.credentialId) { setBusy(false); return }
    let cancelled = false
    setBusy(true)
    api.providerOAuthUsage(current.current).then(result => {
      if (!cancelled) setData(result)
    }).catch(() => {
      if (!cancelled) setError(true)
    }).finally(() => {
      if (!cancelled) setBusy(false)
    })
    return () => { cancelled = true }
  }, [provider.id, auth?.provider, auth?.credentialId, provider.baseUrl, provider.apiFormat, provider.request?.useSystemProxy, supported, refresh])

  if (!supported || !auth?.credentialId) return null
  const label = (value: string) => zh ? ({ Weekly: '每周额度', Session: '会话额度' }[value] ?? value.replace('Code review · ', '代码审查 · ').replace(/^(\d+)h$/, '$1 小时额度')) : value
  return <section aria-label={zh ? '账户用量' : 'Account usage'} className="mb-5 rounded-xl border border-[var(--border)] bg-[var(--bg-secondary)] p-4">
    <div className="flex items-center justify-between gap-3">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-medium">{zh ? '账户用量' : 'Account usage'}</h3>
        {data?.plan && <span className="rounded-full border border-[var(--border)] px-2 py-0.5 text-xs opacity-70">{data.plan}</span>}
      </div>
      <Button size="sm" disabled={busy} onClick={() => setRefresh(n => n + 1)}>
        <RefreshCw size={12} className={busy ? 'animate-spin' : ''} />{zh ? '刷新' : 'Refresh'}
      </Button>
    </div>
    {busy && <p className="mt-3 text-xs opacity-60" role="status">{zh ? '正在获取账户额度…' : 'Fetching account limits…'}</p>}
    {error && <p className="mt-3 text-xs text-red-500" role="alert">{zh ? '暂时无法获取用量，请检查网络或授权后重试。' : 'Could not fetch usage. Check your connection or authorization and retry.'}</p>}
    {data && data.windows.length === 0 && <p className="mt-3 text-xs opacity-60">{zh ? '供应商暂未返回额度数据' : 'No quota data returned by the provider'}</p>}
    {auth?.provider === 'antigravity' && data && data.windows.length > 0 && <p className="mt-2 text-xs opacity-60">{zh ? '显示接口返回的额度组；部分账号可能不提供完整周额度。' : 'Reported quota groups; full weekly limits may not be available for every account.'}</p>}
    {data?.windows.map((window, i) => {
      const remaining = window.usedPercent === null ? null : Math.max(0, Math.min(100, 100 - window.usedPercent))
      return <div key={`${window.label}-${i}`} className="mt-3">
        <div className="mb-1.5 flex items-center justify-between gap-3 text-xs">
          <span>{label(window.label)}</span>
          <span className="tabular-nums">{remaining === null ? (zh ? '暂无比例' : 'Unavailable') : `${zh ? '剩余 ' : ''}${remaining.toFixed(0)}%${zh ? '' : ' remaining'}`}</span>
        </div>
        {remaining !== null && <div role="progressbar" aria-label={label(window.label)} aria-valuemin={0} aria-valuemax={100} aria-valuenow={remaining} className="h-1.5 overflow-hidden rounded-full bg-black/5 dark:bg-white/10">
          <div className={`h-full rounded-full ${remaining <= 10 ? 'bg-amber-500' : 'bg-indigo-500'}`} style={{ width: `${remaining}%` }} />
        </div>}
        <div className="mt-1 flex flex-wrap justify-between gap-x-3 text-[11px] opacity-60">
          {window.used !== null && window.limit !== null && <span>{zh ? '已用 ' : 'Used '}{window.used.toLocaleString()} / {window.limit.toLocaleString()}</span>}
          {window.resetsAt === null && window.resetHint && <span>{window.resetHint}</span>}
          {window.resetsAt !== null && <span>{zh ? '重置于 ' : 'Resets '}{new Date(window.resetsAt * 1000).toLocaleString(zh ? 'zh-CN' : 'en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</span>}
        </div>
      </div>
    })}
    {data && <p className="mt-3 text-[11px] opacity-50">{zh ? '账户共享额度 · 更新于 ' : 'Shared account limits · Updated '}{new Date(data.fetchedAt * 1000).toLocaleTimeString()}</p>}
  </section>
}
