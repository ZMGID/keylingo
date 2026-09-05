//! Awaited workflow hooks. Legacy `hooks.rs` remains an observational channel.
//! Input follows Claude/Codex event envelopes; decisions never bypass host approval.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "SubagentStart",
    "PreToolUse",
    "PostToolUse",
];
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handler {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub command: String,
    /// When present, execute the command directly without a shell.
    pub args: Option<Vec<String>>,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub shell: Option<String>,
}
fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Clone)]
pub struct Hook {
    pub owner: String,
    pub foreign_tool_names: bool,
    pub event: String,
    pub matcher: Option<regex::Regex>,
    pub handler: Handler,
    pub root: PathBuf,
    pub data: PathBuf,
}

#[derive(Debug, Default, Clone)]
pub struct Outcome {
    pub context: Vec<String>,
    pub updated_input: Option<Value>,
    pub denied: Option<String>,
    pub ask: Option<String>,
}

/// Strict parsing: unsupported behavior is reported, never silently activated.
pub fn parse(
    value: &Value,
    owner: &str,
    root: &std::path::Path,
    data: &std::path::Path,
) -> Result<Vec<Hook>, String> {
    let groups = value
        .get("hooks")
        .unwrap_or(value)
        .as_object()
        .ok_or("hooks must be an object")?;
    let mut hooks = Vec::new();
    for (event, entries) in groups {
        if !EVENTS.contains(&event.as_str()) {
            return Err(format!("Unsupported hook event: {event}"));
        }
        for entry in entries
            .as_array()
            .ok_or("event handlers must be an array")?
        {
            let matcher = match entry
                .get("matcher")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && *s != "*")
            {
                Some(s) => {
                    let pattern = if s
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|')
                    {
                        format!("^(?:{s})$")
                    } else {
                        s.into()
                    };
                    Some(regex::Regex::new(&pattern).map_err(|e| format!("Invalid matcher: {e}"))?)
                }
                None => None,
            };
            for raw in entry
                .get("hooks")
                .and_then(Value::as_array)
                .ok_or("matcher needs hooks array")?
            {
                if raw.get("async").and_then(Value::as_bool) == Some(true) {
                    return Err(
                        "Async workflow hooks are unsupported; use notification hooks".into(),
                    );
                }
                let handler: Handler =
                    serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
                if handler.kind != "command" {
                    return Err(format!("Unsupported workflow hook type: {}", handler.kind));
                }
                if handler.command.trim().is_empty()
                    || handler.timeout == 0
                    || handler.timeout > 600
                {
                    return Err(
                        "Hook needs a command and a timeout between 1 and 600 seconds".into(),
                    );
                }
                if handler
                    .shell
                    .as_deref()
                    .is_some_and(|s| s != "native" && s != "bash")
                {
                    return Err("Hook shell must be native or bash".into());
                }
                hooks.push(Hook {
                    owner: owner.into(),
                    foreign_tool_names: uuid::Uuid::parse_str(owner).is_ok(),
                    event: event.clone(),
                    matcher: matcher.clone(),
                    handler,
                    root: root.into(),
                    data: data.into(),
                });
            }
        }
    }
    if hooks.len() > 128 {
        return Err("Too many hooks (maximum 128)".into());
    }
    Ok(hooks)
}

