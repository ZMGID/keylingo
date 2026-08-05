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
    /// npm 包名。既用来查最新版，也是 Windows 上的唯一安装路径。
    npm_package: Option<&'static str>,
    /// 官方 shell 安装脚本（macOS/Linux）。存在时优先于 npm——这些 CLI 的脚本装法
    /// 会就地自更新，用 npm 再装一份容易在 PATH 上和脚本装的那份互相遮蔽。
    script_unix: Option<&'static str>,
    docs: &'static str,
    /// 配置目录，相对用户 home。
    config_dir: Option<&'static str>,
}

fn install_spec(agent_id: &str) -> Option<InstallSpec> {
    let spec = match agent_id {
        "claude" => InstallSpec {
            npm_package: Some("@anthropic-ai/claude-code"),
            script_unix: Some("curl -fsSL https://claude.ai/install.sh | bash"),
            docs: "https://docs.claude.com/en/docs/claude-code/overview",
            config_dir: Some(".claude"),
        },
        "codex" => InstallSpec {
            npm_package: Some("@openai/codex"),
            script_unix: None,
            docs: "https://github.com/openai/codex",
            config_dir: Some(".codex"),
        },
        "cursor-agent" => InstallSpec {
            npm_package: None,
            script_unix: Some("curl https://cursor.com/install -fsS | bash"),
            docs: "https://cursor.com/docs/cli",
            config_dir: Some(".cursor"),
        },
        "opencode" => InstallSpec {
            npm_package: Some("opencode-ai"),
            script_unix: Some("curl -fsSL https://opencode.ai/install | bash"),
            docs: "https://opencode.ai/docs/",
            config_dir: Some(".config/opencode"),
        },
        "gemini" => InstallSpec {
            npm_package: Some("@google/gemini-cli"),
            script_unix: None,
            docs: "https://github.com/google-gemini/gemini-cli",
            config_dir: Some(".gemini"),
        },
        "kimi" => InstallSpec {
            npm_package: Some("@moonshot-ai/kimi-code"),
            script_unix: None,
            docs: "https://github.com/MoonshotAI/kimi-code",
            config_dir: Some(".kimi-code"),
        },
        "pi" => InstallSpec {
            npm_package: Some("@earendil-works/pi-coding-agent"),
            script_unix: None,
            docs: "https://github.com/earendil-works/pi",
            config_dir: Some(".pi"),
        },
        "grok" => InstallSpec {
            npm_package: Some("@xai-official/grok"),
            script_unix: Some("curl -fsSL https://x.ai/cli/install.sh | bash"),
            docs: "https://docs.x.ai/docs/cli",
            config_dir: Some(".grok"),
        },
        // hermes 没有公开的一键安装方式，只给文档链接。
        "hermes" => InstallSpec {
            npm_package: None,
            script_unix: None,
            docs: "https://github.com/NousResearch/hermes-cli",
            config_dir: None,
        },
        _ => return None,
    };
    Some(spec)
}

/// 该 CLI 在本机的安装/更新命令。`None` = 只能照文档手动装。
fn install_command(spec: &InstallSpec) -> Option<String> {
    if cfg!(not(windows)) {
        if let Some(script) = spec.script_unix {
            return Some(script.to_string());
        }
    }
    spec.npm_package
        .map(|pkg| format!("npm install -g {pkg}@latest"))
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

/// npm registry 上的最新版。查不到（离线 / 非 npm 包）一律 `None`，绝不因此挡住安装按钮。
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

fn existing_config_dir(spec: &InstallSpec) -> Option<String> {
    let dir: PathBuf = directories::UserDirs::new()?.home_dir().join(spec.config_dir?);
    dir.is_dir().then(|| dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn chat_external_cli_install_info(
    state: tauri::State<'_, crate::state::AppState>,
    agent_id: String,
) -> Result<InstallInfo, String> {
    let def = get_agent_def(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;
    let spec = install_spec(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;

    let local_version = crate::external_agents::spawn::resolve_binary(def)
        .await
        .and_then(|path| crate::external_agents::spawn::cached_cli_version(&path))
        .as_deref()
        .and_then(extract_semver);
    let latest_version = match spec.npm_package {
        Some(pkg) => npm_latest_version(&state.http, pkg).await,
        None => None,
    };
    let update_available = match (&local_version, &latest_version) {
        // 只做字符串不等判断：版本号大小比较要引依赖，而「本地 != 最新」已经足够触发一次更新。
        (Some(local), Some(latest)) => local != latest,
        _ => false,
    };

    Ok(InstallInfo {
        agent_id,
        local_version,
        latest_version,
        update_available,
        command: install_command(&spec),
        docs_url: spec.docs.to_string(),
        config_dir: existing_config_dir(&spec),
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
    let spec = install_spec(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;
    let command_line =
        install_command(&spec).ok_or_else(|| "该 CLI 没有一键安装方式，请照文档手动安装".to_string())?;

    let mut command = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", &command_line]);
        c
    } else {
        // 登录 shell：安装脚本和 npm 常依赖用户 profile 里的 PATH / nvm 初始化。
        let mut c = tokio::process::Command::new("sh");
        c.args(["-lc", &command_line]);
        c
    };
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
    Ok(())
}

/// 在系统文件管理器里打开该 CLI 的配置目录。
#[tauri::command]
pub fn chat_external_cli_open_config_dir(agent_id: String) -> Result<(), String> {
    let spec = install_spec(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;
    let dir = existing_config_dir(&spec).ok_or_else(|| "配置目录还不存在".to_string())?;
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
        assert_eq!(extract_semver("2.1.207 (Claude Code)").as_deref(), Some("2.1.207"));
        assert_eq!(extract_semver("codex-cli 0.146.0").as_deref(), Some("0.146.0"));
        assert_eq!(extract_semver("0.53.1").as_deref(), Some("0.53.1"));
        assert_eq!(extract_semver("v1.2.3-beta").as_deref(), Some("1.2.3"));
        assert_eq!(extract_semver("no version here"), None);
        // 两段号不是版本号，别把它当成版本报给用户。
        assert_eq!(extract_semver("cli 1.2"), None);
    }

    #[test]
    fn every_registered_agent_has_an_install_spec() {
        for def in crate::external_agents::registry::AGENT_DEFS {
            assert!(install_spec(def.id).is_some(), "缺少安装表: {}", def.id);
        }
    }
}
