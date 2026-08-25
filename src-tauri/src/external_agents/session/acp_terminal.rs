//! ACP client `terminal/*` methods.
//!
//! 规格里这是可选能力：agent **可以**把 shell 交给宿主 `terminal/create`，也可以继续
//! 本地 exec。OpenCode / Gemini 的 glob/grep（以及多数 bash）仍在 CLI 进程里跑；
//! **Kimi Code 0.37** 则在 `terminal: true` 时把**全部** `process.spawn` 换成问宿主
//! 要终端，并且只放行 `bash -c`（Glob/Grep 的 `fd`/`rg` 在发 RPC 前就被扔掉）。
//! 握手里 `terminal` 为 false（或方法没实现）时，Kimi 会报
//! `ACP terminal capability is unavailable`——Bash 也会一起死，并不会回到 TUI 后端。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::native_tools::kill_process_group;

const MAX_OUTPUT_BYTE_LIMIT: usize = 8 * 1024 * 1024;

#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

#[derive(Debug, Clone)]
pub struct TerminalExit {
    pub exit_code: Option<i64>,
    pub signal: Option<String>,
}

pub enum TerminalReply {
    Result(Value),
    Pending,
    Error { code: i64, message: String },
}

struct OutputState {
    bytes: Vec<u8>,
    truncated: bool,
    limit: usize,
}

struct Terminal {
    output: Arc<Mutex<OutputState>>,
    exit: Option<TerminalExit>,
    pid: Option<u32>,
}

pub struct AcpTerminalHost {
    session_cwd: PathBuf,
    session_id: Option<String>,
    terminals: HashMap<String, Terminal>,
    pending_waits: HashMap<String, Vec<Value>>,
    aborted_waits: Vec<(Value, i64, String)>,
    completed_waits: Vec<(Value, Value)>,
    /// Spec: after `terminal/release` the RPC id is gone, but the Client SHOULD
    /// keep showing the captured output on the tool card.
    released_output: HashMap<String, String>,
    exit_tx: mpsc::UnboundedSender<(String, TerminalExit)>,
    exit_rx: mpsc::UnboundedReceiver<(String, TerminalExit)>,
}

impl AcpTerminalHost {
    pub fn new(session_cwd: PathBuf) -> Self {
        let (exit_tx, exit_rx) = mpsc::unbounded_channel();
        Self {
            session_cwd,
            session_id: None,
            terminals: HashMap::new(),
            pending_waits: HashMap::new(),
            aborted_waits: Vec::new(),
            completed_waits: Vec::new(),
            released_output: HashMap::new(),
            exit_tx,
            exit_rx,
        }
    }

    /// Last captured stdout/stderr for a live or already-released terminal.
    pub fn preview_output(&self, terminal_id: &str) -> Option<String> {
        if let Some(term) = self.terminals.get(terminal_id) {
            let (output, _) = snapshot_output(&term.output);
            return Some(output);
        }
        self.released_output.get(terminal_id).cloned()
    }

    #[cfg(test)]
    pub fn seed_output(&mut self, terminal_id: &str, output: impl Into<String>) {
        self.released_output
            .insert(terminal_id.to_string(), output.into());
    }

    pub fn set_extra_roots(&mut self, _roots: &[String]) {
        // Spec only requires cwd to be absolute. Workspace confinement is a
        // host policy, not an ACP rule; agents (kimi/cursor) pass temp dirs
        // and the session/new cwd spelling, which may not match extra_roots.
    }

    pub fn set_session_id(&mut self, session_id: impl Into<String>) {
        self.session_id = Some(session_id.into());
    }

    pub fn take_aborted_waits(&mut self) -> Vec<(Value, i64, String)> {
        std::mem::take(&mut self.aborted_waits)
    }

    pub fn take_completed_waits(&mut self) -> Vec<(Value, Value)> {
        std::mem::take(&mut self.completed_waits)
    }

    pub fn exit_rx(&mut self) -> &mut mpsc::UnboundedReceiver<(String, TerminalExit)> {
        &mut self.exit_rx
    }

