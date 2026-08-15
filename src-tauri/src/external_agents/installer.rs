//! 设置页「本地 CLI Agent」的安装/更新支撑：官方安装方式表、npm registry 最新版查询、
//! 带流式日志的安装执行、配置目录打开。
//!
//! ponytail: 查版本 / 跑官方命令 / 打开目录。**没有**卸载、没有取消、没有多后端策略协商。
//! npm 系 CLI 在跑命令前会补 Node（dsh 再补 pnpm）：小白机器上没有 Node 时直接
//! `npm.cmd` 只会丢「找不到文件」。
//! npm / pnpm / yarn / bun 的安装命令一律钉死 `registry.npmjs.org`：国内镜像经常
//! 缺 `@deepseek-ai/*` 这类新包，用户自己的 `.npmrc` 不能把安装带跑偏。
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncReadExt;

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
            // `@deepseek-ai/dsh-app-boot` 把 `@deepseek-ai/cordis-plugin-group` 标成
            // peerDependency，但 CLI 包自己没声明。npm 不会装嵌套 peer，装完
            // `dsh --version` 直接 `ERR_MODULE_NOT_FOUND`。更新时同样带上。
            npm_install_args: &["@deepseek-ai/cordis-plugin-group@latest"],
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

const NODE_DOWNLOAD_URL: &str = "https://nodejs.org";
const NPMJS_REGISTRY: &str = "https://registry.npmjs.org/";
const NPMJS_REGISTRY_FLAG: &str = "--registry=https://registry.npmjs.org/";
const DEEPSEEK_SCOPE_REGISTRY_FLAG: &str = "--@deepseek-ai:registry=https://registry.npmjs.org/";

fn uses_npm(plan: &CommandPlan) -> bool {
    plan.program == "npm" || plan.program == "npm.cmd"
}

fn uses_js_package_manager(plan: &CommandPlan) -> bool {
    matches!(
        plan.program.as_str(),
        "npm" | "npm.cmd" | "pnpm" | "pnpm.cmd" | "yarn" | "yarn.cmd" | "bun" | "bun.exe"
    )
}

/// 覆盖用户 `.npmrc` / `npm_config_registry` 里的淘宝等镜像。CLI `--registry` 盖不住
/// 作用域配置（`@deepseek-ai:registry=`），所以安装 `@deepseek-ai/*` 时另外带一条。
pub(crate) fn pin_official_npm_registry(command: &mut tokio::process::Command) {
    command.env("npm_config_registry", NPMJS_REGISTRY);
    command.env("NPM_CONFIG_REGISTRY", NPMJS_REGISTRY);
}

fn needs_deepseek_scope_registry(package: &str, extra_args: &[&str]) -> bool {
    package.starts_with("@deepseek-ai/")
        || extra_args
            .iter()
            .any(|arg| arg.starts_with("@deepseek-ai/"))
}

/// `node --version` 输出 → (major, minor, patch)。认 `v24.13.0` / `24.13.0`。
fn parse_node_version(text: &str) -> Option<(u64, u64, u64)> {
    let text = text.trim().trim_start_matches('v');
    let mut parts = text
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty());
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

/// dsh 官方 `engines.node`：`^22.19.0 || >=24.0.0`（不含 23）。
fn node_meets_dsh(version: (u64, u64, u64)) -> bool {
    let (major, minor, _) = version;
    (major == 22 && minor >= 19) || major >= 24
}

fn missing_node_message(for_dsh: bool) -> String {
    if for_dsh {
        format!(
            "安装 DeepSeek Harness 需要 Node.js 22.19+ 或 24+（以及 npm）。\
             当前电脑没有可用的 Node.js，自动安装也没有成功。\
             请打开 {NODE_DOWNLOAD_URL} 下载 LTS 版本，装好后重新打开 Kivio 再点安装。"
        )
    } else {
        format!(
            "安装该 CLI 需要 Node.js 和 npm。当前电脑没有可用的 Node.js，自动安装也没有成功。\
             请打开 {NODE_DOWNLOAD_URL} 下载 LTS 版本后重试。"
        )
    }
}

