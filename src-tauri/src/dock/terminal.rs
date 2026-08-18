//! Dock 终端面板：portable-pty 起真实 PTY 会话（用户默认 shell），输出经
//! `dock:terminal-output` 事件流给前端 xterm.js；子进程退出发 `dock:terminal-exit`。
//! 会话登记表走 managed state（同 WorkspaceWatchService 的模式），应用退出时
//! service 随 AppState 一起 drop，各 session 的 Drop 兜底 kill 子进程。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::Emitter;

pub const TERMINAL_OUTPUT_EVENT: &str = "dock:terminal-output";
pub const TERMINAL_EXIT_EVENT: &str = "dock:terminal-exit";

/// 子进程退出的轮询间隔：不占用 wait() 的所有权，close 命令仍能随时 kill。
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// PTY reader 单次读取缓冲：xterm.js 按事件消费，块太小事件洪峰，太大延迟感明显。
const READ_BUF_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutputPayload {
    session_id: String,
    /// UTF-8 边界按 lossy 处理：终端输出本就容忍替换字符，不值得为半个字节组帧。
    data: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalExitPayload {
    session_id: String,
    code: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreateResult {
    pub session_id: String,
}

/// 一个存活会话：master 负责 resize，writer 负责写入，child 负责 kill/查活。
struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // 尽力而为：应用退出 / close 命令走到这里时 child 可能已死，忽略错误。
        let _ = self.child.kill();
    }
}

/// 会话登记表：与 service 解耦成泛型小结构，纯逻辑（增/删/改/幂等）可单测——
/// TerminalService 需要 AppHandle，无法脱离 Tauri 运行时构造。
struct SessionRegistry<T> {
    map: Mutex<HashMap<String, T>>,
}

impl<T> Default for SessionRegistry<T> {
    fn default() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }
}

impl<T> SessionRegistry<T> {
    fn insert(&self, id: String, session: T) -> Result<(), String> {
        let mut map = self
            .map
            .lock()
            .map_err(|_| "terminal sessions lock poisoned".to_string())?;
        map.insert(id, session);
        Ok(())
    }

    /// 摘除会话（幂等）：返回被摘的 session，其 Drop 负责收尾。
    fn take(&self, id: &str) -> Option<T> {
        self.map.lock().ok()?.remove(id)
    }

    /// 锁内可变访问（write / resize / try_wait），会话不存在时报统一错误。
    fn with_mut<R>(
        &self,
        id: &str,
        f: impl FnOnce(&mut T) -> Result<R, String>,
    ) -> Result<R, String> {
        let mut map = self
            .map
            .lock()
            .map_err(|_| "terminal sessions lock poisoned".to_string())?;
        let session = map
            .get_mut(id)
            .ok_or_else(|| format!("terminal session not found: {id}"))?;
        f(session)
    }

    /// 锁内只读探查（exit watcher 用）：会话已被摘除时返回 None，不报错。
    fn peek_mut<R>(&self, id: &str, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut map = self.map.lock().ok()?;
        map.get_mut(id).map(f)
    }
}

pub struct TerminalService {
    app_handle: tauri::AppHandle,
    sessions: SessionRegistry<TerminalSession>,
}

impl TerminalService {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self {
            app_handle,
            sessions: SessionRegistry::default(),
        }
    }

    fn remove_session(&self, session_id: &str) -> Option<TerminalSession> {
        self.sessions.take(session_id)
    }

    fn emit_exit(&self, session_id: &str, code: u32) {
        let payload = TerminalExitPayload {
            session_id: session_id.to_string(),
            code,
        };
        if let Err(error) = self.app_handle.emit(TERMINAL_EXIT_EVENT, payload) {
            eprintln!("dock terminal: emit exit for {session_id} failed: {error}");
        }
    }
}

/// Unix：以用户的**登录 shell** 为准（与 VS Code 默认终端同策略）。
/// 只看 $SHELL 会在「从别的 shell 里起 App」时带错——比如从 bash 环境起 dev，
/// 终端就变 bash，而用户登录 shell 可能是 fish/zsh。macOS 走 dscl 读 OpenDirectory。
#[cfg(not(target_os = "windows"))]
fn resolve_shell(shell_env: Option<&str>) -> String {
    resolve_shell_priority(login_shell().as_deref(), shell_env)
}