    pub fn on_exit(&mut self, terminal_id: String, exit: TerminalExit) -> Vec<(Value, Value)> {
        if let Some(term) = self.terminals.get_mut(&terminal_id) {
            term.exit = Some(exit.clone());
        }
        let ids = self.pending_waits.remove(&terminal_id).unwrap_or_default();
        ids.into_iter()
            .map(|rpc_id| (rpc_id, wait_result(&exit)))
            .collect()
    }

    pub fn close_all(&mut self) {
        let pending: Vec<String> = self.pending_waits.keys().cloned().collect();
        for terminal_id in pending {
            self.abort_waits(&terminal_id, "session closed");
        }
        for term in self.terminals.values() {
            if term.exit.is_none() {
                if let Some(pid) = term.pid {
                    kill_process_group(pid);
                }
            }
        }
        self.terminals.clear();
        self.pending_waits.clear();
    }

    pub fn handle(&mut self, method: &str, params: &Value, rpc_id: &Value) -> TerminalReply {
        if let Err(reply) = self.check_session(params) {
            return reply;
        }
        match method {
            "terminal/create" => self.create(params),
            "terminal/output" => self.output(params),
            "terminal/wait_for_exit" => self.wait_for_exit(params, rpc_id),
            "terminal/kill" => self.kill(params),
            "terminal/release" => self.release(params),
            _ => TerminalReply::Error {
                code: -32601,
                message: format!("Method not found: {method}"),
            },
        }
    }

    fn create(&mut self, params: &Value) -> TerminalReply {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(command) = command else {
            return invalid("terminal/create requires command");
        };
        let args: Vec<String> = params
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let cwd = match self.resolve_cwd(params.get("cwd").and_then(|v| v.as_str())) {
            Ok(cwd) => cwd,
            Err(message) => return invalid(&message),
        };
        let limit = output_byte_limit(params.get("outputByteLimit"));
        let env = parse_env(params.get("env"));

        let mut cmd = Command::new(command);
        cmd.args(&args)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .env("TERM", "dumb")
            .env("NO_COLOR", "1")
            .env("FORCE_COLOR", "0")
            .env("PAGER", "cat");
        for (name, value) in env {
            cmd.env(name, value);
        }
        apply_process_group(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                return TerminalReply::Error {
                    code: -32603,
                    message: format!("Failed to start command: {err}"),
                };
            }
        };
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let output = Arc::new(Mutex::new(OutputState {
            bytes: Vec::new(),
            truncated: false,
            limit,
        }));
        let stdout_task = stdout.map(|pipe| spawn_pipe_reader(output.clone(), pipe));
        let stderr_task = stderr.map(|pipe| spawn_pipe_reader(output.clone(), pipe));

        let terminal_id = format!("term_{}", uuid::Uuid::new_v4());
        let exit_tx = self.exit_tx.clone();
        let wait_id = terminal_id.clone();
        tokio::spawn(async move {
            let exit = wait_exit(child).await;
            // Spec: after the command completes, terminal/output is the final
            // captured output. Wait for pipe EOF before publishing exit.
            if let Some(task) = stdout_task {
                let _ = task.await;
            }
            if let Some(task) = stderr_task {
                let _ = task.await;
            }
            let _ = exit_tx.send((wait_id, exit));
        });