fn dsh_node_too_old_message(found: &str) -> String {
    format!(
        "当前 Node.js 为 {found}，DeepSeek Harness 需要 22.19+ 或 24+（不含 23）。\
         请打开 {NODE_DOWNLOAD_URL} 安装匹配的 LTS 版本后重试。"
    )
}

fn prepend_path_dirs(dirs: &[PathBuf]) {
    let current = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut extra = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let value = dir.to_string_lossy().into_owned();
        let already = extra.iter().any(|item: &String| path_eq(item, &value))
            || current.split(sep).any(|item| path_eq(item, &value));
        if !already {
            extra.push(value);
        }
    }
    if extra.is_empty() {
        return;
    }
    if current.is_empty() {
        std::env::set_var("PATH", extra.join(&sep.to_string()));
    } else {
        extra.push(current);
        std::env::set_var("PATH", extra.join(&sep.to_string()));
    }
}

fn path_eq(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn known_node_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(root) = std::env::var(var) {
                let root = root.trim().trim_end_matches('\\');
                if !root.is_empty() {
                    dirs.push(PathBuf::from(root).join("nodejs"));
                }
            }
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
        dirs.push(PathBuf::from(r"D:\Program Files\nodejs"));
        dirs.push(PathBuf::from(r"E:\Program Files\nodejs"));
    }
    #[cfg(not(windows))]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(&home).join(".local").join("bin"));
        }
    }
    dirs
}

pub(crate) fn expose_node_on_path() {
    crate::path_env::refresh_path_now();
    prepend_path_dirs(&known_node_dirs());
}

pub(crate) async fn ensure_pnpm_for_dsh() -> Result<(), String> {
    expose_node_on_path();
    ensure_pnpm(&|line| eprintln!("[dsh] {line}")).await
}

fn configure_install_command(command: &mut tokio::process::Command, plan: &CommandPlan) {
    expose_node_on_path();
    command.args(&plan.args);
    command.env_remove("npm_config_devdir");
    command.env_remove("NPM_CONFIG_DEVDIR");
    if uses_js_package_manager(plan) {
        pin_official_npm_registry(command);
    }
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
}

async fn probe_version(program: &str) -> Option<String> {
    let mut command = crate::external_agents::spawn::cli_command(program);
    command.arg("--version");
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.no_console_window();
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(8), command.output())
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().find(|line| !line.trim().is_empty())?;
    Some(first.trim().to_string())
}

async fn first_working_program(names: &[&str]) -> Option<String> {
    for name in names {
        if probe_version(name).await.is_some() {
            return Some((*name).to_string());
        }
    }
    None
}

async fn probe_node() -> Option<String> {
    for name in ["node", "node.exe"] {
        if let Some(version) = probe_version(name).await {
            return Some(version);
        }
    }
    None
}

async fn npm_is_ready() -> bool {
    first_working_program(&["npm.cmd", "npm"]).await.is_some()
}

async fn pnpm_is_ready() -> bool {
    first_working_program(&["pnpm.cmd", "pnpm"]).await.is_some()
}

