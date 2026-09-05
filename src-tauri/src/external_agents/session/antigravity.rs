//! Official agy NDJSON protocol (verified with 1.1.26).
//! https://antigravity.google/docs/cli/headless/
//! One init per process, one result per turn. No control RPCs or native image blocks.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::chat::model::ModelUsage;
use crate::external_agents::session::live::{SessionCommand, CANCELLED_SESSION_LOST};
use crate::external_agents::spawn::{cli_command, fold_stderr, kill_agent_process_tree};
use crate::external_agents::types::UnifiedAgentEvent;
use crate::proc::NoConsoleWindow;

const INIT_TIMEOUT: Duration = Duration::from_secs(45);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_CHARS: usize = 8192;

#[derive(Default, Clone, Debug)]
struct Usage {
    input: u64,
    output: u64,
    thinking: u64,
    cache: u64,
    total: u64,
}

impl Usage {
    fn parse(v: &Value) -> Self {
        let n = |k| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Self {
            input: n("input_tokens"),
            output: n("output_tokens"),
            thinking: n("thinking_tokens"),
            cache: n("cache_read_tokens"),
            total: n("total_tokens"),
        }
    }

    fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.thinking = self.thinking.saturating_add(other.thinking);
        self.cache = self.cache.saturating_add(other.cache);
        self.total = self.total.saturating_add(other.total);
    }

    fn since(&self, previous: &Self) -> Self {
        Self {
            input: self.input.saturating_sub(previous.input),
            output: self.output.saturating_sub(previous.output),
            thinking: self.thinking.saturating_sub(previous.thinking),
            cache: self.cache.saturating_sub(previous.cache),
            total: self.total.saturating_sub(previous.total),
        }
    }

    fn model_usage(&self) -> ModelUsage {
        // agy 1.1.26: input=5969, output=554, cache=8132, total=6523.
        // Cache is disjoint from input (and excluded from native total), while thinking
        // is already inside output. Kivio total measures context including cached input.
        ModelUsage {
            input_tokens: Some(self.input),
            output_tokens: Some(self.output),
            total_tokens: Some(
                self.input
                    .saturating_add(self.output)
                    .saturating_add(self.cache),
            ),
            reasoning_tokens: Some(self.thinking),
            cached_input_tokens: Some(self.cache),
            cache_creation_input_tokens: None,
            context_window_tokens: None,
        }
    }
}

#[derive(Default)]
struct TurnStream {
    text: String,
    tools: HashSet<String>,
    finished_tools: HashSet<String>,
    step_usage: HashMap<u64, Usage>,
}