/// 纯优先级逻辑（可测）：登录 shell > $SHELL > /bin/zsh，每级都校验可执行文件存在。
#[cfg(not(target_os = "windows"))]
fn resolve_shell_priority(login: Option<&str>, shell_env: Option<&str>) -> String {
    for candidate in [login, shell_env].into_iter().flatten() {
        let shell = candidate.trim();
        if !shell.is_empty() && is_executable_file(shell) {
            return shell.to_string();
        }
    }
    // kivio 只发 macOS/Windows，zsh 是 macOS 默认 shell。
    "/bin/zsh".to_string()
}

#[cfg(not(target_os = "windows"))]
fn is_executable_file(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// macOS：dscl 读 OpenDirectory 的登录 shell（GUI/CLI 任何启动方式下都准确）。
#[cfg(target_os = "macos")]
fn login_shell() -> Option<String> {
    let user = std::env::var("USER").ok()?;
    let output = std::process::Command::new("dscl")
        .args([".", "-read", &format!("/Users/{user}"), "UserShell"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .strip_prefix("UserShell:")
        .map(str::trim)
        .filter(|shell| shell.starts_with('/'))
        .map(str::to_string)
}

/// Linux：getent passwd 末段即登录 shell（kivio 目前不发 Linux，保留兜底路径）。
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn login_shell() -> Option<String> {
    let user = std::env::var("USER").ok()?;
    let output = std::process::Command::new("getent")
        .args(["passwd", &user])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .rsplit(':')
        .next()
        .filter(|shell| shell.starts_with('/'))
        .map(str::to_string)
}

/// Windows：优先 pwsh（PowerShell 7+），否则系统自带的 powershell。
/// 与 native_tools/shell.rs 的 windows_powershell_exe 同策略，扫 PATH 不引 which。
#[cfg(target_os = "windows")]
fn resolve_shell(_shell_env: Option<&str>) -> String {
    let pwsh_on_path = std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join("pwsh.exe").is_file())
    });
    if pwsh_on_path {
        "pwsh".to_string()
    } else {
        "powershell".to_string()
    }
}

fn make_session_id() -> String {
    format!("term-{}", uuid::Uuid::new_v4())
}

/// 校验 workdir：必须存在且是目录；canonicalize 顺带解掉符号链接与相对分量，
/// 保证 PTY 子进程的 cwd 与文件树 / Git 面板看到的目录一致。
/// Windows 上 canonicalize 会带 `\\?\` 扩展长度前缀，CreateProcess 的
/// lpCurrentDirectory 不认它（PowerShell 起来后 $PWD 变成 FileSystem:: 怪路径），必须摘掉。
fn validate_workdir(workdir: &str) -> Result<std::path::PathBuf, String> {
    let trimmed = workdir.trim();
    if trimmed.is_empty() {
        return Err("workdir is empty".to_string());
    }
    let path = std::fs::canonicalize(trimmed).map_err(|e| format!("resolve workdir: {e}"))?;
    if !path.is_dir() {
        return Err(format!("workdir is not a directory: {}", path.display()));
    }
    #[cfg(target_os = "windows")]
    let path = {
        let text = path.to_string_lossy();
        match text.strip_prefix(r"\\?\") {
            // UNC 形式 `\\?\UNC\server\share` 摘前缀会变成非法路径，保持原样。
            Some(rest) if !rest.starts_with("UNC\\") => std::path::PathBuf::from(rest),
            _ => path.clone(),
        }
    };
    Ok(path)
}

/// 创建终端会话：起 PTY + 默认 shell，reader 线程推输出事件，watcher 线程报退出。
#[tauri::command]
pub async fn dock_terminal_create(
    state: tauri::State<'_, Arc<TerminalService>>,
    workdir: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalCreateResult, String> {
    let service: Arc<TerminalService> = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || create_session(service, &workdir, cols, rows))
        .await
        .map_err(|e| format!("dock_terminal_create join: {e}"))?
}

