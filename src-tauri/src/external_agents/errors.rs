//! Error classification for external CLI agents (task 07-20-cli-session-lifecycle, R2 / 缺陷 4).
//!
//! Session runners return raw error strings (RPC errors, handshake timeouts, process-exit
//! messages). Rather than dumping those into the chat bubble verbatim, `classify` buckets them
//! into an actionable category with a Chinese main message and folds the raw detail (original
//! error + exit code + stderr tail) into a collapsible `<details>` block the frontend already
//! renders. See `research/paseo-reference.md` E.1 for the design this mirrors.

/// Coarse category of an external-agent failure, chosen for the recovery action it implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAgentErrorKind {
    /// Not logged in / token expired (401 / "Authentication required"). Needs user re-login;
    /// never auto-retried (a doomed retry could trigger a login storm).
    Auth,
    /// Handshake or turn timed out — usually slow network or an unresponsive CLI.
    Timeout,
    /// The provider's streaming response was interrupted before the turn completed.
    Transport,
    /// The child process exited / hit EOF before completing the turn.
    Exited,
    /// Any other protocol / RPC failure.
    Protocol,
}

/// A raw error mapped to a user-facing bubble message plus a raw detail blob.
#[derive(Debug, Clone)]
pub struct ClassifiedError {
    pub kind: ExternalAgentErrorKind,
    /// Actionable Chinese main text shown as the bubble body.
    pub user_message: String,
    /// Raw diagnostic (original error + exit code + stderr tail), for the collapsible details.
    pub detail: String,
}

impl ClassifiedError {
    /// Render the full chat-bubble markdown: the actionable message on top, the raw diagnostic in
    /// a collapsed `<details>` block (ChatMarkdown renders these). The raw error never becomes the
    /// bubble's main text.
    pub fn render_bubble(&self) -> String {
        let mut out = self.user_message.clone();
        if !self.detail.trim().is_empty() {
            let detail = self.detail.trim();
            let longest_run = detail
                .split(|ch| ch != '`')
                .map(str::len)
                .max()
                .unwrap_or_default();
            let fence = "`".repeat(longest_run.saturating_add(1).max(3));
            out.push_str(&format!(
                "\n\n{fence}kivio-error-details\n{detail}\n{fence}"
            ));
        }
        out
    }
}

const STDERR_DETAIL_CAP: usize = 2000;

/// (display name, login command) per agent id. Kept here as a static table so `RuntimeAgentDef`
/// stays unchanged. Unknown ids fall back to a generic name with no login command.
fn agent_login_hint(agent_id: &str) -> (&'static str, &'static str) {
    match agent_id {
        "claude" => ("Claude Code", "claude /login"),
        "codex" => ("Codex CLI", "codex login"),
        "cursor-agent" => ("Cursor Agent", "cursor-agent login"),
        "opencode" => ("OpenCode", "opencode auth login"),
        "gemini" => ("Gemini CLI", "gemini"),
        "kimi" => ("Kimi CLI", "kimi"),
        "pi" => ("Pi", "pi"),
        "hermes" => ("Hermes", "hermes"),
        "grok" => ("Grok CLI", "grok"),
        "dsh" => ("DeepSeek Harness", ""),
        _ => ("外部 Agent", ""),
    }
}