        self.terminals.insert(
            terminal_id.clone(),
            Terminal {
                output,
                exit: None,
                pid,
            },
        );
        TerminalReply::Result(json!({ "terminalId": terminal_id }))
    }

    fn output(&self, params: &Value) -> TerminalReply {
        let Some(term) = self.terminal(params) else {
            return unknown_terminal();
        };
        let (output, truncated) = snapshot_output(&term.output);
        let mut result = json!({
            "output": output,
            "truncated": truncated,
        });
        if let Some(exit) = &term.exit {
            result["exitStatus"] = json!({
                "exitCode": exit.exit_code,
                "signal": exit.signal,
            });
        }
        TerminalReply::Result(result)
    }

    fn wait_for_exit(&mut self, params: &Value, rpc_id: &Value) -> TerminalReply {
        let Some(terminal_id) = terminal_id(params) else {
            return unknown_terminal();
        };
        let Some(term) = self.terminals.get(terminal_id) else {
            return unknown_terminal();
        };
        if let Some(exit) = &term.exit {
            return TerminalReply::Result(wait_result(exit));
        }
        self.pending_waits
            .entry(terminal_id.to_string())
            .or_default()
            .push(rpc_id.clone());
        TerminalReply::Pending
    }

    fn kill(&self, params: &Value) -> TerminalReply {
        let Some(term) = self.terminal(params) else {
            return unknown_terminal();
        };
        if term.exit.is_none() {
            if let Some(pid) = term.pid {
                kill_process_group(pid);
            }
        }
        TerminalReply::Result(json!({}))
    }

    fn release(&mut self, params: &Value) -> TerminalReply {
        let Some(terminal_id) = terminal_id(params) else {
            return unknown_terminal();
        };
        let Some(term) = self.terminals.remove(terminal_id) else {
            return unknown_terminal();
        };
        let (output, _) = snapshot_output(&term.output);
        if !output.is_empty() {
            self.released_output.insert(terminal_id.to_string(), output);
        }
        if let Some(exit) = term.exit {
            self.finish_waits(terminal_id, &exit);
        } else {
            if let Some(pid) = term.pid {
                kill_process_group(pid);
            }
            self.abort_waits(terminal_id, "terminal released");
        }
        TerminalReply::Result(json!({}))
    }

    fn terminal(&self, params: &Value) -> Option<&Terminal> {
        self.terminals.get(terminal_id(params)?)
    }

    fn resolve_cwd(&self, requested: Option<&str>) -> Result<PathBuf, String> {
        let Some(requested) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(self.session_cwd.clone());
        };
        let path = PathBuf::from(requested);
        if !path.is_absolute() {
            return Err("cwd must be an absolute path".to_string());
        }
        Ok(path)
    }

    fn check_session(&self, params: &Value) -> Result<(), TerminalReply> {
        let Some(expected) = self.session_id.as_deref() else {
            return Ok(());
        };
        match params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(got) if got == expected => Ok(()),
            Some(_) => Err(invalid("sessionId does not match this session")),
            None => Err(invalid("sessionId is required")),
        }
    }

    fn abort_waits(&mut self, terminal_id: &str, message: &str) {
        if let Some(ids) = self.pending_waits.remove(terminal_id) {
            for rpc_id in ids {
                self.aborted_waits
                    .push((rpc_id, -32603, message.to_string()));
            }
        }
    }

    fn finish_waits(&mut self, terminal_id: &str, exit: &TerminalExit) {
        if let Some(ids) = self.pending_waits.remove(terminal_id) {
            for rpc_id in ids {
                self.completed_waits.push((rpc_id, wait_result(exit)));
            }
        }
    }
}

impl Drop for AcpTerminalHost {
    fn drop(&mut self) {
        self.close_all();
    }
}

fn wait_result(exit: &TerminalExit) -> Value {
    json!({
        "exitCode": exit.exit_code,
        "signal": exit.signal,
    })
}

fn invalid(message: &str) -> TerminalReply {
    TerminalReply::Error {
        code: -32602,
        message: message.to_string(),
    }
}

fn unknown_terminal() -> TerminalReply {
    invalid("unknown terminalId")
}

fn terminal_id(params: &Value) -> Option<&str> {
    params
        .get("terminalId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn parse_env(value: Option<&Value>) -> Vec<(String, String)> {
    let Some(items) = value.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(|v| v.as_str())?;
            let value = item.get("value").and_then(|v| v.as_str())?;
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

fn output_byte_limit(value: Option<&Value>) -> usize {
    match value.and_then(|v| v.as_u64()) {
        // Schema: omitted means no truncation. Keep a host safety cap so a
        // runaway command cannot grow the buffer without bound.
        None => MAX_OUTPUT_BYTE_LIMIT,
        Some(n) => (n as usize).min(MAX_OUTPUT_BYTE_LIMIT),
    }
}

fn apply_process_group(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(crate::proc::CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn spawn_pipe_reader<R>(
    output: Arc<Mutex<OutputState>>,
    mut reader: R,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut state) = output.lock() {
                        append_output(&mut state, &buf[..n]);
                    }
                }
            }
        }
    })
}

async fn wait_exit(mut child: Child) -> TerminalExit {
    match child.wait().await {
        Ok(status) => exit_from_status(status),
        Err(_) => TerminalExit {
            exit_code: None,
            signal: None,
        },
    }
}

