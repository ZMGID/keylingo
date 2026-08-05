//! Himalaya 邮箱连接器：把 settings.email_accounts 同步到 ~/.config/himalaya/config.toml。

use std::io::{copy, Cursor};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::proc::NoConsoleWindow;
use crate::settings::EmailAccountConfig;

const HIMALAYA_RELEASE_TAG: &str = "v1.2.0";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HimalayaStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HimalayaInstallResult {
    pub ok: bool,
    pub already_installed: bool,
    pub message: String,
}

fn kivio_himalaya_install_dir() -> Option<PathBuf> {
    crate::app_data::app_data_dir().map(|dir| dir.join("himalaya-cli"))
}

fn kivio_himalaya_binary_path() -> Option<PathBuf> {
    let dir = kivio_himalaya_install_dir()?;
    #[cfg(windows)]
    {
        let path = dir.join("himalaya.exe");
        return path.is_file().then_some(path);
    }
    #[cfg(not(windows))]
    {
        let path = dir.join("himalaya");
        return path.is_file().then_some(path);
    }
}

fn which_himalaya_on_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let output = Command::new("sh")
            .arg("-c")
            .arg("command -v himalaya")
            .no_console_window()
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
    #[cfg(windows)]
    {
        let output = Command::new("where")
            .arg("himalaya")
            .no_console_window()
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()?
            .trim()
            .to_string();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
}

pub fn resolve_himalaya_binary() -> Option<PathBuf> {
    kivio_himalaya_binary_path().or_else(which_himalaya_on_path)
}

/// Only probe/install-path Himalaya when the email connector has saved accounts.
pub fn resolve_himalaya_binary_when_active(accounts: &[EmailAccountConfig]) -> Option<PathBuf> {
    if accounts.is_empty() {
        None
    } else {
        resolve_himalaya_binary()
    }
}

/// Returns `(PATH env key, merged value)` only when email accounts are configured.
pub fn kivio_himalaya_path_env_when_active(
    accounts: &[EmailAccountConfig],
) -> Option<(String, std::ffi::OsString)> {
    if accounts.is_empty() {
        None
    } else {
        kivio_himalaya_path_env()
    }
}

/// If Kivio manages a himalaya install, return its directory (for PATH prepend).
pub fn kivio_himalaya_bin_dir() -> Option<PathBuf> {
    kivio_himalaya_binary_path().and_then(|path| path.parent().map(Path::to_path_buf))
}

/// Returns `(PATH env key, merged value)` when Kivio manages a himalaya install.
pub fn kivio_himalaya_path_env() -> Option<(String, std::ffi::OsString)> {
    let dir = kivio_himalaya_bin_dir()?;
    Some((shell_path_env_key(), merged_shell_path_with_dir(&dir)))
}

fn shell_path_env_key() -> String {
    #[cfg(windows)]
    {
        "Path".to_string()
    }
    #[cfg(not(windows))]
    {
        "PATH".to_string()
    }
}

fn merged_shell_path_with_dir(dir: &Path) -> std::ffi::OsString {
    let dir = dir.to_string_lossy();
    #[cfg(windows)]
    {
        let existing = std::env::var_os("Path").unwrap_or_default();
        let mut next = std::ffi::OsString::from(dir.as_ref());
        next.push(";");
        next.push(existing);
        next
    }
    #[cfg(not(windows))]
    {
        let existing = std::env::var_os("PATH").unwrap_or_default();
        let mut next = std::ffi::OsString::from(dir.as_ref());
        next.push(":");
        next.push(existing);
        next
    }
}

pub fn himalaya_status() -> HimalayaStatus {
    let Some(binary) = resolve_himalaya_binary() else {
        return HimalayaStatus {
            installed: false,
            version: None,
            path: None,
        };
    };
    let output = Command::new(&binary)
        .arg("--version")
        .no_console_window()
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout)
                .trim()
                .lines()
                .next()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    String::from_utf8_lossy(&out.stderr)
                        .trim()
                        .lines()
                        .next()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(str::to_string)
                });
            HimalayaStatus {
                installed: true,
                version,
                path: Some(binary.display().to_string()),
            }
        }
        _ => HimalayaStatus {
            installed: false,
            version: None,
            path: Some(binary.display().to_string()),
        },
    }
}