pub fn parse_output(event: &str, stdout: &str, code: i32, stderr: &str) -> Result<Outcome, String> {
    let mut out = Outcome::default();
    if code == 2 && matches!(event, "PreToolUse" | "UserPromptSubmit") {
        out.denied = Some(if stderr.trim().is_empty() {
            "Blocked by workflow hook".into()
        } else {
            stderr.trim().into()
        });
        return Ok(out);
    }
    if code != 0 {
        return Err(format!("Hook exited {code}: {}", stderr.trim()));
    }
    let text = stdout.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(out);
    }
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(e) if text.starts_with('{') || text.starts_with('[') => {
            return Err(format!("Invalid hook JSON: {e}"))
        }
        Err(_) if matches!(event, "SessionStart" | "UserPromptSubmit") => {
            out.context.push(text.into());
            return Ok(out);
        }
        Err(_) => return Ok(out), // stdout of tool hooks is diagnostic text, not model instructions.
    };
    let specific = value.get("hookSpecificOutput").unwrap_or(&Value::Null);
    if let Some(declared) = specific.get("hookEventName").and_then(Value::as_str) {
        if declared != event {
            return Err(format!(
                "Hook output event {declared} does not match {event}"
            ));
        }
    }
    if let Some(context) = specific.get("additionalContext").and_then(Value::as_str) {
        out.context.push(context.into());
    }
    if value.get("continue").and_then(Value::as_bool) == Some(false) {
        out.denied = Some(
            value
                .get("stopReason")
                .and_then(Value::as_str)
                .unwrap_or("Stopped by workflow hook")
                .into(),
        );
    }
    if event == "PreToolUse" {
        match specific.get("permissionDecision").and_then(Value::as_str) {
            Some("deny") => {
                out.denied = Some(
                    specific
                        .get("permissionDecisionReason")
                        .and_then(Value::as_str)
                        .unwrap_or(
                            "Workflow hook requires approval; retry after reviewing the hook",
                        )
                        .into(),
                )
            }
            Some("ask") => {
                out.ask = Some(
                    specific
                        .get("permissionDecisionReason")
                        .and_then(Value::as_str)
                        .unwrap_or("Workflow hook requests approval")
                        .into(),
                );
            }
            Some("allow") | None => {}
            Some(other) => return Err(format!("Invalid hook permission decision: {other}")),
        }
        if let Some(input) = specific.get("updatedInput") {
            if !input.is_object() {
                return Err("updatedInput must be an object".into());
            }
            out.updated_input = Some(input.clone());
        }
    }
    if event == "UserPromptSubmit" && value.get("decision").and_then(Value::as_str) == Some("block")
    {
        out.denied = Some(
            value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("Blocked by workflow hook")
                .into(),
        );
    }
    Ok(out)
}

struct ProcessGuard(u32);
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        crate::native_tools::kill_process_group(self.0);
    }
}
async fn read_bounded(mut pipe: impl AsyncRead + Unpin) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    (&mut pipe)
        .take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut data)
        .await
        .map_err(|e| e.to_string())?;
    if data.len() > OUTPUT_LIMIT {
        return Err("Hook output exceeds 64 KiB".into());
    }
    Ok(data)
}

fn environment(hook: &Hook, cwd: &std::path::Path) -> BTreeMap<String, String> {
    [
        ("PLUGIN_ROOT", hook.root.as_path()),
        ("CLAUDE_PLUGIN_ROOT", hook.root.as_path()),
        ("PLUGIN_DATA", hook.data.as_path()),
        ("CLAUDE_PLUGIN_DATA", hook.data.as_path()),
        ("CLAUDE_PROJECT_DIR", cwd),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.to_string_lossy().into_owned()))
    .collect()
}
pub fn expand(value: &str, env: &BTreeMap<String, String>) -> String {
    let mut out = value.to_owned();
    for (key, value) in env {
        out = out.replace(&format!("${{{key}}}"), value);
    }
    out
}

