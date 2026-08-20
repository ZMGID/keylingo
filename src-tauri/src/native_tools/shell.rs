use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

use super::{resolve_tool_existing_dir, NativeToolWorkspace};
use crate::settings::{CHAT_TOOL_MAX_TIMEOUT_MS, CHAT_TOOL_MIN_TIMEOUT_MS};
use crate::state::AppState;

const COMMAND_DENYLIST: &[&str] = &[
    "sudo ",
    "sudo\n",
    "rm -rf /",
    "rm -rf /*",
    ":(){ :|:& };:",
    "mkfs.",
    "dd if=/dev/zero",
    "> /dev/sd",
];

const HOST_PYTHON_PACKAGE_INSTALL_PATTERNS: &[&str] = &[
    "pip install",
    "pip3 install",
    "python -m pip install",
    "python3 -m pip install",
    "uv pip install",
];

/// Dev servers and other long-running processes are spawned in the background.
const LONG_RUNNING_DEV_PATTERNS: &[&str] = &[
    "tauri dev",
    "npm run tauri dev",
    "npm run dev",
    "npm run dev:",
    "next dev",
    "nuxt dev",
    "webpack serve",
    "webpack-dev-server",
    "cargo watch",
    "flutter run",
    "expo start",
    "deno task dev",
];

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn apply_shell_tool_env(cmd: &mut Command, state: Option<&AppState>) {
    // 环境净化（对齐 codex 等）：关掉彩色输出与交互式分页，让给模型读的命令输出更干净。
    // 只设"降噪"类，刻意不设 CI=1 —— 那会改变部分工具行为（如 CRA 把 warning 当 error、
    // 某些 dev server 行为变样），本次目标仅是可读性，不改行为。无 AppState（headless
    // kivio-code）也应用，故放在 state 早返回之前。
    cmd.env("TERM", "dumb");
    cmd.env("NO_COLOR", "1");
    cmd.env("FORCE_COLOR", "0");
    cmd.env("PAGER", "cat");

    let Some(state) = state else {
        return;
    };
    let settings = state.settings_read();
    // PATH 合并：启用插件 bin 目录，再接系统 Path。
    let plugin_dirs = crate::plugins::enabled_bin_dirs();
    if !plugin_dirs.is_empty() {
        #[cfg(windows)]
        let sep = ";";
        #[cfg(not(windows))]
        let sep = ":";
        #[cfg(windows)]
        let key = "Path";
        #[cfg(not(windows))]
        let key = "PATH";

        let mut next = std::ffi::OsString::new();
        for dir in &plugin_dirs {
            if !next.is_empty() {
                next.push(sep);
            }
            next.push(dir.as_os_str());
        }
        let existing = std::env::var_os(key).unwrap_or_default();
        if !existing.is_empty() {
            if !next.is_empty() {
                next.push(sep);
            }
            next.push(existing);
        }
        cmd.env(key, next);
    }
}

/// Windows: the PowerShell executable to run agent shell commands through.
/// Prefer `pwsh` (PowerShell 7+: UTF-8 by default, supports `&&`) when it is on
/// PATH; otherwise fall back to `powershell` (Windows PowerShell 5.1, always
/// present on Windows). Cached for the process lifetime. Mirrors opencode's
/// `win()` shell precedence (pwsh → powershell → …). We run via PowerShell (not
/// `cmd.exe`) so the model's natural modern one-liners work — `cmd.exe` steers
/// models toward removed/hanging tools like `wmic`.
#[cfg(target_os = "windows")]
fn windows_powershell_exe() -> &'static str {
    use std::sync::OnceLock;
    static SHELL: OnceLock<&'static str> = OnceLock::new();
    SHELL.get_or_init(|| if pwsh_on_path() { "pwsh" } else { "powershell" })
}

/// Whether `pwsh.exe` (PowerShell 7+) is discoverable on PATH. Scans PATH
/// directly to avoid pulling a `which` dependency.
#[cfg(target_os = "windows")]
fn pwsh_on_path() -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join("pwsh.exe").is_file())
    })
}

/// Prefix a PowerShell command so its stdout is emitted as UTF-8. Windows
/// PowerShell 5.1 defaults `[Console]::OutputEncoding` to the OEM code page, so
/// without this CJK/non-ASCII output arrives mojibake'd when we read the pipe as
/// UTF-8. pwsh 7 already defaults to UTF-8, so the prefix is a harmless no-op
/// there. The `try/catch` guards the rare case where setting the encoding throws
/// (no console) — the user command still runs.
#[cfg(target_os = "windows")]
fn wrap_ps_command(command: &str) -> String {
    format!(
        "try {{ [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 }} catch {{}}; {command}"
    )
}

/// Windows: absolute path to Git Bash's `bash.exe`, if discoverable. Cached for
/// the process lifetime — installing/uninstalling Git for Windows requires a
/// Kivio restart to be picked up, which is an accepted tradeoff (see R4 design
/// notes; existsSync-per-call like pi does is cheap in Node, less clean in Rust).
///
/// Resolution order (pi's `getShellConfig` precedence, minus the hard error):
/// 1. Known install locations, in order: `%ProgramFiles%\Git\bin\bash.exe` →
///    `%ProgramFiles(x86)%\Git\bin\bash.exe` →
///    `%LocalAppData%\Programs\Git\bin\bash.exe`. Missing env vars just skip
///    that probe rather than failing.
/// 2. Fallback: `where.exe bash.exe`, first line only, re-verified with
///    `Path::is_file` — `where` is known to surface stale/ghost PATH entries.
/// 3. Reject anything resolving to the WSL bash shim
///    (`...\Windows\System32\bash.exe` / `...\Windows\sysnative\bash.exe`):
///    its `/mnt/c/...` filesystem view does not match the Windows paths Kivio
///    passes around, unlike pi (which special-cases WSL via stdin), Kivio
///    just excludes it.
///
/// `None` means "no usable Git Bash" — callers fall back to the existing
/// PowerShell path unchanged. Unlike pi/Claude Code, Kivio never hard-errors:
/// Git for Windows is not an install prerequisite.
#[cfg(target_os = "windows")]
pub fn find_git_bash() -> Option<PathBuf> {
    use std::sync::OnceLock;
    static GIT_BASH: OnceLock<Option<PathBuf>> = OnceLock::new();
    GIT_BASH.get_or_init(detect_git_bash).clone()
}

#[cfg(target_os = "windows")]
fn detect_git_bash() -> Option<PathBuf> {
    let known_locations: [(&str, &[&str]); 3] = [
        ("ProgramFiles", &["Git", "bin", "bash.exe"]),
        ("ProgramFiles(x86)", &["Git", "bin", "bash.exe"]),
        ("LocalAppData", &["Programs", "Git", "bin", "bash.exe"]),
    ];
    for (env_var, tail) in known_locations {
        let Some(base) = std::env::var_os(env_var) else {
            continue;
        };
        let mut candidate = PathBuf::from(base);
        candidate.extend(tail.iter().copied());
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    where_bash_exe().filter(|path| path.is_file() && !is_wsl_bash_path(path))
}

/// Run `where.exe bash.exe` and return the first line as a path, unverified —
/// callers must confirm the file actually exists (`where` is known to surface
/// stale PATH entries that no longer resolve to a real file).
#[cfg(target_os = "windows")]
fn where_bash_exe() -> Option<PathBuf> {
    use crate::proc::NoConsoleWindow;
    let mut cmd = std::process::Command::new("where.exe");
    cmd.arg("bash.exe");
    cmd.no_console_window();
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?.trim();
    if first_line.is_empty() {
        return None;
    }
    Some(PathBuf::from(first_line))
}

/// True when `path` is the WSL bash shim (`...\Windows\System32\bash.exe` or
/// `...\Windows\sysnative\bash.exe`), matched case-insensitively. WSL bash's
/// filesystem view (`/mnt/c/...`) does not match the Windows paths Kivio hands
/// to commands, so it must never be selected as the agent's shell even if it
/// is the first `bash.exe` on PATH.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_wsl_bash_path(path: &Path) -> bool {
    let lowered = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\");
    lowered.ends_with(r"\windows\system32\bash.exe")
        || lowered.ends_with(r"\windows\sysnative\bash.exe")
}