fn release_asset_name() -> Result<&'static str, String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return Ok("himalaya.aarch64-darwin.tgz");
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return Ok("himalaya.x86_64-darwin.tgz");
    }
    #[cfg(target_os = "windows")]
    {
        return Ok("himalaya.x86_64-windows.zip");
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        return Ok("himalaya.x86_64-linux.tgz");
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        return Ok("himalaya.aarch64-linux.tgz");
    }
    #[allow(unreachable_code)]
    Err("Himalaya auto-install is not supported on this platform.".to_string())
}

fn release_asset_url(asset_name: &str) -> String {
    format!(
        "https://github.com/pimalaya/himalaya/releases/download/{HIMALAYA_RELEASE_TAG}/{asset_name}"
    )
}

fn extract_himalaya_from_tgz(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let reader = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(reader);
    for entry in archive
        .entries()
        .map_err(|err| format!("read tgz: {err}"))?
    {
        let mut entry = entry.map_err(|err| format!("read tgz entry: {err}"))?;
        let path = entry
            .path()
            .map_err(|err| format!("read tgz path: {err}"))?
            .to_path_buf();
        let is_binary = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "himalaya" || name == "himalaya.exe");
        if !is_binary {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("create dir: {err}"))?;
        }
        entry
            .unpack(dest)
            .map_err(|err| format!("unpack himalaya: {err}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))
                .map_err(|err| format!("chmod himalaya: {err}"))?;
        }
        return Ok(());
    }
    Err("himalaya binary not found in archive".to_string())
}

fn extract_himalaya_from_zip(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let reader = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|err| format!("open zip: {err}"))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| format!("read zip entry: {err}"))?;
        let name = file.name().replace('\\', "/");
        if !name.ends_with("himalaya.exe") {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("create dir: {err}"))?;
        }
        let mut out = std::fs::File::create(dest).map_err(|err| format!("create binary: {err}"))?;
        copy(&mut file, &mut out).map_err(|err| format!("write binary: {err}"))?;
        return Ok(());
    }
    Err("himalaya.exe not found in archive".to_string())
}

pub async fn install_himalaya(http: &reqwest::Client) -> Result<HimalayaInstallResult, String> {
    if himalaya_status().installed {
        return Ok(HimalayaInstallResult {
            ok: true,
            already_installed: true,
            message: "Himalaya is already installed.".to_string(),
        });
    }

    let install_dir =
        kivio_himalaya_install_dir().ok_or_else(|| "app data directory unavailable".to_string())?;
    std::fs::create_dir_all(&install_dir).map_err(|err| format!("create install dir: {err}"))?;

    let asset_name = release_asset_name()?;
    let url = release_asset_url(asset_name);
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("download {url}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("download {url}: HTTP {}", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("read download body: {err}"))?;

    #[cfg(windows)]
    let dest = install_dir.join("himalaya.exe");
    #[cfg(not(windows))]
    let dest = install_dir.join("himalaya");

    if asset_name.ends_with(".zip") {
        extract_himalaya_from_zip(&bytes, &dest)?;
    } else {
        extract_himalaya_from_tgz(&bytes, &dest)?;
    }

    if !dest.is_file() {
        return Err("install finished but binary is missing".to_string());
    }

    Ok(HimalayaInstallResult {
        ok: true,
        already_installed: false,
        message: format!("Installed Himalaya to {}", dest.display()),
    })
}

#[tauri::command]
pub fn himalaya_status_cmd() -> HimalayaStatus {
    himalaya_status()
}

#[tauri::command]
pub async fn himalaya_install_cmd(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<HimalayaInstallResult, String> {
    install_himalaya(&state.http).await
}

fn himalaya_config_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| {
        dirs.home_dir()
            .join(".config")
            .join("himalaya")
            .join("config.toml")
    })
}

