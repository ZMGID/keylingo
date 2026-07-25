import { SettingRow, HotkeyInput, SettingsGroup } from '../components'
import type { I18n } from '../i18n'
import type { HotkeyScopeKey } from '../SettingsShell'
import type { Settings as SettingsData } from '../../api/tauri'

interface HotkeysTabProps {
  settings: SettingsData
  t: I18n
  recordingTarget: HotkeyScopeKey | null
  onToggleRecording: (target: HotkeyScopeKey) => void
  conflictMessageFor: (scope: HotkeyScopeKey) => string | undefined
  onUpdateSettings: (updates: Partial<SettingsData>) => void
  onUpdateScreenshotTranslation: (updates: Partial<SettingsData['screenshotTranslation']>) => void
  onUpdateScreenshotAnnotate: (updates: Partial<NonNullable<SettingsData['screenshotAnnotate']>>) => void
  onUpdateLens: (updates: Partial<SettingsData['lens']>) => void
}

/** 快捷键标签页。纯展示：状态与冲突计算都留在 SettingsShell。 */
export function HotkeysTab({
  settings,
  t,
  recordingTarget,
  onToggleRecording,
  conflictMessageFor,
  onUpdateSettings,
  onUpdateScreenshotTranslation,
  onUpdateScreenshotAnnotate,
  onUpdateLens,
}: HotkeysTabProps) {
  return (
    <SettingsGroup title={t.tabHotkeys} className="kv-hotkey-list">
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
      <SettingRow label={t.screenshotHotkey}>
        <HotkeyInput
          inline
          value={settings.screenshotTranslation?.hotkey ?? ''}
          placeholder="CommandOrControl+Shift+A"
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
          placeholder="CommandOrControl+Shift+T"
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
          placeholder="CommandOrControl+Shift+R"
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
          placeholder="CommandOrControl+Shift+S"
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
          placeholder="CommandOrControl+Shift+G"
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
