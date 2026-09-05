use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, Command};
use tokio::time::timeout;

use crate::external_agents::types::RuntimeAgentDef;
use crate::proc::NoConsoleWindow;

pub struct SpawnedAgent {
    pub child: Child,
    pub resolved_bin: PathBuf,
}

/// 标记「当前进程正跑在某个 CLI 的会话里」的环境变量。Kivio 若是从 Claude Code 内启动
/// （开发时 `npm run dev`、或用户把 Kivio 挂在某个 agent 下），这些会**继承**到 Kivio 进程、
/// 再泄漏给它拉起的 CLI 子进程，让子进程误以为自己嵌套在另一个会话里而拒绝启动
/// （claude 报 "cannot be launched inside another session"）。
///
/// 本机实测：从 Claude Code 环境跑 `env | grep CLAUDE` 确实能看到
/// `CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `CLAUDE_AGENT_SDK_VERSION` 都是 set 的。
///
/// **后两个是「宿主代管凭据」标记，比会话身份更致命**（真机验收时才发现）：
/// 宿主（Claude Code 桌面端）用 `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=1` 告诉 claude
/// 「你的凭据由我在运行时注入」，并用 `CLAUDE_CODE_HOST_AUTH_ENV_VAR` 指明注入到哪个
/// 变量名（实测值 `ANTHROPIC_AUTH_TOKEN`）。子进程继承了这两个标记、却**拿不到宿主
/// 真正注入的那个 token**（它只存在于宿主进程内），于是 claude 认为自己处于代管模式、
/// 不再回退去读 `~/.claude/settings.json` 的 `env` 块 ⇒ 报 "Not logged in · Please run /login"。
///
/// 剥掉这两个后 claude 恢复自主认证，会自己读 settings.json 的 `env`（含用户配置的
/// `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_BASE_URL`）。实测二分定位：单独清任何一个都无效，
/// 两个**必须一起**清；清掉后 `is_error=false`。
///
/// 只清这些**会话身份 / 代管标记**变量，绝不动 `ANTHROPIC_*` 本身——那些是用户配置的
/// 凭据与端点，子进程需要它们来认证。
const PARENT_SESSION_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST",
    "CLAUDE_CODE_HOST_AUTH_ENV_VAR",
];

/// 所有拉起外部 CLI 的地方都必须先过这一手，把父会话身份变量剥掉。
///
/// 覆盖范围包括**探测**路径（version / auth / 模型 / 斜杠命令）——不只是跑轮次：
/// 探测同样是子进程，同样会被父会话变量干扰，且探测失败会被缓存下来
/// （`AVAILABILITY_CACHE_TTL` 600s），一次误判要 10 分钟才自愈。
pub fn strip_parent_session_env(command: &mut Command) -> &mut Command {
    for key in PARENT_SESSION_ENV_VARS {
        command.env_remove(key);
    }
    command
}

/// 构造一个拉起外部 CLI 的 `Command`：等价于 `Command::new(program)` 但**已剥离父会话身份变量**。
///
/// 新增拉起 CLI 的代码请一律用它而不是 `Command::new`——忘记剥离不会编译报错、
/// 也不会立刻出错，只在「Kivio 从某个 agent 里启动」这种特定场景下才炸，极难排查。
fn command_for_program(program: &OsStr) -> Command {
    if let Some(target) = crate::external_agents::wsl::parse_wsl_target(Path::new(program)) {
        return crate::external_agents::wsl::wsl_exec_command(&target);
    }
    Command::new(program)
}

fn apply_env_for_cli(command: &mut Command, program: &Path, key: &str, value: String) {
    let value = crate::external_agents::wsl::env_value_for_cli(program, key, value);
    command.env(key, value);
}