/// Shell-syntax sentence appended to the `bash` tool description so the model
/// always knows which shell its commands actually run in (opencode #16479:
/// invisible shell auto-detection makes models guess the wrong syntax). Only
/// non-empty on Windows when Git Bash was selected — the PowerShell fallback
/// keeps the existing description untouched, and Unix is bash-like already.
/// The forward-slash rule guards against opencode #15810: models writing
/// `C:\Users\...` into bash, where `\U` is eaten as an escape sequence.
pub fn run_command_shell_hint() -> &'static str {
    #[cfg(target_os = "windows")]
    if find_git_bash().is_some() {
        return " On this machine commands run in Git Bash — use bash syntax (pipes, heredoc, `$VAR`, `$(seq ...)`), NOT PowerShell cmdlets. Write Windows paths with forward slashes (C:/Users/...) — backslashes are escape characters in bash.";
    }
    ""
}

/// Build the PowerShell invocation of `command` (`<pwsh|powershell> -NoLogo
/// -NoProfile -NonInteractive -Command …`). Split out of `build_shell_command`
/// so tests can exercise the PowerShell-specific quoting/UTF-8 behavior
/// directly, independent of whether this machine also has Git Bash installed
/// (which would otherwise make `build_shell_command` pick bash instead).
#[cfg(target_os = "windows")]
fn build_windows_powershell_command(command: &str) -> Command {
    let mut c = Command::new(windows_powershell_exe());
    // 用 .arg()(而非 cmd.exe 路径的 raw_arg):PowerShell 用标准 CommandLineToArgvW
    // 解析自身参数,认 \" 转义。把整段脚本作为**单个带引号参数**传给 -Command,内部
    // 引号才能原样还原(python -c "..." 不被拆散)。raw_arg 会让脚本裸拼到命令行,
    // PowerShell 的 argv 分词会在 -Command 处理前吃掉内部引号 → 命令被拆坏。
    c.arg("-NoLogo");
    c.arg("-NoProfile");
    c.arg("-NonInteractive");
    c.arg("-Command");
    c.arg(wrap_ps_command(command));
    c
}

/// Build the platform shell `Command` that runs `command`. Windows prefers Git
/// Bash when discoverable (`find_git_bash`): `bash.exe -c <command>`, the
/// whole command as a single `.arg()` (same shape as the PowerShell branch
/// below — bash's own argv parsing keeps quoting/heredocs intact). No Git
/// Bash found → unchanged fallback to PowerShell
/// (`<pwsh|powershell> -NoLogo -NoProfile -NonInteractive -Command …`); every
/// other platform uses `sh -c`. Callers still set cwd/stdio/env/creation
/// flags/kill_on_drop themselves — this only owns the program + argument shape so
/// the foreground and background paths cannot drift apart.
pub(crate) fn build_shell_command(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        if let Some(bash) = find_git_bash() {
            let mut c = Command::new(bash);
            // 与 PowerShell 分支同法:.arg() 把整段命令当单个 argv 传给 bash 的
            // -c,由 bash 自己的引号/转义规则解析,不在 Kivio 侧二次拆分。
            c.arg("-c");
            c.arg(command);
            return c;
        }
        build_windows_powershell_command(command)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

pub async fn run_command(
    workspace: &NativeToolWorkspace,
    default_timeout_ms: u64,
    arguments: &Value,
    state: Option<&AppState>,
    conversation_id: Option<&str>,
) -> Result<String, String> {
    let command = arguments
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "run_command requires command".to_string())?;

    let lowered = command.to_ascii_lowercase();
    for denied in COMMAND_DENYLIST {
        if lowered.contains(denied) {
            return Err("command is blocked by safety policy".to_string());
        }
    }
    let allow_host_python_package_install = arguments
        .get("allow_host_python_package_install")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !allow_host_python_package_install {
        for denied in HOST_PYTHON_PACKAGE_INSTALL_PATTERNS {
            if lowered.contains(denied) {
                return Err(
                    "run_command cannot install Python packages or modify the host Python environment unless allow_host_python_package_install is true. Do not retry with variants — if the user explicitly wants host installs, create/activate a venv and pass allow_host_python_package_install=true."
                        .to_string(),
                );
            }
        }
    } else if HOST_PYTHON_PACKAGE_INSTALL_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
    {
        if !lowered.contains("--user")
            && !lowered.contains("venv")
            && !lowered.contains(".venv")
            && !lowered.contains("virtualenv")
        {
            return Err(
                "Host Python package installs must target a user or virtual environment; add --user or run inside a venv."
                    .to_string(),
            );
        }
    }

    let explicit_cwd = arguments
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty());

    let (command, cd_extracted) = normalize_run_command(command, explicit_cwd)?;

    let cwd = if let Some(cd_path) = cd_extracted.as_deref() {
        resolve_tool_existing_dir(workspace, Some(cd_path))?
    } else {
        resolve_command_cwd(arguments, workspace)?
    };

    if !cwd.is_dir() {
        return Err(format!(
            "Working directory is not a directory: {}",
            cwd.display()
        ));
    }

    let background = arguments
        .get("background")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| is_long_running_dev_command(&command));
    if background {
        return run_shell_command_background(&command, cwd, state, conversation_id).await;
    }

    let timeout_ms = arguments
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(default_timeout_ms)
        .clamp(CHAT_TOOL_MIN_TIMEOUT_MS, CHAT_TOOL_MAX_TIMEOUT_MS)
        .max(default_timeout_ms);

    let output = run_shell_command(&command, cwd, timeout_ms, state).await?;
    let formatted = offload_large_output(format_command_output(&output));
    if let Some(code) = output.status_code {
        if code != 0 {
            return Err(formatted);
        }
    }
    Ok(formatted)
}

/// Above this size, a command's full output is written to a temp file and the
/// returned text is tail-truncated. The full log path is noted in the head of
/// the returned text so the model can read the complete output if needed.
/// `files.rs` also cites it: a `head -c` bigger than this would be offloaded to
/// a log the model then has to `read` — which is what it was escaping in the
/// first place.
pub(super) const MAX_INLINE_COMMAND_OUTPUT_BYTES: usize = 16 * 1024;

/// Tail-truncation caps for the inline body: keep the END of the output (where
/// errors and final results live), bounded by both a line count and a byte size,
/// whichever hits first. Same budget as `read` — one shared pair of numbers so
/// no tool can be the one that forgot its output ceiling (pi `truncate.ts`).
const TAIL_MAX_LINES: usize = super::TOOL_OUTPUT_MAX_LINES;
const TAIL_MAX_BYTES: usize = super::TOOL_OUTPUT_MAX_BYTES;

/// Keep the LAST `TAIL_MAX_LINES` lines / `TAIL_MAX_BYTES` bytes of `text`,
/// dropping earlier lines. Returns `(kept_text, dropped_line_count)` where a
/// non-zero count means truncation happened. Whole lines only (never a partial
/// line), and the byte budget is applied after the line budget.
fn tail_truncate(text: &str) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    // First, cap by line count (keep the tail).
    let mut start = total.saturating_sub(TAIL_MAX_LINES);
    // Then walk backward dropping leading lines until the kept tail fits the byte
    // budget (counting the trailing newline each line contributes).
    let mut kept_bytes: usize = lines[start..].iter().map(|line| line.len() + 1).sum();
    while kept_bytes > TAIL_MAX_BYTES && start < total {
        kept_bytes -= lines[start].len() + 1;
        start += 1;
    }
    if start == 0 {
        return (text.to_string(), 0);
    }
    let kept = lines[start..].join("\n");
    (kept, start)
}

