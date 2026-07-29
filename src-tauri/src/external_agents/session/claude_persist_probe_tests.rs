//! Real-machine probe (B1 feasibility): does the `claude` CLI stay alive after emitting a
//! `result` frame, so **one process can serve a whole conversation**?
//!
//! Kivio currently spawns a fresh `claude` per turn and reattaches context with `--resume`.
//! The persistent-session redesign (B1) hinges on one unknown that cannot be answered from
//! docs: in `-p --input-format stream-json` mode, after the CLI writes `{"type":"result"}`,
//! does it **keep reading stdin** for the next user message, or exit?
//!
//! Answer, measured on claude 2.1.220 / Windows (2026-07-29): **it keeps reading.** stdin
//! stays open, the process serves round after round, `session_id` is stable, and each round
//! is delimited by exactly one `result` frame. It exits (code 0) only when stdin is closed.
//!
//! These tests are `#[ignore]`d: they spawn the real CLI, need a working login, and cost
//! tokens. Run explicitly (Windows — plain `cargo test` binaries fail with `0xC0000139`):
//!
//! ```powershell
//! pwsh scripts/win-cargo-test.ps1 --lib claude_persist -- --ignored --nocapture
//! ```
//!
//! What they assert is deliberately falsifiable: **round 2 gets its own `result`, and its
//! answer contains the number the model was told in round 1** — i.e. it's the same live
//! session, not a fresh process silently re-answering.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::time::timeout;

use crate::external_agents::defs::claude::{build_claude_args, CLAUDE_AGENT_DEF};
use crate::external_agents::spawn::{cli_command, resolve_binary};
use crate::external_agents::types::{RuntimeBuildOptions, RuntimeContext};

/// A single round's worth of decoded stdout, up to and including its `result` frame.
struct Round {
    /// `(type, subtype)` of every non-`stream_event` frame, in order.
    frames: Vec<(String, Option<String>)>,
    /// The terminating `result` frame, if one arrived before the deadline / EOF.
    result: Option<serde_json::Value>,
}

impl Round {
    fn has_frame(&self, ty: &str, subtype: Option<&str>) -> bool {
        self.frames
            .iter()
            .any(|(t, s)| t == ty && s.as_deref() == subtype)
    }

    fn result_text(&self) -> String {
        self.result
            .as_ref()
            .and_then(|r| r.get("result"))
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// Production argv, straight from the shipping builder — the probe is worthless if it tests a
/// simplified command line. Only `session_id` is filled in per run.
fn probe_args(session_id: &str) -> Vec<String> {
    build_claude_args(
        &RuntimeContext {
            extra_allowed_dirs: vec![],
            resume_session_id: None,
            new_session_id: Some(session_id.to_string()),
            include_partial_messages: true,
        },
        &RuntimeBuildOptions {
            model: None,
            reasoning: None,
            sandbox: None,
        },
        None,
    )
}

/// Mirrors `spawn::stream_json_user_content`'s non-slash branch (a text content block plus the
/// `parent_tool_use_id` key the CLI expects).
fn user_line(text: &str) -> String {
    format!(
        "{}\n",
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
            "parent_tool_use_id": serde_json::Value::Null,
        })
    )
}

async fn write_user(child: &mut Child, text: &str) {
    let stdin = child.stdin.as_mut().expect("stdin piped");
    stdin
        .write_all(user_line(text).as_bytes())
        .await
        .expect("write user message");
    stdin.flush().await.expect("flush stdin");
}

/// Read stdout until this round's `result` frame (or the deadline / EOF).
async fn read_round<R>(lines: &mut tokio::io::Lines<BufReader<R>>, label: &str) -> Round
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut round = Round {
        frames: vec![],
        result: None,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            eprintln!("[{label}] timed out waiting for result");
            return round;
        }
        let line = match timeout(remaining, lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => {
                eprintln!("[{label}] stdout EOF (process closed its output)");
                return round;
            }
            Ok(Err(e)) => panic!("[{label}] stdout read error: {e}"),
            Err(_) => {
                eprintln!("[{label}] timed out waiting for result");
                return round;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            eprintln!("[{label}] non-JSON stdout: {line}");
            continue;
        };
        let ty = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if ty == "stream_event" {
            continue; // partial-message noise; not a round boundary signal
        }
        let subtype = value
            .get("subtype")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        eprintln!("[{label}] frame {ty}/{subtype:?}");
        round.frames.push((ty.clone(), subtype));
        if ty == "result" {
            eprintln!("[{label}] result = {value}");
            round.result = Some(value);
            return round;
        }
    }
}