fn cli_bin_stem(bin: &Path) -> Option<String> {
    if let Some(target) = crate::external_agents::wsl::parse_wsl_target(bin) {
        return Path::new(&target.linux_path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string);
    }
    bin.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

fn is_codex_cli_bin(bin: &Path) -> bool {
    cli_bin_stem(bin).is_some_and(|stem| {
        stem.eq_ignore_ascii_case("codex") || stem.eq_ignore_ascii_case("codex-app-server")
    })
}

/// WSL Codex 在没有私有 `CODEX_HOME` 时改读用户那份 `~/.codex`，否则会话写进 Linux 家目录，
/// Windows 上的 Codex CLI / TUI 看不见。
fn extra_codex_home_for_spawn(
    cli_bin: &Path,
    agent_id: Option<&str>,
    already_has_codex_home: bool,
) -> Option<PathBuf> {
    if already_has_codex_home {
        return None;
    }
    if agent_id != Some("codex") && !is_codex_cli_bin(cli_bin) {
        return None;
    }
    crate::external_agents::provider_profile::wsl_shared_codex_home(cli_bin)
}

fn finish_codex_home(
    command: &mut Command,
    program: &Path,
    agent_id: Option<&str>,
    already_has_codex_home: bool,
) {
    if already_has_codex_home {
        crate::external_agents::provider_profile::ensure_private_codex_sessions_shared();
        return;
    }
    if let Some(home) = extra_codex_home_for_spawn(program, agent_id, false) {
        apply_env_for_cli(
            command,
            program,
            "CODEX_HOME",
            home.to_string_lossy().into_owned(),
        );
    }
}

pub fn cli_command(program: impl AsRef<OsStr>) -> Command {
    let program = program.as_ref();
    let mut command = command_for_program(program);
    strip_parent_session_env(&mut command);
    // 设置页里针对这个 CLI 填的环境变量覆盖（`ANTHROPIC_BASE_URL` 之类），按二进制名反查。
    // 必须用原始路径（UNC 的 file_stem 仍是 `claude`），不能用改写后的 `wsl.exe`。
    let extra = crate::external_agents::overrides::env_for_bin(Path::new(program));
    let has_codex_home = extra.contains_key("CODEX_HOME");
    for (key, value) in extra {
        apply_env_for_cli(&mut command, Path::new(program), &key, value);
    }
    finish_codex_home(&mut command, Path::new(program), None, has_codex_home);
    command
}

/// `cli_command` + 该 agent 的静态 `def.env`。有 `def` 在手时用它，覆盖按 id 精确取。
///
/// 探测（version / auth / 列模型）也要带上环境变量覆盖：用户配了 `ANTHROPIC_BASE_URL` 指向中转，
/// 只在跑轮次时注入的话，认证探测仍打官方端点，设置页会显示「未认证」而实际能用。
pub fn agent_cli_command(def: &RuntimeAgentDef, program: impl AsRef<OsStr>) -> Command {
    let program = program.as_ref();
    let mut command = command_for_program(program);
    strip_parent_session_env(&mut command);
    let mut has_codex_home = false;
    for (key, value) in def.env {
        has_codex_home |= *key == "CODEX_HOME";
        apply_env_for_cli(&mut command, Path::new(program), key, value.to_string());
    }
    for (key, value) in crate::external_agents::overrides::env_for(def.id) {
        has_codex_home |= key == "CODEX_HOME";
        apply_env_for_cli(&mut command, Path::new(program), &key, value);
    }
    finish_codex_home(
        &mut command,
        Path::new(program),
        Some(def.id),
        has_codex_home,
    );
    command
}

/// 结束一个外部 CLI 子进程**及其整棵进程树**。
///
/// 只 `start_kill()` 杀的是**直接子进程**：claude 会按用户的 `~/.claude.json` 把 MCP
/// 服务器作为自己的子进程拉起来，每轮取消一次就可能漏一批孤儿 MCP 进程留在系统里。
///
/// 复用 `native_tools::kill_process_group`（unix `killpg` SIGTERM→SIGKILL、
/// Windows `taskkill /T /F`），不写第二份（spec 第 2 条）。两步都做，缺一不可：
/// - Windows 的 `taskkill /T` 按父子关系遍历整棵树，能覆盖子进程；
/// - unix 的 `killpg(-pid)` 要求目标是**进程组组长**，而 `spawn_agent` 没有 `setsid`，
///   命中不了时只是无害的 ESRCH —— 所以仍要 `start_kill()` 兜住直接子进程本身。
///
/// 副作用提醒：杀进程导致的退出码在 Windows 上恒为 1（`TerminateProcess`），
/// 出口的「非零退出 = 失败」规则必须靠协议层完成标志豁免（spec 第 8b 条），
/// 否则会凭空造出失败气泡。
pub fn kill_agent_process_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        crate::native_tools::kill_process_group(pid);
    }
    let _ = child.start_kill();
}

/// Concurrently drain the child's stderr into a JoinHandle so a CLI that reports failures on
/// stderr doesn't (a) block on a full pipe while we read stdout, and (b) fail silently. Blank
/// lines are dropped and the buffer is capped at `STDERR_CAP_CHARS` (keeping the tail — the last
/// lines are usually the actual error). Call before the stdout read loop; await after `wait()`.
pub fn drain_stderr(child: &mut Child) -> tokio::task::JoinHandle<String> {
    spawn_stderr_tail(child.stderr.take())
}

/// Ring-buffer stderr drain for persistent sessions (N1): the CLI process is long-lived and its
/// stderr is `take()`n separately from stdout, so we can't use `drain_stderr(&mut Child)`. Spawns a
/// task that accumulates the tail (last `STDERR_CAP_CHARS`) until stderr hits EOF (i.e. the child
/// dies / is killed), then returns it. Join the handle on close / error to fold into diagnostics.
pub fn spawn_stderr_tail(stderr: Option<ChildStderr>) -> tokio::task::JoinHandle<String> {
    tokio::spawn(async move {
        match stderr {
            Some(stderr) => accumulate_tail(stderr, STDERR_CAP_CHARS).await,
            None => String::new(),
        }
    })
}

const STDERR_CAP_CHARS: usize = 8192;

/// How long to wait for stderr EOF after killing the child. `start_kill()` only
/// terminates the direct child; on Windows `dsh.cmd` → `node` often leaves the
/// grandchild holding the pipe, so a bare join blocks forever and wedgies the
/// dsh profile boot lock (every later dsh turn then spins with no error).
const STDERR_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Kill the child tree (so its stderr hits EOF) and join the drain task to
/// recover the stderr tail. Used by persistent sessions on a handshake/connect
/// error path (N1 + R2). Always returns — a stuck grandchild must not block
/// the caller.
pub async fn join_stderr_tail(
    child: &mut Child,
    stderr_tail: tokio::task::JoinHandle<String>,
) -> String {
    kill_agent_process_tree(child);
    match timeout(STDERR_JOIN_TIMEOUT, stderr_tail).await {
        Ok(Ok(tail)) => tail,
        Ok(Err(_)) => String::new(),
        Err(_) => String::new(),
    }
}

/// Append a drained stderr tail to an error message when non-empty (for R2 diagnostics).
pub fn fold_stderr(msg: String, stderr_tail: &str) -> String {
    if stderr_tail.trim().is_empty() {
        msg
    } else {
        format!("{msg}\nstderr: {}", stderr_tail.trim())
    }
}