fn escape_toml_basic(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn render_himalaya_config_toml(accounts: &[EmailAccountConfig]) -> String {
    let mut out = String::from(
        "# Generated by Kivio — do not edit manually if you use Kivio email connector.\n\n",
    );
    for account in accounts {
        if account.email.trim().is_empty() {
            continue;
        }
        let id = if account.id.trim().is_empty() {
            crate::settings::email_account_id_from_address(&account.email)
        } else {
            account.id.trim().to_string()
        };
        out.push_str(&format!("[accounts.{id}]\n"));
        out.push_str(&format!(
            "email = {}\n",
            escape_toml_basic(account.email.trim())
        ));
        out.push_str(&format!(
            "display-name = {}\n",
            escape_toml_basic(if account.display_name.trim().is_empty() {
                account.email.trim()
            } else {
                account.display_name.trim()
            })
        ));
        if account.is_default {
            out.push_str("default = true\n");
        }
        out.push_str("backend.type = \"imap\"\n");
        out.push_str(&format!(
            "backend.host = {}\n",
            escape_toml_basic(account.imap_host.trim())
        ));
        out.push_str(&format!("backend.port = {}\n", account.imap_port));
        out.push_str(&format!(
            "backend.encryption.type = {}\n",
            escape_toml_basic(account.imap_encryption.trim())
        ));
        out.push_str(&format!(
            "backend.login = {}\n",
            escape_toml_basic(account.email.trim())
        ));
        out.push_str("backend.auth.type = \"password\"\n");
        out.push_str(&format!(
            "backend.auth.raw = {}\n",
            escape_toml_basic(account.password.as_str())
        ));
        out.push_str("message.send.backend.type = \"smtp\"\n");
        out.push_str(&format!(
            "message.send.backend.host = {}\n",
            escape_toml_basic(account.smtp_host.trim())
        ));
        out.push_str(&format!(
            "message.send.backend.port = {}\n",
            account.smtp_port
        ));
        out.push_str(&format!(
            "message.send.backend.encryption.type = {}\n",
            escape_toml_basic(account.smtp_encryption.trim())
        ));
        out.push_str(&format!(
            "message.send.backend.login = {}\n",
            escape_toml_basic(account.email.trim())
        ));
        out.push_str("message.send.backend.auth.type = \"password\"\n");
        out.push_str(&format!(
            "message.send.backend.auth.raw = {}\n\n",
            escape_toml_basic(account.password.as_str())
        ));
    }
    out
}

pub fn sync_himalaya_config(accounts: &[EmailAccountConfig]) -> Result<(), String> {
    let path = himalaya_config_path().ok_or_else(|| "home directory unavailable".to_string())?;
    if accounts.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|err| format!("remove {}: {err}", path.display()))?;
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let body = render_himalaya_config_toml(accounts);
    std::fs::write(&path, body).map_err(|err| format!("write {}: {err}", path.display()))
}

