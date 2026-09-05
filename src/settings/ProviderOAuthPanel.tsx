import { useEffect, useRef, useState } from 'react'
import { api, type ModelProvider, type ProviderOAuthLogin } from '../api/tauri'
import { Button } from '../components/Button'
import { FieldBlock, Select } from './components'
import type { Lang } from './i18n'

export function ProviderOAuthPanel({ provider, lang, onUpdateProvider }: {
  provider: ModelProvider
  lang: Lang
  onUpdateProvider: (id: string, updates: Partial<ModelProvider>) => void
}) {
  const zh = lang === 'zh'
  const auth = provider.request?.oauth
  const [login, setLogin] = useState<ProviderOAuthLogin | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const generation = useRef(0)
  const loginRef = useRef<ProviderOAuthLogin | null>(null)
  const current = useRef({ provider, onUpdateProvider })
  current.current = { provider, onUpdateProvider }

  useEffect(() => () => {
    generation.current += 1
    if (loginRef.current) void api.providerOAuthCancel(loginRef.current.loginId).catch(() => {})
  }, [])

  useEffect(() => {
    if (!login) return
    let stopped = false
    let timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      try {
        const result = await api.providerOAuthPoll(login.loginId)
        if (stopped) {
          if (result.auth?.credentialId) void api.providerOAuthDisconnect(result.auth.credentialId).catch(() => {})
          return
        }
        if (result.status === 'authorized' && result.auth) {
          const { provider: latest, onUpdateProvider: update } = current.current
          const request = { ...latest.request, oauth: result.auth }
          update(latest.id, { request })
          loginRef.current = null
          setLogin(null)
          setBusy(false)
          setNotice(zh ? '授权成功。请点击「管理模型」获取并启用模型，设置会自动保存。' : 'Signed in. Open Models to fetch and enable models. Settings save automatically.')
          return
        }
        timer = setTimeout(() => void poll(), Math.max(result.interval, 3) * 1000)
      } catch (err) {
        if (stopped) return
        void api.providerOAuthCancel(login.loginId).catch(() => {})
        loginRef.current = null
        setLogin(null)
        setBusy(false)
        setError(String(err))
      }
    }
    timer = setTimeout(() => void poll(), login.interval * 1000)
    return () => { stopped = true; clearTimeout(timer) }
  }, [login, zh])

  const start = async () => {
    if (!auth) return
    const run = ++generation.current
    setBusy(true); setError(''); setNotice('')
    try {
      const next = await api.providerOAuthStart(auth.provider, provider.request?.useSystemProxy !== false)
      if (generation.current !== run) { await api.providerOAuthCancel(next.loginId); return }
      loginRef.current = next
      setLogin(next)
      try { await api.openExternal(next.verificationUrl) }
      catch { if (generation.current === run) setError(zh ? '无法自动打开浏览器，请点击下方授权链接。' : 'Could not open your browser. Use the authorization link below.') }
    } catch (err) {
      if (generation.current === run) { setError(String(err)); setBusy(false) }
    }
  }

  const cancel = () => {
    generation.current += 1
    if (loginRef.current) void api.providerOAuthCancel(loginRef.current.loginId).catch(() => {})
    loginRef.current = null; setLogin(null); setBusy(false)
  }

  const disconnect = async () => {
    if (!auth?.credentialId) return
    setBusy(true); setError('')
    try {
      await api.providerOAuthDisconnect(auth.credentialId)
      onUpdateProvider(provider.id, { request: { ...provider.request, oauth: { provider: auth.provider } } })
      setNotice('')
    } catch (err) { setError(String(err)) }
    finally { setBusy(false) }
  }

  return <>
    <FieldBlock label={zh ? '认证方式' : 'Authentication'}>
      <Select value={auth?.provider ?? 'api_key'} disabled={busy || Boolean(auth?.credentialId)}
        onChange={(value) => {
          setError(''); setNotice('')
          if (value === 'api_key') {
            onUpdateProvider(provider.id, { request: { ...provider.request, oauth: null } })
          } else {
            const kind = value as 'codex' | 'kimi' | 'antigravity'
            onUpdateProvider(provider.id, {
              request: { ...provider.request, oauth: { provider: kind } },
              baseUrl: kind === 'antigravity' ? 'https://daily-cloudcode-pa.googleapis.com' : kind === 'codex' ? 'https://chatgpt.com/backend-api/codex' : 'https://api.kimi.com/coding/v1',
              apiFormat: kind === 'antigravity' ? 'gemini' : kind === 'codex' ? 'openai_responses' : 'openai_chat',
              availableModels: [], enabledModels: [],
            })
          }
        }} options={[
          { value: 'api_key', label: 'API Key' },
          { value: 'codex', label: 'Codex OAuth · ChatGPT' },
          { value: 'kimi', label: 'Kimi OAuth · Kimi Code' },
          { value: 'antigravity', label: 'Antigravity OAuth · Google' },
        ]} />
    </FieldBlock>
    {auth && <FieldBlock label={zh ? '账号授权' : 'Account authorization'}
      description={zh ? '在浏览器中登录账号。凭证保存在系统凭证库，调用模型时自动刷新。' : 'Sign in through your browser. Credentials stay in the system credential store and refresh automatically.'}>
      <div className="flex items-center gap-2">
        <span className="text-sm" role="status">{auth.credentialId ? (zh ? '已授权' : 'Authorized') : (zh ? '尚未授权' : 'Not signed in')}</span>
        {auth.credentialId
          ? <Button size="sm" disabled={busy} onClick={() => void disconnect()}>{zh ? '断开授权' : 'Disconnect'}</Button>
          : <Button size="sm" disabled={busy} onClick={() => void start()}>{busy ? (zh ? '等待授权…' : 'Waiting…') : (zh ? '登录授权' : 'Sign in')}</Button>}
        {busy && !auth.credentialId && <Button size="sm" onClick={cancel}>{zh ? '取消' : 'Cancel'}</Button>}
      </div>
      {login && <div className="mt-3 space-y-2 rounded-lg border border-[var(--border)] p-3">
        <p className="text-sm">{login.userCode
          ? (zh ? '在授权页面输入设备码：' : 'Enter this device code on the authorization page:')
          : (zh ? '请在浏览器中完成 Google 登录，完成后会自动返回授权结果。' : 'Complete Google sign-in in your browser. Kivio will receive the authorization automatically.')}</p>
        {login.userCode && <code className="block select-text text-lg tracking-widest">{login.userCode}</code>}
        <button type="button" className="text-sm text-indigo-500 hover:underline" onClick={() => void api.openExternal(login.verificationUrl).catch(err => setError(String(err)))}>
          {zh ? '打开授权页面 ↗' : 'Open authorization page ↗'}
        </button>
        <p className="text-xs opacity-70">{zh ? '有效期至 ' : 'Expires at '}{new Date(login.expiresAt * 1000).toLocaleTimeString()}</p>
      </div>}
      {notice && <p className="mt-2 text-sm" role="status">{notice}</p>}
      {error && <p className="mt-2 text-sm text-red-500" role="alert">{error}</p>}
    </FieldBlock>}
  </>
}
