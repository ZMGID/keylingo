//! Windows → WSL 桥：探测并拉起装在 WSL 发行版里的 CLI。
//!
//! Kivio 是 Win32 GUI，`where.exe` 只扫 Windows PATH。很多人把 claude / codex 装在
//! Ubuntu 里；Windows 侧要么完全看不见，要么只看见 `appendWindowsPath` 带过来的 `.cmd`
//! 垫片（能 `--version`，登录态却在 Linux `$HOME`）。
//!
//! 策略（保守：不抢已经能用的 Windows 原生安装）：
//! 1. Windows PATH 上有能拉起来的 Win32 候选，仍优先用它。
//! 2. Windows 一个都没有时，再问默认 / 正在运行的 WSL 发行版。
//! 3. 定位时丢掉 `/mnt/<盘符>/...`，避免再次命中 Windows 垫片。
//! 4. 命中结果编成 `\\wsl$\<distro>\<linux-path>`；`cli_command` 认出 UNC 后改走
//!    `wsl.exe -d <distro> -e /bin/bash -c ...`。
//! 5. 自定义路径可填该 UNC，或填 Linux 绝对路径（套到默认发行版）。
//!
//! 双装时 Windows 优先；要强制用 WSL，把自定义路径指到 UNC。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use tokio::process::Command;

#[cfg(windows)]
use crate::external_agents::registry::AGENT_DEFS;
#[cfg(windows)]
use crate::proc::NoConsoleWindow;
#[cfg(windows)]
use std::process::Stdio;
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use tokio::time::timeout;

#[cfg(windows)]
const LIST_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(windows)]
const LOCATE_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslTarget {
    pub distro: String,
    pub linux_path: String,
}

#[derive(Debug, Clone, Default)]
struct LocatorSnapshot {
    bins: HashMap<String, WslTarget>,
    default_distro: Option<String>,
}

#[derive(Debug, Clone)]
struct Distro {
    name: String,
    is_default: bool,
    running: bool,
}

static LOCATOR: LazyLock<Mutex<Option<LocatorSnapshot>>> = LazyLock::new(|| Mutex::new(None));