async fn execute(hook: &Hook, cwd: &std::path::Path, input: &Value) -> Result<Outcome, String> {
    std::fs::create_dir_all(&hook.data).map_err(|e| e.to_string())?;
    let env = environment(hook, cwd);
    let mut command = if let Some(args) = &hook.handler.args {
        let mut cmd = tokio::process::Command::new(expand(&hook.handler.command, &env));
        cmd.args(args.iter().map(|a| expand(a, &env)));
        cmd
    } else {
        let mut script = hook.handler.command.clone();
        // Translate variable syntax, not values: paths must not become shell source.
        let default_shell = crate::native_tools::build_shell_command("");
        let program = default_shell
            .as_std()
            .get_program()
            .to_string_lossy()
            .to_ascii_lowercase();
        if cfg!(windows)
            && hook.handler.shell.as_deref() != Some("bash")
            && (program.contains("powershell") || program.contains("pwsh"))
        {
            for key in env.keys() {
                script = script.replace(&format!("${{{key}}}"), &format!("$env:{key}"));
            }
        }
        if hook.handler.shell.as_deref() == Some("bash") {
            let mut cmd = tokio::process::Command::new("bash");
            cmd.args(["-c", &script]);
            cmd
        } else {
            crate::native_tools::build_shell_command(&script)
        }
    };
    command
        .current_dir(cwd)
        .envs(env)
        .env("KIVIO_HOOK_EVENT", &hook.event)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        use crate::proc::NoConsoleWindow;
        command.no_console_window();
    }
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start hook: {e}"))?;
    let _guard = ProcessGuard(child.id().ok_or("Hook process has no id")?);
    let mut stdin = child.stdin.take().ok_or("Missing hook stdin")?;
    let stdout = child.stdout.take().ok_or("Missing hook stdout")?;
    let stderr = child.stderr.take().ok_or("Missing hook stderr")?;
    let body = format!("{input}\n");
    if body.len() > 1024 * 1024 {
        return Err("Hook input exceeds 1 MiB".into());
    }
    let result = tokio::time::timeout(Duration::from_secs(hook.handler.timeout), async {
        let write = async move {
            if let Err(error) = stdin.write_all(body.as_bytes()).await {
                if error.kind() != std::io::ErrorKind::BrokenPipe {
                    return Err(error.to_string());
                }
            }
            let closed = stdin.shutdown().await;
            drop(stdin); // ChildStdin shutdown alone does not close the pipe on Windows.
            match closed {
                Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                    Err(error.to_string())
                }
                _ => Ok(()),
            }
        };
        let wait = async { child.wait().await.map_err(|e| e.to_string()) };
        let (_, stdout, stderr, status) =
            tokio::try_join!(write, read_bounded(stdout), read_bounded(stderr), wait)?;
        parse_output(
            &hook.event,
            &String::from_utf8_lossy(&stdout),
            status.code().unwrap_or(-1),
            &String::from_utf8_lossy(&stderr),
        )
    })
    .await
    .map_err(|_| format!("Hook timed out after {}s", hook.handler.timeout))?;
    result
}