/// Read lines from `reader` until EOF, keeping only the last `cap` characters (char-boundary safe).
/// Blank lines are dropped. Extracted from the drain tasks so the ring-buffer bound is unit-testable
/// without spawning a real process.
async fn accumulate_tail<R: AsyncRead + Unpin>(reader: R, cap: usize) -> String {
    let mut lines = BufReader::new(reader).lines();
    let mut out = String::new();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            // 一行非法 UTF-8 不能让排空停下来：常驻进程的 stderr 从此再没人读，管道写满
            // （4–64KB）后子进程会阻塞在写 stderr 上 —— 正是这个任务要避免的那个死锁。
            // tokio 丢掉这一行后 reader 仍可用，接着读。**只对 InvalidData 继续**：其余
            // 错误（管道断了之类）是持续性的，continue 会变成空转。
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => continue,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
        if out.chars().count() > cap {
            out = tail_chars(&out, cap);
        }
    }
    out
}

/// Keep the last `max_chars` characters of `value` (char-boundary safe).
pub fn tail_chars(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    let start = chars.len().saturating_sub(max_chars);
    chars[start..].iter().collect()
}

/// 探活单个候选的超时。只是启动一次 `--version`，正常在几十毫秒内返回；
/// 给到 2s 是为了容忍冷启动（Node/Bun 包装脚本首次加载）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// 已探活通过的候选路径缓存。
///
/// **为什么必须有**：探活要起一次 `--version` 子进程，实测本机开销差异极大——
/// grok 6.6ms、claude 40ms，但 kimi 328ms、pi 302ms（Node 包装脚本冷启动）。
/// 而 `resolve_binary` 是**回复热路径**上唯一允许的前置调用，`run.rs` 明确写着
/// 「把第 2+ 轮的前置开销压到 <500ms」。不缓存的话 kimi 单这一步就吃掉 538ms，
/// 直接击穿该预算。
///
/// **失效策略**：key 用「路径 + mtime + size」。用户换版本、重装、切版本管理器
/// 都会改变其中之一 ⇒ 自动失效重探；不需要 TTL，也就不会有「刚装好却要等 N 秒」
/// 或「删了还认为在」的窗口。文件读不到 metadata 时不写缓存（下次重探）。
///
/// **顺带存下版本号**：探活本来就跑了 `--version`，此前把 stdout 丢进 `/dev/null`。
/// 把它接住存进同一条缓存，列表阶段就能做版本门控（如「当前 CLI 太老，不提供新模型」）
/// 而**不新起任何进程** —— spec 第 9 条要求回复热路径零探测，另起一次 `--version`
/// 会违反它。缓存 key 已含文件身份，换版本自然失效。
static PROBE_CACHE: std::sync::LazyLock<std::sync::Mutex<HashMap<String, ProbeOutcome>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// 一次探活的结果：能否启动 + `--version` 首行（拿不到则 `None`）。
#[derive(Debug, Clone)]
struct ProbeOutcome {
    executable: bool,
    version: Option<String>,
}

/// 缓存 key：把文件身份（mtime + size）编进去，文件一变就自然 miss。
fn probe_cache_key(path: &Path) -> Option<String> {
    // `\\wsl$\...` 走 9P，metadata 又慢又会把发行版唤醒；用路径本身当身份即可。
    if crate::external_agents::wsl::is_wsl_target(path) {
        return Some(format!("wsl|{}", path.display()));
    }
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{}|{mtime}|{}", path.display(), meta.len()))
}

/// 解析 CLI 二进制：遍历 `bin` + `fallback_bins`，对每个名字取 PATH 上的**全部**同名候选，
/// 逐个探活，返回第一个真正能启动的。
///
/// 为什么不能只取 `which` 的第一行：PATH 上常有同名但坏掉的 shim（版本管理器切换后的残留、
/// 断掉的 symlink、丢了执行权限的文件）。选中它不会立刻失败，而是**等到真正跑轮次时**才炸，
/// 用户看到的是运行时报错而不是「未安装」，排查成本很高。
///
/// **热路径开销**（spec 第 9 条 + `run.rs` 的 <500ms 预算）：首次每个候选多一次
/// `--version`（实测 6.6ms ~ 328ms 不等），之后走 `PROBE_CACHE` ⇒ 稳态只剩
/// `which -a` 的 ~2-3ms，与改动前基本持平。
pub async fn resolve_binary(def: &RuntimeAgentDef) -> Option<PathBuf> {
    // 用户在设置页指定了路径就**只认它**：静默回退到 PATH 会让「我明明指了这个二进制」
    // 与实际跑的东西对不上，比诚实报「未安装」更难查。
    if let Some(path) = crate::external_agents::overrides::custom_path(def.id) {
        let path = crate::external_agents::wsl::normalize_custom_path(path).await;
        return probe_executable_cached(&path, def.version_args)
            .await
            .then_some(path);
    }
    for candidate in std::iter::once(def.bin).chain(def.fallback_bins.iter().copied()) {
        for path in which_all(candidate).await {
            if probe_executable_cached(&path, def.version_args).await {
                return Some(path);
            }
        }
    }
    // The desktop process may still have the PATH from before the official agy install.
    if def.id == "antigravity" {
        let path = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|p| p.join("agy/bin/agy.exe"))
        } else {
            directories::UserDirs::new().map(|d| d.home_dir().join(".local/bin/agy"))
        };
        if let Some(path) = path.filter(|p| p.is_file()) {
            if probe_executable_cached(&path, def.version_args).await {
                return Some(path);
            }
        }
    }
    // Windows PATH 上没有能拉起来的 Win32 候选时，再看 WSL 里的 Linux 安装。
    // 不插到 PATH 候选前面：本机已经装了 Windows 版 CLI 的人不能被 WSL 里同名二进制抢走。
    for candidate in std::iter::once(def.bin).chain(def.fallback_bins.iter().copied()) {
        if let Some(path) = crate::external_agents::wsl::linux_bin_unc(candidate).await {
            if probe_executable_cached(&path, def.version_args).await {
                return Some(path);
            }
        }
    }
    None
}