async fn stream_install_child(
    mut child: tokio::process::Child,
    emit: &impl Fn(String),
    timeout_secs: u64,
) -> bool {
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out_acc = Vec::new();
    let mut err_acc = Vec::new();
    let mut out_buf = [0u8; 1024];
    let mut err_buf = [0u8; 1024];

    let pump = async {
        loop {
            let out = async {
                match stdout.as_mut() {
                    Some(reader) => Some(reader.read(&mut out_buf).await),
                    None => std::future::pending().await,
                }
            };
            let err = async {
                match stderr.as_mut() {
                    Some(reader) => Some(reader.read(&mut err_buf).await),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                Some(result) = out => match result {
                    Ok(0) => {
                        if let Some(line) = flush_progress_line(&mut out_acc) {
                            emit(line);
                        }
                        stdout = None;
                    }
                    Ok(n) => {
                        for line in take_progress_lines(&mut out_acc, &out_buf[..n]) {
                            emit(line);
                        }
                    }
                    Err(e) => {
                        emit(format!("读取安装输出失败: {e}"));
                        stdout = None;
                    }
                },
                Some(result) = err => match result {
                    Ok(0) => {
                        if let Some(line) = flush_progress_line(&mut err_acc) {
                            emit(line);
                        }
                        stderr = None;
                    }
                    Ok(n) => {
                        for line in take_progress_lines(&mut err_acc, &err_buf[..n]) {
                            emit(line);
                        }
                    }
                    Err(e) => {
                        emit(format!("读取安装输出失败: {e}"));
                        stderr = None;
                    }
                },
                else => break,
            }
        }
        child.wait().await
    };

    match tokio::time::timeout(Duration::from_secs(timeout_secs), pump).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => {
            emit(format!("安装命令异常结束: {e}"));
            false
        }
        Err(_) => {
            emit(format!("安装超时（{timeout_secs}s），已放弃"));
            false
        }
    }
}

async fn run_logged_plan(
    plan: &CommandPlan,
    emit: &impl Fn(String),
    timeout_secs: u64,
) -> Result<(), String> {
    emit(format!("$ {}", plan.display));
    let mut command = crate::external_agents::spawn::cli_command(&plan.program);
    configure_install_command(&mut command, plan);
    let child = command
        .no_console_window()
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("启动命令失败: {e}"))?;
    if stream_install_child(child, emit, timeout_secs).await {
        Ok(())
    } else {
        Err(format!("命令失败：{}", plan.display))
    }
}

async fn bootstrap_node(emit: &impl Fn(String)) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut candidates = vec!["winget".to_string()];
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local)
                    .join("Microsoft")
                    .join("WindowsApps")
                    .join("winget.exe")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        let names: Vec<&str> = candidates.iter().map(String::as_str).collect();
        if let Some(winget) = first_working_program(&names).await {
            emit("未检测到 Node.js，正在通过 winget 安装 Node.js LTS…".to_string());
            let plan = CommandPlan::direct(
                winget,
                &[
                    "install",
                    "--id",
                    "OpenJS.NodeJS.LTS",
                    "-e",
                    "--source",
                    "winget",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ],
            );
            run_logged_plan(&plan, emit, 300).await?;
            prepend_path_dirs(&known_node_dirs());
            return Ok(());
        }
    }
    #[cfg(unix)]
    {
        if let Some(brew) =
            first_working_program(&["brew", "/opt/homebrew/bin/brew", "/usr/local/bin/brew"]).await
        {
            emit("未检测到 Node.js，正在通过 Homebrew 安装 Node.js…".to_string());
            let plan = CommandPlan::direct(brew, &["install", "node"]);
            run_logged_plan(&plan, emit, 300).await?;
            prepend_path_dirs(&known_node_dirs());
            return Ok(());
        }
    }
    Err("本机没有可用的 Node.js 安装器（Windows 需要 winget，macOS 需要 Homebrew）".to_string())
}

async fn ensure_node(for_dsh: bool, emit: &impl Fn(String)) -> Result<(), String> {
    expose_node_on_path();
    if let Some(version) = probe_node().await {
        if npm_is_ready().await {
            if for_dsh {
                match parse_node_version(&version) {
                    Some(parsed) if node_meets_dsh(parsed) => return Ok(()),
                    _ => return Err(dsh_node_too_old_message(&version)),
                }
            }
            return Ok(());
        }
    }

    match bootstrap_node(emit).await {
        Ok(()) => {}
        Err(err) => emit(format!("自动安装 Node.js 未成功：{err}")),
    }
    expose_node_on_path();

    let Some(version) = probe_node().await else {
        return Err(missing_node_message(for_dsh));
    };
    if !npm_is_ready().await {
        return Err(missing_node_message(for_dsh));
    }
    if for_dsh {
        match parse_node_version(&version) {
            Some(parsed) if node_meets_dsh(parsed) => {}
            _ => return Err(dsh_node_too_old_message(&version)),
        }
    }
    emit(format!("Node.js {version} 已就绪"));
    Ok(())
}