fn create_session(
    service: Arc<TerminalService>,
    workdir: &str,
    cols: u16,
    rows: u16,
) -> Result<TerminalCreateResult, String> {
    let cwd = validate_workdir(workdir)?;
    let cols = cols.max(2);
    let rows = rows.max(1);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("open pty: {e}"))?;

    let shell = resolve_shell(std::env::var("SHELL").ok().as_deref());
    let mut cmd = CommandBuilder::new(&shell);
    // Windows 的 shell 一定是 pwsh/powershell：吞掉版权横幅，第一屏直接是提示符。
    #[cfg(target_os = "windows")]
    cmd.arg("-NoLogo");
    cmd.cwd(&cwd);
    // portable-pty 默认 TERM=dumb，starship 等提示符框架会直接罢工；
    // xterm.js 的能力对齐 xterm-256color。
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // 父进程（比如从 CI/工具环境启动 dev）可能带 NO_COLOR=1，chalk 系的 CLI
    // （claude 等）见到它就输出全灰——终端是给人看的，强制摘除。
    cmd.env_remove("NO_COLOR");
    // Finder/Dock 起的 GUI 进程经常没有 UTF-8 locale。BSD `ls` 把 C locale 下
    // 的「不可打印」字符（包括中文）替换成 `?`。与 PATH 补齐同一类缺口：这里
    // 显式写到 PTY，避免继承来的 LC_ALL=C 盖掉进程里刚补上的 LANG。
    let locale = crate::path_env::utf8_locale_overrides();
    cmd.env("LANG", locale.lang);
    if locale.remove_lc_all {
        cmd.env_remove("LC_ALL");
    }
    if locale.remove_lc_ctype {
        cmd.env_remove("LC_CTYPE");
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell {shell}: {e}"))?;
    // slave 侧句柄只用于 spawn；留着会让 child 退出后 master read 永远等不到 EOF。
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take pty writer: {e}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone pty reader: {e}"))?;

    let session_id = make_session_id();
    service.sessions.insert(
        session_id.clone(),
        TerminalSession {
            master: pair.master,
            writer,
            child,
        },
    )?;

    // reader 线程：PTY 输出 → `dock:terminal-output`。EOF/读失败（child 退出后
    // master 关闭）即结束；会话已被 close 移除时静默退出。
    {
        let service = Arc::clone(&service);
        let event_session_id = session_id.clone();
        thread::Builder::new()
            .name("dock-terminal-reader".to_string())
            .spawn(move || {
                let mut buf = [0u8; READ_BUF_SIZE];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => return,
                        Ok(n) => {
                            let payload = TerminalOutputPayload {
                                session_id: event_session_id.clone(),
                                data: String::from_utf8_lossy(&buf[..n]).into_owned(),
                            };
                            if let Err(error) =
                                service.app_handle.emit(TERMINAL_OUTPUT_EVENT, payload)
                            {
                                eprintln!(
                                    "dock terminal: emit output for {event_session_id} failed: {error}"
                                );
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
            })
            .map_err(|e| format!("spawn terminal reader thread: {e}"))?;
    }

    // exit watcher 线程：轮询 try_wait（不拿走 child 所有权，close 才能 kill），
    // 退出时发 `dock:terminal-exit` 并从表里摘除（摘下的 session drop 即兜底 kill）。
    {
        let service = Arc::clone(&service);
        let watch_session_id = session_id.clone();
        thread::Builder::new()
            .name("dock-terminal-exit-watch".to_string())
            .spawn(move || loop {
                thread::sleep(EXIT_POLL_INTERVAL);
                // peek_mut 返回 None = 会话已被 close/重建摘除，本线程静默退出。
                let Some(status) = service
                    .sessions
                    .peek_mut(&watch_session_id, |session| session.child.try_wait())
                else {
                    return;
                };
                match status {
                    Ok(Some(status)) => {
                        service.remove_session(&watch_session_id);
                        service.emit_exit(&watch_session_id, status.exit_code());
                        return;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        service.remove_session(&watch_session_id);
                        service.emit_exit(&watch_session_id, 0);
                        return;
                    }
                }
            })
            .map_err(|e| format!("spawn terminal exit-watch thread: {e}"))?;
    }

    Ok(TerminalCreateResult { session_id })
}

/// 前端 xterm onData → 写 PTY（用户键盘输入）。
#[tauri::command]
pub async fn dock_terminal_write(
    state: tauri::State<'_, Arc<TerminalService>>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let service: Arc<TerminalService> = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.sessions.with_mut(&session_id, |session| {
            session
                .writer
                .write_all(data.as_bytes())
                .map_err(|e| format!("write pty: {e}"))?;
            session
                .writer
                .flush()
                .map_err(|e| format!("flush pty: {e}"))?;
            Ok(())
        })
    })
    .await
    .map_err(|e| format!("dock_terminal_write join: {e}"))?
}