/// `None` ⇒ still running.
fn exit_status(child: &mut Child) -> Option<std::process::ExitStatus> {
    child.try_wait().expect("try_wait")
}

/// THE question: three rounds over one process, stdin never closed, with a memory carried
/// across rounds to prove it's one live session rather than a silent respawn.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
async fn claude_serves_multiple_rounds_over_one_persistent_process() {
    let bin = resolve_binary(&CLAUDE_AGENT_DEF)
        .await
        .expect("claude CLI not found on PATH — cannot run this probe");
    let session_id = uuid::Uuid::new_v4().to_string();
    let workdir = std::env::temp_dir().join(format!("kivio-claude-persist-{session_id}"));
    std::fs::create_dir_all(&workdir).expect("create probe workdir");

    // `cli_command` strips PARENT_SESSION_ENV_VARS. Without it the child refuses to start
    // ("cannot be launched inside another session") whenever the test host is itself a CLI session.
    let mut child = cli_command(&bin)
        .args(probe_args(&session_id))
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn claude");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();

    write_user(&mut child, "Answer with one word only. Remember the number 42.").await;
    let r1 = read_round(&mut lines, "round1").await;
    assert!(
        r1.result.is_some(),
        "round 1 produced no result frame — the CLI is not usable in this environment \
         (check login / auth before reading anything into this)"
    );
    assert!(
        r1.has_frame("system", Some("init")),
        "round 1 should open with system/init"
    );
    assert!(
        exit_status(&mut child).is_none(),
        "process exited right after the first result — persistent sessions are impossible"
    );

    // The whole point: stdin stays OPEN and we just write the next message.
    write_user(
        &mut child,
        "What number did I just ask you to remember? Reply with just the number.",
    )
    .await;
    let r2 = read_round(&mut lines, "round2").await;
    assert!(
        r2.result.is_some(),
        "round 2 produced no result frame — the CLI stopped consuming stdin after round 1"
    );
    assert!(
        r2.result_text().contains("42"),
        "round 2 lost round 1's context (answer was {:?}) — this is not one live session",
        r2.result_text()
    );
    assert!(
        r2.has_frame("system", Some("init")),
        "system/init repeats every round, not just the first — B1 must not treat it as one-shot"
    );

    // Third round, to rule out a fluke, and to pin that `result` is a per-round boundary.
    write_user(
        &mut child,
        "Add 1 to that number and reply with just the result.",
    )
    .await;
    let r3 = read_round(&mut lines, "round3").await;
    assert!(r3.result.is_some(), "round 3 produced no result frame");
    assert!(
        r3.result_text().contains("43"),
        "round 3 answer was {:?}, expected 43",
        r3.result_text()
    );

    // session_id is stable across rounds — B1 can persist it once.
    let sid = |r: &Round| {
        r.result
            .as_ref()
            .and_then(|v| v.get("session_id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(sid(&r1), session_id, "round 1 session_id");
    assert_eq!(sid(&r2), session_id, "session_id changed between rounds");
    assert_eq!(sid(&r3), session_id, "session_id changed between rounds");

    // `modelUsage[*].contextWindow` is the denominator the context-usage UI wants. Assert a
    // real non-zero sample exists so a future CLI version dropping the field is caught here.
    let model_usage = r3
        .result
        .as_ref()
        .and_then(|v| v.get("modelUsage"))
        .and_then(|v| v.as_object())
        .cloned()
        .expect("result.modelUsage present");
    let (model, usage) = model_usage.iter().next().expect("modelUsage non-empty");
    let window = usage.get("contextWindow").and_then(|v| v.as_u64());
    assert!(
        window.is_some_and(|w| w > 0),
        "modelUsage[{model}].contextWindow should be a positive number, got {window:?}"
    );

    // Closing stdin is what ends the process — so B1's shutdown path is "close stdin, then wait".
    drop(child.stdin.take());
    let status = timeout(Duration::from_secs(30), child.wait())
        .await
        .expect("process should exit shortly after stdin closes")
        .expect("wait");
    assert!(status.success(), "clean shutdown exit status: {status:?}");

    let _ = std::fs::remove_dir_all(&workdir);
}

/// A long-lived process must survive an idle gap between user turns (nobody types continuously).
/// Measured: alive and responsive after 35s idle, no keep-alive traffic needed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
async fn claude_persistent_process_survives_an_idle_gap() {
    let bin = resolve_binary(&CLAUDE_AGENT_DEF)
        .await
        .expect("claude CLI not found on PATH — cannot run this probe");
    let session_id = uuid::Uuid::new_v4().to_string();
    let workdir = std::env::temp_dir().join(format!("kivio-claude-idle-{session_id}"));
    std::fs::create_dir_all(&workdir).expect("create probe workdir");

    let mut child = cli_command(&bin)
        .args(probe_args(&session_id))
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn claude");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();

    write_user(&mut child, "Reply with just the word READY.").await;
    let first = read_round(&mut lines, "idle/round1").await;
    assert!(first.result.is_some(), "warm-up round produced no result");

    tokio::time::sleep(Duration::from_secs(35)).await;
    assert!(
        exit_status(&mut child).is_none(),
        "process self-terminated during a 35s idle gap — B1 would need a keep-alive"
    );

    write_user(&mut child, "Reply with just the word DONE.").await;
    let after = read_round(&mut lines, "idle/round2").await;
    assert!(
        after.result.is_some(),
        "no result after the idle gap — the process was alive but had stopped reading stdin"
    );
    assert!(
        after.result_text().to_uppercase().contains("DONE"),
        "post-idle answer was {:?}",
        after.result_text()
    );

    let _ = child.kill().await;
    let _ = std::fs::remove_dir_all(&workdir);
}

/// Cancellation without killing the process. With a persistent session, `kill()` would destroy
/// the conversation, so stopping a turn must go through the CLI's `control_request` channel
/// (advertised as `interrupt_receipt_v1` in the `system/init` frame's `capabilities`).
///
/// Measured: the interrupt is acknowledged, the round still closes with a `result` frame —
/// but `is_error: true`, `subtype: "error_during_execution"`, `terminal_reason:
/// "aborted_streaming"`, `result: null` — and the process happily serves the next round.
/// B1 must therefore treat "aborted" as a cancellation, not as a failure.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
async fn claude_interrupt_ends_the_round_but_not_the_process() {
    let bin = resolve_binary(&CLAUDE_AGENT_DEF)
        .await
        .expect("claude CLI not found on PATH — cannot run this probe");
    let session_id = uuid::Uuid::new_v4().to_string();
    let workdir = std::env::temp_dir().join(format!("kivio-claude-interrupt-{session_id}"));
    std::fs::create_dir_all(&workdir).expect("create probe workdir");

    let mut child = cli_command(&bin)
        .args(probe_args(&session_id))
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn claude");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();

    write_user(
        &mut child,
        "Write a long, detailed 800-word essay about the history of clouds in art. \
         Start immediately.",
    )
    .await;

    // Wait for the answer to actually start streaming, then interrupt.
    let mut saw_delta = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                if line.contains("\"content_block_delta\"") {
                    saw_delta = true;
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("stdout read error: {e}"),
            Err(_) => break,
        }
    }
    assert!(saw_delta, "never saw a text delta to interrupt");

    let request_id = "kivio-probe-interrupt-1";
    {
        let stdin = child.stdin.as_mut().expect("stdin piped");
        let payload = format!(
            "{}\n",
            serde_json::json!({
                "type": "control_request",
                "request_id": request_id,
                "request": { "subtype": "interrupt" },
            })
        );
        stdin
            .write_all(payload.as_bytes())
            .await
            .expect("write interrupt");
        stdin.flush().await.expect("flush stdin");
    }

    let aborted = read_round(&mut lines, "interrupt/round1").await;
    let result = aborted
        .result
        .as_ref()
        .expect("interrupted round still emits a result frame");
    assert_eq!(
        result.get("terminal_reason").and_then(|v| v.as_str()),
        Some("aborted_streaming"),
        "interrupted round should report an aborted terminal_reason, got {result}"
    );
    assert!(
        aborted.has_frame("control_response", None),
        "interrupt should be acknowledged with a control_response frame"
    );
    assert!(
        exit_status(&mut child).is_none(),
        "interrupt killed the process — cancellation would destroy the session"
    );

    write_user(&mut child, "Reply with just the word ALIVE.").await;
    let next = read_round(&mut lines, "interrupt/round2").await;
    assert!(
        next.result_text().to_uppercase().contains("ALIVE"),
        "process did not serve a round after being interrupted (answer {:?})",
        next.result_text()
    );

    let _ = child.kill().await;
    let _ = std::fs::remove_dir_all(&workdir);
}