#[derive(Debug, Clone, Default)]
pub struct Runtime {
    pub hooks: Vec<Hook>,
    pub cwd: PathBuf,
    pub agent_type: String,
    pub prompt: Option<String>,
}
impl Runtime {
    pub fn input(&self, event: &str, session_id: &str) -> Value {
        json!({ "schema_version": 1, "hook_event_name": event, "session_id": session_id, "cwd": self.cwd, "agent_type": self.agent_type })
    }
    pub async fn run(&self, event: &str, mut input: Value) -> Result<Outcome, String> {
        let mut result = Outcome::default();
        for hook in self.hooks.iter().filter(|h| h.event == event) {
            let foreign = hook.foreign_tool_names;
            if uuid::Uuid::parse_str(&hook.owner).is_ok()
                && !crate::plugins::packages::owner_enabled(&hook.owner)
            {
                continue;
            }
            let translated = if foreign {
                compatible_input(&input)
            } else {
                input.clone()
            };
            let key = translated
                .get("tool_name")
                .or_else(|| translated.get("source"))
                .or_else(|| translated.get("agent_type"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if hook
                .matcher
                .as_ref()
                .is_some_and(|re| !re.is_match(key) && !(key == "Agent" && re.is_match("Task")))
            {
                continue;
            }
            let mut output = execute(hook, &self.cwd, &translated)
                .await
                .map_err(|e| format!("{} / {event}: {e}", hook.owner))?;
            if foreign {
                if let Some(updated) = output.updated_input.take() {
                    output.updated_input = Some(native_input(&input, updated));
                }
            }
            if let Some(updated) = &output.updated_input {
                input["tool_input"] = updated.clone();
                result.updated_input = Some(updated.clone());
            }
            result.context.extend(output.context);
            if output.ask.is_some() {
                result.ask = output.ask;
            }
            if result.context.iter().map(String::len).sum::<usize>() > OUTPUT_LIMIT {
                return Err("Combined hook context exceeds 64 KiB".into());
            }
            if output.denied.is_some() {
                result.denied = output.denied;
                break;
            }
        }
        Ok(result)
    }
}

fn compatible_input(input: &Value) -> Value {
    let mut out = input.clone();
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let alias = match tool {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "glob" => "Glob",
        "grep" => "Grep",
        "agent" => "Agent",
        _ => tool,
    };
    if !tool.is_empty() {
        out["tool_name"] = json!(alias);
        out["kivio_tool_name"] = json!(tool);
    }
    if matches!(tool, "read" | "write" | "edit") {
        if let Some(fields) = out.get_mut("tool_input").and_then(Value::as_object_mut) {
            if let Some(path) = fields.remove("path") {
                fields.insert("file_path".into(), path);
            }
        }
    }
    for path in ["/agent_type", "/tool_input/subagent_type"] {
        if let Some(value) = out.pointer_mut(path) {
            if let Some((_, name)) = value.as_str().and_then(|s| s.split_once(':')) {
                *value = json!(name);
            }
        }
    }
    out
}
fn native_input(original: &Value, mut updated: Value) -> Value {
    if matches!(
        original["tool_name"].as_str(),
        Some("read" | "write" | "edit")
    ) {
        if let Some(fields) = updated.as_object_mut() {
            if let Some(path) = fields.remove("file_path") {
                fields.insert("path".into(), path);
            }
        }
    }
    if let Some(name) = original
        .pointer("/tool_input/subagent_type")
        .and_then(Value::as_str)
    {
        if name
            .split_once(':')
            .is_some_and(|(_, short)| updated["subagent_type"].as_str() == Some(short))
        {
            updated["subagent_type"] = json!(name);
        }
    }
    updated
}

type SessionCell = std::sync::Arc<tokio::sync::Mutex<Option<Vec<String>>>>;
fn session_cell(runtime: &Runtime, session: &str) -> SessionCell {
    use std::hash::{Hash, Hasher};
    static CACHE: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, SessionCell>>> =
        std::sync::OnceLock::new();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}:{:?}", runtime.cwd, runtime.hooks).hash(&mut hash);
    let key = format!("{session}:{}", hash.finish());
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 256 && !cache.contains_key(&key) {
        if let Some(oldest) = cache.keys().next().cloned() {
            cache.remove(&oldest);
        }
    }
    cache.entry(key).or_default().clone()
}

/// Cancellation covers stdin, process output and all handlers, not only tool execution.
pub async fn run_for_host(
    host: &dyn crate::chat::agent::AgentHost,
    event: &str,
    input: Value,
    session: &str,
    generation: u64,
) -> Result<Outcome, String> {
    let Some(runtime) = host.workflow_hooks() else {
        return Ok(Outcome::default());
    };
    if !host.is_generation_active(session, generation) {
        return Err("cancelled".into());
    }
    tokio::select! {
        result = runtime.run(event, input) => result,
        _ = host.wait_for_generation_inactive(session, generation) => Err("cancelled".into()),
    }
}

pub async fn start_context(
    host: &dyn crate::chat::agent::AgentHost,
    session: &str,
    generation: u64,
    resumed: bool,
    subagent: bool,
) -> Result<Vec<String>, String> {
    let Some(runtime) = host.workflow_hooks() else {
        return Ok(vec![]);
    };
    if runtime.hooks.is_empty() {
        return Ok(vec![]);
    }
    let cell = session_cell(runtime, session);
    let mut cached = tokio::select! {
        guard = cell.lock() => guard,
        _ = host.wait_for_generation_inactive(session, generation) => return Err("cancelled".into()),
    };
    if let Some(context) = &*cached {
        return Ok(context.clone());
    }
    let event = if subagent {
        "SubagentStart"
    } else {
        "SessionStart"
    };
    let mut input = runtime.input(event, session);
    if !subagent {
        input["source"] = json!(if resumed { "resume" } else { "startup" });
    }
    let out = run_for_host(host, event, input, session, generation).await?;
    if let Some(reason) = out.denied {
        return Err(reason);
    }
    *cached = Some(out.context.clone());
    Ok(out.context)
}

pub async fn remember_context(runtime: &Runtime, session: &str, context: &[String]) {
    if context.is_empty() {
        return;
    }
    let cell = session_cell(runtime, session);
    let mut cached = cell.lock().await;
    let values = cached.get_or_insert_with(Vec::new);
    for text in context {
        if values.last() != Some(text) {
            values.push(text.clone());
        }
    }
    while values.iter().map(String::len).sum::<usize>() > OUTPUT_LIMIT {
        values.remove(0);
    }
}

pub async fn compact_context(
    host: &dyn crate::chat::agent::AgentHost,
    session: &str,
    generation: u64,
) -> Result<Vec<String>, String> {
    let Some(runtime) = host.workflow_hooks().filter(|r| !r.hooks.is_empty()) else {
        return Ok(vec![]);
    };
    let mut input = runtime.input("SessionStart", session);
    input["source"] = json!("compact");
    let outcome = run_for_host(host, "SessionStart", input, session, generation).await?;
    if let Some(reason) = outcome.denied {
        return Err(reason);
    }
    remember_context(runtime, session, &outcome.context).await;
    Ok(outcome.context)
}

pub fn inject_context(messages: &mut Vec<Value>, context: &[String]) {
    if context.is_empty() {
        return;
    }
    // Keep it in the system prefix so every provider and compaction sees the same context.
    let end = messages
        .iter()
        .position(|m| m["role"] != "system")
        .unwrap_or(messages.len());
    messages.insert(end, json!({"role":"system", "content":format!("[Workflow hook context]\n{}", context.join("\n\n"))}));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatibility_translation_preserves_native_schema_and_agent_namespace() {
        let original = json!({"tool_name":"write","tool_input":{"path":"old","content":"x"}});
        let translated = compatible_input(&original);
        assert_eq!(translated["tool_name"], "Write");
        assert_eq!(translated["tool_input"]["file_path"], "old");
        let updated = native_input(&original, json!({"file_path":"new","content":"y"}));
        assert_eq!(updated, json!({"path":"new","content":"y"}));
        let agent = json!({"tool_name":"agent","tool_input":{"subagent_type":"example:review","prompt":"task"}});
        let input = compatible_input(&agent);
        assert_eq!(input["tool_input"]["subagent_type"], "review");
        assert_eq!(
            native_input(&agent, input["tool_input"].clone())["subagent_type"],
            "example:review"
        );
    }

    fn node_hook(dir: &std::path::Path, script: &str, timeout: u64) -> Runtime {
        let hooks = parse(&json!({"SessionStart":[{"hooks":[{"type":"command","command":"node","args":["-e",script],"timeout":timeout}]}]}), "fixture", dir, dir).unwrap();
        Runtime {
            hooks,
            cwd: dir.into(),
            ..Default::default()
        }
    }
    #[tokio::test]
    async fn dropping_a_hook_future_terminates_its_process() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = node_hook(dir.path(), "const fs=require('fs');fs.writeFileSync('started','1');setTimeout(()=>fs.writeFileSync('finished','1'),1000);process.stdin.resume()", 5);
        let task = tokio::spawn(async move {
            runtime
                .run("SessionStart", runtime.input("SessionStart", "s"))
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("started").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("hook actually started");
        task.abort();
        let _ = task.await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(
            !dir.path().join("finished").exists(),
            "cancelled hook must not keep running"
        );
    }
    #[tokio::test]
    async fn direct_argv_delivers_utf8_input_and_plugin_environment() {
        let dir = tempfile::Builder::new()
            .prefix("hook 中文 spaces ")
            .tempdir()
            .unwrap();
        let runtime = node_hook(dir.path(), "let s='';process.stdin.setEncoding('utf8');process.stdin.on('data',x=>s+=x);process.stdin.on('end',()=>process.stdout.write(JSON.stringify({hookSpecificOutput:{hookEventName:'SessionStart',additionalContext:JSON.parse(s).session_id+'|'+process.env.PLUGIN_ROOT}})))", 5);
        let output = runtime
            .run(
                "SessionStart",
                runtime.input("SessionStart", "测试-session"),
            )
            .await
            .unwrap();
        assert_eq!(
            output.context,
            [format!("测试-session|{}", dir.path().display())]
        );
    }
    #[tokio::test]
    async fn timeout_covers_a_script_that_never_reads_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = node_hook(dir.path(), "setInterval(()=>{},1000)", 1);
        let mut input = runtime.input("SessionStart", "s");
        input["prompt"] = json!("x".repeat(200_000));
        let started = std::time::Instant::now();
        assert!(runtime
            .run("SessionStart", input)
            .await
            .unwrap_err()
            .contains("timed out"));
        assert!(started.elapsed().as_secs() < 5);
    }
    #[tokio::test]
    async fn oversized_output_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = node_hook(
            dir.path(),
            "process.stdin.resume();process.stdout.write('x'.repeat(70000))",
            5,
        );
        assert!(runtime
            .run("SessionStart", runtime.input("SessionStart", "s"))
            .await
            .unwrap_err()
            .contains("64 KiB"));
    }
    #[test]
    fn event_output_is_not_interchangeable() {
        assert!(parse_output(
            "PreToolUse",
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart"}}"#,
            0,
            ""
        )
        .is_err());
        assert!(parse_output("PreToolUse", "diagnostic", 0, "")
            .unwrap()
            .context
            .is_empty());
        assert_eq!(
            parse_output("SessionStart", "rules", 0, "")
                .unwrap()
                .context,
            ["rules"]
        );
    }
    #[test]
    fn decisions_and_updated_input() {
        let out = parse_output("PreToolUse", r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"no","updatedInput":{"path":"safe"}}}"#, 0, "").unwrap();
        assert_eq!(out.denied.as_deref(), Some("no"));
        assert_eq!(out.updated_input.unwrap()["path"], "safe");
        assert!(parse_output("PreToolUse", "", 2, "blocked")
            .unwrap()
            .denied
            .is_some());
        assert!(parse_output("SessionStart", "{broken", 0, "").is_err());
    }
    #[test]
    fn unsupported_config_fails_before_execution() {
        let root = std::path::Path::new(".");
        assert!(parse(&json!({"Stop": []}), "test", root, root).is_err());
        assert!(parse(
            &json!({"PreToolUse": [{"matcher":"[", "hooks":[]}]}),
            "test",
            root,
            root
        )
        .is_err());
        assert!(parse(
            &json!({"SessionStart": [{"hooks":[{"type":"prompt", "command":"hi"}]}]}),
            "test",
            root,
            root
        )
        .is_err());
    }
    #[tokio::test]
    async fn runner_delivers_json_and_consumes_output() {
        let dir = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) {
            "echo hook-context"
        } else {
            "cat >/dev/null; printf hook-context"
        };
        let hooks = parse(
            &json!({"SessionStart":[{"hooks":[{"type":"command","command":script}]}]}),
            "fixture",
            dir.path(),
            dir.path(),
        )
        .unwrap();
        let runtime = Runtime {
            hooks,
            cwd: dir.path().into(),
            ..Default::default()
        };
        assert_eq!(
            runtime
                .run("SessionStart", runtime.input("SessionStart", "s"))
                .await
                .unwrap()
                .context,
            ["hook-context"]
        );
    }
}