fn offload_large_output(formatted: String) -> String {
    if formatted.len() <= MAX_INLINE_COMMAND_OUTPUT_BYTES {
        return formatted;
    }
    let lines = formatted.lines().count();
    let bytes = formatted.len();
    let path = std::env::temp_dir().join(format!("kivio-bash-{}.log", uuid::Uuid::new_v4()));
    let log_note = match std::fs::write(&path, &formatted) {
        Ok(()) => Some(format!(
            "[full output: {lines} lines, {bytes} bytes — complete log saved to {}. Read it with the `read` tool (use offset/limit or grep it) if the tail below is not enough.]",
            path.display()
        )),
        // Best-effort: if the temp write fails, still tail-truncate inline.
        Err(_) => None,
    };

    // Keep the END of the output — errors and final results live there.
    let (tail, dropped) = tail_truncate(&formatted);
    let mut out = String::new();
    if let Some(note) = log_note {
        out.push_str(&note);
        out.push('\n');
    }
    if dropped > 0 {
        out.push_str(&format!("[... {dropped} earlier lines truncated ...]\n"));
    }
    out.push_str(&tail);
    out
}

fn resolve_command_cwd(
    arguments: &Value,
    workspace: &NativeToolWorkspace,
) -> Result<PathBuf, String> {
    resolve_tool_existing_dir(workspace, arguments.get("cwd").and_then(|v| v.as_str()))
}

/// Reject fragile `cd ... &&` prefixes; auto-strip simple `cd foo &&` forms.
fn normalize_run_command(
    command: &str,
    explicit_cwd: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let Some((cd_path, rest)) = parse_leading_cd_prefix(command) else {
        return Ok((command.to_string(), None));
    };

    if explicit_cwd.is_some() {
        return Err(
            "run_command: do not combine the `cwd` parameter with `cd ... &&` in `command`. \
             Set `cwd` to the target directory and run only the remaining shell command."
                .to_string(),
        );
    }

    if cd_path.contains(' ') {
        return Err(format!(
            "run_command: paths with spaces must use the `cwd` parameter instead of `cd ... &&`.\n\
             Suggested cwd: {cd_path}\n\
             Suggested command: {rest}"
        ));
    }

    Ok((rest, Some(cd_path)))
}

fn parse_leading_cd_prefix(command: &str) -> Option<(String, String)> {
    let trimmed = command.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("cd ") {
        return None;
    }

    let after_cd = trimmed.get(3..)?.trim_start();
    let (path_part, rest) = find_cd_and_separator(after_cd)?;
    let cd_path = strip_shell_quotes(path_part.trim());
    let rest = rest.trim();
    if cd_path.is_empty() || rest.is_empty() {
        return None;
    }
    Some((cd_path, rest.to_string()))
}

fn find_cd_and_separator(command: &str) -> Option<(&str, &str)> {
    for pattern in [" && ", "&&"] {
        if let Some(idx) = command.find(pattern) {
            let path = command.get(..idx)?.trim();
            let rest = command.get(idx + pattern.len()..)?.trim();
            if !path.is_empty() && !rest.is_empty() {
                return Some((path, rest));
            }
        }
    }
    None
}

fn strip_shell_quotes(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn is_long_running_dev_command(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    if LONG_RUNNING_DEV_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
    {
        return true;
    }

    if lowered.contains("vite build") || lowered.contains("vite preview") {
        return false;
    }

    lowered.starts_with("vite")
        || lowered.starts_with("npx vite")
        || lowered.contains(" npx vite")
        || lowered.contains("&& vite")
        || lowered.contains("; vite")
}

/// Filename prefix for per-job background-command logs in temp_dir. App startup
/// GC and the app-exit sweep both look for this prefix.
pub const BG_CMD_LOG_PREFIX: &str = "kivio-bgcmd-";

/// Lifecycle of a tracked background command. Mirrors the MCP Tasks status
/// vocabulary (running/completed/failed/cancelled) in Kivio terms.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum BackgroundCommandStatus {
    /// Process still alive (or we have not yet observed it exit).
    Running,
    /// Process exited on its own; `code` is the OS exit code if available.
    Exited { code: Option<i32> },
    /// Process group was killed via `kill_background` or an app-exit sweep.
    Killed,
    /// We failed to spawn / wait on the process; `message` describes why.
    Error { message: String },
}

impl BackgroundCommandStatus {
    fn is_terminal(&self) -> bool {
        !matches!(self, BackgroundCommandStatus::Running)
    }
}

