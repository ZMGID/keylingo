import { useCallback, useEffect, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { packageApi, type PluginPackage } from '../api/pluginPackages'
import { refreshSettings } from '../api/settingsCache'
import { Button } from '../components/Button'
import type { Lang } from '../settings/i18n'

export function PluginPackages({ lang }: { lang: Lang }) {
  const zh = lang === 'zh'
  const [packages, setPackages] = useState<PluginPackage[]>([])
  const [source, setSource] = useState('')
  const [subdirectory, setSubdirectory] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const refresh = useCallback(async () => setPackages(await packageApi.list()), [])
  useEffect(() => { void refresh().catch(e => setError(String(e))) }, [refresh])
  const act = async (action: () => Promise<unknown>) => {
    setBusy(true)
    setError('')
    try { await action(); await refresh() } catch (e) { setError(String(e)) } finally { setBusy(false) }
  }
  return <section className="mb-6 space-y-3 rounded-xl border border-neutral-200 p-4 dark:border-neutral-700" aria-label={zh ? '通用插件' : 'Plugin packages'}>
    <h3 className="text-sm font-semibold">{zh ? '通用插件' : 'Plugin packages'}</h3>
    <p className="text-xs text-neutral-500">{zh
      ? '导入 Kivio / Claude Code / Codex 格式的插件包，供内置 Kivio Agent 使用。个人范围生效；外部 CLI 使用各自的插件配置。'
      : 'Import Kivio / Claude Code / Codex packages for the built-in Kivio Agent. Packages apply across your projects; external CLIs use their own configuration.'}</p>
    <form className="flex flex-wrap gap-2" onSubmit={e => { e.preventDefault(); void act(async () => { await packageApi.import(source.trim(), subdirectory.trim() || undefined); setSource(''); setSubdirectory('') }) }}>
      <input className="kv-input min-w-48 flex-1" aria-label={zh ? '插件来源' : 'Plugin source'} value={source} disabled={busy}
        onChange={e => setSource(e.target.value)} placeholder={zh ? '本地插件目录或 HTTPS Git 仓库地址' : 'Local plugin directory or HTTPS Git repository'} />
      <input className="kv-input w-48" aria-label={zh ? '插件子目录' : 'Plugin subdirectory'} value={subdirectory} disabled={busy} onChange={e => setSubdirectory(e.target.value)} placeholder={zh ? '子目录（可选）' : 'Subdirectory (optional)'} />
      <Button type="button" disabled={busy} onClick={() => void act(async () => { const path = await open({ directory: true, multiple: false }); if (typeof path === 'string') setSource(path) })}>{zh ? '选择目录' : 'Choose folder'}</Button>
      <Button type="submit" disabled={busy || !source.trim()}>{busy ? (zh ? '处理中…' : 'Working…') : (zh ? '导入' : 'Import')}</Button>
      <Button type="button" disabled={busy} onClick={() => void act(refresh)}>{zh ? '刷新' : 'Refresh'}</Button>
    </form>
    <p className="text-xs text-neutral-500">{zh ? '导入后默认停用。启用会加载包内能力并允许执行其 Hook 脚本；包内依赖不会自动安装。' : 'Imported packages start disabled. Enabling loads their capabilities and permits hook scripts to run. Dependencies are not installed automatically.'}</p>
    {error && <p role="alert" className="text-xs text-red-500 whitespace-pre-wrap">{error}</p>}
    {!packages.length && <p className="text-xs text-neutral-500">{zh ? '尚未导入通用插件。' : 'No plugin packages imported.'}</p>}
    {packages.map(plugin => <div key={plugin.id} className="border-t border-neutral-200 pt-3 dark:border-neutral-700">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0"><strong className="text-sm">{plugin.name}</strong> <span className="text-xs text-neutral-500">{plugin.version ?? ''} · {plugin.format}</span></div>
        <div className="flex gap-2">
          <Button disabled={busy || (!plugin.enabled && plugin.diagnostics.length > 0)} onClick={() => void act(async () => { await packageApi.setEnabled(plugin.id, !plugin.enabled); await refreshSettings() })}>{plugin.enabled ? (zh ? '停用' : 'Disable') : (zh ? '启用' : 'Enable')}</Button>
          <Button disabled={busy} onClick={() => void act(async () => { await packageApi.remove(plugin.id); await refreshSettings() })}>{zh ? '移除' : 'Remove'}</Button>
        </div>
      </div>
      <p className="mt-1 text-xs text-neutral-500">{plugin.description}</p>
      <p className="mt-1 break-all text-xs text-neutral-500">{plugin.source}{plugin.revision ? ` · ${plugin.revision.slice(0, 10)}` : ''}</p>
      <p className="mt-1 text-xs">{Object.entries(plugin.components).filter(([, n]) => n > 0).map(([key, n]) => `${key}: ${n}`).join(' · ')}</p>
      {plugin.diagnostics.map((diagnostic, index) => <p key={index} className="mt-1 text-xs text-amber-600">{diagnostic}</p>)}
    </div>)}
  </section>
}