async fn ensure_pnpm(emit: &impl Fn(String)) -> Result<(), String> {
    if pnpm_is_ready().await {
        return Ok(());
    }
    emit("未检测到 pnpm，正在安装（dsh 的插件安装依赖它）…".to_string());
    let plan = npm_install_plan("pnpm", &[], host_platform());
    if let Err(err) = run_logged_plan(&plan, emit, 180).await {
        emit(format!("自动安装 pnpm 未成功：{err}"));
    }
    expose_node_on_path();
    if pnpm_is_ready().await {
        emit("pnpm 已就绪".to_string());
        return Ok(());
    }
    Err(format!(
        "DeepSeek Harness 需要 pnpm 来安装 profile 插件，但当前电脑没有 pnpm。\
         请打开 https://pnpm.io/installation 安装后重试。"
    ))
}

async fn prepare_npm_toolchain(
    agent_id: &str,
    plan: &CommandPlan,
    emit: &impl Fn(String),
) -> Result<(), String> {
    let for_dsh = agent_id == "dsh";
    if !uses_npm(plan) && !for_dsh {
        return Ok(());
    }
    ensure_node(for_dsh, emit).await?;
    if for_dsh {
        ensure_pnpm(emit).await?;
    }
    Ok(())
}

fn npm_install_plan(package: &str, extra_args: &[&str], platform: HostPlatform) -> CommandPlan {
    let program = if platform == HostPlatform::Windows {
        "npm.cmd"
    } else {
        "npm"
    };
    // `--progress=false`：npm 默认进度条用 `\r` 刷同一行。按行读会看起来像卡住，
    // 管道缓冲区填满后安装进程还会死锁。关掉进度条，改走普通日志行。
    let mut args = vec!["install", "-g", "--progress=false", NPMJS_REGISTRY_FLAG];
    if needs_deepseek_scope_registry(package, extra_args) {
        args.push(DEEPSEEK_SCOPE_REGISTRY_FLAG);
    }
    args.extend_from_slice(extra_args);
    let package = format!("{package}@latest");
    args.push(&package);
    CommandPlan::direct(program, &args)
}