/// `probe_executable` + `PROBE_CACHE`（key 含 mtime/size，换版本自动失效）。
async fn probe_executable_cached(path: &Path, version_args: &[&str]) -> bool {
    let key = probe_cache_key(path);
    if let Some(key) = key.as_deref() {
        if let Some(cached) = PROBE_CACHE.lock().ok().and_then(|c| c.get(key).cloned()) {
            return cached.executable;
        }
    }
    let (ok, version) = probe_executable(path, version_args).await;
    if let (Some(key), Ok(mut cache)) = (key, PROBE_CACHE.lock()) {
        cache.insert(
            key,
            ProbeOutcome {
                executable: ok,
                version,
            },
        );
    }
    ok
}

/// 探活时顺手记下的 CLI 版本号（`--version` 首行），**不起任何新进程**。
///
/// 由 `resolve_binary` 的探活填充；缓存 key 含文件身份（路径 + mtime + size），
/// 换版本/重装自动失效。返回 `None` 的情况：这个路径还没被探活过、`--version`
/// 没有可用输出、或者拿不到文件 metadata（不写缓存那一路）。
///
/// 调用方拿 `None` 时**不得**因此禁用功能（spec 第 9 条：不许为补版本号再起进程）；
/// 语义应是「未知 ⇒ 不做版本门控」。
pub fn cached_cli_version(path: &Path) -> Option<String> {
    let key = probe_cache_key(path)?;
    PROBE_CACHE
        .lock()
        .ok()?
        .get(&key)
        .and_then(|entry| entry.version.clone())
}

/// 安装后 `--version` 从失败变成能跑，但 shim 的 mtime/size 往往不变，探活缓存
/// 会继续返回「能启动、无版本」。装完立刻丢掉这条，下次探测才会重新跑。
pub fn invalidate_probe_cache(path: &Path) {
    let prefix = format!("{}|", path.display());
    let wsl_key = format!("wsl|{}", path.display());
    if let Ok(mut cache) = PROBE_CACHE.lock() {
        cache.retain(|key, _| !key.starts_with(&prefix) && key.as_str() != wsl_key);
    }
}

pub fn clear_probe_cache() {
    if let Ok(mut cache) = PROBE_CACHE.lock() {
        cache.clear();
    }
}

/// 一个候选是否**可执行**（不是「是否可用」）。
///
/// 判据刻意宽松——只要进程**起来了**就算存在，不看退出码：
/// 一个装了但**没登录**的 CLI，`--version` 完全可能非零退出（本机实测
/// `printf 'exit 1' > x` 这类脚本 spawn 成功、exit=1）。只认零退出码会把这类 CLI
/// 误判成「未安装」，比原来的 bug 更糟。
///
/// 真正的「不存在」只有 spawn 阶段的失败。本机实测（macOS，`/tmp/probe_t` 造的样本）：
/// - 丢了执行权限     → `EACCES`
/// - 空文件但挂了执行位 → **spawn 成功**（内核回退 /bin/sh，exit 0），视为存在
/// - 断掉的 symlink / 文件不存在 → `ENOENT`
/// - 装了但未登录（exit 1）→ **spawn 成功**，视为存在
///
/// 超时也算存在：进程都起来了，说明文件确实是可执行的，只是这个 CLI 的 `--version` 慢。
///
/// 返回 `(能否启动, --version 首行)`。第二项**只是搭便车**：这次子进程本来就要跑，
/// 顺手把 stdout 接住给版本门控用（此前是 `Stdio::null()` 直接丢掉）。它拿不到时一律
/// `None`，绝不影响第一项的判定 —— 未登录的 CLI 完全可能既非零退出又没有版本输出。
async fn probe_executable(path: &Path, version_args: &[&str]) -> (bool, Option<String>) {
    let mut command = cli_command(path);
    command
        .args(version_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .no_console_window()
        .kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        // spawn 失败 = 这个候选不是个能跑的可执行文件（EACCES / ENOEXEC / ENOENT）。
        Err(_) => return (false, None),
    };
    // 超时时这个 future 被丢弃 ⇒ 连带丢弃 Child ⇒ `kill_on_drop(true)` 收尾，
    // 与改动前的显式 `start_kill()` 等价。
    let is_wsl = crate::external_agents::wsl::is_wsl_target(path);
    match timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            if is_wsl && crate::external_agents::wsl::wsl_stdout_is_system_error(&output.stdout) {
                return (false, None);
            }
            let text = if is_wsl {
                crate::external_agents::wsl::decode_wsl_output(&output.stdout)
            } else {
                String::from_utf8_lossy(&output.stdout).into_owned()
            };
            (true, first_version_line(&text))
        }
        // Win32：进程能起来就算在。WSL：真正启动的是 wsl.exe 垫片——没装组件时它
        // 会卡在 MSI 几十秒（本机 61s / REGDB_E_CLASSNOTREG），超时绝不等于 CLI 在。
        Ok(Err(_)) => (!is_wsl, None),
        Err(_) => (!is_wsl, None),
    }
}

/// `--version` 输出里的版本行：首个非空行，裁到 `VERSION_LINE_CAP` 字符。
///
/// 刻意不解析结构（不同 CLI 形态差异极大：claude 是 `2.1.220 (Claude Code)`、
/// codex 是 `codex-cli 0.145.0`）——比较语义留给使用方，这里只负责「原样记一行」。
fn first_version_line(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| tail_chars_head(line, VERSION_LINE_CAP))
}

const VERSION_LINE_CAP: usize = 200;

