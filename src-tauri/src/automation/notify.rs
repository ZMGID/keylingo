//! Unattended system notification. Failures are logged; the run still continues
//! with the interpolated body as node output.

use std::process::Command;

pub(crate) fn show(title: &str, body: &str) {
    let title = sanitize_toast_text(title, 80);
    let body = sanitize_toast_text(body, 240);
    #[cfg(target_os = "macos")]
    macos_notify(&title, &body);
    #[cfg(target_os = "windows")]
    windows_notify(&title, &body);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        eprintln!("automation notify: {title}: {body}");
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
    let _ = Command::new("osascript").arg("-e").arg(script).spawn();
}

#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "windows")]
fn windows_notify(title: &str, body: &str) {
    use crate::proc::NoConsoleWindow;
    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    );
    let script = format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
         $xml = New-Object Windows.Data.Xml.Dom.XmlDocument; \
         $xml.LoadXml(@'\n{xml}\n'@); \
         $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Kivio Desktop').Show($toast)"
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .no_console_window()
        .spawn();
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

    #[test]
    fn strips_newlines_so_powershell_here_string_cannot_close() {
        let out = sanitize_toast_text("ok\r\n'@\nGet-Process", 240);
        assert!(!out.contains('\n'));
        assert!(!out.contains('\r'));
        assert_eq!(out, "ok'@Get-Process");
    }
}