impl TurnStream {
    fn handle(&mut self, value: &Value, sink: &mut dyn FnMut(UnifiedAgentEvent)) {
        if value["event"] != "step_update" {
            return;
        }
        let step = &value["step_update"];
        let Some(index) = step["step_index"].as_u64() else {
            return;
        };
        if step["usage"].is_object() {
            self.step_usage.insert(index, Usage::parse(&step["usage"]));
        }
        match step["step_type"].as_str() {
            Some("agent_response") => {
                if let Some(delta) = step["text_delta"].as_str().filter(|s| !s.is_empty()) {
                    self.text.push_str(delta);
                    sink(UnifiedAgentEvent::TextDelta {
                        delta: delta.into(),
                    });
                }
            }
            Some("tool") => {
                let id = format!(
                    "agy-{}-{index}",
                    step["conversation_id"].as_str().unwrap_or("step")
                );
                let info = &step["tool_info"];
                let terminal = matches!(
                    step["state"].as_str(),
                    Some("DONE" | "ERROR" | "CANCELED" | "INTERRUPTED")
                );
                let name = info["name"]
                    .as_str()
                    .or_else(|| step["tool_name"].as_str())
                    .unwrap_or("tool");
                // Some ACTIVE transitions have only a name. Wait for parameters (or DONE)
                // rather than creating a permanently empty tool card.
                if (info.get("parameters").is_some() || terminal) && self.tools.insert(id.clone()) {
                    sink(UnifiedAgentEvent::ToolUse {
                        id: id.clone(),
                        name: name.into(),
                        input: info.get("parameters").cloned().unwrap_or_else(|| json!({})),
                    });
                }
                if terminal && self.finished_tools.insert(id.clone()) {
                    let error = info.get("error").filter(|v| !v.is_null());
                    let output = error.or_else(|| info.get("output"));
                    let content = output
                        .map(|v| {
                            v.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| v.to_string())
                        })
                        .unwrap_or_else(|| {
                            if step["state"] == "DONE" {
                                String::new()
                            } else {
                                format!(
                                    "Tool ended with {}",
                                    step["state"].as_str().unwrap_or("ERROR")
                                )
                            }
                        });
                    sink(UnifiedAgentEvent::ToolResult {
                        tool_use_id: id,
                        content,
                        is_error: error.is_some() || step["state"] != "DONE",
                    });
                }
            }
            _ => {} // Unknown future steps must not break the conversation.
        }
    }

    fn finish(
        &self,
        result: &Value,
        previous: &mut Option<Usage>,
        sink: &mut dyn FnMut(UnifiedAgentEvent),
    ) -> Result<(), String> {
        // result.response repeats the streamed text. Only use it when no text was streamed.
        if self.text.is_empty() {
            if let Some(text) = result["response"].as_str().filter(|s| !s.is_empty()) {
                sink(UnifiedAgentEvent::TextDelta { delta: text.into() });
            }
        }
        let cumulative = result
            .get("usage")
            .filter(|v| v.is_object())
            .map(Usage::parse);
        let usage = if !self.step_usage.is_empty() {
            // Step counters are per-step, including the FIRST turn after --conversation.
            // Subtracting zero from that first cumulative result would bill the old history again.
            let mut sum = Usage::default();
            for item in self.step_usage.values() {
                sum.add(item);
            }
            Some(sum)
        } else {
            cumulative
                .as_ref()
                .zip(previous.as_ref())
                .map(|(current, old)| current.since(old))
        };
        if let Some(usage) = usage {
            sink(UnifiedAgentEvent::Usage {
                usage: usage.model_usage(),
            });
        }
        if cumulative.is_some() {
            *previous = cumulative;
        }
        if let Some(denied) = result["denied_actions"]
            .as_array()
            .filter(|v| !v.is_empty())
        {
            let names: Vec<_> = denied
                .iter()
                .filter_map(|v| v["display_name"].as_str().or_else(|| v["action"].as_str()))
                .collect();
            let message = format!("Antigravity CLI 权限配置拒绝了操作：{}。请调整 CLI 的 permissions.allow 或聊天底栏权限模式。", names.join(", "));
            if self.text.trim().is_empty()
                && result["response"].as_str().unwrap_or("").trim().is_empty()
            {
                return Err(message);
            }
            sink(UnifiedAgentEvent::StatusNote { text: message });
        }
        match result["status"].as_str() {
            Some("SUCCESS") => Ok(()),
            Some("CANCELED" | "INTERRUPTED") => Err("cancelled".into()),
            status => Err(result["error"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "Antigravity turn ended with {}",
                        status.unwrap_or("missing status")
                    )
                })),
        }
    }
}

pub struct AntigravitySession {
    bin: PathBuf,
    cwd: PathBuf,
    args: Vec<String>,
    child: Child,
    stdin: ChildStdin,
    reader: Lines<BufReader<ChildStdout>>,
    stderr_task: tokio::task::JoinHandle<()>,
    stderr_rx: mpsc::Receiver<String>,
    stderr_tail: Arc<Mutex<String>>,
    native_id: String,
    previous_usage: Option<Usage>,
}

