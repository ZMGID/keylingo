//! 设置页「本地 CLI Agent」的安装/更新支撑：官方安装方式表、npm registry 最新版查询、
//! 带流式日志的安装执行、配置目录打开。
//!
//! ponytail: 只做「查版本 / 跑一条官方命令 / 打开目录」三件事。**没有**卸载、没有取消、
//! 没有多后端策略协商（ccgui 那套 1800 行的 installer 里九成是它自己的多引擎场景）。
//! 需要卸载时再加——用户自己 `npm uninstall -g` 也就一行。
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::external_agents::registry::get_agent_def;
use crate::proc::NoConsoleWindow;

/// 一个 CLI 的官方安装/更新方式。
struct InstallSpec {
    /// npm 包名。用来查最新版，也在没有官方平台安装脚本时作为安装回退。
    npm_package: Option<&'static str>,
    /// 少数包需要额外安装参数（例如 Pi 官方要求禁用 lifecycle scripts）。
    npm_install_args: &'static [&'static str],
    /// PyPI 包名；目前 Hermes 用它查稳定版，安装仍走官方脚本。
    pypi_package: Option<&'static str>,
    /// 官方 shell 安装脚本（macOS/Linux）。存在时优先于 npm——这些 CLI 的脚本装法
    /// 会就地自更新，用 npm 再装一份容易在 PATH 上和脚本装的那份互相遮蔽。
    script_unix: Option<&'static str>,
    /// 官方 PowerShell 安装脚本。没有时回落 npm。
    script_windows: Option<&'static str>,
    /// 已安装后的自更新参数；始终交给探测到的那个绝对二进制执行。
    update_args: Option<&'static [&'static str]>,
    docs: &'static str,
    /// 配置目录，相对用户 home。
    config_dir: Option<&'static str>,
}