/// `needle` 作为独立 token 出现（前后都不是字母/数字）才算命中——避免 "401" 误伤
/// 行号（app.js:4012）、端口（localhost:40100）等无关数字串。
fn contains_token(hay: &str, needle: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = hay[start..].find(needle) {
        let abs = start + pos;
        let before_ok = hay[..abs]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = hay[abs + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        start = abs + needle.len();
    }
    false
}

fn detect_kind(raw: &str, exit_code: Option<i32>, stderr_tail: &str) -> ExternalAgentErrorKind {
    let hay = format!("{raw}\n{stderr_tail}").to_lowercase();
    if hay.contains("authentication required")
        || hay.contains("unauthorized")
        || hay.contains("not logged in")
        || hay.contains("please log in")
        || hay.contains("please login")
        || hay.contains("login required")
        // claude 在 assistant 帧的 `error` 字段上用的是这两个机器码（不是自然语言），
        // 见 `stream/claude.rs::assistant_error_report`。不认它们的话这类失败会落进
        // Protocol，用户拿不到 `claude /login` 这条可操作提示。
        || hay.contains("authentication_failed")
        || hay.contains("oauth_org_not_allowed")
        || hay.contains("missing credential")
        || hay.contains("missing_credential")
        || contains_token(&hay, "401")
    {
        ExternalAgentErrorKind::Auth
    } else if hay.contains("timeout") || hay.contains("timed out") {
        ExternalAgentErrorKind::Timeout
    } else if hay.contains("stream_read_error")
        || hay.contains("stream read error")
        || hay.contains("stream ended unexpectedly")
        || hay.contains("stream ended before")
    {
        ExternalAgentErrorKind::Transport
    } else if hay.contains("exited")
        || hay.contains("eof")
        || matches!(exit_code, Some(code) if code != 0)
    {
        ExternalAgentErrorKind::Exited
    } else {
        ExternalAgentErrorKind::Protocol
    }
}

fn tail_chars(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    let start = chars.len().saturating_sub(max_chars);
    let mut out: String = chars[start..].iter().collect();
    if start > 0 {
        out = format!("…{out}");
    }
    out
}

/// Classify a raw session error into an actionable bubble. `stderr_tail` is the drained stderr
/// tail (may be empty); `agent_id` selects the login hint for `Auth` errors.
pub fn classify(
    raw: &str,
    exit_code: Option<i32>,
    stderr_tail: &str,
    agent_id: &str,
) -> ClassifiedError {
    let kind = detect_kind(raw, exit_code, stderr_tail);
    let (name, login) = agent_login_hint(agent_id);

    let user_message = match kind {
        ExternalAgentErrorKind::Auth => {
            if agent_id == "dsh" {
                "DeepSeek Harness 缺少 API 密钥。请到设置 → 本地 CLI 管理 → DeepSeek Harness 填写官方 DeepSeek 密钥后重试。"
                    .to_string()
            } else if login.is_empty() {
                format!("{name} 未登录或登录凭证已失效，请重新登录后重试。")
            } else {
                format!(
                    "{name} 未登录或登录凭证已失效。请在终端运行 `{login}` 重新登录，然后重试。"
                )
            }
        }
        ExternalAgentErrorKind::Timeout => {
            format!("{name} 握手或响应超时。可能是网络缓慢或 CLI 无响应，请稍后重试。")
        }
        ExternalAgentErrorKind::Transport => format!(
            "{name} 的模型流式响应中途断开，本轮未完成。通常是模型中转站、上游服务或网络连接暂时异常，请重试。"
        ),
        ExternalAgentErrorKind::Exited => match exit_code {
            Some(code) => {
                format!("{name} 进程意外退出（退出码 {code}），请确认 CLI 可正常启动后重试。")
            }
            None => format!("{name} 进程意外退出，请确认 CLI 可正常启动后重试。"),
        },
        ExternalAgentErrorKind::Protocol => {
            format!("{name} 通信出错，请重试；若持续失败请检查 CLI 版本与登录状态。")
        }
    };

    let mut detail = format!("原始错误：{}", raw.trim());
    if let Some(code) = exit_code {
        detail.push_str(&format!("\n退出码：{code}"));
    }
    if !stderr_tail.trim().is_empty() {
        detail.push_str("\nstderr：\n");
        detail.push_str(&tail_chars(stderr_tail.trim(), STDERR_DETAIL_CAP));
    }

    ClassifiedError {
        kind,
        user_message,
        detail,
    }
}

/// Convenience for the retry gate: whether a raw error is an auth failure (never auto-retried).
pub fn is_auth_error(raw: &str, agent_id: &str) -> bool {
    let _ = agent_id;
    detect_kind(raw, None, "") == ExternalAgentErrorKind::Auth
}

#[cfg(test)]
mod tests {
    use super::*;

    /// claude 的 assistant 帧用机器码而不是自然语言报认证失败——必须也归到 Auth，
    /// 否则用户拿到的是无从下手的 Protocol 提示，而不是 `claude /login`。
    #[test]
    fn classifies_claude_machine_auth_codes_as_auth() {
        for raw in [
            "claude 报告错误：authentication_failed",
            "claude 报告错误：oauth_org_not_allowed",
        ] {
            let classified = classify(raw, None, "", "claude");
            assert_eq!(classified.kind, ExternalAgentErrorKind::Auth, "{raw}");
            assert!(classified.user_message.contains("claude /login"), "{raw}");
        }
        // 计费错误不是认证错误，不得被顺手归进 Auth。
        assert_ne!(
            classify("claude 报告错误：billing_error", None, "", "claude").kind,
            ExternalAgentErrorKind::Auth
        );
    }

    #[test]
    fn classifies_auth_from_rpc_message() {
        let c = classify("Authentication required", None, "", "grok");
        assert_eq!(c.kind, ExternalAgentErrorKind::Auth);
        assert!(c.user_message.contains("grok"));
        assert!(c.user_message.contains("Grok CLI"));
    }

    #[test]
    fn classifies_auth_from_401_or_unauthorized() {
        assert_eq!(
            classify("HTTP 401", None, "", "claude").kind,
            ExternalAgentErrorKind::Auth
        );
        assert_eq!(
            classify("request Unauthorized", None, "", "codex").kind,
            ExternalAgentErrorKind::Auth
        );
        // "401" 必须是独立 token——行号/端口等长数字串不误判为 Auth。
        assert_ne!(
            classify("panic at app.js:4012", None, "", "codex").kind,
            ExternalAgentErrorKind::Auth
        );
        assert_ne!(
            classify("connect refused localhost:40100", None, "", "codex").kind,
            ExternalAgentErrorKind::Auth
        );
        assert_eq!(
            classify("server returned status 401.", None, "", "codex").kind,
            ExternalAgentErrorKind::Auth
        );
    }

    #[test]
    fn classifies_timeout() {
        assert_eq!(
            classify(
                "initialize: ACP handshake timeout",
                None,
                "",
                "cursor-agent"
            )
            .kind,
            ExternalAgentErrorKind::Timeout
        );
        assert_eq!(
            classify("request timed out", None, "", "codex").kind,
            ExternalAgentErrorKind::Timeout
        );
    }

    #[test]
    fn classifies_exited_from_message_or_exit_code() {
        assert_eq!(
            classify("ACP agent exited during handshake", None, "", "grok").kind,
            ExternalAgentErrorKind::Exited
        );
        assert_eq!(
            classify("read output failed", Some(2), "", "kimi").kind,
            ExternalAgentErrorKind::Exited
        );
        assert_eq!(
            classify(
                "codex app-server exited before completion",
                None,
                "",
                "codex"
            )
            .kind,
            ExternalAgentErrorKind::Exited
        );
    }

    #[test]
    fn classifies_protocol_as_fallback() {
        assert_eq!(
            classify("invalid session/new response", None, "", "opencode").kind,
            ExternalAgentErrorKind::Protocol
        );
    }

    #[test]
    fn classifies_dsh_missing_credential_as_auth() {
        for raw in ["missing credential", "MISSING_CREDENTIAL"] {
            let classified = classify(raw, None, "", "dsh");
            assert_eq!(classified.kind, ExternalAgentErrorKind::Auth, "{raw}");
            assert!(classified.user_message.contains("API 密钥"), "{raw}");
            assert!(classified.user_message.contains("本地 CLI 管理"), "{raw}");
            assert!(!classified.user_message.contains("重新登录"), "{raw}");
        }
    }

    #[test]
    fn auth_login_hint_per_agent() {
        assert!(classify("Authentication required", None, "", "claude")
            .user_message
            .contains("claude /login"));
        assert!(classify("Authentication required", None, "", "grok")
            .user_message
            .contains("`grok`"));
        // Unknown agent → generic name, no backtick login command.
        let unknown = classify("Authentication required", None, "", "mystery");
        assert!(unknown.user_message.contains("外部 Agent"));
        assert!(!unknown.user_message.contains('`'));
    }

    #[test]
    fn render_bubble_folds_detail_and_hides_raw_from_main_text() {
        let c = classify(
            "session-new: Authentication required",
            None,
            "boom stderr",
            "grok",
        );
        let bubble = c.render_bubble();
        // Main text is actionable; raw diagnostics use the frontend's safe disclosure block.
        assert!(bubble.starts_with("Grok CLI 未登录"));
        assert!(bubble.contains("```kivio-error-details"));
        assert!(!bubble.contains("<details>"));
        assert!(bubble.contains("session-new: Authentication required"));
        assert!(bubble.contains("boom stderr"));
    }

    #[test]
    fn classifies_stream_read_error_as_transport_even_with_nonzero_exit() {
        let c = classify("stream_read_error", Some(1), "", "pi");
        assert_eq!(c.kind, ExternalAgentErrorKind::Transport);
        assert!(c.user_message.contains("流式响应中途断开"));
        assert!(!c.user_message.contains("进程意外退出"));
    }

    #[test]
    fn detail_fence_expands_past_embedded_backticks() {
        let c = classify("protocol failed", None, "stderr with ``` inside", "pi");
        let bubble = c.render_bubble();
        assert!(bubble.contains("````kivio-error-details"));
        assert!(bubble.ends_with("````"));
    }

    #[test]
    fn detail_includes_exit_code_and_truncates_stderr() {
        let long = "x".repeat(5000);
        let c = classify("exited", Some(9), &long, "codex");
        assert!(c.detail.contains("退出码：9"));
        // stderr tail capped (+ ellipsis), far below the raw 5000 chars.
        assert!(c.detail.chars().count() < 2200);
        assert!(c.detail.contains('…'));
    }

    #[test]
    fn is_auth_error_matches_classify() {
        assert!(is_auth_error("Authentication required", "grok"));
        assert!(!is_auth_error("ACP handshake timeout", "grok"));
    }
}