fn js_global_add_plan(
    program: &str,
    prefix: &[&str],
    package: &str,
    extra_args: &[&str],
) -> CommandPlan {
    let latest = format!("{package}@latest");
    let mut args = prefix.to_vec();
    args.push(NPMJS_REGISTRY_FLAG);
    if needs_deepseek_scope_registry(package, extra_args) {
        args.push(DEEPSEEK_SCOPE_REGISTRY_FLAG);
    }
    args.extend_from_slice(extra_args);
    args.push(&latest);
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
    extra_args: &[&str],
) -> Option<CommandPlan> {
    let normalized = normalized_resolved_path(path);
    if normalized.contains(&format!("/cellar/{brew_formula}/")) {
        return Some(CommandPlan::direct("brew", &["upgrade", brew_formula]));
    }
    if normalized.contains("/.pnpm/") || normalized.contains("/pnpm/global/") {
        return Some(js_global_add_plan(
            "pnpm",
            &["add", "-g"],
            package,
            extra_args,
        ));
    }
    if normalized.contains("/.config/yarn/global/") || normalized.contains("/.yarn/global/") {
        return Some(js_global_add_plan(
            "yarn",
            &["global", "add"],
            package,
            extra_args,
        ));
    }
    if normalized.contains("/.bun/install/global/") {
        return Some(js_global_add_plan(
            "bun",
            &["add", "-g"],
            package,
            extra_args,
        ));
    }
    if normalized.contains("/node_modules/")
        || (platform == HostPlatform::Windows
            && matches!(path.extension().and_then(|ext| ext.to_str()), Some("cmd")))
    {
        return Some(npm_install_plan(package, extra_args, platform));
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
            spec.npm_install_args,
        );
    }
    if agent_id == "gemini" {
        return managed_package_update_plan(
            resolved_path,
            "@google/gemini-cli",
            "gemini-cli",
            platform,
            spec.npm_install_args,
        );
    }
    if agent_id == "dsh" {
        return managed_package_update_plan(
            resolved_path,
            "@deepseek-ai/dsh",
            "dsh",
            platform,
            spec.npm_install_args,
        );
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

async fn finish_dsh_install(
    def: &crate::external_agents::types::RuntimeAgentDef,
    emit: &impl Fn(String),
) -> Result<(), String> {
    let Some(bin) = crate::external_agents::spawn::resolve_binary(def).await else {
        return Err(
            "DeepSeek Harness 已装上，但当前进程还找不到 dsh 命令。请关掉 Kivio 再打开，然后重新扫描。"
                .to_string(),
        );
    };
    emit("正在初始化 Kivio dsh profile（首次需要下载插件）…".to_string());
    crate::external_agents::dsh_profile::ensure_profile_ready(&bin, None, None).await?;
    emit("dsh profile 已就绪".to_string());
    Ok(())
}

/// 把一块输出拆成日志行。`\n` 和 `\r` 都算换行——npm 进度条只写 `\r`，按 `\n` 读
/// 会一直等，管道塞满后安装进程自己也写不动。
fn take_progress_lines(acc: &mut Vec<u8>, chunk: &[u8]) -> Vec<String> {
    acc.extend_from_slice(chunk);
    let mut out = Vec::new();
    loop {
        let Some(i) = acc.iter().position(|&b| b == b'\n' || b == b'\r') else {
            break;
        };
        let line = String::from_utf8_lossy(&acc[..i]).trim().to_string();
        let crlf = acc[i] == b'\r' && acc.get(i + 1) == Some(&b'\n');
        acc.drain(..i + if crlf { 2 } else { 1 });
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

fn flush_progress_line(acc: &mut Vec<u8>) -> Option<String> {
    if acc.is_empty() {
        return None;
    }
    let line = String::from_utf8_lossy(acc).trim().to_string();
    acc.clear();
    (!line.is_empty()).then_some(line)
}

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
    let emit_done = |success: bool| {
        let _ = app.emit(
            "external-cli-install",
            InstallLogEvent {
                agent_id: agent_id.clone(),
                line: None,
                done: true,
                success,
            },
        );
    };
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

    if let Err(err) = prepare_npm_toolchain(&agent_id, &plan, &emit).await {
        emit_done(false);
        return Err(err);
    }

    let mut command = crate::external_agents::spawn::cli_command(&plan.program);
    configure_install_command(&mut command, &plan);
    let child = command
        .no_console_window()
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            emit_done(false);
            if e.kind() == std::io::ErrorKind::NotFound && uses_npm(&plan) {
                missing_node_message(agent_id == "dsh")
            } else {
                format!("启动安装命令失败: {e}")
            }
        })?;

    emit(format!("$ {command_line}"));

    let success = stream_install_child(child, &emit, INSTALL_TIMEOUT_SECS).await;
    if success {
        // npm -g 刚把 shims 写进 %APPDATA%\npm；装之前这个目录可能还不存在，
        // PATH 里也就没有它。不刷新的话紧接着的探测会说「未安装」。
        expose_node_on_path();
        if agent_id == "dsh" {
            if let Err(err) = finish_dsh_install(def, &emit).await {
                emit(err.clone());
                emit_done(false);
                return Err(err);
            }
        }
    }

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

    #[test]
    fn dsh_install_and_npm_update_pull_in_the_missing_app_boot_peer() {
        let spec = install_spec("dsh").unwrap();
        let install = install_plan(&spec, HostPlatform::Windows).unwrap();
        assert_eq!(install.program, "npm.cmd");
        assert!(install
            .args
            .contains(&"@deepseek-ai/dsh@latest".to_string()));
        assert!(install
            .args
            .contains(&"--registry=https://registry.npmjs.org/".to_string()));
        assert!(install.args.contains(
            &"--@deepseek-ai:registry=https://registry.npmjs.org/".to_string()
        ));
        assert!(install
            .args
            .contains(&"@deepseek-ai/cordis-plugin-group@latest".to_string()));

        let update = update_plan(
            "dsh",
            &spec,
            Path::new(r"C:\Users\u\AppData\Roaming\npm\dsh.cmd"),
            HostPlatform::Windows,
        )
        .unwrap();
        assert!(update
            .args
            .contains(&"@deepseek-ai/cordis-plugin-group@latest".to_string()));
        assert!(update
            .args
            .contains(&"--registry=https://registry.npmjs.org/".to_string()));
    }

    #[test]
    fn npm_install_pins_official_registry_and_deepseek_scope() {
        let dsh = npm_install_plan(
            "@deepseek-ai/dsh",
            &["@deepseek-ai/cordis-plugin-group@latest"],
            HostPlatform::Windows,
        );
        assert_eq!(dsh.program, "npm.cmd");
        assert!(dsh
            .args
            .contains(&"--registry=https://registry.npmjs.org/".to_string()));
        assert!(dsh.args.contains(
            &"--@deepseek-ai:registry=https://registry.npmjs.org/".to_string()
        ));

        let claude = npm_install_plan("@anthropic-ai/claude-code", &[], HostPlatform::Unix);
        assert!(claude
            .args
            .contains(&"--registry=https://registry.npmjs.org/".to_string()));
        assert!(!claude
            .args
            .iter()
            .any(|arg| arg.contains("@deepseek-ai:registry")));
    }

    #[test]
    fn parse_node_version_reads_common_version_lines() {
        assert_eq!(parse_node_version("v24.13.0"), Some((24, 13, 0)));
        assert_eq!(parse_node_version("22.19.1"), Some((22, 19, 1)));
        assert_eq!(parse_node_version("v22.19.0-pre"), Some((22, 19, 0)));
        assert_eq!(parse_node_version("not a version"), None);
    }

    #[test]
    fn dsh_node_engine_matches_official_range() {
        assert!(!node_meets_dsh((18, 20, 0)));
        assert!(!node_meets_dsh((20, 19, 0)));
        assert!(!node_meets_dsh((22, 18, 0)));
        assert!(node_meets_dsh((22, 19, 0)));
        assert!(!node_meets_dsh((23, 0, 0)));
        assert!(node_meets_dsh((24, 0, 0)));
        assert!(node_meets_dsh((25, 1, 0)));
    }

    #[test]
    fn blank_machine_errors_point_at_the_download_page() {
        let dsh = missing_node_message(true);
        assert!(dsh.contains("nodejs.org"));
        assert!(dsh.contains("22.19"));
        let other = missing_node_message(false);
        assert!(other.contains("nodejs.org"));
        assert!(dsh_node_too_old_message("v18.20.0").contains("v18.20.0"));
    }

    #[test]
    fn progress_lines_split_on_cr_and_lf() {
        let mut acc = Vec::new();
        assert_eq!(
            take_progress_lines(&mut acc, b"downloading\radded 12 packages\n"),
            vec!["downloading", "added 12 packages"]
        );
        assert!(acc.is_empty());

        assert_eq!(
            take_progress_lines(&mut acc, b"partial"),
            Vec::<String>::new()
        );
        assert_eq!(flush_progress_line(&mut acc).as_deref(), Some("partial"));
        assert_eq!(
            take_progress_lines(&mut acc, b"\r\n  \nwarn\r\n"),
            vec!["warn"]
        );
    }
}
