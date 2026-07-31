import { useMemo, useState } from 'react'
import { SettingRow, HotkeyInput, SettingsGroup } from '../components'
import { Button } from '../../components/Button'
import type { I18n } from '../i18n'
import type { HotkeyScopeKey } from '../SettingsShell'
import type { Settings as SettingsData } from '../../api/tauri'
import { DEFAULT_HOTKEYS } from '../hotkeyDefaults'
import { formatHotkey, getPlatform } from '../utils'

interface HotkeysTabProps {
  settings: SettingsData
  t: I18n
  recordingTarget: HotkeyScopeKey | null
  onToggleRecording: (target: HotkeyScopeKey) => void
  conflictMessageFor: (scope: HotkeyScopeKey) => string | undefined
  /** scope → 与之冲突的另一 scope（SettingsShell 客户端冲突检测结果） */
  hotkeyConflicts: Partial<Record<HotkeyScopeKey, HotkeyScopeKey>>
  onUpdateSettings: (updates: Partial<SettingsData>) => void
  onUpdateScreenshotTranslation: (updates: Partial<SettingsData['screenshotTranslation']>) => void
  onUpdateScreenshotAnnotate: (updates: Partial<NonNullable<SettingsData['screenshotAnnotate']>>) => void
  onUpdateLens: (updates: Partial<SettingsData['lens']>) => void
}

const ALL_SCOPES: HotkeyScopeKey[] = [
  'main',
  'chat',
  'closeChat',
  'screenshotTranslation',
  'screenshotTranslationText',
  'screenshotTranslationReplace',
  'screenshotAnnotate',
  'lens',
]

function scopeLabel(scope: HotkeyScopeKey, t: I18n): string {
  switch (scope) {
    case 'main':
      return t.tabTranslate
    case 'chat':
      return t.chatHotkeyLabel
    case 'closeChat':
      return t.closeChatHotkeyLabel
    case 'screenshotTranslation':
      return t.screenshotHotkey
    case 'screenshotTranslationText':
      return t.screenshotTextHotkey
    case 'screenshotTranslationReplace':
      return t.replaceTranslateHotkey
    case 'screenshotAnnotate':
      return t.annotateHotkeyLabel
    case 'lens':
      return t.lensTabLabel
  }
}

function hotkeyForScope(settings: SettingsData, scope: HotkeyScopeKey): string {
  switch (scope) {
    case 'main':
      return settings.hotkey || ''
    case 'chat':
      return settings.chatHotkey || ''
    case 'closeChat':
      return settings.closeChatHotkey || ''
    case 'screenshotTranslation':
      return settings.screenshotTranslation?.hotkey ?? ''
    case 'screenshotTranslationText':
      return settings.screenshotTranslation?.textHotkey ?? ''
    case 'screenshotTranslationReplace':
      return settings.screenshotTranslation?.replaceHotkey ?? ''
    case 'screenshotAnnotate':
      return settings.screenshotAnnotate?.hotkey ?? ''
    case 'lens':
      return settings.lens?.hotkey ?? ''
  }
}