#[cfg(windows)]
const LOCATE_SCRIPT: &str = r#"
export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh" >/dev/null 2>&1
[ -s "$HOME/.cargo/env" ] && . "$HOME/.cargo/env" >/dev/null 2>&1
[ -s "$HOME/.asdf/asdf.sh" ] && . "$HOME/.asdf/asdf.sh" >/dev/null 2>&1
if command -v fnm >/dev/null 2>&1; then eval "$(fnm env --shell bash 2>/dev/null)" || true; fi
PATH="$HOME/.local/bin:$HOME/.npm-global/bin:$HOME/.volta/bin:$HOME/.bun/bin:$PATH"
PATH=$(printf '%s' "$PATH" | tr ':' '\n' | grep -vE '^/mnt/[a-zA-Z](/|$)' | grep -v '^$' | paste -sd:)
export PATH
for name in "$@"; do
  p=$(command -v -- "$name" 2>/dev/null) || continue
  case "$p" in
    /mnt/[a-zA-Z]/*) continue ;;
    *.cmd|*.exe|*.bat) continue ;;
  esac
  printf '%s\t%s\n' "$name" "$p"
done
"#;

pub fn parse_wsl_target(path: impl AsRef<Path>) -> Option<WslTarget> {
    parse_wsl_unc(&path.as_ref().to_string_lossy())
}

pub fn is_wsl_target(path: impl AsRef<Path>) -> bool {
    parse_wsl_target(path).is_some()
}

pub fn parse_wsl_unc(raw: &str) -> Option<WslTarget> {
    let normalized = raw.trim().replace('/', "\\");
    let rest = normalized
        .strip_prefix(r"\\wsl$\")
        .or_else(|| normalized.strip_prefix(r"\\wsl.localhost\"))?;
    let (distro, linux_rel) = rest.split_once('\\')?;
    if distro.is_empty() || linux_rel.is_empty() {
        return None;
    }
    Some(WslTarget {
        distro: distro.to_string(),
        linux_path: format!("/{}", linux_rel.replace('\\', "/")),
    })
}

pub fn to_unc(distro: &str, linux_path: &str) -> PathBuf {
    let rel = linux_path
        .trim()
        .strip_prefix('/')
        .unwrap_or(linux_path.trim())
        .replace('/', "\\");
    PathBuf::from(format!(r"\\wsl$\{distro}\{rel}"))
}

pub fn host_path_to_wsl(path: &Path) -> Option<String> {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(raw.as_ref());
    if let Some(target) = parse_wsl_unc(stripped) {
        return Some(target.linux_path);
    }
    if let Some(unc) = stripped.strip_prefix(r"UNC\") {
        if let Some(target) = parse_wsl_unc(&format!(r"\\{unc}")) {
            return Some(target.linux_path);
        }
    }
    let s = stripped.replace('/', "\\");
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = s.get(2..).unwrap_or("");
        let rest = rest.trim_start_matches('\\').replace('\\', "/");
        return Some(if rest.is_empty() {
            format!("/mnt/{drive}")
        } else {
            format!("/mnt/{drive}/{rest}")
        });
    }
    let slash = s.replace('\\', "/");
    if slash.starts_with('/') {
        return Some(slash);
    }
    None
}

pub fn path_for_cli(cli_bin: &Path, host_path: &Path) -> PathBuf {
    if parse_wsl_target(cli_bin).is_none() {
        return host_path.to_path_buf();
    }
    host_path_to_wsl(host_path)
        .map(PathBuf::from)
        .unwrap_or_else(|| host_path.to_path_buf())
}

pub fn is_windows_drive_mount(linux_path: &str) -> bool {
    let Some(rest) = linux_path.strip_prefix("/mnt/") else {
        return false;
    };
    let mut chars = rest.chars();
    let Some(drive) = chars.next() else {
        return false;
    };
    if !drive.is_ascii_alphabetic() {
        return false;
    }
    matches!(chars.next(), None | Some('/'))
}

/// `wsl.exe -d Distro -e /bin/bash -c 'PATH=dirname:$PATH; exec "$0" "$@"' <linux_path>`。
///
/// 调用方再 `.args(cli_args)` 接在 linux_path 后面，变成 bash 的 `"$@"`。
/// 把二进制目录前置进 PATH：nvm 的 shebang 是 `#!/usr/bin/env node`，同目录才有 Linux node。
pub fn wsl_exec_command(target: &WslTarget) -> Command {
    let mut command = Command::new("wsl.exe");
    command
        .arg("-d")
        .arg(&target.distro)
        .arg("-e")
        .arg("/bin/bash")
        .arg("-c")
        .arg(r#"export PATH="$(dirname "$0"):$PATH"; exec "$0" "$@""#)
        .arg(&target.linux_path);
    command.env("WSL_UTF8", "1");
    command
}

pub fn invalidate_locator_cache() {
    if let Ok(mut guard) = LOCATOR.lock() {
        *guard = None;
    }
}

pub async fn normalize_custom_path(path: PathBuf) -> PathBuf {
    #[cfg(not(windows))]
    {
        path
    }
    #[cfg(windows)]
    {
        if parse_wsl_target(&path).is_some() {
            return path;
        }
        let text = path.to_string_lossy();
        if text.starts_with('/') && !text.starts_with("//") {
            if let Some(distro) = cached_or_list_default_distro().await {
                return to_unc(&distro, &text);
            }
        }
        path
    }
}

pub async fn linux_bin_unc(bin_name: &str) -> Option<PathBuf> {
    #[cfg(not(windows))]
    {
        let _ = bin_name;
        return None;
    }
    #[cfg(windows)]
    {
        let snapshot = ensure_snapshot().await;
        snapshot
            .bins
            .get(bin_name)
            .map(|target| to_unc(&target.distro, &target.linux_path))
    }
}

#[cfg(windows)]
async fn cached_or_list_default_distro() -> Option<String> {
    if let Ok(guard) = LOCATOR.lock() {
        if let Some(snapshot) = guard.as_ref() {
            return snapshot.default_distro.clone();
        }
    }
    ensure_snapshot().await.default_distro
}

#[cfg(windows)]
async fn ensure_snapshot() -> LocatorSnapshot {
    if let Ok(guard) = LOCATOR.lock() {
        if let Some(snapshot) = guard.as_ref() {
            return snapshot.clone();
        }
    }
    let snapshot = locate_bins().await;
    if let Ok(mut guard) = LOCATOR.lock() {
        if let Some(existing) = guard.as_ref() {
            return existing.clone();
        }
        *guard = Some(snapshot.clone());
    }
    snapshot
}

#[cfg(windows)]
async fn locate_bins() -> LocatorSnapshot {
    let distros = list_distros().await;
    let default_distro = distros
        .iter()
        .find(|d| d.is_default)
        .or_else(|| distros.first())
        .map(|d| d.name.clone());
    let names = cli_bin_names();
    let mut bins = HashMap::new();
    for distro in &distros {
        for (name, linux_path) in locate_in_distro(&distro.name, &names).await {
            bins.entry(name).or_insert(WslTarget {
                distro: distro.name.clone(),
                linux_path,
            });
        }
        if names.iter().all(|name| bins.contains_key(*name)) {
            break;
        }
    }
    LocatorSnapshot {
        bins,
        default_distro,
    }
}

fn is_ignored_distro(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("docker") || lower.contains("podman") || lower.contains("rancher")
}

pub fn parse_distro_list(text: &str) -> Vec<(String, bool, bool)> {
    parse_distro_list_ranked(text)
        .into_iter()
        .map(|d| (d.name, d.is_default, d.running))
        .collect()
}

fn parse_distro_list_ranked(text: &str) -> Vec<Distro> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let upper = trimmed.to_ascii_uppercase();
        if upper.contains("NAME") && upper.contains("STATE") {
            continue;
        }
        let is_default = trimmed.starts_with('*');
        let rest = trimmed.trim_start_matches('*').trim();
        let mut parts = rest.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        if is_ignored_distro(name) {
            continue;
        }
        let state = parts.next().unwrap_or("");
        // `wsl -l -v` 失败时 stdout 仍可能是 UTF-16 报错（本机实测
        // `Wsl/CallMsi/Install/REGDB_E_CLASSNOTREG`）。没有合法 STATE 的行
        // 一律丢掉，否则会把报错词当成发行版名再去 `wsl -d …` 空转。
        if !is_wsl_distro_state(state) {
            continue;
        }
        let running = state.eq_ignore_ascii_case("Running");
        rows.push(Distro {
            name: name.to_string(),
            is_default,
            running,
        });
    }
    rows.sort_by_key(|d| (!d.is_default, !d.running));
    rows
}

fn is_wsl_distro_state(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "running" | "stopped" | "installing" | "uninstalling" | "converting"
    )
}

/// `wsl.exe` 自己的失败（没装组件 / MSI / 发行版不存在），不是 CLI 的 `--version`。
pub fn wsl_stdout_is_system_error(bytes: &[u8]) -> bool {
    let text = decode_wsl_output(bytes);
    let upper = text.to_ascii_uppercase();
    upper.contains("WSL/") || upper.contains("REGDB_E_") || upper.contains("HRESULT")
}

pub fn decode_wsl_output(bytes: &[u8]) -> String {
    let bytes = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else {
        bytes
    };
    let sample = bytes.len().min(40);
    let utf16_le_hits = bytes[..sample]
        .chunks(2)
        .filter(|chunk| chunk.len() == 2 && chunk[1] == 0 && chunk[0] != 0)
        .count();
    if sample >= 8 && utf16_le_hits >= 4 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

pub fn parse_locate_output(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, path)) = line.split_once('\t') else {
            continue;
        };
        let name = name.trim();
        let path = path.trim();
        if name.is_empty() || !path.starts_with('/') || is_windows_drive_mount(path) {
            continue;
        }
        if path.ends_with(".cmd") || path.ends_with(".exe") || path.ends_with(".bat") {
            continue;
        }
        out.push((name.to_string(), path.to_string()));
    }
    out
}

#[cfg(windows)]
fn cli_bin_names() -> Vec<&'static str> {
    let mut names = Vec::new();
    for def in AGENT_DEFS {
        for name in std::iter::once(def.bin).chain(def.fallback_bins.iter().copied()) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

#[cfg(windows)]
async fn list_distros() -> Vec<Distro> {
    let Some(bytes) = wsl_stdout(&["-l", "-v"], LIST_TIMEOUT).await else {
        return Vec::new();
    };
    parse_distro_list_ranked(&decode_wsl_output(&bytes))
}

#[cfg(windows)]
async fn locate_in_distro(distro: &str, names: &[&str]) -> Vec<(String, String)> {
    if names.is_empty() {
        return Vec::new();
    }
    let mut args: Vec<String> = vec![
        "-d".into(),
        distro.into(),
        "-e".into(),
        "/bin/bash".into(),
        "-c".into(),
        LOCATE_SCRIPT.to_string(),
        "_".into(),
    ];
    args.extend(names.iter().map(|name| (*name).to_string()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some(bytes) = wsl_stdout(&arg_refs, LOCATE_TIMEOUT).await else {
        return Vec::new();
    };
    parse_locate_output(&decode_wsl_output(&bytes))
}

#[cfg(windows)]
async fn wsl_stdout(args: &[&str], limit: Duration) -> Option<Vec<u8>> {
    let mut command = Command::new("wsl.exe");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .no_console_window()
        .kill_on_drop(true);
    let child = command.spawn().ok()?;
    let output = timeout(limit, child.wait_with_output()).await.ok()?.ok()?;
    if !output.status.success() || wsl_stdout_is_system_error(&output.stdout) {
        return None;
    }
    Some(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unc_wsl_admin_share() {
        let t = parse_wsl_unc(r"\\wsl$\Ubuntu\home\me\.nvm\versions\node\v22.14.0\bin\claude")
            .expect("unc");
        assert_eq!(t.distro, "Ubuntu");
        assert_eq!(
            t.linux_path,
            "/home/me/.nvm/versions/node/v22.14.0/bin/claude"
        );
    }

    #[test]
    fn parse_unc_wsl_localhost_and_forward_slashes() {
        let t = parse_wsl_unc(r"\\wsl.localhost\Ubuntu-24.04\usr\bin\codex").expect("localhost");
        assert_eq!(t.distro, "Ubuntu-24.04");
        assert_eq!(t.linux_path, "/usr/bin/codex");
        let t = parse_wsl_unc("//wsl$/Debian/usr/local/bin/pi").expect("slash");
        assert_eq!(t.distro, "Debian");
        assert_eq!(t.linux_path, "/usr/local/bin/pi");
    }

    #[test]
    fn parse_unc_rejects_ordinary_windows_and_network_paths() {
        assert!(parse_wsl_unc(r"C:\Users\me\AppData\Roaming\npm\claude.cmd").is_none());
        assert!(parse_wsl_unc(r"\\server\share\claude").is_none());
        assert!(parse_wsl_unc(r"\\wsl$\Ubuntu").is_none());
    }

    #[test]
    fn to_unc_round_trips_with_parse() {
        let unc = to_unc("Ubuntu", "/usr/bin/claude");
        let t = parse_wsl_target(&unc).expect("roundtrip");
        assert_eq!(t.distro, "Ubuntu");
        assert_eq!(t.linux_path, "/usr/bin/claude");
    }

    #[test]
    fn host_drive_letter_becomes_mnt() {
        assert_eq!(
            host_path_to_wsl(Path::new(r"C:\Users\me\proj")),
            Some("/mnt/c/Users/me/proj".into())
        );
        assert_eq!(
            host_path_to_wsl(Path::new(r"\\?\E:\ZM database\kivioC")),
            Some("/mnt/e/ZM database/kivioC".into())
        );
        assert_eq!(
            host_path_to_wsl(Path::new(r"E:\ZM database\kivioC")),
            Some("/mnt/e/ZM database/kivioC".into())
        );
        assert_eq!(host_path_to_wsl(Path::new(r"D:\")), Some("/mnt/d".into()));
    }

    #[test]
    fn host_wsl_unc_becomes_linux_path() {
        assert_eq!(
            host_path_to_wsl(Path::new(r"\\wsl$\Ubuntu\home\me\work")),
            Some("/home/me/work".into())
        );
    }

    #[test]
    fn path_for_cli_only_translates_when_bin_is_wsl() {
        let wsl_bin = PathBuf::from(r"\\wsl$\Ubuntu\usr\bin\claude");
        let win_bin = PathBuf::from(r"C:\npm\claude.cmd");
        let host = Path::new(r"E:\proj");
        assert_eq!(path_for_cli(&wsl_bin, host), PathBuf::from("/mnt/e/proj"));
        assert_eq!(path_for_cli(&win_bin, host), host.to_path_buf());
    }

    #[test]
    fn windows_drive_mount_detection() {
        assert!(is_windows_drive_mount("/mnt/c/Users/me/claude.cmd"));
        assert!(is_windows_drive_mount("/mnt/c"));
        assert!(!is_windows_drive_mount("/mnt/wslg/runtime"));
        assert!(!is_windows_drive_mount("/home/me/.nvm/bin/claude"));
        assert!(!is_windows_drive_mount("/usr/bin/claude"));
    }

    #[test]
    fn locate_output_drops_windows_mounts_and_cmd_shims() {
        let parsed = parse_locate_output(
            "\
claude\t/home/me/.nvm/versions/node/v22/bin/claude
codex\t/mnt/c/Users/me/AppData/Roaming/npm/codex
pi\t/usr/bin/pi
gemini\t/mnt/c/Program Files/gemini.exe
",
        );
        assert_eq!(
            parsed,
            vec![
                (
                    "claude".into(),
                    "/home/me/.nvm/versions/node/v22/bin/claude".into()
                ),
                ("pi".into(), "/usr/bin/pi".into()),
            ]
        );
    }

    #[test]
    fn distro_list_skips_docker_and_ranks_default_running_first() {
        let text = "\
  NAME              STATE           VERSION
* Ubuntu-24.04      Stopped         2
  Debian            Running         2
  docker-desktop    Running         2
  podman-machine    Stopped         2
";
        let rows = parse_distro_list(text);
        let names: Vec<&str> = rows.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Ubuntu-24.04", "Debian"]);
        assert!(rows[0].1, "default first");
        assert!(rows[1].2, "running debian next");
    }

    #[test]
    fn decode_utf16le_wsl_list() {
        let mut bytes = Vec::new();
        for unit in "Ubuntu\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_wsl_output(&bytes).trim(), "Ubuntu");
    }

    #[test]
    fn decode_utf8_passthrough() {
        assert_eq!(decode_wsl_output(b"Debian\n").trim(), "Debian");
    }

    /// 本机 `wsl.exe -l -v` 在未安装组件时的真实 stdout（UTF-16LE，无 BOM，110 字节）。
    /// 高位非零的汉字 + 后面的 `Wsl/CallMsi/...` 必须被认成 UTF-16，且不能解析出发行版。
    fn captured_wsl_not_installed_stdout() -> Vec<u8> {
        vec![
            0xA1, 0x6C, 0x09, 0x67, 0xE8, 0x6C, 0x8C, 0x51, 0x7B, 0x7C, 0x20, 0x00, 0x0D, 0x00,
            0x0A, 0x00, 0x19, 0x95, 0xEF, 0x8B, 0xE3, 0x4E, 0x01, 0x78, 0x3A, 0x00, 0x20, 0x00,
            0x57, 0x00, 0x73, 0x00, 0x6C, 0x00, 0x2F, 0x00, 0x43, 0x00, 0x61, 0x00, 0x6C, 0x00,
            0x6C, 0x00, 0x4D, 0x00, 0x73, 0x00, 0x69, 0x00, 0x2F, 0x00, 0x49, 0x00, 0x6E, 0x00,
            0x73, 0x00, 0x74, 0x00, 0x61, 0x00, 0x6C, 0x00, 0x6C, 0x00, 0x2F, 0x00, 0x52, 0x00,
            0x45, 0x00, 0x47, 0x00, 0x44, 0x00, 0x42, 0x00, 0x5F, 0x00, 0x45, 0x00, 0x5F, 0x00,
            0x43, 0x00, 0x4C, 0x00, 0x41, 0x00, 0x53, 0x00, 0x53, 0x00, 0x4E, 0x00, 0x4F, 0x00,
            0x54, 0x00, 0x52, 0x00, 0x45, 0x00, 0x47, 0x00, 0x0D, 0x00, 0x0A, 0x00,
        ]
    }

    #[test]
    fn captured_uninstalled_wsl_error_is_not_a_distro() {
        let bytes = captured_wsl_not_installed_stdout();
        assert!(wsl_stdout_is_system_error(&bytes));
        let text = decode_wsl_output(&bytes);
        assert!(
            text.to_ascii_uppercase().contains("WSL/CALLMSI"),
            "decoded={text:?}"
        );
        assert!(
            parse_distro_list(&text).is_empty(),
            "error text parsed as distros: {text:?}"
        );
    }

    #[test]
    fn distro_list_ignores_lines_without_a_known_state() {
        let text = "\
没有注册类
错误代码: Wsl/CallMsi/Install/REGDB_E_CLASSNOTREG
  NAME              STATE           VERSION
* Ubuntu-24.04      Stopped         2
";
        let rows = parse_distro_list(text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "Ubuntu-24.04");
    }

    #[test]
    fn wsl_exec_command_targets_the_linux_binary() {
        let target = parse_wsl_unc(r"\\wsl$\Ubuntu\usr\bin\claude").expect("unc");
        let cmd = wsl_exec_command(&target);
        let std = cmd.as_std();
        assert_eq!(std.get_program(), "wsl.exe");
        let args: Vec<String> = std
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "-d");
        assert_eq!(args[1], "Ubuntu");
        assert_eq!(args[2], "-e");
        assert_eq!(args[3], "/bin/bash");
        assert_eq!(args[4], "-c");
        assert!(args[5].contains("dirname"), "{args:?}");
        assert_eq!(args[6], "/usr/bin/claude");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn linux_bin_unc_finishes_quickly_when_wsl_has_no_distro() {
        invalidate_locator_cache();
        let started = std::time::Instant::now();
        let found = linux_bin_unc("claude").await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "locator hung for {elapsed:?} (wsl.exe MSI stub must be killed on timeout)"
        );
        if let Some(path) = found {
            assert!(
                is_wsl_target(&path),
                "locator returned a non-WSL path: {path:?}"
            );
        }
    }
}