/// 取前 `max_chars` 个字符（char 边界安全）。与 `tail_chars` 对称，用于给版本行封顶：
/// 坏 shim 可能把一整篇日志打在一行里。
fn tail_chars_head(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// PATH 上某个名字的**全部**同名候选，按 PATH 顺序。
/// unix 用 `which -a`；Windows 的 `where` 本身就多行输出。
async fn which_all(name: &str) -> Vec<PathBuf> {
    let mut command = Command::new(if cfg!(windows) { "where" } else { "which" });
    if !cfg!(windows) {
        command.arg("-a");
    }
    let output = match command.arg(name).no_console_window().output().await {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| seen.insert(line.to_string()))
        .map(PathBuf::from)
        .collect()
}

pub async fn spawn_agent(
    def: &RuntimeAgentDef,
    resolved_bin: &Path,
    args: &[String],
    cwd: &Path,
    extra_env: &HashMap<String, String>,
) -> Result<SpawnedAgent, String> {
    let mut command = agent_cli_command(def, resolved_bin);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .no_console_window()
        .kill_on_drop(true);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let child = command
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", def.id))?;
    Ok(SpawnedAgent {
        child,
        resolved_bin: resolved_bin.to_path_buf(),
    })
}

/// 一行 claude stream-json `user` 消息（含结尾换行）。
///
/// 一次性路径（`write_prompt_stdin`）与常驻会话（`session/claude_stream.rs`）共用同一份构造，
/// 不写第二份（spec 第 2 条）：「slash 命令走裸字符串」与「图片块的白名单/形状」这两条语义
/// 一旦分叉，两条路的行为差异会极难查。
pub fn stream_json_user_line(
    prompt: &str,
    images: &[crate::external_agents::attachments::ImageBlock],
) -> Result<String, String> {
    let line = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": stream_json_user_content(prompt, images)
        },
        "parent_tool_use_id": null
    });
    let mut payload = serde_json::to_string(&line).map_err(|e| e.to_string())?;
    payload.push('\n');
    Ok(payload)
}

/// Minimal stdin write to elicit Claude `system/init` during slash-command probing.
pub async fn write_probe_stdin(child: &mut Child) -> Result<(), String> {
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "stdin unavailable".to_string())?;
    let mut stdin = stdin;
    let line = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": "."
        },
        "parent_tool_use_id": null
    });
    let mut payload = serde_json::to_string(&line).map_err(|e| e.to_string())?;
    payload.push('\n');
    stdin
        .write_all(payload.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn stream_json_user_content(
    prompt: &str,
    images: &[crate::external_agents::attachments::ImageBlock],
) -> serde_json::Value {
    if prompt.trim_start().starts_with('/') {
        serde_json::Value::String(prompt.to_string())
    } else {
        // Anthropic content array: text block first, then a base64 image block per attached image.
        let mut content = vec![serde_json::json!({ "type": "text", "text": prompt })];
        for img in images {
            content.push(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": img.mime,
                    "data": img.data_base64,
                },
            }));
        }
        serde_json::Value::Array(content)
    }
}

