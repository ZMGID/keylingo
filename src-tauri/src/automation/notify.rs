//! Native system notification shared by unattended automations and chat completion.
//! Failures are logged and never interrupt the originating task.

use std::process::Command;

pub(crate) fn show(app: &tauri::AppHandle, title: &str, body: &str) {
    #[cfg(not(target_os = "windows"))]
    let _ = app;
    let title = sanitize_toast_text(title, 80);
    let body = sanitize_toast_text(body, 240);
    #[cfg(target_os = "macos")]
    macos_notify(&title, &body);
    #[cfg(target_os = "windows")]
    windows_notify(
        &app.config().identifier,
        app.config()
            .product_name
            .as_deref()
            .unwrap_or("Kivio Desktop"),
        &title,
        &body,
    );
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        eprintln!("system notification: {title}: {body}");
    }
}

/// Collapse control breaks so Windows PowerShell here-strings / AppleScript
/// literals cannot be closed by interpolated node output.
fn sanitize_toast_text(s: &str, max: usize) -> String {
    let collapsed: String = s
        .chars()
        .filter(|ch| *ch != '\0' && *ch != '\n' && *ch != '\r')
        .collect();
    truncate(&collapsed, max)
}

fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if out.chars().count() >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(target_os = "macos")]
fn macos_notify(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_escape(body),
        applescript_escape(title)
    );
    if let Err(error) = Command::new("osascript").arg("-e").arg(script).spawn() {
        eprintln!("system notification failed to start: {error}");
    }
}

#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "windows")]
fn windows_notify(app_id: &str, display_name: &str, title: &str, body: &str) {
    use crate::proc::NoConsoleWindow;
    use std::process::Stdio;

    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    );
    let script = windows_script(app_id, display_name, &xml);
    let child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .no_console_window()
        .spawn();

    match child {
        Ok(child) => {
            std::thread::spawn(move || match child.wait_with_output() {
                Ok(output) if !output.status.success() => eprintln!(
                    "system notification failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                Err(error) => eprintln!("system notification wait failed: {error}"),
                _ => {}
            });
        }
        Err(error) => eprintln!("system notification failed to start: {error}"),
    }
}

#[cfg(target_os = "windows")]
fn windows_registration_script(app_id: &str, display_name: &str) -> String {
    // A display name alone can make Show() succeed and leave history records
    // without a visible notification. Register the same AUMID as the installer,
    // including for `tauri dev`, which has no installer to register it for us.
    // Only register sender metadata; never change the user's notification settings.
    let app_id = app_id.replace('\'', "''");
    let display_name = display_name.replace('\'', "''");
    format!(
        "$ErrorActionPreference = 'Stop'; \
         $appId = '{app_id}'; \
         $appKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\\Classes\\AppUserModelId\\' + $appId); \
         try {{ \
             $appKey.SetValue('DisplayName', '{display_name}', [Microsoft.Win32.RegistryValueKind]::String); \
             $appKey.SetValue('ShowInSettings', 1, [Microsoft.Win32.RegistryValueKind]::DWord); \
         }} finally {{ $appKey.Dispose() }}; "
    )
}

#[cfg(target_os = "windows")]
fn windows_script(app_id: &str, display_name: &str, xml: &str) -> String {
    let registration = windows_registration_script(app_id, display_name);
    let toast = windows_toast_script(xml);
    format!(
        "{registration}{toast}\
         $notifier = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId); \
         $notifier.Show($toast); \
         # Query after Show: a first-time sender has no notification settings yet.\n\
         $setting = $notifier.get_Setting(); \
         if ($setting -ne 0) {{ throw ('Notifications disabled for ' + $appId + ': ' + $setting) }}"
    )
}

#[cfg(target_os = "windows")]
fn windows_toast_script(xml: &str) -> String {
    format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
         $xml = [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]::new(); \
         $xml.LoadXml(@'\n{xml}\n'@); \
         $toast = [Windows.UI.Notifications.ToastNotification, Windows.UI.Notifications, ContentType = WindowsRuntime]::new($xml); "
    )
}

#[cfg(target_os = "windows")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::sanitize_toast_text;

    #[cfg(target_os = "windows")]
    use super::windows_script;

    #[test]
    fn strips_control_breaks_from_notification_text() {
        let out = sanitize_toast_text("ok\r\n'@\nGet-Process", 240);
        assert!(!out.contains('\n'));
        assert!(!out.contains('\r'));
        assert_eq!(out, "ok'@Get-Process");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_script_activates_the_winrt_xml_type_explicitly() {
        let script = windows_script("com.zmair.kivio", "Kivio Desktop", "<toast />");
        assert!(script.contains(
            "[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]::new()"
        ));
        assert!(script.contains("$ErrorActionPreference = 'Stop'"));
    }

    #[cfg(target_os = "windows")]
    fn run_powershell(script: &str) -> String {
        use crate::proc::NoConsoleWindow;
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                script,
            ])
            .no_console_window()
            .output()
            .expect("Windows PowerShell must be available");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("UTF-8 test output")
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn registers_sender_identity_without_clobbering_existing_metadata() {
        // A unique, temporary sender key; no toast is sent by this test.
        let app_id = format!("com.zmair.kivio.test.{}", uuid::Uuid::new_v4());
        let registration = super::windows_registration_script(&app_id, "Kivio's 测试");
        let script = format!(
            "$ErrorActionPreference = 'Stop'; \
             [Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
             $testPath = 'Software\\Classes\\AppUserModelId\\{app_id}'; \
             try {{ \
                 $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($testPath); \
                 $key.SetValue('CustomActivator', 'preserved'); $key.Dispose(); \
                 {registration}\
                 $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($testPath); \
                 try {{ \
                     Write-Output $key.GetValue('DisplayName'); \
                     Write-Output $key.GetValue('ShowInSettings'); \
                     Write-Output $key.GetValue('CustomActivator'); \
                 }} finally {{ $key.Dispose() }}; \
             }} finally {{ [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($testPath, $false) }}"
        );
        let output = run_powershell(&script);
        assert_eq!(
            output.lines().collect::<Vec<_>>(),
            ["Kivio's 测试", "1", "preserved"]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn winrt_accepts_notification_xml_and_keeps_content_literal() {
        let body = sanitize_toast_text("中文 & <tag> \"quoted\"\n'@\n$(throw 'injected')", 240);
        let xml = format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text></binding></visual></toast>",
            super::xml_escape(&body)
        );
        let toast = super::windows_toast_script(&xml);
        let full_script = windows_script("com.zmair.kivio", "Kivio Desktop", &xml);
        let script = format!(
            "$ErrorActionPreference = 'Stop'; \
             [Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
             [scriptblock]::Create('{}') | Out-Null; \
             {toast}\
             Write-Output $toast.Content.DocumentElement.InnerText",
            full_script.replace('\'', "''")
        );
        assert_eq!(run_powershell(&script).trim_end(), body);
    }
}