/// A registered background command. Holds the leader pid (for process-group
/// kill) and the path to the per-job output log. Survives across turns; cleaned
/// up only by `kill_background` or the app-exit sweep.
///
/// `kill_tx` signals the owning waiter task to kill the process group and reap
/// it. Killing is driven through the waiter (which still owns the live `Child`)
/// rather than by signaling `pid` directly from the lock holder: that closes a
/// reap-then-kill TOCTOU where a job exits, the waiter returns from `wait()`
/// (releasing the pid/pgid back to the OS) but has not yet taken the lock to set
/// a terminal status, and a concurrent `kill_background` / app-exit sweep reads
/// the still-`Running` record and `kill`s a pid the OS may have reused.
#[derive(Debug)]
pub struct BackgroundCommand {
    pub job_id: String,
    /// 发起该作业的会话 id。`bash_output`（列表模式）/`kill_background` 只看本会话的
    /// 作业，避免 A 会话列出、甚至 kill 掉 B 会话起的进程。`None` = 无会话上下文
    /// （测试里种的作业 / 非 agent 路径），此时不参与会话过滤。
    pub conversation_id: Option<String>,
    pub pid: Option<u32>,
    pub command: String,
    pub cwd: String,
    pub log_path: PathBuf,
    pub status: BackgroundCommandStatus,
    pub started_at: SystemTime,
    /// One-shot kill signal to the owning waiter task. `None` once consumed (the
    /// kill has been requested) or when there is no live waiter (seeded jobs in
    /// tests). Sending requests a process-group kill + reap inside the waiter,
    /// which is the only place that still holds the live `Child`.
    pub kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Kill an entire process group / tree given the leader pid. Unix: SIGTERM the
/// process group then SIGKILL as a fallback. Windows: `taskkill /T /F` walks the
/// child tree. macOS+Linux both spawn the group via `setsid`, so `-pid` targets
/// the whole group.
pub fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        let gid = pid as libc::pid_t;
        unsafe {
            // Graceful first; if the group ignores SIGTERM, follow with SIGKILL.
            libc::kill(-gid, libc::SIGTERM);
            libc::kill(-gid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // /T kills the whole tree, /F forces it. Detached background commands
        // are their own process group (CREATE_NEW_PROCESS_GROUP), so this
        // reaches the children too. CREATE_NO_WINDOW suppresses the taskkill
        // console flash.
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
    }
}

async fn run_shell_command_background(
    command: &str,
    cwd: PathBuf,
    state: Option<&AppState>,
    conversation_id: Option<&str>,
) -> Result<String, String> {
    let job_id = uuid::Uuid::new_v4().to_string();
    let log_path = std::env::temp_dir().join(format!("{BG_CMD_LOG_PREFIX}{job_id}.log"));

    // Open the per-job log for stdout+stderr capture. Two independent handles so
    // both streams write concurrently (the OS interleaves appends).
    let stdout_file = std::fs::File::create(&log_path)
        .map_err(|err| format!("Failed to create background log file: {err}"))?;
    let stderr_file = stdout_file
        .try_clone()
        .map_err(|err| format!("Failed to clone background log handle: {err}"))?;

    let mut cmd = build_shell_command(command);
    cmd.current_dir(cwd.as_path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(stdout_file))
        .stderr(std::process::Stdio::from(stderr_file));
    apply_shell_tool_env(&mut cmd, state);
    #[cfg(target_os = "windows")]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        // 只用 CREATE_NO_WINDOW（+ 新进程组，便于 taskkill /T 按树杀）：cmd.exe 拿到隐藏
        // 控制台，其 npm/vite/electron 子孙进程继承该隐藏控制台 → 全程无可见窗口。
        // 不要叠加 DETACHED_PROCESS：按 MSDN，它与 CREATE_NO_WINDOW 同设会使后者失效，
        // 且让 cmd.exe 无控制台可继承 → 控制台子孙各自新建**可见**窗口（就是那个黑框）。
        // 跨轮存活由 kill_on_drop(false) + waiter 持有 Child 保证，不依赖 DETACHED_PROCESS。
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    // Keep the child alive when its handle is dropped: background commands
    // survive across turns and are killed only by kill_background / app-exit.
    cmd.kill_on_drop(false);
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Failed to start background command: {err}"))?;
    let pid = child.id();
    let pid_text = pid
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // No registry (e.g. headless kivio-code without an AppState): fall back to
    // the legacy fire-and-forget shape so behavior never regresses.
    let Some(state) = state else {
        return Ok(format!(
            "background: true\npid: {pid_text}\ncwd: {}\ncommand: {command}\n\nStarted in the background. (No job registry available in this context; the process keeps running and is not tracked.)\n",
            cwd.display()
        ));
    };

    // One-shot kill channel: the waiter task owns the live Child, so it is the
    // only place that can safely race a self-exit against a kill request. The
    // registry only holds the sender, never kills a pid directly.
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

    state.register_background_command(BackgroundCommand {
        job_id: job_id.clone(),
        conversation_id: conversation_id.map(str::to_string),
        pid,
        command: command.to_string(),
        cwd: cwd.display().to_string(),
        log_path: log_path.clone(),
        status: BackgroundCommandStatus::Running,
        started_at: SystemTime::now(),
        kill_tx: Some(kill_tx),
    });

    // Reap the child off-thread: race a self-exit against a kill request. The
    // waiter owns the Child for its whole lifetime, so a kill always targets the
    // still-live process group (no reap-then-kill TOCTOU). The OS keeps writing
    // the log until the process exits.
    let waiter_state = state.background_commands_handle();
    let waiter_job = job_id.clone();
    tauri::async_runtime::spawn(async move {
        let status = tokio::select! {
            reaped = child.wait() => match reaped {
                Ok(exit) => BackgroundCommandStatus::Exited { code: exit.code() },
                Err(err) => BackgroundCommandStatus::Error {
                    message: format!("wait failed: {err}"),
                },
            },
            _ = kill_rx => {
                // Kill the whole process group (children too), then reap the
                // leader. We still own `child`, so `pid` cannot have been reused.
                if let Some(pid) = pid {
                    kill_process_group(pid);
                }
                let _ = child.wait().await;
                BackgroundCommandStatus::Killed
            }
        };
        let mut map = waiter_state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = map.get_mut(&waiter_job) {
            // Do not clobber a Killed status set by kill_background; the kill
            // path is the authority on a killed job's terminal status.
            if !matches!(job.status, BackgroundCommandStatus::Killed) {
                job.status = status;
            }
        }
    });

    Ok(format!(
        "background: true\njob_id: {job_id}\npid: {pid_text}\ncwd: {}\ncommand: {command}\n\nStarted in the background; it keeps running after this tool returns and survives across turns until you call kill_background or the app exits. Poll its output and exit status with bash_output (job_id: {job_id}); list all background jobs by calling bash_output with no job_id; stop it with kill_background (job_id: {job_id}). Do not start the same dev server twice.\n",
        cwd.display()
    ))
}

/// `bash_output` tool: incremental read of a tracked background job's captured
/// output since `since_offset`, plus current status and exit code.
/// 该作业是否属于调用方会话。调用方无会话上下文（`None`）时不设限（headless /
/// 测试路径）；作业本身无会话归属时同样放行（旧作业 / 测试种入）。
fn job_visible_to(job: &BackgroundCommand, caller: Option<&str>) -> bool {
    match (caller, job.conversation_id.as_deref()) {
        (Some(caller), Some(owner)) => caller == owner,
        _ => true,
    }
}

pub fn bash_output(
    state: &AppState,
    arguments: &Value,
    conversation_id: Option<&str>,
) -> Result<String, String> {
    let job_id = arguments
        .get("job_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "bash_output requires job_id".to_string())?;
    let since_offset = arguments
        .get("since_offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let (status, log_path, command) = {
        let map = state.background_commands_handle();
        let map = map.lock().unwrap_or_else(|e| e.into_inner());
        let job = map
            .get(job_id)
            .filter(|job| job_visible_to(job, conversation_id))
            .ok_or_else(|| format!("No background job with job_id {job_id}"))?;
        (
            job.status.clone(),
            job.log_path.clone(),
            job.command.clone(),
        )
    };

    let bytes = std::fs::read(&log_path).unwrap_or_default();
    let start = (since_offset as usize).min(bytes.len());
    let new_text = String::from_utf8_lossy(&bytes[start..]).into_owned();
    let new_offset = bytes.len() as u64;

    let status_line = match &status {
        BackgroundCommandStatus::Running => "status: running".to_string(),
        BackgroundCommandStatus::Exited { code } => match code {
            Some(c) => format!("status: exited\nexit_code: {c}"),
            None => "status: exited\nexit_code: unknown".to_string(),
        },
        BackgroundCommandStatus::Killed => "status: killed".to_string(),
        BackgroundCommandStatus::Error { message } => format!("status: error\nerror: {message}"),
    };

    let header =
        format!("job_id: {job_id}\ncommand: {command}\n{status_line}\nnext_offset: {new_offset}\n");
    let body = if new_text.is_empty() {
        if status.is_terminal() {
            "(no new output)".to_string()
        } else {
            "(no new output yet; poll again)".to_string()
        }
    } else {
        offload_large_output(format!("output:\n{new_text}"))
    };
    Ok(format!("{header}\n{body}"))
}

/// `list_background` tool: list this conversation's tracked background jobs.
pub fn list_background(
    state: &AppState,
    _arguments: &Value,
    conversation_id: Option<&str>,
) -> Result<String, String> {
    let map = state.background_commands_handle();
    let map = map.lock().unwrap_or_else(|e| e.into_inner());
    let mut jobs: Vec<&BackgroundCommand> = map
        .values()
        .filter(|job| job_visible_to(job, conversation_id))
        .collect();
    if jobs.is_empty() {
        return Ok("(no background jobs)".to_string());
    }
    jobs.sort_by_key(|j| j.started_at);
    let mut out = String::new();
    for job in jobs {
        let status = match &job.status {
            BackgroundCommandStatus::Running => "running".to_string(),
            BackgroundCommandStatus::Exited { code } => match code {
                Some(c) => format!("exited(code={c})"),
                None => "exited".to_string(),
            },
            BackgroundCommandStatus::Killed => "killed".to_string(),
            BackgroundCommandStatus::Error { message } => format!("error({message})"),
        };
        let age_secs = job.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        out.push_str(&format!(
            "job_id: {}\n  status: {status}\n  command: {}\n  cwd: {}\n  started: {age_secs}s ago\n",
            job.job_id, job.command, job.cwd
        ));
    }
    Ok(out)
}

/// `kill_background` tool: kill a tracked job's process group and mark it Killed.
/// Only the owning conversation's jobs are addressable.
pub fn kill_background(
    state: &AppState,
    arguments: &Value,
    conversation_id: Option<&str>,
) -> Result<String, String> {
    let job_id = arguments
        .get("job_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "kill_background requires job_id".to_string())?;

    let map = state.background_commands_handle();
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    let job = map
        .get_mut(job_id)
        .filter(|job| job_visible_to(job, conversation_id))
        .ok_or_else(|| format!("No background job with job_id {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(format!(
            "job_id: {job_id} already finished (status unchanged); nothing to kill."
        ));
    }
    // Mark Killed under the lock, then signal the waiter to kill+reap. The
    // waiter still owns the live Child, so it kills the live process group
    // rather than a pid this lock holder might read after a reap (TOCTOU). If
    // there is no waiter (e.g. a seeded test job), fall back to a direct
    // process-group kill of the recorded pid.
    job.status = BackgroundCommandStatus::Killed;
    match job.kill_tx.take() {
        Some(kill_tx) => {
            let _ = kill_tx.send(());
        }
        None => {
            if let Some(pid) = job.pid {
                kill_process_group(pid);
            }
        }
    }
    Ok(format!("job_id: {job_id} killed."))
}

#[derive(Debug)]
struct CommandOutput {
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
}

async fn run_shell_command(
    command: &str,
    cwd: PathBuf,
    timeout_ms: u64,
    state: Option<&AppState>,
) -> Result<CommandOutput, String> {
    exec_shell_command(build_shell_command(command), cwd, timeout_ms, state).await
}

/// Spawn an already-built shell `Command` and capture its output with a
/// timeout. Split from `run_shell_command` so tests can exercise a specific
/// shell builder (e.g. the PowerShell fallback) regardless of which shell
/// `build_shell_command` would pick on this machine.
async fn exec_shell_command(
    mut cmd: Command,
    cwd: PathBuf,
    timeout_ms: u64,
    state: Option<&AppState>,
) -> Result<CommandOutput, String> {
    cmd.current_dir(cwd)
        // stdin 必须为 null:coding-agent 的 shell 命令绝不能读交互终端的 stdin。
        // 否则子进程会继承父进程(TUI 的 pty)stdin,抢占/消费它 → TUI 输入线程 EOF → 会话中途退出。
        // null stdin 意味着任何尝试读 stdin 的命令立即得到 EOF,而非偷走 TUI 输入。
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    apply_shell_tool_env(&mut cmd, state);
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.kill_on_drop(true);
    #[cfg(target_os = "macos")]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .map_err(|err| format!("Failed to start command: {err}"))?;
    let child_pid = child.id();

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| {
        terminate_command_group(child_pid);
        format!("Command timed out after {timeout_ms}ms")
    })?
    .map_err(|err| format!("Command failed: {err}"))?;

    Ok(CommandOutput {
        status_code: result.status.code(),
        stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
    })
}

#[cfg(target_os = "macos")]
fn terminate_command_group(child_pid: Option<u32>) {
    if let Some(pid) = child_pid {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn terminate_command_group(_child_pid: Option<u32>) {}

fn format_command_output(output: &CommandOutput) -> String {
    let mut out = String::new();
    if let Some(code) = output.status_code {
        out.push_str(&format!("exit_code: {code}\n"));
    }
    if !output.stdout.is_empty() {
        out.push_str("stdout:\n");
        out.push_str(&output.stdout);
        if !output.stdout.ends_with('\n') {
            out.push('\n');
        }
    }
    if !output.stderr.is_empty() {
        out.push_str("stderr:\n");
        out.push_str(&output.stderr);
        if !output.stderr.ends_with('\n') {
            out.push('\n');
        }
    }
    if out.trim().is_empty() {
        out.push_str("(no output)\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_tools::user_home_dir;

    #[test]
    fn default_cwd_uses_first_workspace_root_when_configured() {
        let home = user_home_dir().expect("home should be available in tests");
        let root = home.join(".kivio-chat-test-root");
        std::fs::create_dir_all(&root).expect("mkdir");
        let args = serde_json::json!({ "command": "pwd" });
        let workspace = NativeToolWorkspace::global(&[root.to_string_lossy().into_owned()]);
        let cwd = resolve_command_cwd(&args, &workspace).expect("workspace root should resolve");
        let canonical_root = std::fs::canonicalize(&root).expect("canonical workspace root");

        assert_eq!(cwd, canonical_root);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn command_cwd_allows_temp_directory_outside_home() {
        let dir = std::env::temp_dir().join(format!("kivio_cmd_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let args = serde_json::json!({ "command": "pwd", "cwd": dir.to_string_lossy() });
        let workspace = NativeToolWorkspace::global(&[]);
        let cwd = resolve_command_cwd(&args, &workspace).expect("temp cwd should resolve");

        assert_eq!(
            cwd,
            std::fs::canonicalize(&dir).expect("canonical temp dir")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn apply_shell_tool_env_injects_output_hygiene() {
        // 无 AppState（headless 路径）也应注入降噪 env；刻意不含 CI。
        let mut cmd = Command::new("cmd");
        apply_shell_tool_env(&mut cmd, None);
        let envs: std::collections::HashMap<String, Option<String>> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(envs.get("TERM"), Some(&Some("dumb".to_string())));
        assert_eq!(envs.get("NO_COLOR"), Some(&Some("1".to_string())));
        assert_eq!(envs.get("FORCE_COLOR"), Some(&Some("0".to_string())));
        assert_eq!(envs.get("PAGER"), Some(&Some("cat".to_string())));
        // 刻意不设 CI（避免改变工具行为，如 CRA warning-as-error）。
        assert!(!envs.contains_key("CI"));
    }

    #[test]
    fn format_command_output_includes_nonzero_exit_code() {
        let output = CommandOutput {
            status_code: Some(1),
            stdout: String::new(),
            stderr: "boom\n".to_string(),
        };

        let formatted = format_command_output(&output);

        assert!(formatted.contains("exit_code: 1"));
        assert!(formatted.contains("stderr:\nboom"));
    }

    #[test]
    fn offload_large_output_passes_small_output_through() {
        let small = "exit_code: 0\nstdout:\nhello\n".to_string();
        assert_eq!(offload_large_output(small.clone()), small);
    }

    // ---- Background command registry + polling (PR2) ----

    fn bg_test_state() -> AppState {
        AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir().join("kivio-bgcmd-test-usage"),
        )
    }

    /// A short cross-platform command that prints a known token and exits 0.
    fn echo_command(token: &str) -> String {
        #[cfg(target_os = "windows")]
        {
            format!("echo {token}")
        }
        #[cfg(not(target_os = "windows"))]
        {
            format!("printf '%s' {token}")
        }
    }

    async fn poll_until_terminal(state: &AppState, args: &Value) -> String {
        for _ in 0..100 {
            let out = bash_output(state, args, None).expect("bash_output should succeed");
            if !out.contains("status: running") {
                return out;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("background job never reached a terminal status");
    }

    #[tokio::test]
    async fn background_command_is_tracked_and_output_polls_to_exit() {
        let state = bg_test_state();
        let token = "KIVIO_BG_OK";
        let workspace = NativeToolWorkspace::global(&[]);
        let started = run_command(
            &workspace,
            5_000,
            &serde_json::json!({
                "command": echo_command(token),
                "cwd": std::env::temp_dir().to_string_lossy(),
                "background": true,
            }),
            Some(&state),
            None,
        )
        .await
        .expect("background run_command should return immediately");

        // The dispatch result carries a job_id and tells the model how to poll.
        assert!(
            started.contains("background: true"),
            "missing banner: {started}"
        );
        let job_id = started
            .lines()
            .find_map(|l| l.strip_prefix("job_id: "))
            .map(str::to_string)
            .expect("job_id in background result");
        assert!(started.contains("bash_output"));

        // Registry insert is observable immediately.
        let listed =
            list_background(&state, &serde_json::json!({}), None).expect("list_background");
        assert!(listed.contains(&job_id), "job not listed: {listed}");

        // Poll bash_output until the process exits; assert captured output + code.
        let args = serde_json::json!({ "job_id": job_id });
        let out = poll_until_terminal(&state, &args).await;
        assert!(out.contains("status: exited"), "expected exit: {out}");
        assert!(out.contains("exit_code: 0"), "expected exit_code 0: {out}");
        assert!(
            out.contains(token),
            "captured output should contain token: {out}"
        );
    }

    #[tokio::test]
    async fn bash_output_incremental_offset_reads_only_new_bytes() {
        let state = bg_test_state();
        let job_id = uuid::Uuid::new_v4().to_string();
        let log_path = std::env::temp_dir().join(format!("{BG_CMD_LOG_PREFIX}{job_id}.log"));
        std::fs::write(&log_path, b"hello").expect("seed log");
        state.register_background_command(BackgroundCommand {
            job_id: job_id.clone(),
            conversation_id: None,
            pid: None,
            command: "seed".to_string(),
            cwd: ".".to_string(),
            log_path: log_path.clone(),
            status: BackgroundCommandStatus::Exited { code: Some(0) },
            started_at: SystemTime::now(),
            kill_tx: None,
        });

        // First read from offset 0 sees all bytes and reports next_offset = 5.
        let first = bash_output(&state, &serde_json::json!({ "job_id": job_id }), None).unwrap();
        assert!(first.contains("hello"), "{first}");
        assert!(first.contains("next_offset: 5"), "{first}");

        // Append more, then read from the prior offset → only the new bytes.
        std::fs::write(&log_path, b"helloWORLD").expect("append log");
        let second = bash_output(
            &state,
            &serde_json::json!({ "job_id": job_id, "since_offset": 5 }),
            None,
        )
        .unwrap();
        assert!(second.contains("WORLD"), "{second}");
        assert!(
            !second.contains("hello\n"),
            "should not re-read old bytes: {second}"
        );
        assert!(second.contains("next_offset: 10"), "{second}");

        let _ = std::fs::remove_file(&log_path);
    }

    /// B3: 后台作业按会话隔离 —— A 会话不得列出/读取/kill B 会话起的作业。
    #[test]
    fn background_jobs_are_scoped_to_their_conversation() {
        let state = bg_test_state();
        let seed = |job_id: &str, conv: Option<&str>| {
            let log_path = std::env::temp_dir().join(format!("{BG_CMD_LOG_PREFIX}{job_id}.log"));
            std::fs::write(&log_path, b"x").expect("seed log");
            state.register_background_command(BackgroundCommand {
                job_id: job_id.to_string(),
                conversation_id: conv.map(str::to_string),
                pid: None,
                command: format!("cmd-{job_id}"),
                cwd: ".".to_string(),
                log_path,
                status: BackgroundCommandStatus::Running,
                started_at: SystemTime::now(),
                kill_tx: None,
            });
        };
        let job_a = format!("conv-a-{}", uuid::Uuid::new_v4());
        let job_b = format!("conv-b-{}", uuid::Uuid::new_v4());
        let job_legacy = format!("legacy-{}", uuid::Uuid::new_v4());
        seed(&job_a, Some("conv-a"));
        seed(&job_b, Some("conv-b"));
        seed(&job_legacy, None);

        // 列表：只看到本会话的 + 无归属的旧作业，看不到别的会话。
        let listed = list_background(&state, &serde_json::json!({}), Some("conv-a")).unwrap();
        assert!(listed.contains(&job_a), "own job missing: {listed}");
        assert!(
            !listed.contains(&job_b),
            "other conversation's job leaked: {listed}"
        );
        assert!(
            listed.contains(&job_legacy),
            "unowned job should stay visible: {listed}"
        );

        // 读取输出：跨会话按「不存在」处理。
        assert!(bash_output(
            &state,
            &serde_json::json!({ "job_id": job_b }),
            Some("conv-a")
        )
        .is_err());
        assert!(bash_output(
            &state,
            &serde_json::json!({ "job_id": job_a }),
            Some("conv-a")
        )
        .is_ok());

        // kill：跨会话拒绝，本会话放行。
        assert!(kill_background(
            &state,
            &serde_json::json!({ "job_id": job_b }),
            Some("conv-a")
        )
        .is_err());
        assert!(kill_background(
            &state,
            &serde_json::json!({ "job_id": job_a }),
            Some("conv-a")
        )
        .is_ok());
        // 无会话上下文（UI / headless）不设限：仍能操作任意作业。
        assert!(kill_background(&state, &serde_json::json!({ "job_id": job_b }), None).is_ok());

        for id in [&job_a, &job_b, &job_legacy] {
            let _ = std::fs::remove_file(
                std::env::temp_dir().join(format!("{BG_CMD_LOG_PREFIX}{id}.log")),
            );
        }
    }

    #[tokio::test]
    async fn kill_background_marks_killed_and_terminal_is_noop() {
        let state = bg_test_state();
        let workspace = NativeToolWorkspace::global(&[]);
        // A long-lived command so it is still running when we kill it. On
        // Windows the syntax depends on which shell run_command selected:
        // Git Bash understands `sleep`, the PowerShell fallback needs
        // `Start-Sleep` (a bash-only literal would exit instantly there and
        // race the kill below).
        #[cfg(target_os = "windows")]
        let long = if find_git_bash().is_some() {
            "sleep 30"
        } else {
            "Start-Sleep -Seconds 30"
        };
        #[cfg(not(target_os = "windows"))]
        let long = "sleep 30";
        let started = run_command(
            &workspace,
            5_000,
            &serde_json::json!({
                "command": long,
                "cwd": std::env::temp_dir().to_string_lossy(),
                "background": true,
            }),
            Some(&state),
            None,
        )
        .await
        .expect("background spawn");
        let job_id = started
            .lines()
            .find_map(|l| l.strip_prefix("job_id: "))
            .map(str::to_string)
            .expect("job_id");

        let killed =
            kill_background(&state, &serde_json::json!({ "job_id": job_id }), None).unwrap();
        assert!(killed.contains("killed"), "{killed}");

        // Status is Killed and stays Killed even after the waiter reaps the child.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let out = bash_output(&state, &serde_json::json!({ "job_id": job_id }), None).unwrap();
        assert!(
            out.contains("status: killed"),
            "expected killed status: {out}"
        );

        // Killing an already-terminal job is a no-op (no error).
        let again =
            kill_background(&state, &serde_json::json!({ "job_id": job_id }), None).unwrap();
        assert!(again.contains("already finished"), "{again}");
    }

    /// Unix: poll `kill(pid, 0)` until the pid is gone (ESRCH) or the budget
    /// expires. Returns true once the pid no longer exists. We send no signal
    /// (sig 0 is a liveness probe). A reaped child whose pid has not been reused
    /// reports ESRCH; we accept EPERM as "not ours / gone for our purposes".
    #[cfg(unix)]
    async fn wait_until_pid_dead(pid: u32, attempts: usize) -> bool {
        for _ in 0..attempts {
            let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
            if !alive {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        false
    }

    /// kill_background must ACTUALLY terminate the process, not just flip a
    /// status string. Spawn a long sleeper, capture its pid, kill it, and poll
    /// the OS until the pid is gone. A no-op kill (or a dropped `-pgid`) would
    /// leave the pid alive and fail this test.
    #[cfg(unix)]
    #[tokio::test]
    async fn kill_background_actually_terminates_process() {
        let state = bg_test_state();
        let workspace = NativeToolWorkspace::global(&[]);
        let started = run_command(
            &workspace,
            5_000,
            &serde_json::json!({
                "command": "sleep 30",
                "cwd": std::env::temp_dir().to_string_lossy(),
                "background": true,
            }),
            Some(&state),
            None,
        )
        .await
        .expect("background spawn");
        let job_id = started
            .lines()
            .find_map(|l| l.strip_prefix("job_id: "))
            .map(str::to_string)
            .expect("job_id");
        let pid: u32 = started
            .lines()
            .find_map(|l| l.strip_prefix("pid: "))
            .and_then(|p| p.trim().parse().ok())
            .expect("pid");

        // It is alive before the kill.
        assert!(
            unsafe { libc::kill(pid as libc::pid_t, 0) } == 0,
            "sleeper should be alive before kill"
        );

        kill_background(&state, &serde_json::json!({ "job_id": job_id }), None).unwrap();
        assert!(
            wait_until_pid_dead(pid, 100).await,
            "kill_background must actually terminate the process (pid {pid} still alive)"
        );
    }

    /// The whole process GROUP must die, not just the leader. The `sh -c` leader
    /// is spawned in its own session (setsid), and it backgrounds a grandchild
    /// `sleep`. `kill_background` SIGKILLs the group (`kill(-pgid)`), so the
    /// grandchild — which shares the group — must die too.
    #[cfg(unix)]
    #[tokio::test]
    async fn kill_background_kills_whole_group_including_grandchild() {
        let state = bg_test_state();
        let workspace = NativeToolWorkspace::global(&[]);
        // Print the grandchild pid, then keep the leader alive so the group
        // stays up until we kill it.
        let cmd = "sleep 45 & echo GRANDCHILD_PID=$!; wait";
        let started = run_command(
            &workspace,
            5_000,
            &serde_json::json!({
                "command": cmd,
                "cwd": std::env::temp_dir().to_string_lossy(),
                "background": true,
            }),
            Some(&state),
            None,
        )
        .await
        .expect("background spawn");
        let job_id = started
            .lines()
            .find_map(|l| l.strip_prefix("job_id: "))
            .map(str::to_string)
            .expect("job_id");

        // Poll bash_output until the grandchild pid appears in the captured log.
        let mut grandchild_pid: Option<u32> = None;
        for _ in 0..100 {
            let out = bash_output(&state, &serde_json::json!({ "job_id": job_id }), None).unwrap();
            if let Some(pid) = out
                .lines()
                .find_map(|l| l.trim().strip_prefix("GRANDCHILD_PID="))
                .and_then(|p| p.trim().parse::<u32>().ok())
            {
                grandchild_pid = Some(pid);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let grandchild_pid = grandchild_pid.expect("grandchild pid printed to log");
        assert!(
            unsafe { libc::kill(grandchild_pid as libc::pid_t, 0) } == 0,
            "grandchild should be alive before kill"
        );

        kill_background(&state, &serde_json::json!({ "job_id": job_id }), None).unwrap();
        assert!(
            wait_until_pid_dead(grandchild_pid, 100).await,
            "the whole group must die: grandchild pid {grandchild_pid} still alive after kill"
        );
    }

    #[tokio::test]
    async fn bash_output_unknown_job_errors() {
        let state = bg_test_state();
        let err = bash_output(&state, &serde_json::json!({ "job_id": "nope" }), None)
            .expect_err("unknown job should error");
        assert!(err.contains("No background job"), "{err}");
    }

    #[test]
    fn kill_all_background_commands_clears_registry_and_logs() {
        let state = bg_test_state();
        let job_id = uuid::Uuid::new_v4().to_string();
        let log_path = std::env::temp_dir().join(format!("{BG_CMD_LOG_PREFIX}{job_id}.log"));
        std::fs::write(&log_path, b"x").expect("seed log");
        state.register_background_command(BackgroundCommand {
            job_id: job_id.clone(),
            conversation_id: None,
            pid: None, // no real process → no kill, just registry/log cleanup
            command: "seed".to_string(),
            cwd: ".".to_string(),
            log_path: log_path.clone(),
            status: BackgroundCommandStatus::Exited { code: Some(0) },
            started_at: SystemTime::now(),
            kill_tx: None,
        });
        let _ = state.kill_all_background_commands();
        // Registry cleared and the per-job log removed.
        let listed = list_background(&state, &serde_json::json!({}), None).unwrap();
        assert!(listed.contains("no background jobs"), "{listed}");
        assert!(!log_path.exists(), "log should be removed on sweep");
    }

    /// 删对话时只收本对话的后台作业：别的对话（和无归属的测试作业）必须原样留着。
    /// 这条挂了意味着删一个对话会连带杀掉另一个对话正在跑的 dev server。
    #[test]
    fn kill_background_commands_for_conversation_only_touches_its_own_jobs() {
        let state = bg_test_state();
        let seed = |conversation_id: Option<&str>| {
            let job_id = uuid::Uuid::new_v4().to_string();
            let log_path = std::env::temp_dir().join(format!("{BG_CMD_LOG_PREFIX}{job_id}.log"));
            std::fs::write(&log_path, b"x").expect("seed log");
            state.register_background_command(BackgroundCommand {
                job_id: job_id.clone(),
                conversation_id: conversation_id.map(str::to_string),
                pid: None, // 无真实进程：只验注册表与日志的清理
                command: "seed".to_string(),
                cwd: ".".to_string(),
                log_path: log_path.clone(),
                status: BackgroundCommandStatus::Running,
                started_at: SystemTime::now(),
                kill_tx: None,
            });
            (job_id, log_path)
        };
        let (doomed, doomed_log) = seed(Some("conv_doomed"));
        let (other, other_log) = seed(Some("conv_other"));
        let (orphan, orphan_log) = seed(None);

        state.kill_background_commands_for_conversation("conv_doomed");

        let listed = list_background(&state, &serde_json::json!({}), None).unwrap();
        assert!(!listed.contains(&doomed), "本对话的作业应被摘除：{listed}");
        assert!(listed.contains(&other), "别的对话的作业必须留着：{listed}");
        assert!(listed.contains(&orphan), "无归属的作业不该被误杀：{listed}");
        assert!(!doomed_log.exists(), "本对话的日志应被清掉");
        assert!(other_log.exists() && orphan_log.exists(), "其它日志不该动");

        let _ = std::fs::remove_file(&other_log);
        let _ = std::fs::remove_file(&orphan_log);
    }

    #[test]
    fn offload_large_output_writes_temp_file_and_notes_path() {
        let big = "x".repeat(MAX_INLINE_COMMAND_OUTPUT_BYTES + 1);
        let result = offload_large_output(big.clone());
        assert!(result.starts_with("[full output:"));
        assert!(result.contains("kivio-bash-"));
        assert!(result.contains("complete log saved to"));
        // The full body is still present inline (the loop truncates the middle).
        assert!(result.contains(&big));
        // The referenced temp file exists and holds the full output; clean up.
        let path = result
            .lines()
            .next()
            .and_then(|line| {
                line.find("saved to ")
                    .map(|i| &line[i + "saved to ".len()..])
            })
            .and_then(|rest| rest.split(". Read it").next())
            .map(|p| p.to_string())
            .expect("temp path in note");
        assert_eq!(std::fs::read_to_string(&path).expect("read temp log"), big);
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_run_command_preserves_embedded_quotes() {
        // 回归:内部双引号必须原样到达目标程序。走 PowerShell 分支直测(本机装了
        // Git Bash 时 run_shell_command 会选 bash,那就测不到 PS 的引号路径了):
        // .arg()(而非 raw_arg)让 python -c "..." 的内部引号原样传到 PowerShell。
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let out = rt
            .block_on(exec_shell_command(
                build_windows_powershell_command("python -c \"print(40 + 2)\""),
                std::env::temp_dir(),
                30_000,
                None,
            ))
            .expect("spawn should succeed");
        // python 不在 PATH 时跳过,不让本机环境决定测试成败。
        let unavailable = out.stderr.contains("not recognized")
            || out.stderr.to_lowercase().contains("cannot find")
            || out.stderr.contains("找不到");
        if !unavailable {
            assert!(
                out.stdout.contains("42"),
                "embedded quotes mangled? stdout={:?} stderr={:?}",
                out.stdout,
                out.stderr
            );
        }
    }

    /// The Windows PowerShell fallback must execute through PowerShell, not
    /// cmd.exe. `Write-Output (1+1)` is a PowerShell-only construct — cmd.exe
    /// would fail to parse it. Tests the fallback builder directly (machines
    /// with Git Bash installed would otherwise route to bash).
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_run_command_executes_via_powershell() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let out = rt
            .block_on(exec_shell_command(
                build_windows_powershell_command("Write-Output (1+1)"),
                std::env::temp_dir(),
                30_000,
                None,
            ))
            .expect("spawn should succeed");
        assert_eq!(out.status_code, Some(0), "exit != 0: {out:?}");
        assert!(
            out.stdout.contains('2'),
            "expected PowerShell to evaluate (1+1)=2; stdout={:?} stderr={:?}",
            out.stdout,
            out.stderr
        );
    }

    /// Non-ASCII stdout must decode as UTF-8. Windows PowerShell 5.1 defaults its
    /// output encoding to the OEM code page; the UTF-8 prefix in wrap_ps_command
    /// is what keeps CJK from arriving mojibake'd. Guards that fix (on the
    /// PowerShell fallback path — Git Bash emits UTF-8 natively).
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_run_command_outputs_utf8() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let out = rt
            .block_on(exec_shell_command(
                build_windows_powershell_command("Write-Output '你好'"),
                std::env::temp_dir(),
                30_000,
                None,
            ))
            .expect("spawn should succeed");
        assert!(
            out.stdout.contains("你好"),
            "non-ASCII output should be UTF-8; got stdout={:?} stderr={:?}",
            out.stdout,
            out.stderr
        );
    }

    /// WSL 的 bash 垫片(System32/sysnative)必须被拒:它的 /mnt/c 文件系统视图
    /// 与 Kivio 传的 Windows 路径语义不符。大小写不敏感;正/反斜杠都认;
    /// 正常 Git Bash 安装位不受影响。
    #[test]
    fn wsl_bash_paths_are_rejected() {
        assert!(is_wsl_bash_path(Path::new(r"C:\Windows\System32\bash.exe")));
        assert!(is_wsl_bash_path(Path::new(
            r"C:\Windows\sysnative\bash.exe"
        )));
        // 大小写不敏感 + 正斜杠形态。
        assert!(is_wsl_bash_path(Path::new(r"c:\WINDOWS\SYSTEM32\BASH.EXE")));
        assert!(is_wsl_bash_path(Path::new("C:/Windows/System32/bash.exe")));
        // Git Bash 与其他 bash 不误伤。
        assert!(!is_wsl_bash_path(Path::new(
            r"C:\Program Files\Git\bin\bash.exe"
        )));
        assert!(!is_wsl_bash_path(Path::new(
            r"C:\Users\me\AppData\Local\Programs\Git\bin\bash.exe"
        )));
        assert!(!is_wsl_bash_path(Path::new(r"C:\msys64\usr\bin\bash.exe")));
        // 后缀匹配必须带 \windows\ 前缀,别的 System32 目录名不触发。
        assert!(!is_wsl_bash_path(Path::new(r"D:\tools\System32\bash.exe")));
    }

    /// Git Bash 命中时,run_command 真跑 bash 语法(heredoc / $(seq) / 管道)。
    /// 无 Git Bash 的机器上跳过(回落 PowerShell 属另一条已测路径)。
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_run_command_executes_bash_syntax_when_git_bash_present() {
        if find_git_bash().is_none() {
            return; // 本机无 Git Bash:回落 PowerShell,由上面的 PS 测试覆盖。
        }
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let out = rt
            .block_on(run_shell_command(
                "cat <<EOF\nheredoc_ok\nEOF\nfor i in $(seq 1 3); do echo \"n=$i\"; done | tail -n 1",
                std::env::temp_dir(),
                30_000,
                None,
            ))
            .expect("spawn should succeed");
        assert_eq!(out.status_code, Some(0), "exit != 0: {out:?}");
        assert!(
            out.stdout.contains("heredoc_ok"),
            "heredoc should work; stdout={:?} stderr={:?}",
            out.stdout,
            out.stderr
        );
        assert!(
            out.stdout.contains("n=3"),
            "bash loop/pipe should work; stdout={:?} stderr={:?}",
            out.stdout,
            out.stderr
        );
    }

    #[test]
    fn normalize_run_command_rejects_cd_with_spaces() {
        let err = normalize_run_command("cd /Users/zmair/ZM database/foo && npm install", None)
            .expect_err("spaced cd path should require cwd");

        assert!(err.contains("Suggested cwd: /Users/zmair/ZM database/foo"));
        assert!(err.contains("Suggested command: npm install"));
    }

    #[test]
    fn normalize_run_command_rejects_cd_when_cwd_is_set() {
        let err = normalize_run_command("cd foo && npm install", Some("/tmp/project"))
            .expect_err("cd and cwd should conflict");

        assert!(err.contains("do not combine"));
    }

    #[test]
    fn normalize_run_command_strips_simple_cd_prefix() {
        let (command, cwd) =
            normalize_run_command("cd focus-pomodoro && npm install", None).expect("normalize");

        assert_eq!(command, "npm install");
        assert_eq!(cwd.as_deref(), Some("focus-pomodoro"));
    }

    #[test]
    fn is_long_running_dev_command_detects_common_dev_servers() {
        assert!(is_long_running_dev_command("npm run tauri dev"));
        assert!(is_long_running_dev_command("npx vite --port 5173"));
        assert!(!is_long_running_dev_command("npm run build"));
        assert!(!is_long_running_dev_command("vite build"));
    }

    #[tokio::test]
    async fn run_command_blocks_host_python_package_installs() {
        let err = run_command(
            &NativeToolWorkspace::global(&[]),
            1_000,
            &serde_json::json!({ "command": "python3 -m pip install matplotlib" }),
            None,
            None,
        )
        .await
        .expect_err("pip installs should be blocked");

        assert!(err.contains("allow_host_python_package_install"));
    }

    // A coding-agent shell command must never read the interactive terminal's stdin.
    // The child is spawned with Stdio::null() for stdin, so a command that reads stdin
    // (e.g. `cat` with no file args) gets immediate EOF and returns promptly instead of
    // blocking forever waiting on the parent's terminal. If stdin were inherited, this
    // test would hang (and in the TUI would steal the input thread, exiting the session).
    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn run_command_stdin_is_null_so_readers_get_eof() {
        let dir = std::env::temp_dir().join(format!("kivio_stdin_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let workspace = NativeToolWorkspace::global(&[dir.to_string_lossy().into_owned()]);

        // `cat` with no args reads stdin to EOF. With null stdin this returns immediately.
        // Wrap in tokio::time::timeout as a hard backstop so a regression fails fast
        // instead of hanging the test suite.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_command(
                &workspace,
                2_000,
                &serde_json::json!({ "command": "cat" }),
                None,
                None,
            ),
        )
        .await
        .expect("cat should return promptly because stdin is null (EOF), not hang");

        let output = result.expect("cat with null stdin should succeed");
        // No stdin content → empty captured stdout.
        assert!(
            !output.contains("Command timed out"),
            "command must not time out: {output}"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tail_truncate_keeps_end_under_line_budget() {
        let mut body = String::new();
        for i in 0..(TAIL_MAX_LINES + 500) {
            body.push_str(&format!("line {i}\n"));
        }
        let (kept, dropped) = tail_truncate(&body);
        assert_eq!(dropped, 500, "first 500 lines dropped, tail kept");
        let kept_lines: Vec<&str> = kept.lines().collect();
        assert_eq!(kept_lines.len(), TAIL_MAX_LINES);
        // The LAST line (where errors/results live) is preserved.
        assert_eq!(
            *kept_lines.last().unwrap(),
            format!("line {}", TAIL_MAX_LINES + 500 - 1)
        );
        // The first kept line is line 500 (earlier lines were dropped).
        assert_eq!(kept_lines[0], "line 500");
    }

    #[test]
    fn tail_truncate_keeps_end_under_byte_budget() {
        // Few lines but each huge → byte budget (not line budget) forces truncation.
        let big_line = "z".repeat(20 * 1024);
        let body = format!("{big_line}\n{big_line}\n{big_line}\nFINAL ERROR LINE\n");
        let (kept, dropped) = tail_truncate(&body);
        assert!(dropped > 0, "byte budget should drop leading huge lines");
        assert!(kept.len() <= TAIL_MAX_BYTES + 32);
        // The final line is always retained.
        assert!(kept.ends_with("FINAL ERROR LINE"));
    }

    #[test]
    fn tail_truncate_passes_small_output_through() {
        let small = "a\nb\nc\n";
        let (kept, dropped) = tail_truncate(small);
        assert_eq!(dropped, 0);
        assert_eq!(kept, "a\nb\nc\n");
    }

    #[test]
    fn offload_large_output_tail_truncates_and_marks() {
        let mut body = String::new();
        for i in 0..(TAIL_MAX_LINES + 1000) {
            body.push_str(&format!(
                "row {i} ----------------------------------------\n"
            ));
        }
        assert!(body.len() > MAX_INLINE_COMMAND_OUTPUT_BYTES);
        let result = offload_large_output(body);
        // Full log path noted in the head.
        assert!(result.contains("complete log saved to"));
        // Tail-truncation marker present.
        assert!(result.contains("earlier lines truncated"));
        // The END of the output is kept (last row), not the head (row 0 dropped).
        assert!(result.contains(&format!("row {}", TAIL_MAX_LINES + 1000 - 1)));
        assert!(!result.contains("\nrow 0 -"));

        // Clean up the temp log referenced in the note.
        if let Some(path) = result
            .lines()
            .find(|l| l.contains("complete log saved to"))
            .and_then(|line| {
                line.find("saved to ")
                    .map(|i| &line[i + "saved to ".len()..])
            })
            .and_then(|rest| rest.split(". Read it").next())
        {
            let _ = std::fs::remove_file(path);
        }
    }
}