fn exit_from_status(status: std::process::ExitStatus) -> TerminalExit {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return TerminalExit {
                exit_code: None,
                signal: Some(signal.to_string()),
            };
        }
    }
    TerminalExit {
        exit_code: status.code().map(|code| code as i64),
        signal: None,
    }
}

fn snapshot_output(output: &Arc<Mutex<OutputState>>) -> (String, bool) {
    let Ok(state) = output.lock() else {
        return (String::new(), false);
    };
    (
        String::from_utf8_lossy(&state.bytes).into_owned(),
        state.truncated,
    )
}

fn append_output(state: &mut OutputState, chunk: &[u8]) {
    state.bytes.extend_from_slice(chunk);
    if state.bytes.len() <= state.limit {
        return;
    }
    let overflow = state.bytes.len() - state.limit;
    state.bytes.drain(..overflow);
    let skip = state
        .bytes
        .iter()
        .position(|b| *b & 0b1100_0000 != 0b1000_0000)
        .unwrap_or(state.bytes.len());
    if skip > 0 {
        state.bytes.drain(..skip);
    }
    state.truncated = true;
}

#[cfg(test)]
impl OutputState {
    fn new_for_test(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
            limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn truncates_from_the_front_on_a_utf8_boundary() {
        let mut state = OutputState::new_for_test(3);
        append_output(&mut state, "éé".as_bytes());
        assert!(state.truncated);
        assert_eq!(String::from_utf8(state.bytes.clone()).expect("utf-8"), "é");
    }

    #[test]
    fn output_byte_limit_zero_retains_nothing() {
        let mut state = OutputState::new_for_test(0);
        append_output(&mut state, b"hello");
        assert!(state.truncated);
        assert!(state.bytes.is_empty());
    }

    #[tokio::test]
    async fn create_wait_output_and_release_a_short_command() {
        let cwd = std::env::temp_dir();
        let mut host = AcpTerminalHost::new(cwd.clone());
        let (command, args) = echo_command("kivio-acp-term");
        let created = host.handle(
            "terminal/create",
            &json!({
                "command": command,
                "args": args,
                "cwd": cwd.to_string_lossy(),
            }),
            &json!(1),
        );
        let terminal_id = match created {
            TerminalReply::Result(value) => value["terminalId"]
                .as_str()
                .expect("terminalId")
                .to_string(),
            other => panic!("create failed: {other:?}"),
        };

        let exit = tokio::time::timeout(Duration::from_secs(5), host.exit_rx().recv())
            .await
            .expect("exit event")
            .expect("terminal exit");
        assert_eq!(exit.0, terminal_id);
        let replies = host.on_exit(exit.0, exit.1);
        assert!(replies.is_empty(), "no waiter was registered yet");

        match host.handle(
            "terminal/wait_for_exit",
            &json!({ "terminalId": terminal_id }),
            &json!(2),
        ) {
            TerminalReply::Result(value) => {
                assert_eq!(value["exitCode"], json!(0));
            }
            other => panic!("wait after exit should be immediate: {other:?}"),
        }

        match host.handle(
            "terminal/output",
            &json!({ "terminalId": terminal_id }),
            &json!(3),
        ) {
            TerminalReply::Result(value) => {
                let output = value["output"].as_str().unwrap_or("");
                assert!(output.contains("kivio-acp-term"), "output={output:?}");
                assert_eq!(value["truncated"], json!(false));
                assert_eq!(value["exitStatus"]["exitCode"], json!(0));
            }
            other => panic!("output failed: {other:?}"),
        }

        match host.handle(
            "terminal/release",
            &json!({ "terminalId": terminal_id }),
            &json!(4),
        ) {
            TerminalReply::Result(_) => {}
            other => panic!("release failed: {other:?}"),
        }
        let preview = host
            .preview_output(&terminal_id)
            .expect("release must keep captured output for the tool card");
        assert!(
            preview.contains("kivio-acp-term"),
            "preview after release={preview:?}"
        );
        match host.handle(
            "terminal/output",
            &json!({ "terminalId": terminal_id }),
            &json!(5),
        ) {
            TerminalReply::Error { code, .. } => assert_eq!(code, -32602),
            other => panic!("released terminal must be invalid: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_exit_stays_pending_until_the_process_ends() {
        let cwd = std::env::temp_dir();
        let mut host = AcpTerminalHost::new(cwd.clone());
        let (command, args) = sleep_command();
        let created = host.handle(
            "terminal/create",
            &json!({
                "command": command,
                "args": args,
                "cwd": cwd.to_string_lossy(),
            }),
            &json!(1),
        );
        let terminal_id = match created {
            TerminalReply::Result(value) => value["terminalId"]
                .as_str()
                .expect("terminalId")
                .to_string(),
            other => panic!("create failed: {other:?}"),
        };
        match host.handle(
            "terminal/wait_for_exit",
            &json!({ "terminalId": terminal_id }),
            &json!(2),
        ) {
            TerminalReply::Pending => {}
            other => panic!("running command must pend wait_for_exit: {other:?}"),
        }
        let exit = tokio::time::timeout(Duration::from_secs(5), host.exit_rx().recv())
            .await
            .expect("exit event")
            .expect("terminal exit");
        let replies = host.on_exit(exit.0, exit.1);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].0, json!(2));
        assert_eq!(replies[0].1["exitCode"], json!(0));
        let _ = host.handle(
            "terminal/release",
            &json!({ "terminalId": terminal_id }),
            &json!(3),
        );
    }

    #[test]
    fn rejects_relative_cwd() {
        let mut host = AcpTerminalHost::new(std::env::temp_dir());
        match host.handle(
            "terminal/create",
            &json!({
                "command": "echo",
                "cwd": "relative/path",
            }),
            &json!(1),
        ) {
            TerminalReply::Error { code, message } => {
                assert_eq!(code, -32602);
                assert!(message.contains("absolute"), "{message}");
            }
            other => panic!("expected relative cwd rejection, got {other:?}"),
        }
    }

    #[test]
    fn rejects_mismatched_session_id() {
        let mut host = AcpTerminalHost::new(std::env::temp_dir());
        host.set_session_id("sess-1");
        match host.handle(
            "terminal/create",
            &json!({
                "sessionId": "other",
                "command": "echo",
            }),
            &json!(1),
        ) {
            TerminalReply::Error { code, message } => {
                assert_eq!(code, -32602);
                assert!(message.contains("sessionId"), "{message}");
            }
            other => panic!("expected sessionId rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn release_answers_pending_wait_for_exit() {
        let cwd = std::env::temp_dir();
        let mut host = AcpTerminalHost::new(cwd.clone());
        let (command, args) = sleep_command();
        let created = host.handle(
            "terminal/create",
            &json!({
                "command": command,
                "args": args,
                "cwd": cwd.to_string_lossy(),
            }),
            &json!(1),
        );
        let terminal_id = match created {
            TerminalReply::Result(value) => value["terminalId"]
                .as_str()
                .expect("terminalId")
                .to_string(),
            other => panic!("create failed: {other:?}"),
        };
        match host.handle(
            "terminal/wait_for_exit",
            &json!({ "terminalId": terminal_id }),
            &json!(2),
        ) {
            TerminalReply::Pending => {}
            other => panic!("expected pending wait: {other:?}"),
        }
        match host.handle(
            "terminal/release",
            &json!({ "terminalId": terminal_id }),
            &json!(3),
        ) {
            TerminalReply::Result(_) => {}
            other => panic!("release failed: {other:?}"),
        }
        let aborted = host.take_aborted_waits();
        assert_eq!(aborted.len(), 1);
        assert_eq!(aborted[0].0, json!(2));
        assert_eq!(aborted[0].1, -32603);
    }

    fn echo_command(text: &str) -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            ("cmd", vec!["/C".to_string(), format!("echo {text}")])
        } else {
            ("printf", vec![format!("{text}\n")])
        }
    }

    fn sleep_command() -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            (
                "cmd",
                vec!["/C".to_string(), "ping -n 2 127.0.0.1 >NUL".to_string()],
            )
        } else {
            ("sleep", vec!["0.2".to_string()])
        }
    }
}

impl std::fmt::Debug for TerminalReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Result(value) => write!(f, "Result({value})"),
            Self::Pending => write!(f, "Pending"),
            Self::Error { code, message } => write!(f, "Error({code}, {message})"),
        }
    }
}
