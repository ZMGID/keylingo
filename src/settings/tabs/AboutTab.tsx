import { RefreshCw, Download, ExternalLink } from 'lucide-react'
import { Toggle, SettingRow, SettingsGroup } from '../components'
import { Button } from '../../components/Button'
import { ChatMarkdown } from '../../chat/ChatMarkdown'
import type { I18n, Lang } from '../i18n'
import type { Settings as SettingsData, UpdateInfo } from '../../api/tauri'

/** 应用信息：图标 / 名称 / 版本 / 开发者。 */
export function AppInfoGroup({
  t,
  lang,
  appVersion,
}: {
  t: I18n
  lang: Lang
  appVersion: string
}) {
  return (
    <SettingsGroup title={lang === 'zh' ? '应用' : 'Application'}>
      <div className="kv-panel mb-2">
        <div className="flex items-center gap-3">
          <div className="w-12 h-12 rounded-[10px] overflow-hidden shrink-0">
            <img src="/icon.png" alt="Kivio Desktop" className="w-full h-full object-contain" />
          </div>
          <div className="min-w-0">
            <div className="kv-page-title">Kivio Desktop</div>
            <div className="kv-panel-body">{lang === 'zh' ? '屏幕级 AI 助手' : 'Screen-level AI Assistant'}</div>
          </div>
        </div>
      </div>
      <SettingRow label={t.currentVersion}>
        <span className="kv-tag">v{appVersion}</span>
      </SettingRow>
      <SettingRow label={lang === 'zh' ? '开发者' : 'Developer'}>
        <span className="kv-row-desc">ZM</span>
      </SettingRow>
    </SettingsGroup>
  )
}

/**
 * 更新检查的两段式状态（检查 → 下载 → 安装）。
 *
 * 打包成一个对象而不是 6 个平铺 props：它们是同一个状态机的分量，
 * 从来一起变化，拆开传只会让调用点更长而不更清晰。
 */
export interface UpdateFlowState {
  status: string
  info: UpdateInfo | null
  downloadState: string
  downloadPercent: number
  downloadError: string
}

/** 更新检查与下载安装。 */
export function UpdateGroup({
  settings,
  t,
  update,
  onUpdateSettings,
  onCheck,
  onDownloadAndInstall,
  onInstall,
  onOpenReleasePage,
  onOpenGithubReleases,
  onDismiss,
}: {
  settings: SettingsData | null
  t: I18n
  update: UpdateFlowState
  onUpdateSettings: (updates: Partial<SettingsData>) => void
  onCheck: () => void
  onDownloadAndInstall: () => void
  onInstall: () => void
  onOpenReleasePage: () => void
  onOpenGithubReleases: () => void
  onDismiss: () => void
}) {
  const { status, info, downloadState, downloadPercent, downloadError } = update

  return (
    <SettingsGroup title={t.checkUpdate}>
      <SettingRow label={t.autoCheckUpdate}>
        <Toggle
          checked={settings?.autoCheckUpdate ?? true}
          onChange={(v) => onUpdateSettings({ autoCheckUpdate: v })}
        />
      </SettingRow>
      <SettingRow
        label={t.checkUpdate}
        description={status === 'up-to-date' ? t.upToDate : undefined}
      >
        <Button
          size="sm"
          onClick={onCheck}
          disabled={status === 'checking'}
          data-tauri-drag-region="false"
        >
          <RefreshCw size={11} className={status === 'checking' ? 'animate-spin' : ''} />
          {status === 'checking' ? t.checkingUpdate : t.checkUpdate}
        </Button>
      </SettingRow>

      {status === 'check-failed' && (
        <div className="kv-panel mt-2">
          <div className="kv-panel-body mb-2 text-amber-700 dark:text-amber-400">
            {t.updateCheckFailed}
          </div>
          <Button
            size="sm"
            onClick={onOpenGithubReleases}
            data-tauri-drag-region="false"
          >
            {t.downloadFromGithub}
          </Button>
        </div>
      )}

      {status === 'available' && info && (
        <div className="kv-panel info mt-2">
          <div className="kv-panel-title">
            {t.updateAvailable}
            <span className="kv-tag accent ml-auto">v{info.version}</span>
          </div>
          {info.body && (
            <div className="custom-scrollbar mb-3 max-h-40 overflow-y-auto text-[12px] leading-relaxed">
              <ChatMarkdown content={info.body} />
            </div>
          )}

          {downloadState === 'downloading' && (
            <div className="mb-3">
              <div className="flex items-center justify-between kv-panel-body mb-1">
                <span>{t.downloading}</span>
                <span className="font-mono tabular-nums">{downloadPercent}%</span>
              </div>
              <div className="kv-progress">
                <div style={{ width: `${downloadPercent}%` }} />
              </div>
            </div>
          )}

          {downloadState === 'failed' && downloadError && (
            <div className="kv-inline-error mb-3">
              {t.downloadFailed}: {downloadError}
            </div>
          )}

          <div className="flex gap-2 flex-wrap">
            {downloadState === 'idle' && (
              <>
                <Button
                  variant="primary"
                  onClick={onDownloadAndInstall}
                  data-tauri-drag-region="false"
                >
                  <Download size={12} />
                  {t.downloadAndInstall}
                </Button>
                <Button
                  onClick={onOpenReleasePage}
                  data-tauri-drag-region="false"
                >
                  <ExternalLink size={12} />
                  {t.downloadFromGithub}
                </Button>
              </>
            )}
            {downloadState === 'downloading' && (
              <Button disabled>
                <RefreshCw size={12} className="animate-spin" />
                {t.downloading}
              </Button>
            )}
            {downloadState === 'downloaded' && (
              <Button
                variant="primary"
                onClick={onInstall}
                data-tauri-drag-region="false"
              >
                <Download size={12} />
                {t.installAndRestart}
              </Button>
            )}
            {downloadState === 'failed' && (
              <>
                <Button
                  variant="primary"
                  onClick={onDownloadAndInstall}
                  data-tauri-drag-region="false"
                >
                  <RefreshCw size={12} />
                  {t.retryDownload}
                </Button>
                <Button
                  onClick={onOpenReleasePage}
                  data-tauri-drag-region="false"
                >
                  <ExternalLink size={12} />
                  {t.downloadFromGithub}
                </Button>
              </>
            )}
            <Button
              variant="ghost"
              onClick={onDismiss}
              data-tauri-drag-region="false"
            >
              {t.updateLater}
            </Button>
          </div>
        </div>
      )}
    </SettingsGroup>
  )
}