impl AntigravitySession {
    pub async fn connect(
        bin: &Path,
        args: &[String],
        cwd: &Path,
        resume: Option<&str>,
    ) -> Result<Self, String> {
        let mut command = cli_command(bin);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .no_console_window()
            .kill_on_drop(true);
        if let Some(id) = resume {
            command.args(["--conversation", id]);
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Start Antigravity CLI: {e}"))?;
        let stdin = child.stdin.take().ok_or("Antigravity stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Antigravity stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("Antigravity stderr unavailable")?;
        let tail = Arc::new(Mutex::new(String::new()));
        let stderr_tail = tail.clone();
        let (stderr_tx, stderr_rx) = mpsc::channel(32);
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                {
                    let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                    tail.push_str(&line);
                    tail.push('\n');
                    if tail.chars().count() > STDERR_CHARS {
                        *tail = tail
                            .chars()
                            .rev()
                            .take(STDERR_CHARS)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                    }
                }
                // Never let a full diagnostics channel block the child's stderr pipe.
                let _ = stderr_tx.try_send(line);
            }
        });
        let mut session = Self {
            bin: bin.to_path_buf(),
            cwd: cwd.to_path_buf(),
            args: args.to_vec(),
            child,
            stdin,
            reader: BufReader::new(stdout).lines(),
            stderr_task,
            stderr_rx,
            stderr_tail,
            native_id: String::new(),
            previous_usage: resume.is_none().then(Usage::default),
        };
        let init = timeout(INIT_TIMEOUT, async {
            loop {
                let line = session.reader.next_line().await.map_err(|e| e.to_string())?.ok_or("Antigravity exited before init")?;
                if line.trim().is_empty() { continue; }
                let value: Value = serde_json::from_str(&line).map_err(|e| format!("Invalid Antigravity init: {e}"))?;
                if value["event"] == "init" {
                    let id = value["conversation_id"].as_str().filter(|id| !id.is_empty()).ok_or("Antigravity init missing conversation_id")?;
                    if resume.is_some_and(|expected| expected != id) { return Err("Antigravity resumed a different conversation; refusing to replace the binding".into()); }
                    return Ok::<String, String>(id.into());
                }
                if value["event"] == "result" { return Err(value["result"]["error"].as_str().unwrap_or("Antigravity failed before init").into()); }
            }
        }).await.unwrap_or_else(|_| Err("Antigravity init timed out; check agy version and sign-in".into()));
        match init {
            Ok(id) => {
                session.native_id = id;
                Ok(session)
            }
            Err(error) => {
                session.shutdown().await;
                Err(fold_stderr(
                    error,
                    &session
                        .stderr_tail
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()),
                ))
            }
        }
    }

    pub fn session_id(&self) -> &str {
        &self.native_id
    }
    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
    }

    async fn shutdown(&mut self) {
        kill_agent_process_tree(&mut self.child);
        let _ = timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await;
        if timeout(SHUTDOWN_TIMEOUT, &mut self.stderr_task)
            .await
            .is_err()
        {
            self.stderr_task.abort();
        }
    }

    async fn run_turn(
        &mut self,
        prompt: &str,
        events: &mpsc::Sender<UnifiedAgentEvent>,
        commands: &mut mpsc::Receiver<SessionCommand>,
    ) -> Result<(), String> {
        use crate::external_agents::antigravity_slash::{
            command_name, is_report, is_terminal_only, report,
        };
        if let Some(name) = command_name(prompt) {
            let has_args = prompt.trim().split_whitespace().count() > 1;
            if is_terminal_only(name) || (is_report(name) && has_args) {
                let message = if is_report(name) {
                    format!("/{name} 当前适配为只读查询，请不带参数执行。模型、推理强度和权限可在会话菜单中修改；其他配置请在 agy 原生终端中修改。")
                } else {
                    format!("/{name} 需要 Antigravity 交互式终端，当前流式协议没有对应接口。输入 /help 可查看已适配命令。")
                };
                let _ = events
                    .send(UnifiedAgentEvent::TextDelta { delta: message })
                    .await;
                return Ok(());
            }
            if is_report(name) {
                if name == "help" {
                    let _ = events
                        .send(UnifiedAgentEvent::TextDelta {
                            delta: crate::external_agents::antigravity_slash::help_text(),
                        })
                        .await;
                    return Ok(());
                }
                // A failed report must not close the native conversation. Cancel/Close
                // still cancels the subprocess future (kill_on_drop) and the session.
                let pending = report(&self.bin, &self.cwd, &self.args, prompt);
                tokio::pin!(pending);
                let result = loop {
                    tokio::select! {
                        result = &mut pending => break result,
                        command = commands.recv() => match command {
                            Some(SessionCommand::Steer { accepted, .. }) => { let _ = accepted.send(false); }
                            Some(SessionCommand::StopTask { .. }) => {}
                            _ => return Err(CANCELLED_SESSION_LOST.into()),
                        }
                    }
                };
                let text = match result {
                    Ok(text) if !text.is_empty() => text,
                    Ok(_) => "命令已完成，没有返回内容。".into(),
                    Err(error) => format!("命令执行失败：{error}"),
                };
                let _ = events
                    .send(UnifiedAgentEvent::TextDelta { delta: text })
                    .await;
                return Ok(());
            }
            // Skill slash commands are expanded by agy, preserving native arguments
            // and the existing conversation. Do not prepend Kivio instructions.
        }
        let line = format!("{}\n", json!({"event":"user","message":{"content":prompt}}));
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        self.stdin.flush().await.map_err(|e| e.to_string())?;
        let mut stream = TurnStream::default();
        let mut stderr_open = true;
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(SessionCommand::Cancel | SessionCommand::Close) | None => return Err(CANCELLED_SESSION_LOST.into()),
                    Some(SessionCommand::Steer { accepted, .. }) => { let _ = accepted.send(false); }
                    Some(SessionCommand::RunTurn { done, .. }) => { let _ = done.send(Err("Antigravity session already busy".into())); }
                    Some(SessionCommand::StopTask { .. }) => {}
                },
                note = self.stderr_rx.recv(), if stderr_open => match note {
                    Some(text) if !text.trim().is_empty() => { let _ = events.send(UnifiedAgentEvent::StatusNote { text }).await; }
                    None => stderr_open = false,
                    _ => {}
                },
                line = self.reader.next_line() => {
                    let line = line.map_err(|e| e.to_string())?.ok_or("Antigravity exited before result")?;
                    if line.trim().is_empty() { continue; }
                    let value: Value = serde_json::from_str(&line).map_err(|e| format!("Invalid Antigravity stream: {e}"))?;
                    let payload = match value["event"].as_str() {
                        Some("result") => &value["result"],
                        Some("step_update") => &value["step_update"],
                        _ => &value,
                    };
                    if payload["conversation_id"].as_str().is_some_and(|id| !id.is_empty() && id != self.native_id) {
                        return Err("Antigravity event belongs to a different conversation".into());
                    }
                    let mut emitted = Vec::new();
                    stream.handle(&value, &mut |event| emitted.push(event));
                    let result = (value["event"] == "result").then(|| stream.finish(&value["result"], &mut self.previous_usage, &mut |event| emitted.push(event)));
                    for event in emitted { events.send(event).await.map_err(|_| "closed".to_string())?; }
                    if let Some(result) = result { return result; }
                }
            }
        }
    }
}