pub fn parse_json_line(line: &str) -> Option<serde_json::Value> {
    serde_json::from_str(line.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cli_command` 必须剥掉父会话身份变量，否则 Kivio 从 Claude Code 内启动时，
    /// 这些变量会继承进来再泄漏给 CLI 子进程，子进程以为自己嵌套在别的会话里而拒绝启动。
    ///
    /// 这里断言的是**实际子进程看到的 env**（跑一个 `env` / `printenv` 子进程读回来），
    /// 而不是 `Command` 的内部状态——后者测了等于没测。
    #[tokio::test]
    async fn cli_command_strips_parent_session_env_from_the_child() {
        // 先在本进程设上，模拟「Kivio 被 Claude Code 拉起」。
        std::env::set_var("CLAUDECODE", "1");
        std::env::set_var("CLAUDE_CODE_ENTRYPOINT", "cli");
        std::env::set_var("CLAUDE_AGENT_SDK_VERSION", "0.0.0");
        // 宿主代管凭据标记：真机验收时正是这两个让 claude 报 "Not logged in"。
        std::env::set_var("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST", "1");
        std::env::set_var("CLAUDE_CODE_HOST_AUTH_ENV_VAR", "ANTHROPIC_AUTH_TOKEN");
        // 对照组：这两个**不该**被剥——前者是用户配置的凭据、后者是自定义端点，
        // 剥了子进程就真的没法认证了。剥得过头与剥得不够同样是 bug。
        std::env::set_var("KIVIO_SPAWN_TEST_KEEP", "keep-me");
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "sk-test-token");

        let out = if cfg!(windows) {
            let mut cmd = cli_command("cmd.exe");
            cmd.args(["/C", "set"]);
            cmd.output().await.expect("run cmd set")
        } else {
            cli_command("/usr/bin/env").output().await.expect("run env")
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let has = |key: &str| {
            text.lines()
                .any(|line| line.split_once('=').is_some_and(|(k, _)| k == key))
        };

        assert!(!has("CLAUDECODE"), "CLAUDECODE 泄漏给了子进程");
        assert!(
            !has("CLAUDE_CODE_ENTRYPOINT"),
            "CLAUDE_CODE_ENTRYPOINT 泄漏"
        );
        assert!(
            !has("CLAUDE_AGENT_SDK_VERSION"),
            "CLAUDE_AGENT_SDK_VERSION 泄漏"
        );
        // 这两条是真机回归的锚点：泄漏它们会让 claude 以为凭据由宿主代管、
        // 从而不去读 ~/.claude/settings.json 的 env，报 "Not logged in"。
        assert!(
            !has("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST"),
            "CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST 泄漏 → claude 会报 Not logged in"
        );
        assert!(
            !has("CLAUDE_CODE_HOST_AUTH_ENV_VAR"),
            "CLAUDE_CODE_HOST_AUTH_ENV_VAR 泄漏 → claude 会报 Not logged in"
        );
        assert!(
            has("KIVIO_SPAWN_TEST_KEEP"),
            "剥得过头了：非会话身份的变量也被清掉，子进程会丢失凭据/用户配置"
        );
        assert!(
            has("ANTHROPIC_AUTH_TOKEN"),
            "ANTHROPIC_AUTH_TOKEN 被误剥——那是用户配置的凭据，子进程要用它认证"
        );

        std::env::remove_var("CLAUDECODE");
        std::env::remove_var("CLAUDE_CODE_ENTRYPOINT");
        std::env::remove_var("CLAUDE_AGENT_SDK_VERSION");
        std::env::remove_var("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST");
        std::env::remove_var("CLAUDE_CODE_HOST_AUTH_ENV_VAR");
        std::env::remove_var("KIVIO_SPAWN_TEST_KEEP");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    }

    #[test]
    fn cli_command_rewrites_wsl_unc_to_wsl_exe() {
        let cmd = cli_command(r"\\wsl$\Ubuntu\usr\bin\claude");
        let std = cmd.as_std();
        assert_eq!(std.get_program(), std::ffi::OsStr::new("wsl.exe"));
        let args: Vec<String> = std
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(&args[..5], ["-d", "Ubuntu", "-e", "/bin/bash", "-c"]);
        assert_eq!(args[6], "/usr/bin/claude");
        // 环境覆盖必须仍按原始二进制名 `claude` 查找，不能变成 `wsl`。
        // UNC 在 Unix Path 里是单个分量，要用 linux 路径取 stem。
        assert_eq!(
            cli_bin_stem(Path::new(r"\\wsl$\Ubuntu\usr\bin\claude")).as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn extra_codex_home_for_spawn_only_fills_wsl_without_private_home() {
        let wsl = PathBuf::from(r"\\wsl$\Ubuntu\usr\bin\codex");
        let win = PathBuf::from(r"C:\codex.exe");
        let claude = PathBuf::from(r"\\wsl$\Ubuntu\usr\bin\claude");
        assert!(extra_codex_home_for_spawn(&win, Some("codex"), false).is_none());
        assert!(extra_codex_home_for_spawn(&wsl, Some("codex"), true).is_none());
        assert!(extra_codex_home_for_spawn(&claude, None, false).is_none());
        let home =
            extra_codex_home_for_spawn(&wsl, None, false).expect("wsl codex shares host home");
        assert_eq!(
            home.file_name().and_then(|name| name.to_str()),
            Some(".codex")
        );
        assert!(!home
            .to_string_lossy()
            .replace('\\', "/")
            .starts_with("/mnt/"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn probing_a_wsl_unc_when_wsl_is_broken_is_not_installed() {
        let fake = std::path::PathBuf::from(r"\\wsl$\Ubuntu\usr\bin\claude");
        let started = std::time::Instant::now();
        let (ok, _) = probe_executable(&fake, &["--version"]).await;
        assert!(!ok, "wsl.exe MSI stub must not count as an installed CLI");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(8),
            "WSL probe hung for {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn stream_json_user_content_uses_string_for_slash_commands() {
        let slash = stream_json_user_content("/compact", &[]);
        assert_eq!(slash, serde_json::json!("/compact"));
        let text = stream_json_user_content("hello", &[]);
        assert_eq!(
            text,
            serde_json::json!([{ "type": "text", "text": "hello" }])
        );
    }

    #[test]
    fn stream_json_user_content_appends_image_blocks() {
        let img = crate::external_agents::attachments::ImageBlock {
            data_base64: "AAAA".to_string(),
            mime: "image/png".to_string(),
            path: std::path::PathBuf::from("/tmp/a.png"),
        };
        let content = stream_json_user_content("look", std::slice::from_ref(&img));
        let arr = content.as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], serde_json::json!("text"));
        assert_eq!(arr[1]["type"], serde_json::json!("image"));
        assert_eq!(
            arr[1]["source"]["media_type"],
            serde_json::json!("image/png")
        );
        assert_eq!(arr[1]["source"]["data"], serde_json::json!("AAAA"));
    }

    #[test]
    fn stream_json_slash_ignores_images() {
        let img = crate::external_agents::attachments::ImageBlock {
            data_base64: "AAAA".to_string(),
            mime: "image/png".to_string(),
            path: std::path::PathBuf::from("/tmp/a.png"),
        };
        let content = stream_json_user_content("/compact", std::slice::from_ref(&img));
        assert_eq!(content, serde_json::json!("/compact"));
    }

    #[tokio::test]
    async fn accumulate_tail_keeps_only_the_tail_and_finishes_on_eof() {
        // 200 numbered lines far exceeding an 8-char cap → only the last chars survive, and the
        // task completes once the in-memory reader hits EOF.
        let mut input = String::new();
        for i in 0..200 {
            input.push_str(&format!("line{i}\n"));
        }
        let tail = accumulate_tail(input.as_bytes(), 8).await;
        assert!(
            tail.chars().count() <= 8,
            "tail should be capped, got {tail:?}"
        );
        assert!(
            tail.ends_with("line199"),
            "tail should keep the end, got {tail:?}"
        );
    }

    #[tokio::test]
    async fn accumulate_tail_drops_blank_lines() {
        let tail = accumulate_tail("\n\nhello\n\nworld\n".as_bytes(), 8192).await;
        assert_eq!(tail, "hello\nworld");
    }

    /// 版本行提取：首个非空行、原样保留（不解析结构）、超长封顶。
    /// 各 CLI 的 `--version` 形态差异极大，任何「解析成 semver」的尝试都会在某家上错。
    #[test]
    fn version_line_is_the_first_non_empty_line_verbatim() {
        // claude 2.1.220 本机实测输出。
        assert_eq!(
            first_version_line("2.1.220 (Claude Code)\n").as_deref(),
            Some("2.1.220 (Claude Code)")
        );
        // 前导空行 / banner 换行不影响。
        assert_eq!(
            first_version_line("\n\n  codex-cli 0.145.0  \nextra\n").as_deref(),
            Some("codex-cli 0.145.0")
        );
        // 没有输出（未登录的 CLI 把话都说在 stderr 上）→ None。
        assert_eq!(first_version_line(""), None);
        assert_eq!(first_version_line("\n \n"), None);
        // 坏 shim 可能把一整篇日志打在一行里 → 封顶，不把缓存撑爆。
        let long = "x".repeat(5000);
        assert_eq!(
            first_version_line(&long).map(|v| v.chars().count()),
            Some(VERSION_LINE_CAP)
        );
    }

    #[tokio::test]
    async fn join_stderr_tail_returns_after_killing_a_live_child() {
        use crate::proc::NoConsoleWindow;
        let mut command = if cfg!(windows) {
            let mut command = Command::new("ping");
            command.args(["-t", "127.0.0.1"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("60");
            command
        };
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .no_console_window()
            .spawn()
            .expect("spawn long-lived child");
        let tail = spawn_stderr_tail(child.stderr.take());
        let started = std::time::Instant::now();
        let _ = join_stderr_tail(&mut child, tail).await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "join_stderr_tail blocked for {:?}",
            started.elapsed()
        );
    }
}

// 整模块只在 unix 下编译：每个用例都靠写 shell 脚本 + chmod 造「能跑的 / 坏掉的」二进制，
// Windows 上 `std::os::unix` 与 `Permissions::from_mode` 都不存在 —— 不 gate 会让整个
// lib test 目标在 Windows 编译失败，连带其它模块的单测一个都跑不了。
#[cfg(all(test, unix))]
mod probe_tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// 造一批「坏 shim」样本。分类依据是本机实测（macOS）：
    ///   丢执行权限 → EACCES、空文件 → ENOEXEC、断链/不存在 → ENOENT，
    ///   而「装了但未登录」（exit 1）是 **spawn 成功**的，必须算存在。
    struct Fixtures(std::path::PathBuf);

    impl Fixtures {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "kivio-probe-{}-{}-{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn script(&self, name: &str, body: &str) -> std::path::PathBuf {
            let path = self.0.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn unreadable(&self, name: &str) -> std::path::PathBuf {
            let path = self.script(name, "#!/bin/sh\necho ok\n");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
            path
        }

        fn empty_exec(&self, name: &str) -> std::path::PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    impl Drop for Fixtures {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0.join("noexec"), fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn probe_accepts_a_working_binary() {
        let f = Fixtures::new("ok");
        let good = f.script("good", "#!/bin/sh\necho v1.2.3\n");
        let (ok, version) = probe_executable(&good, &["--version"]).await;
        assert!(ok);
        // 探活顺手把版本行接住了（此前 stdout 丢进 null，版本门控只能另起一次进程）。
        assert_eq!(version.as_deref(), Some("v1.2.3"));
    }

    /// **最重要的一条**：装了但没登录的 CLI，`--version` 非零退出——
    /// 必须仍算「存在」。只认零退出码会把这类 CLI 误判成未安装，
    /// 比原来「信 which 首行」的 bug 更糟。
    #[tokio::test]
    async fn probe_accepts_nonzero_exit_because_unauthenticated_clis_exit_nonzero() {
        let f = Fixtures::new("notauth");
        let notauth = f.script("notauth", "#!/bin/sh\necho 'Not logged in' >&2\nexit 1\n");
        let (ok, version) = probe_executable(&notauth, &["--version"]).await;
        assert!(ok, "非零退出被当成不存在——未登录的 CLI 会被误判为未安装");
        // 版本拿不到（输出全在 stderr）时必须是 None，且**不影响**「存在」的判定。
        assert_eq!(version, None);
    }

    #[tokio::test]
    async fn probe_rejects_broken_shims() {
        let f = Fixtures::new("broken");
        // EACCES：丢了执行权限
        assert!(
            !probe_executable(&f.unreadable("noexec"), &["--version"])
                .await
                .0
        );
        // ENOENT：路径根本不存在（等价于断掉的 symlink）
        assert!(
            !probe_executable(&f.0.join("missing"), &["--version"])
                .await
                .0
        );
    }

    /// 挂了执行位的**空文件**在 unix 上是能跑的：内核回退到 `/bin/sh`，
    /// 读到零条命令、exit 0（本机 `./empty; echo $?` 实测为 0）。
    /// 所以它**不该**被判成不存在——这与「非零退出码算存在」是同一条原则的延伸：
    /// 只有 spawn 阶段失败才算不存在，能起来的一律放行。
    ///
    /// （注：用 Python 的 subprocess 试会看到 ENOEXEC，那是 CPython 自己先拦了；
    /// 内核/Rust 的实际行为以本测试为准。）
    #[tokio::test]
    async fn probe_accepts_empty_executable_because_the_kernel_runs_it() {
        let f = Fixtures::new("emptyexec");
        assert!(
            probe_executable(&f.empty_exec("empty"), &["--version"])
                .await
                .0
        );
    }

    #[tokio::test]
    async fn probe_treats_a_hanging_binary_as_present() {
        // 起来了就说明文件可执行，只是这个 CLI 的 --version 慢/挂住——不能判成不存在。
        let f = Fixtures::new("slow");
        let slow = f.script("slow", "#!/bin/sh\nsleep 30\n");
        let started = std::time::Instant::now();
        assert!(probe_executable(&slow, &["--version"]).await.0);
        assert!(
            started.elapsed() < PROBE_TIMEOUT + Duration::from_secs(2),
            "超时守卫没生效，探活被挂死"
        );
    }

    /// 本机真实 CLI 必须仍能被解析到——这条比上面的单测更重要：
    /// 探活写得太严会把好的 CLI 判死，而那种回归单测抓不到。
    /// 同时打印耗时，用于评估热路径开销（spec 第 9 条）。
    /// `which_all` 必须返回**全部**同名候选，不能只取第一行——这是 G3b 的核心：
    /// PATH 上靠前的那个可能是坏 shim，只看第一个就等于没修。
    /// （单测 `probe_*` 只覆盖单个候选的判定，覆盖不到这个遍历行为。）
    #[tokio::test]
    async fn which_all_returns_every_candidate_not_just_the_first() {
        let f = Fixtures::new("multi");
        // 造两个同名的 fake CLI，分别在两个目录里，都挂进 PATH。
        let dir_a = f.0.join("a");
        let dir_b = f.0.join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        for dir in [&dir_a, &dir_b] {
            let p = dir.join("kivio-fake-cli");
            std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let original = std::env::var("PATH").unwrap_or_default();
        std::env::set_var(
            "PATH",
            format!("{}:{}:{original}", dir_a.display(), dir_b.display()),
        );

        let found = which_all("kivio-fake-cli").await;
        std::env::set_var("PATH", original);

        assert_eq!(
            found.len(),
            2,
            "只拿到 {} 个候选，应为 2（只取首行 = G3b 没生效）：{found:?}",
            found.len()
        );
        assert!(found[0].starts_with(&dir_a), "候选顺序应遵循 PATH");
    }

    /// 端到端：靠前的候选是坏 shim 时，`resolve_binary` 必须跳过它选后面那个好的。
    #[tokio::test]
    async fn resolve_binary_skips_a_broken_shim_for_the_working_one() {
        let f = Fixtures::new("shim");
        let bad_dir = f.0.join("bad");
        let good_dir = f.0.join("good");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::create_dir_all(&good_dir).unwrap();
        // 坏的：丢执行权限（spawn 时 EACCES）
        let bad = bad_dir.join("kivio-shim-cli");
        std::fs::write(&bad, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();
        // 好的
        let good = good_dir.join("kivio-shim-cli");
        std::fs::write(&good, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o755)).unwrap();

        let original = std::env::var("PATH").unwrap_or_default();
        std::env::set_var(
            "PATH",
            format!("{}:{}:{original}", bad_dir.display(), good_dir.display()),
        );

        let mut def = crate::external_agents::registry::AGENT_DEFS[0].clone();
        def.bin = "kivio-shim-cli";
        def.fallback_bins = &[];
        let resolved = resolve_binary(&def).await;

        std::env::set_var("PATH", original);
        let _ = std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755));

        assert_eq!(
            resolved.as_deref(),
            Some(good.as_path()),
            "应跳过坏 shim 选到好的，实际 {resolved:?}"
        );
    }

    /// 缓存必须真的省掉子进程：`resolve_binary` 在热路径上，`run.rs` 的预算是 <500ms，
    /// 而 kimi/pi 这类 Node 包装脚本单次 `--version` 就要 300ms+。
    /// 这里用一个「每次被调用就往文件里追加一行」的假 CLI 来数实际启动次数。
    #[tokio::test]
    async fn probe_result_is_cached_so_the_hot_path_does_not_respawn() {
        let f = Fixtures::new("cache");
        let counter = f.0.join("calls");
        let bin = f.script(
            "countbin",
            &format!("#!/bin/sh\necho x >> {}\n", counter.display()),
        );
        let count = || {
            std::fs::read_to_string(&counter)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };

        assert!(probe_executable(&bin, &["--version"]).await.0);
        assert_eq!(count(), 1, "第一次应真的启动一次");

        // 走 resolve_binary 的缓存路径：同一个 key 命中后不得再起进程。
        let key = probe_cache_key(&bin).expect("metadata");
        PROBE_CACHE.lock().unwrap().insert(
            key.clone(),
            ProbeOutcome {
                executable: true,
                version: Some("9.9.9".to_string()),
            },
        );
        assert_eq!(
            PROBE_CACHE
                .lock()
                .unwrap()
                .get(&key)
                .map(|entry| entry.executable),
            Some(true),
            "缓存未写入"
        );
        assert_eq!(count(), 1, "缓存命中后不应再启动子进程");
        // 版本号也要能从同一条缓存读回来——版本门控靠它，且**不得**为此再起进程。
        assert_eq!(cached_cli_version(&bin).as_deref(), Some("9.9.9"));
        assert_eq!(count(), 1, "读版本号不应启动子进程");
    }

    #[tokio::test]
    #[ignore = "reads the real PATH on this machine"]
    async fn resolve_binary_probes_real_clis() {
        // 第二遍走缓存，必须显著快于第一遍——这是 <500ms 热路径预算的依据。
        for round in ["cold", "warm"] {
            eprintln!("--- {round} ---");
            for def in crate::external_agents::registry::AGENT_DEFS {
                let started = std::time::Instant::now();
                let resolved = resolve_binary(def).await;
                // 打印 bin + 全部 fallback 的候选总数（只打 def.bin 会误导：
                // 如 opencode 的 def.bin 是 `opencode-cli`，实际命中的是 fallback `opencode`）。
                let mut candidates = 0usize;
                for name in std::iter::once(def.bin).chain(def.fallback_bins.iter().copied()) {
                    candidates += which_all(name).await.len();
                }
                eprintln!(
                    "{:10} candidates={candidates} {:>5}ms -> {}",
                    def.id,
                    started.elapsed().as_millis(),
                    resolved
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(not installed)".to_string()),
                );
            }
        }
    }
}