/** 快捷键标签页。冲突检测与恢复默认在页内完成；状态写入仍走 SettingsShell updaters。 */
export function HotkeysTab({
  settings,
  t,
  recordingTarget,
  onToggleRecording,
  conflictMessageFor,
  hotkeyConflicts,
  onUpdateSettings,
  onUpdateScreenshotTranslation,
  onUpdateScreenshotAnnotate,
  onUpdateLens,
}: HotkeysTabProps) {
  const [checkBanner, setCheckBanner] = useState<{ ok: boolean; lines: string[] } | null>(null)
  const platform = useMemo(() => getPlatform(), [])

  const handleRestoreDefaults = () => {
    onUpdateSettings({
      hotkey: DEFAULT_HOTKEYS.hotkey,
      chatHotkey: DEFAULT_HOTKEYS.chatHotkey,
      closeChatHotkey: DEFAULT_HOTKEYS.closeChatHotkey,
    })
    onUpdateScreenshotTranslation({
      hotkey: DEFAULT_HOTKEYS.screenshotHotkey,
      textHotkey: DEFAULT_HOTKEYS.screenshotTextHotkey,
      replaceHotkey: DEFAULT_HOTKEYS.screenshotReplaceHotkey,
    })
    onUpdateScreenshotAnnotate({ hotkey: DEFAULT_HOTKEYS.screenshotAnnotateHotkey })
    onUpdateLens({ hotkey: DEFAULT_HOTKEYS.lensHotkey })
    setCheckBanner({ ok: true, lines: [t.hotkeyRestoreDone] })
  }

  const handleCheckConflicts = () => {
    const seen = new Set<string>()
    const lines: string[] = []
    for (const scope of ALL_SCOPES) {
      const partner = hotkeyConflicts[scope]
      if (!partner) continue
      const pairKey = [scope, partner].sort().join('|')
      if (seen.has(pairKey)) continue
      seen.add(pairKey)
      const raw = hotkeyForScope(settings, scope).trim()
      const pretty = formatHotkey(raw, platform).join('+') || raw
      lines.push(
        t.hotkeyCheckPair
          .replace('{a}', scopeLabel(scope, t))
          .replace('{b}', scopeLabel(partner, t))
          .replace('{hotkey}', pretty),
      )
    }
    if (lines.length === 0) {
      setCheckBanner({ ok: true, lines: [t.hotkeyCheckOk] })
    } else {
      setCheckBanner({
        ok: false,
        lines: [t.hotkeyCheckFound.replace('{count}', String(lines.length)), ...lines],
      })
    }
  }

  return (
    <SettingsGroup
      className="kv-hotkey-list"
      title={
        <div className="flex items-center justify-between gap-2">
          <span>{t.tabHotkeys}</span>
          <div className="flex shrink-0 items-center gap-1.5 normal-case tracking-normal">
            <Button
              size="sm"
              variant="ghost"
              onClick={handleCheckConflicts}
              data-tauri-drag-region="false"
            >
              {t.hotkeyCheckConflicts}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={handleRestoreDefaults}
              data-tauri-drag-region="false"
            >
              {t.hotkeyRestoreDefaults}
            </Button>
          </div>
        </div>
      }
    >
      {checkBanner && (
        <div
          className={`mb-2 rounded-lg px-3 py-2 text-[12px] leading-5 ${
            checkBanner.ok
              ? 'bg-emerald-50 text-emerald-700 dark:bg-emerald-500/10 dark:text-emerald-300'
              : 'bg-red-50 text-red-600 dark:bg-red-500/10 dark:text-red-300'
          }`}
          role="status"
        >
          {checkBanner.lines.map((line, i) => (
            <p key={i} className={i > 0 ? 'mt-0.5' : undefined}>
              {line}
            </p>
          ))}
        </div>
      )}

      <SettingRow label={t.tabTranslate}>
        <HotkeyInput
          inline
          value={settings.hotkey}
          placeholder={t.hotkeyPlaceholder}
          recording={recordingTarget === 'main'}
          onToggleRecording={() => onToggleRecording('main')}
          recordLabel={t.hotkeyRecord}
          recordingLabel={t.hotkeyRecording}
          recordingPlaceholder={t.hotkeyRecordingPlaceholder}
          onClear={() => onUpdateSettings({ hotkey: '' })}
          clearLabel={t.hotkeyClear}
          error={conflictMessageFor('main')}
        />
      </SettingRow>
      <SettingRow label={t.chatHotkeyLabel}>
        <HotkeyInput
          inline
          value={settings.chatHotkey}
          placeholder={t.hotkeyPlaceholder}
          recording={recordingTarget === 'chat'}
          onToggleRecording={() => onToggleRecording('chat')}
          recordLabel={t.hotkeyRecord}
          recordingLabel={t.hotkeyRecording}
          recordingPlaceholder={t.hotkeyRecordingPlaceholder}
          onClear={() => onUpdateSettings({ chatHotkey: '' })}
          clearLabel={t.hotkeyClear}
          error={conflictMessageFor('chat')}
        />
      </SettingRow>
      <SettingRow label={t.closeChatHotkeyLabel}>
        <HotkeyInput
          inline
          value={settings.closeChatHotkey}
          placeholder={t.hotkeyPlaceholder}
          recording={recordingTarget === 'closeChat'}
          onToggleRecording={() => onToggleRecording('closeChat')}
          recordLabel={t.hotkeyRecord}
          recordingLabel={t.hotkeyRecording}
          recordingPlaceholder={t.hotkeyRecordingPlaceholder}
          onClear={() => onUpdateSettings({ closeChatHotkey: '' })}
          clearLabel={t.hotkeyClear}
          error={conflictMessageFor('closeChat')}
        />
      </SettingRow>
      <SettingRow label={t.screenshotHotkey}>
        <HotkeyInput
          inline
          value={settings.screenshotTranslation?.hotkey ?? ''}
          placeholder={t.hotkeyPlaceholder}
          recording={recordingTarget === 'screenshotTranslation'}
          onToggleRecording={() => onToggleRecording('screenshotTranslation')}
          recordLabel={t.hotkeyRecord}
          recordingLabel={t.hotkeyRecording}
          recordingPlaceholder={t.hotkeyRecordingPlaceholder}
          onClear={() => onUpdateScreenshotTranslation({ hotkey: '' })}
          clearLabel={t.hotkeyClear}
          error={conflictMessageFor('screenshotTranslation')}
        />
      </SettingRow>
      <SettingRow label={t.screenshotTextHotkey}>
        <HotkeyInput
          inline
          value={settings.screenshotTranslation?.textHotkey ?? ''}
          placeholder={t.hotkeyPlaceholder}
          recording={recordingTarget === 'screenshotTranslationText'}
          onToggleRecording={() => onToggleRecording('screenshotTranslationText')}
          recordLabel={t.hotkeyRecord}
          recordingLabel={t.hotkeyRecording}
          recordingPlaceholder={t.hotkeyRecordingPlaceholder}
          onClear={() => onUpdateScreenshotTranslation({ textHotkey: '' })}
          clearLabel={t.hotkeyClear}
          error={conflictMessageFor('screenshotTranslationText')}
        />
      </SettingRow>
      <SettingRow label={t.replaceTranslateHotkey}>
        <HotkeyInput
          inline
          value={settings.screenshotTranslation?.replaceHotkey ?? ''}
          placeholder={t.hotkeyPlaceholder}
          recording={recordingTarget === 'screenshotTranslationReplace'}
          onToggleRecording={() => onToggleRecording('screenshotTranslationReplace')}
          recordLabel={t.hotkeyRecord}
          recordingLabel={t.hotkeyRecording}
          recordingPlaceholder={t.hotkeyRecordingPlaceholder}
          onClear={() => onUpdateScreenshotTranslation({ replaceHotkey: '' })}
          clearLabel={t.hotkeyClear}
          error={conflictMessageFor('screenshotTranslationReplace')}
        />
      </SettingRow>
      <SettingRow label={t.annotateHotkeyLabel}>
        <HotkeyInput
          inline
          value={settings.screenshotAnnotate?.hotkey ?? ''}
          placeholder={t.hotkeyPlaceholder}
          recording={recordingTarget === 'screenshotAnnotate'}
          onToggleRecording={() => onToggleRecording('screenshotAnnotate')}
          recordLabel={t.hotkeyRecord}
          recordingLabel={t.hotkeyRecording}
          recordingPlaceholder={t.hotkeyRecordingPlaceholder}
          onClear={() => onUpdateScreenshotAnnotate({ hotkey: '' })}
          clearLabel={t.hotkeyClear}
          error={conflictMessageFor('screenshotAnnotate')}
        />
      </SettingRow>
      <SettingRow label={t.lensTabLabel}>
        <HotkeyInput
          inline
          value={settings.lens?.hotkey ?? ''}
          placeholder={t.hotkeyPlaceholder}
          recording={recordingTarget === 'lens'}
          onToggleRecording={() => onToggleRecording('lens')}
          recordLabel={t.hotkeyRecord}
          recordingLabel={t.hotkeyRecording}
          recordingPlaceholder={t.hotkeyRecordingPlaceholder}
          onClear={() => onUpdateLens({ hotkey: '' })}
          clearLabel={t.hotkeyClear}
          error={conflictMessageFor('lens')}
        />
      </SettingRow>
    </SettingsGroup>
  )
}