fn run_himalaya(args: &[&str]) -> Result<String, String> {
    let binary = resolve_himalaya_binary().ok_or_else(|| {
        "Himalaya is not installed — open the Email connector and tap Install.".to_string()
    })?;
    let output = Command::new(&binary)
        .args(args)
        .no_console_window()
        .output()
        .map_err(|err| format!("failed to run himalaya: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(if stdout.is_empty() { stderr } else { stdout })
    } else {
        Err(if stderr.is_empty() {
            if stdout.is_empty() {
                format!("himalaya exited with {}", output.status)
            } else {
                stdout
            }
        } else {
            stderr
        })
    }
}

pub fn test_himalaya_account(
    account: &EmailAccountConfig,
    all_accounts: &[EmailAccountConfig],
) -> Result<String, String> {
    let mut accounts: Vec<EmailAccountConfig> = all_accounts.to_vec();
    let id = if account.id.trim().is_empty() {
        crate::settings::email_account_id_from_address(&account.email)
    } else {
        account.id.trim().to_string()
    };
    if let Some(existing) = accounts.iter_mut().find(|item| item.id == id) {
        *existing = account.clone();
    } else {
        accounts.push(account.clone());
    }
    sync_himalaya_config(&accounts)?;
    run_himalaya(&["folder", "list", "--account", &id])
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailProviderPreset {
    pub id: String,
    pub label: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_encryption: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_encryption: String,
}

#[tauri::command]
pub fn list_email_provider_presets() -> Vec<EmailProviderPreset> {
    vec![
        EmailProviderPreset {
            id: "gmail".into(),
            label: "Gmail".into(),
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            imap_encryption: "tls".into(),
            smtp_host: "smtp.gmail.com".into(),
            smtp_port: 587,
            smtp_encryption: "start-tls".into(),
        },
        EmailProviderPreset {
            id: "outlook".into(),
            label: "Outlook / Microsoft 365".into(),
            imap_host: "outlook.office365.com".into(),
            imap_port: 993,
            imap_encryption: "tls".into(),
            smtp_host: "smtp-mail.outlook.com".into(),
            smtp_port: 587,
            smtp_encryption: "start-tls".into(),
        },
        EmailProviderPreset {
            id: "icloud".into(),
            label: "iCloud".into(),
            imap_host: "imap.mail.me.com".into(),
            imap_port: 993,
            imap_encryption: "tls".into(),
            smtp_host: "smtp.mail.me.com".into(),
            smtp_port: 587,
            smtp_encryption: "start-tls".into(),
        },
        EmailProviderPreset {
            id: "qq".into(),
            label: "QQ Mail".into(),
            imap_host: "imap.qq.com".into(),
            imap_port: 993,
            imap_encryption: "tls".into(),
            smtp_host: "smtp.qq.com".into(),
            smtp_port: 587,
            smtp_encryption: "start-tls".into(),
        },
        EmailProviderPreset {
            id: "custom".into(),
            label: "Custom".into(),
            imap_host: String::new(),
            imap_port: 993,
            imap_encryption: "tls".into(),
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_encryption: "start-tls".into(),
        },
    ]
}

#[tauri::command]
pub fn test_himalaya_email_cmd(
    account: EmailAccountConfig,
    existing_accounts: Option<Vec<EmailAccountConfig>>,
) -> Result<String, String> {
    let mut merged = existing_accounts.unwrap_or_default();
    merged.push(account);
    let settings = crate::settings::sanitize_settings(crate::settings::Settings {
        email_accounts: merged,
        ..Default::default()
    });
    let tested = settings
        .email_accounts
        .last()
        .cloned()
        .ok_or_else(|| "email is required".to_string())?;
    test_himalaya_account(&tested, &settings.email_accounts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_config_includes_imap_and_smtp() {
        let account = EmailAccountConfig {
            id: "work".into(),
            email: "user@example.com".into(),
            display_name: "User".into(),
            password: "secret".into(),
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            imap_encryption: "tls".into(),
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            smtp_encryption: "start-tls".into(),
            is_default: true,
        };
        let toml = render_himalaya_config_toml(&[account]);
        assert!(toml.contains("[accounts.work]"));
        assert!(toml.contains("backend.host = \"imap.example.com\""));
        assert!(toml.contains("message.send.backend.host = \"smtp.example.com\""));
        assert!(toml.contains("backend.auth.raw = \"secret\""));
    }

    #[test]
    fn escape_toml_quotes_password() {
        let escaped = escape_toml_basic("pa\"ss");
        assert_eq!(escaped, "\"pa\\\"ss\"");
    }

    #[test]
    fn render_config_empty_accounts_has_no_account_sections() {
        let toml = render_himalaya_config_toml(&[]);
        assert!(!toml.contains("[accounts."));
        assert!(toml.contains("Generated by Kivio"));
    }

    #[test]
    fn merged_shell_path_prepends_directory() {
        let merged = merged_shell_path_with_dir(Path::new("/kivio/himalaya-cli"));
        let merged = merged.to_string_lossy();
        assert!(merged.starts_with("/kivio/himalaya-cli"));
    }
}