fn install_spec(agent_id: &str) -> Option<InstallSpec> {
    let spec = match agent_id {
        "claude" => InstallSpec {
            npm_package: Some("@anthropic-ai/claude-code"),
            npm_install_args: &[],
            pypi_package: None,
            script_unix: Some("curl -fsSL https://claude.ai/install.sh | bash"),
            script_windows: Some("irm https://claude.ai/install.ps1 | iex"),
            update_args: Some(&["update"]),
            docs: "https://code.claude.com/docs/en/setup",
            config_dir: Some(".claude"),
        },
        "codex" => InstallSpec {
            npm_package: Some("@openai/codex"),
            npm_install_args: &[],
            pypi_package: None,
            script_unix: None,
            script_windows: None,
            update_args: Some(&["update"]),
            docs: "https://developers.openai.com/codex/cli/",
            config_dir: Some(".codex"),
        },
        "cursor-agent" => InstallSpec {
            npm_package: None,
            npm_install_args: &[],
            pypi_package: None,
            script_unix: Some("curl https://cursor.com/install -fsS | bash"),
            script_windows: Some("irm 'https://cursor.com/install?win32=true' | iex"),
            update_args: Some(&["update"]),
            docs: "https://cursor.com/docs/cli",
            config_dir: Some(".cursor"),
        },
        "opencode" => InstallSpec {
            npm_package: Some("opencode-ai"),
            npm_install_args: &[],
            pypi_package: None,
            script_unix: Some("curl -fsSL https://opencode.ai/install | bash"),
            script_windows: None,
            update_args: Some(&["upgrade"]),
            docs: "https://opencode.ai/docs/",
            config_dir: Some(".config/opencode"),
        },
        "gemini" => InstallSpec {
            npm_package: Some("@google/gemini-cli"),
            npm_install_args: &[],
            pypi_package: None,
            script_unix: None,
            script_windows: None,
            // Gemini 尚无稳定的非交互 update 子命令；按 npm/Homebrew 来源单独处理。
            update_args: None,
            docs: "https://www.geminicli.com/docs/get-started/installation",
            config_dir: Some(".gemini"),
        },
        "kimi" => InstallSpec {
            npm_package: Some("@moonshot-ai/kimi-code"),
            npm_install_args: &[],
            pypi_package: None,
            script_unix: Some("curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash"),
            script_windows: Some("irm https://code.kimi.com/kimi-code/install.ps1 | iex"),
            // `kimi upgrade` 在无 TTY 时只打印手动命令、不执行；按来源单独处理。
            update_args: None,
            docs: "https://moonshotai.github.io/kimi-code/en/guides/getting-started.html",
            config_dir: Some(".kimi-code"),
        },
        "pi" => InstallSpec {
            npm_package: Some("@earendil-works/pi-coding-agent"),
            npm_install_args: &["--ignore-scripts"],
            pypi_package: None,
            script_unix: None,
            script_windows: None,
            update_args: Some(&["update", "--self"]),
            docs: "https://github.com/badlogic/pi-mono",
            config_dir: Some(".pi"),
        },
        "grok" => InstallSpec {
            npm_package: Some("@xai-official/grok"),
            npm_install_args: &[],
            pypi_package: None,
            script_unix: Some("curl -fsSL https://x.ai/cli/install.sh | bash"),
            script_windows: None,
            update_args: Some(&["update"]),
            docs: "https://docs.x.ai/build/cli",
            config_dir: Some(".grok"),
        },
        "dsh" => InstallSpec {
            npm_package: Some("@deepseek-ai/dsh"),
            npm_install_args: &[],
            pypi_package: None,
            script_unix: None,
            script_windows: None,
            // dsh 没有自更新命令；`update_plan` 按 npm/pnpm/yarn/bun 安装来源更新。
            update_args: None,
            docs: "https://github.com/deepseek-ai/deepseek-harness",
            config_dir: Some(".dsh"),
        },
        "hermes" => InstallSpec {
            npm_package: None,
            npm_install_args: &[],
            pypi_package: Some("hermes-agent"),
            script_unix: Some("curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash"),
            script_windows: Some("iex (irm https://hermes-agent.nousresearch.com/install.ps1)"),
            update_args: Some(&["update"]),
            docs: "https://hermes-agent.nousresearch.com/docs/getting-started/installation",
            config_dir: Some(".hermes"),
        },
        _ => return None,
    };
    Some(spec)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPlatform {
    Unix,
    Windows,
}

fn host_platform() -> HostPlatform {
    if cfg!(windows) {
        HostPlatform::Windows
    } else {
        HostPlatform::Unix
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandPlan {
    program: String,
    args: Vec<String>,
    display: String,
}

impl CommandPlan {
    fn direct(program: impl Into<String>, args: &[&str]) -> Self {
        let program = program.into();
        let args = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        let display = std::iter::once(program.as_str())
            .chain(args.iter().map(String::as_str))
            .map(display_token)
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            program,
            args,
            display,
        }
    }

    fn unix_script(script: &str) -> Self {
        Self {
            program: "bash".to_string(),
            args: vec!["-lc".to_string(), format!("set -o pipefail; {script}")],
            display: script.to_string(),
        }
    }

    fn powershell(script: &str) -> Self {
        Self {
            program: "powershell.exe".to_string(),
            args: vec![
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                script.to_string(),
            ],
            display: script.to_string(),
        }
    }
}

fn display_token(value: &str) -> String {
    if value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn npm_install_plan(package: &str, extra_args: &[&str], platform: HostPlatform) -> CommandPlan {
    let program = if platform == HostPlatform::Windows {
        "npm.cmd"
    } else {
        "npm"
    };
    let mut args = vec!["install", "-g"];
    args.extend_from_slice(extra_args);
    let package = format!("{package}@latest");
    args.push(&package);
    CommandPlan::direct(program, &args)
}

fn install_plan(spec: &InstallSpec, platform: HostPlatform) -> Option<CommandPlan> {
    match platform {
        HostPlatform::Unix => spec.script_unix.map(CommandPlan::unix_script),
        HostPlatform::Windows => spec.script_windows.map(CommandPlan::powershell),
    }
    .or_else(|| {
        spec.npm_package
            .map(|package| npm_install_plan(package, spec.npm_install_args, platform))
    })
}

fn normalized_resolved_path(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn managed_package_update_plan(
    path: &Path,
    package: &str,
    brew_formula: &str,
    platform: HostPlatform,
) -> Option<CommandPlan> {
    let normalized = normalized_resolved_path(path);
    if normalized.contains(&format!("/cellar/{brew_formula}/")) {
        return Some(CommandPlan::direct("brew", &["upgrade", brew_formula]));
    }
    if normalized.contains("/.pnpm/") || normalized.contains("/pnpm/global/") {
        return Some(CommandPlan::direct(
            "pnpm",
            &["add", "-g", &format!("{package}@latest")],
        ));
    }
    if normalized.contains("/.config/yarn/global/") || normalized.contains("/.yarn/global/") {
        return Some(CommandPlan::direct(
            "yarn",
            &["global", "add", &format!("{package}@latest")],
        ));
    }
    if normalized.contains("/.bun/install/global/") {
        return Some(CommandPlan::direct(
            "bun",
            &["add", "-g", &format!("{package}@latest")],
        ));
    }
    if normalized.contains("/node_modules/")
        || (platform == HostPlatform::Windows
            && matches!(path.extension().and_then(|ext| ext.to_str()), Some("cmd")))
    {
        return Some(npm_install_plan(package, &[], platform));
    }
    None
}

/// 已安装时优先让当前二进制自更新；Kimi/Gemini 没有可用的无交互入口，按安装来源处理。
fn update_plan(
    agent_id: &str,
    spec: &InstallSpec,
    resolved_path: &Path,
    platform: HostPlatform,
) -> Option<CommandPlan> {
    if agent_id == "kimi" {
        let normalized = normalized_resolved_path(resolved_path);
        if normalized.contains("/.kimi-code/bin/kimi") {
            return install_plan(spec, platform);
        }
        return managed_package_update_plan(
            resolved_path,
            "@moonshot-ai/kimi-code",
            "kimi-code",
            platform,
        );
    }
    if agent_id == "gemini" {
        return managed_package_update_plan(
            resolved_path,
            "@google/gemini-cli",
            "gemini-cli",
            platform,
        );
    }
    if agent_id == "dsh" {
        return managed_package_update_plan(resolved_path, "@deepseek-ai/dsh", "dsh", platform);
    }
    spec.update_args
        .map(|args| CommandPlan::direct(resolved_path.to_string_lossy().into_owned(), args))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    pub agent_id: String,
    pub local_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    /// 可直接执行的安装/更新命令；`None` 时前端只显示文档链接。
    pub command: Option<String>,
    pub docs_url: String,
    /// 存在的配置目录绝对路径（不存在则 `None`，不去创建）。
    pub config_dir: Option<String>,
}

/// npm registry 上的最新版。查不到（离线 / 非 npm 包）一律 `None`。
async fn npm_latest_version(http: &reqwest::Client, package: &str) -> Option<String> {
    let url = format!("https://registry.npmjs.org/{package}/latest");
    let value: serde_json::Value = http
        .get(&url)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    value.get("version")?.as_str().map(str::to_string)
}

async fn pypi_latest_version(http: &reqwest::Client, package: &str) -> Option<String> {
    let url = format!("https://pypi.org/pypi/{package}/json");
    let value: serde_json::Value = http
        .get(&url)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    value
        .get("info")?
        .get("version")?
        .as_str()
        .map(str::to_string)
}

async fn latest_version(http: &reqwest::Client, spec: &InstallSpec) -> Option<String> {
    if let Some(package) = spec.npm_package {
        npm_latest_version(http, package).await
    } else if let Some(package) = spec.pypi_package {
        pypi_latest_version(http, package).await
    } else {
        None
    }
}

/// `--version` 输出里抓语义化版本号：CLI 们的首行格式各不相同
/// （`2.1.207 (Claude Code)` / `codex-cli 0.146.0` / 裸 `0.53.1`），只比对版本号本身。
fn extract_semver(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = None;
    for (idx, ch) in text.char_indices() {
        if ch.is_ascii_digit() {
            if start.is_none() {
                start = Some(idx);
            }
        } else if ch != '.' {
            if let Some(s) = start {
                let candidate = &text[s..idx];
                if candidate.matches('.').count() >= 2 {
                    return Some(candidate.to_string());
                }
            }
            start = None;
        }
    }
    let s = start?;
    let candidate = std::str::from_utf8(&bytes[s..]).ok()?;
    (candidate.matches('.').count() >= 2).then(|| candidate.to_string())
}

fn version_is_newer(local: &str, latest: &str) -> bool {
    let parse = |version: &str| -> Option<Vec<u64>> {
        let parts = version
            .split('.')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        (parts.len() >= 3).then_some(parts)
    };
    let (Some(local), Some(latest)) = (parse(local), parse(latest)) else {
        return false;
    };
    let length = local.len().max(latest.len());
    (0..length)
        .find_map(|index| {
            let local_part = local.get(index).copied().unwrap_or(0);
            let latest_part = latest.get(index).copied().unwrap_or(0);
            (local_part != latest_part).then_some(latest_part > local_part)
        })
        .unwrap_or(false)
}

fn existing_config_dir(agent_id: &str, spec: &InstallSpec) -> Option<String> {
    let dir = if agent_id == "dsh" {
        std::env::var_os("DSH_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| directories::UserDirs::new().map(|dirs| dirs.home_dir().join(".dsh")))?
    } else {
        directories::UserDirs::new()?
            .home_dir()
            .join(spec.config_dir?)
    };
    dir.is_dir().then(|| dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn chat_external_cli_install_info(
    state: tauri::State<'_, crate::state::AppState>,
    agent_id: String,
) -> Result<InstallInfo, String> {
    let def = get_agent_def(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;
    let spec = install_spec(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;

    let resolved_path = crate::external_agents::spawn::resolve_binary(def).await;
    let local_version = resolved_path
        .as_deref()
        .and_then(crate::external_agents::spawn::cached_cli_version)
        .as_deref()
        .and_then(extract_semver);
    let latest_version = latest_version(&state.http, &spec).await;
    let update_available = match (&local_version, &latest_version) {
        (Some(local), Some(latest)) => version_is_newer(local, latest),
        _ => false,
    };
    let command = match resolved_path.as_deref() {
        Some(path) => update_plan(&agent_id, &spec, path, host_platform()),
        None => install_plan(&spec, host_platform()),
    };
    let config_dir = existing_config_dir(&agent_id, &spec);

    Ok(InstallInfo {
        agent_id,
        local_version,
        latest_version,
        update_available,
        command: command.map(|plan| plan.display),
        docs_url: spec.docs.to_string(),
        config_dir,
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallLogEvent {
    agent_id: String,
    line: Option<String>,
    done: bool,
    success: bool,
}

const INSTALL_TIMEOUT_SECS: u64 = 300;

/// 跑 `install_command` 并把 stdout/stderr 按行流到 `external-cli-install` 事件。
///
/// ponytail: 不支持中途取消——安装是低频一次性动作，超时（300s）就自己结束。
#[tauri::command]
pub async fn chat_external_cli_install(app: AppHandle, agent_id: String) -> Result<(), String> {
    let def = get_agent_def(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;
    let spec = install_spec(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;
    let resolved_path = crate::external_agents::spawn::resolve_binary(def).await;
    let plan = match resolved_path.as_deref() {
        Some(path) => update_plan(&agent_id, &spec, path, host_platform())
            .ok_or_else(|| "无法安全识别该 CLI 的安装来源，请按官方文档手动更新".to_string())?,
        None => install_plan(&spec, host_platform())
            .ok_or_else(|| "该 CLI 没有一键安装方式，请按官方文档手动安装".to_string())?,
    };
    let command_line = plan.display.clone();

    let mut command = crate::external_agents::spawn::cli_command(&plan.program);
    command.args(&plan.args);
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_console_window()
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("启动安装命令失败: {e}"))?;

    let emit = |line: String| {
        let _ = app.emit(
            "external-cli-install",
            InstallLogEvent {
                agent_id: agent_id.clone(),
                line: Some(line),
                done: false,
                success: false,
            },
        );
    };
    emit(format!("$ {command_line}"));

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut out_lines = stdout.map(|s| BufReader::new(s).lines());
    let mut err_lines = stderr.map(|s| BufReader::new(s).lines());

    let pump = async {
        loop {
            let out = async {
                match out_lines.as_mut() {
                    Some(lines) => lines.next_line().await.ok().flatten(),
                    None => std::future::pending().await,
                }
            };
            let err = async {
                match err_lines.as_mut() {
                    Some(lines) => lines.next_line().await.ok().flatten(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                Some(line) = out => emit(line),
                Some(line) = err => emit(line),
                else => break,
            }
        }
        child.wait().await
    };

    let success = match tokio::time::timeout(
        std::time::Duration::from_secs(INSTALL_TIMEOUT_SECS),
        pump,
    )
    .await
    {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => {
            emit(format!("安装命令异常结束: {e}"));
            false
        }
        Err(_) => {
            emit(format!("安装超时（{INSTALL_TIMEOUT_SECS}s），已放弃"));
            false
        }
    };

    let _ = app.emit(
        "external-cli-install",
        InstallLogEvent {
            agent_id,
            line: None,
            done: true,
            success,
        },
    );
    if success {
        Ok(())
    } else {
        Err("安装或更新命令执行失败".to_string())
    }
}

/// 在系统文件管理器里打开该 CLI 的配置目录。
#[tauri::command]
pub fn chat_external_cli_open_config_dir(agent_id: String) -> Result<(), String> {
    let spec = install_spec(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;
    let dir =
        existing_config_dir(&agent_id, &spec).ok_or_else(|| "配置目录还不存在".to_string())?;
    open_path(Path::new(&dir))
}

fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let program = "xdg-open";

    std::process::Command::new(program)
        .arg(path)
        .no_console_window()
        .spawn()
        .map_err(|e| format!("打开目录失败: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_semver_handles_each_cli_version_line() {
        assert_eq!(
            extract_semver("2.1.207 (Claude Code)").as_deref(),
            Some("2.1.207")
        );
        assert_eq!(
            extract_semver("codex-cli 0.146.0").as_deref(),
            Some("0.146.0")
        );
        assert_eq!(extract_semver("0.53.1").as_deref(), Some("0.53.1"));
        assert_eq!(extract_semver("v1.2.3-beta").as_deref(), Some("1.2.3"));
        assert_eq!(extract_semver("no version here"), None);
        // 两段号不是版本号，别把它当成版本报给用户。
        assert_eq!(extract_semver("cli 1.2"), None);
    }

    #[test]
    fn update_is_available_only_when_latest_version_is_newer() {
        assert!(version_is_newer("1.18.13", "1.19.0"));
        assert!(!version_is_newer("1.18.13", "1.18.13"));
        assert!(!version_is_newer("1.19.0", "1.18.13"));
        assert!(!version_is_newer("1.18.13.0", "1.18.13"));
    }

    #[test]
    fn installed_clis_use_their_own_update_commands() {
        let cases: &[(&str, &[&str])] = &[
            ("claude", &["update"]),
            ("codex", &["update"]),
            ("cursor-agent", &["update"]),
            ("opencode", &["upgrade"]),
            ("pi", &["update", "--self"]),
            ("grok", &["update"]),
            ("hermes", &["update"]),
        ];
        for (agent_id, expected_args) in cases {
            let spec = install_spec(agent_id).unwrap();
            let path = PathBuf::from(format!("/custom/bin/{agent_id}"));
            let plan = update_plan(agent_id, &spec, &path, HostPlatform::Unix).unwrap();
            assert_eq!(plan.program, path.to_string_lossy());
            assert_eq!(plan.args, *expected_args, "{agent_id}");
        }
    }

    #[test]
    fn kimi_and_gemini_updates_preserve_recognized_install_source() {
        let kimi = install_spec("kimi").unwrap();
        let native = update_plan(
            "kimi",
            &kimi,
            Path::new("/Users/u/.kimi-code/bin/kimi"),
            HostPlatform::Unix,
        )
        .unwrap();
        assert_eq!(native.program, "bash");
        assert!(native
            .display
            .contains("code.kimi.com/kimi-code/install.sh"));

        let npm = update_plan(
            "kimi",
            &kimi,
            Path::new("/prefix/lib/node_modules/@moonshot-ai/kimi-code/dist/main.mjs"),
            HostPlatform::Unix,
        )
        .unwrap();
        assert_eq!(npm.program, "npm");
        assert!(npm
            .args
            .contains(&"@moonshot-ai/kimi-code@latest".to_string()));

        let gemini = install_spec("gemini").unwrap();
        let brew = update_plan(
            "gemini",
            &gemini,
            Path::new("/opt/homebrew/Cellar/gemini-cli/1.2.3/bin/gemini"),
            HostPlatform::Unix,
        )
        .unwrap();
        assert_eq!(brew.display, "brew upgrade gemini-cli");
        assert!(update_plan(
            "gemini",
            &gemini,
            Path::new("/custom/bin/gemini"),
            HostPlatform::Unix,
        )
        .is_none());
    }

    #[test]
    fn install_commands_follow_official_platform_recipes() {
        let kimi = install_spec("kimi").unwrap();
        assert!(install_plan(&kimi, HostPlatform::Unix)
            .unwrap()
            .display
            .contains("code.kimi.com/kimi-code/install.sh"));
        assert_eq!(
            install_plan(&kimi, HostPlatform::Windows).unwrap().program,
            "powershell.exe"
        );

        let pi = install_plan(&install_spec("pi").unwrap(), HostPlatform::Unix).unwrap();
        assert!(pi.args.contains(&"--ignore-scripts".to_string()));

        let hermes = install_spec("hermes").unwrap();
        assert!(install_plan(&hermes, HostPlatform::Unix)
            .unwrap()
            .display
            .contains("hermes-agent.nousresearch.com/install.sh"));
        assert!(hermes.pypi_package.is_some());
    }

    #[test]
    fn every_registered_agent_has_an_install_spec() {
        for def in crate::external_agents::registry::AGENT_DEFS {
            assert!(install_spec(def.id).is_some(), "缺少安装表: {}", def.id);
        }
    }
}
