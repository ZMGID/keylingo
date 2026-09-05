import { useEffect, useRef, useState } from 'react'
import { api, type ModelProvider, type ProviderOAuthAccount } from '../api/tauri'
import { Button } from '../components/Button'
import type { Lang } from './i18n'

export function ProviderAccountIdentity({ provider, lang }: { provider: ModelProvider; lang: Lang }) {
  const [account, setAccount] = useState<ProviderOAuthAccount | null>(null)
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [retry, setRetry] = useState(0)
  const current = useRef(provider)
  current.current = provider
  const zh = lang === 'zh'
  useEffect(() => {
    let stopped = false
    setAccount(null); setStatus('loading')
    void (async () => {
      try {
        const result = await api.providerOAuthAccount(current.current)
        if (!stopped) { setAccount(result); setStatus('ready') }
      } catch { if (!stopped) setStatus('error') }
    })()
    return () => { stopped = true }
  }, [provider.id, provider.request?.oauth?.provider, provider.request?.oauth?.credentialId, provider.baseUrl, provider.apiFormat, provider.request?.useSystemProxy, retry])
  return <div className="mt-2 rounded-lg border border-[var(--border)] px-3 py-2 text-sm" aria-label={zh ? '当前授权账号' : 'Authorized account'}>
    <div className="mb-1 text-xs opacity-60">{zh ? '当前账号' : 'Current account'}</div>
    {status === 'loading' && <span role="status" className="text-xs opacity-60">{zh ? '正在读取账号…' : 'Loading account…'}</span>}
    {status === 'error' && <div className="flex items-center justify-between gap-2">
      <span className="text-xs opacity-70">{zh ? '账号信息暂时无法读取' : 'Account information unavailable'}</span>
      <Button size="sm" onClick={() => setRetry(n => n + 1)}>{zh ? '重试' : 'Retry'}</Button>
    </div>}
    {status === 'ready' && <>
      {account?.email && <div className="break-all select-text font-medium">{account.email}</div>}
      {account?.name && account.name !== account.email && <div className="break-words">{account.name}</div>}
      {account?.accountId && <div className="break-all select-text text-xs opacity-70">{zh ? '账号 ID：' : 'Account ID: '}{account.accountId}</div>}
      {!account?.email && !account?.name && !account?.accountId && <span className="text-xs opacity-60">{zh ? '供应商未返回账号标识' : 'The provider did not return an account identity'}</span>}
    </>}
  </div>
}