impl Drop for AntigravitySession {
    fn drop(&mut self) {
        kill_agent_process_tree(&mut self.child);
        self.stderr_task.abort();
    }
}

pub fn spawn_antigravity_session_actor(
    mut session: AntigravitySession,
) -> mpsc::Sender<SessionCommand> {
    let (tx, mut rx) = mpsc::channel(8);
    tokio::spawn(async move {
        loop {
            let command = tokio::select! {
                command = rx.recv() => match command { Some(command) => command, None => break },
                _ = session.child.wait() => break,
            };
            match command {
                SessionCommand::RunTurn {
                    prompt,
                    images,
                    events,
                    done,
                    ..
                } => {
                    let result = if images.is_empty() {
                        session.run_turn(&prompt, &events, &mut rx).await
                    } else {
                        Err("Antigravity stream accepts text only; images must be supplied as file paths".into())
                    };
                    if let Err(error) = result {
                        session.shutdown().await;
                        let error = if error == CANCELLED_SESSION_LOST
                            || error == "cancelled"
                            || error == "closed"
                        {
                            error
                        } else {
                            fold_stderr(
                                error,
                                &session
                                    .stderr_tail
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner()),
                            )
                        };
                        let _ = done.send(Err(error));
                        return;
                    }
                    let _ = done.send(Ok(()));
                }
                SessionCommand::Close | SessionCommand::Cancel => break,
                SessionCommand::Steer { accepted, .. } => {
                    let _ = accepted.send(false);
                }
                SessionCommand::StopTask { .. } => {}
            }
        }
        session.shutdown().await;
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ask(control: &mpsc::Sender<SessionCommand>, prompt: &str) -> Vec<UnifiedAgentEvent> {
        let (events, mut rx) = mpsc::channel(256);
        let (done, completion) = tokio::sync::oneshot::channel();
        control
            .send(SessionCommand::RunTurn {
                prompt: prompt.into(),
                model: None,
                reasoning: None,
                images: vec![],
                extra_writable_roots: vec![],
                events,
                done,
                approvals: None,
            })
            .await
            .unwrap();
        timeout(Duration::from_secs(90), completion)
            .await
            .expect("turn timeout")
            .unwrap()
            .unwrap();
        let mut events = vec![];
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    fn text(events: &[UnifiedAgentEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                UnifiedAgentEvent::TextDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    #[ignore = "requires signed-in agy; runs model requests in an isolated temp workspace"]
    async fn live_antigravity_multiturn_tools_cancel_and_resume() {
        use crate::external_agents::defs::antigravity::ANTIGRAVITY_AGENT_DEF;
        use crate::external_agents::types::{RuntimeBuildOptions, RuntimeContext};
        let bin = crate::external_agents::spawn::resolve_binary(&ANTIGRAVITY_AGENT_DEF)
            .await
            .expect("agy binary");
        let cwd = std::env::temp_dir().join(format!("kivio-agy-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).unwrap();
        let fixture = cwd.join("probe.txt");
        std::fs::write(&fixture, "AGY_TOOL_PROBE_72941").unwrap();
        let args = (ANTIGRAVITY_AGENT_DEF.build_args)(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: None,
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: Some("plan".into()),
            },
            None,
        );
        let session = AntigravitySession::connect(&bin, &args, &cwd, None)
            .await
            .unwrap();
        let id = session.session_id().to_string();
        let pid = session.child_pid();
        let control = spawn_antigravity_session_actor(session);
        assert!(text(
            &ask(
                &control,
                "Do not use tools. Remember the word kumquat. Reply only OK."
            )
            .await
        )
        .contains("OK"));
        let catalog = crate::external_agents::antigravity_slash::builtin_commands();
        assert!(catalog.iter().any(|command| command.name == "usage"));
        let report = ask(&control, "/help").await;
        assert!(text(&report).contains("/usage"));
        assert!(!report
            .iter()
            .any(|event| matches!(event, UnifiedAgentEvent::Usage { .. })));
        assert!(text(&ask(&control, "/model low").await).contains("只读"));
        assert!(text(&ask(&control, "/clear").await).contains("交互式终端"));
        let skills = text(&ask(&control, "/skills").await);
        if skills
            .lines()
            .any(|line| line.starts_with("antigravity-guide\t"))
        {
            assert!(text(
                &ask(
                    &control,
                    "/antigravity-guide Reply with exactly AGY_SLASH_OK. Do not use tools."
                )
                .await
            )
            .contains("AGY_SLASH_OK"));
        }
        assert!(text(
            &ask(
                &control,
                "Do not use tools. What word did I ask you to remember? Reply only that word."
            )
            .await
        )
        .to_lowercase()
        .contains("kumquat"));
        let events = ask(&control, &format!("For this turn use view_file to read exactly this absolute file path: {}. Return its contents. Do not search other directories, edit files or execute shell commands.", fixture.display())).await;
        assert!(
            text(&events).contains("AGY_TOOL_PROBE_72941"),
            "file response: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, UnifiedAgentEvent::ToolUse { .. })),
            "tool start missing: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, UnifiedAgentEvent::ToolResult { .. })),
            "tool end missing: {events:?}"
        );
        // Cancel an active turn. Closing stdin alone would let it keep running.
        let (events, _rx) = mpsc::channel(256);
        let (done, completion) = tokio::sync::oneshot::channel();
        control
            .send(SessionCommand::RunTurn {
                prompt: "Do not use tools. Explain the first 100 prime numbers in detail.".into(),
                model: None,
                reasoning: None,
                images: vec![],
                extra_writable_roots: vec![],
                events,
                done,
                approvals: None,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        control.send(SessionCommand::Cancel).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(20), completion)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err(),
            CANCELLED_SESSION_LOST
        );
        timeout(Duration::from_secs(5), control.closed())
            .await
            .unwrap();
        let session = AntigravitySession::connect(&bin, &args, &cwd, Some(&id))
            .await
            .unwrap();
        assert_eq!(session.session_id(), id);
        assert_ne!(session.child_pid(), pid);
        let control = spawn_antigravity_session_actor(session);
        let events = ask(
            &control,
            "Do not use tools. What word did I ask you to remember earlier? Reply only that word.",
        )
        .await;
        assert!(text(&events).to_lowercase().contains("kumquat"));
        assert!(events
            .iter()
            .any(|e| matches!(e, UnifiedAgentEvent::Usage { .. })));
        control.send(SessionCommand::Close).await.unwrap();
        timeout(Duration::from_secs(20), control.closed())
            .await
            .unwrap();
        // Only the exact fixture created above is removed; never recursively delete the workspace.
        std::fs::remove_file(fixture).unwrap();
        let _ = std::fs::remove_dir(cwd);
    }

    #[test]
    fn streams_deltas_once_and_uses_per_step_usage_after_resume() {
        let mut stream = TurnStream::default();
        let mut events = vec![];
        for (state, delta) in [("ACTIVE", "kumquat"), ("DONE", "\n")] {
            stream.handle(&json!({"event":"step_update","step_update":{"step_index":3,"step_type":"agent_response","state":state,"text_delta":delta,"usage":{"input_tokens":100,"output_tokens":7,"thinking_tokens":5,"cache_read_tokens":80,"total_tokens":107}}}), &mut |e| events.push(e));
        }
        stream.finish(&json!({"status":"SUCCESS","response":"kumquat\n","usage":{"input_tokens":900,"output_tokens":80,"total_tokens":980}}), &mut None, &mut |e| events.push(e)).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, UnifiedAgentEvent::TextDelta { .. }))
                .count(),
            2
        );
        let UnifiedAgentEvent::Usage { usage } = events.last().unwrap() else {
            panic!("usage missing")
        };
        assert_eq!(usage.total_tokens, Some(187));
        assert_eq!(usage.reasoning_tokens, Some(5));
        assert_eq!(usage.cached_input_tokens, Some(80));
    }

    #[test]
    fn tools_pair_active_and_done_and_preserve_errors() {
        let mut stream = TurnStream::default();
        let mut events = vec![];
        for state in ["ACTIVE", "ERROR", "ERROR"] {
            stream.handle(&json!({"event":"step_update","step_update":{"conversation_id":"native","step_index":4,"step_type":"tool","state":state,"tool_name":"run_command","tool_info":{"parameters":{"CommandLine":"echo hi"},"error":{"type":"denied","message":"permission denied"}}}}), &mut |e| events.push(e));
        }
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], UnifiedAgentEvent::ToolUse { id, input, .. } if id == "agy-native-4" && input["CommandLine"] == "echo hi")
        );
        assert!(
            matches!(&events[1], UnifiedAgentEvent::ToolResult { is_error: true, content, .. } if content.contains("permission denied"))
        );
    }

    #[test]
    fn cumulative_fallback_is_differenced_and_failed_status_is_not_success() {
        let stream = TurnStream::default();
        let mut previous = Some(Usage {
            input: 100,
            output: 10,
            total: 110,
            ..Default::default()
        });
        let mut events = vec![];
        assert!(stream.finish(&json!({"status":"ERROR","error":"invalid model","usage":{"input_tokens":120,"output_tokens":13,"total_tokens":133}}), &mut previous, &mut |e| events.push(e)).unwrap_err().contains("invalid model"));
        assert!(
            matches!(&events[0], UnifiedAgentEvent::Usage { usage } if usage.total_tokens == Some(23))
        );
        assert_eq!(
            stream
                .finish(&json!({"status":"INTERRUPTED"}), &mut previous, &mut |_| {})
                .unwrap_err(),
            "cancelled"
        );
        assert!(stream
            .finish(&json!({"status":"WAITING"}), &mut previous, &mut |_| {})
            .is_err());
    }

    #[test]
    fn soft_denial_with_success_status_never_becomes_an_empty_success() {
        let stream = TurnStream::default();
        let result = json!({"status":"SUCCESS","response":"","denied_actions":[{"action":"read_file","display_name":"ListDir"}]});
        assert!(stream
            .finish(&result, &mut None, &mut |_| {})
            .unwrap_err()
            .contains("ListDir"));
    }

    #[test]
    fn final_only_response_is_preserved_and_unknown_steps_are_ignored() {
        let mut stream = TurnStream::default();
        let mut events = vec![];
        stream.handle(&json!({"event":"step_update","step_update":{"step_index":5,"step_type":"future_step"}}), &mut |e| events.push(e));
        stream
            .finish(
                &json!({"status":"SUCCESS","response":"answer"}),
                &mut None,
                &mut |e| events.push(e),
            )
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(text(&events), "answer");
    }
}