/// xterm FitAddon 量出的行列变化 → PTY resize（TIOCSWINSZ / ConPTY resize）。
#[tauri::command]
pub async fn dock_terminal_resize(
    state: tauri::State<'_, Arc<TerminalService>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let service: Arc<TerminalService> = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.sessions.with_mut(&session_id, |session| {
            session
                .master
                .resize(PtySize {
                    rows: rows.max(1),
                    cols: cols.max(2),
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| format!("resize pty: {e}"))?;
            Ok(())
        })
    })
    .await
    .map_err(|e| format!("dock_terminal_resize join: {e}"))?
}

/// 关闭会话：摘除 + drop（Drop impl 负责 kill 子进程）。幂等：不存在也返回 Ok。
#[tauri::command]
pub async fn dock_terminal_close(
    state: tauri::State<'_, Arc<TerminalService>>,
    session_id: String,
) -> Result<(), String> {
    let service: Arc<TerminalService> = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.remove_session(&session_id);
        Ok(())
    })
    .await
    .map_err(|e| format!("dock_terminal_close join: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_has_term_prefix_and_is_unique() {
        let a = make_session_id();
        let b = make_session_id();
        assert!(a.starts_with("term-"));
        assert!(b.starts_with("term-"));
        assert_ne!(a, b);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn shell_resolution_unix() {
        // 登录 shell 优先于 $SHELL（桌面 App 以登录 shell 为准）。
        assert_eq!(
            resolve_shell_priority(Some("/bin/zsh"), Some("/bin/bash")),
            "/bin/zsh"
        );
        // 登录 shell 缺失/不可执行 → 回退 $SHELL。
        assert_eq!(
            resolve_shell_priority(Some("/nonexistent/shell"), Some("/bin/bash")),
            "/bin/bash"
        );
        assert_eq!(resolve_shell_priority(None, Some("/bin/bash")), "/bin/bash");
        // 空串 / 纯空白视为未设置，最终回退 /bin/zsh。
        assert_eq!(resolve_shell_priority(Some("  "), Some("  ")), "/bin/zsh");
        assert_eq!(resolve_shell_priority(None, None), "/bin/zsh");
        // dscl 在本机应能解析出当前用户的登录 shell（macOS only 隐含在 cfg 里）。
        let login = login_shell();
        assert!(login.is_some(), "login_shell() should resolve on macOS");
    }

    #[test]
    fn validate_workdir_rejects_empty_and_missing() {
        assert!(validate_workdir("").is_err());
        assert!(validate_workdir("   ").is_err());
        assert!(validate_workdir("/definitely/not/a/real/path/kivio").is_err());
    }

    #[test]
    fn validate_workdir_rejects_file_accepts_dir() {
        let dir = std::env::temp_dir().join(format!("kivio-dock-term-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let file = dir.join("a.txt");
        let mut f = std::fs::File::create(&file).expect("create temp file");
        f.write_all(b"x").expect("write temp file");
        drop(f);

        assert!(validate_workdir(file.to_string_lossy().as_ref()).is_err());
        let resolved = validate_workdir(dir.to_string_lossy().as_ref()).expect("dir valid");
        assert!(resolved.is_dir());
        // Windows: CreateProcess 不认 `\\?\` 前缀，canonicalize 的结果必须已摘除。
        #[cfg(target_os = "windows")]
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "workdir leaked extended-length prefix: {}",
            resolved.display()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn session_registry_insert_take_with_mut() {
        let registry = SessionRegistry::<u32>::default();
        registry.insert("term-a".to_string(), 1).expect("insert");
        // with_mut 锁内可变访问；不存在的 id 报统一错误。
        let bumped = registry
            .with_mut("term-a", |v| {
                *v += 1;
                Ok(*v)
            })
            .expect("with_mut existing");
        assert_eq!(bumped, 2);
        assert!(registry.with_mut("term-missing", |v| Ok(*v)).is_err());
        // take 返回被摘值且幂等。
        assert_eq!(registry.take("term-a"), Some(2));
        assert_eq!(registry.take("term-a"), None);
        // peek_mut 对已摘除的会话返回 None（exit watcher 的静默退出路径）。
        assert_eq!(registry.peek_mut("term-a", |v| *v), None);
    }
}
