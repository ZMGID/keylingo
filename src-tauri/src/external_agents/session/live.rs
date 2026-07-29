//! Persistent cross-turn session registry for external CLI agents (Phase 2).
//!
//! A live session keeps the CLI process alive across user turns so the server holds prior
//! context natively (no full-history replay). Each session is owned by a dedicated actor task
//! reachable only through an `mpsc::Sender<SessionCommand>` — the registry never holds the
//! `Child` or any lock across a turn await, only the cheap clonable control sender.

use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use crate::external_agents::types::UnifiedAgentEvent;

/// 一条待用户答复的工具审批询问（claude 的 `control_request` / `can_use_tool`）。
///
/// 会话侧只负责把它送出去、并在收到 `ApprovalDecision` 后回一条 `control_response`；
/// 「弹卡片给谁看」是宿主（app 层）的事，会话里没有 `AppHandle` 也不该有。
#[derive(Debug, Clone)]
pub struct ApprovalAsk {
    /// CLI 的 `control_request.request_id`。**回复时必须原样回显**，否则对端匹配不到。
    pub request_id: String,
    /// claude 的 `toolu_…`。用作 Kivio 侧的 `toolCallId`，这样审批卡和工具卡指向同一个 id。
    /// CLI 偶尔不给（schema 里是 optional），缺失时回落到 `request_id`。
    pub tool_call_id: String,
    /// CLI 报的工具原名（`Write` / `Bash` / `mcp__server__tool`，PascalCase 有意义，不归一化）。
    pub tool_name: String,
    /// 工具入参原文，用于卡片上的摘要。
    pub input: serde_json::Value,
    /// CLI 标记「这个工具要用户在卡片上直接作答」（`AskUserQuestion` / `ExitPlanMode`）。
    /// 那类工具批准之后还会再发一条我们尚未实现的 `request_user_dialog`，所以一律直接拒
    /// （见 `claude_stream::approval_verdict`）。
    pub requires_user_interaction: bool,
}

/// 用户对某条询问的答复。`request_id` 是路由键，与 `ApprovalAsk` 一一对应。
#[derive(Debug, Clone)]
pub struct ApprovalDecision {
    pub request_id: String,
    pub approved: bool,
}

/// 一轮的权限审批通道。宿主持 `requests` 的接收端与 `decisions` 的发送端；会话持另一半。
///
/// 为什么不用 `oneshot` 挂在 `ApprovalAsk` 里：一轮里可以有**多条**并发的询问
/// （claude 会并行调工具），共用一条回程通道比给每条询问单独管一个 future 简单得多，
/// 而 `request_id` 已经是天然的路由键。
pub struct ApprovalBridge {
    /// 会话 → 宿主。
    pub requests: mpsc::Sender<ApprovalAsk>,
    /// 宿主 → 会话。
    pub decisions: mpsc::Receiver<ApprovalDecision>,
}

/// A command sent to a live session's actor task.
pub enum SessionCommand {
    /// Run one turn: write the prompt, stream `UnifiedAgentEvent`s into `events`, and report the
    /// terminal result through `done`. The actor processes exactly one turn at a time.
    RunTurn {
        prompt: String,
        model: Option<String>,
        reasoning: Option<String>,
        /// 本轮用户消息的原生图片块（ACP → image content block；Codex → localImage 临时文件）。空=无图。
        images: Vec<crate::external_agents::attachments::ImageBlock>,
        events: mpsc::Sender<UnifiedAgentEvent>,
        done: oneshot::Sender<Result<(), String>>,
        /// 本轮的权限审批通道。`None` = 宿主不接权限询问（未启用 / 协议不支持）⇒ 会话对
        /// `can_use_tool` 仍走那条 fail-closed 的 error 兜底，绝不沉默（spec 第 29 条）。
        approvals: Option<ApprovalBridge>,
    },
    /// Interrupt the in-flight turn without killing the process (protocol-level interrupt).
    Cancel,
    /// Shut the session down (close stdin + kill the child) and end the actor.
    Close,
}

/// 「用户已取消，但这个会话本身已经不能再用了」的哨兵。
///
/// 出口按**取消**呈现（不弹错误气泡、不发上下文重置提示、更不重发本轮 prompt——用户刚刚
/// 才把它停掉），但注册表条目必须**丢弃**：进程已经死了，或者协议级取消超时后被硬 `Close`
/// 掉了。用普通的 `"cancelled"` 会让 claude 的「取消后保留常驻会话」把一个死 actor 留下来，
/// 下一轮才发现。
pub const CANCELLED_SESSION_LOST: &str = "__cancelled_session_lost__";

/// 影响 CLI **启动参数**的配置指纹。
///
/// 常驻打破了一个此前白捡的便宜：换模型 / 换 sandbox 档位 / 换 reasoning 档位 / 改系统提示
/// 或 Memory，靠的是「下一轮换个新进程带新 flag」自动生效。进程一常驻就不生效了 ——
/// 界面显示一套、会话实际跑另一套，这**违反 spec 第 8 条**（UI 所见必须与会话实际配置一致），
/// 是功能退步而不是缺功能。指纹变了就换个进程。
///
/// 只有把这些配置放在**启动参数**里的 CLI 需要它（目前只有 claude：`--model` / `--effort` /
/// `--permission-mode` / `--append-system-prompt-file` 全是启动 flag）。ACP / codex 能在会话内
/// 改模型与推理档位，指纹恒为 `default()`，永不触发重连，行为不变。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchConfig {
    /// `model|reasoning|sandbox`，恒可知。
    pub flags: String,
    /// 启动时注入的会话级系统指令哈希。`None` = 本轮不注入（斜杠命令走 passthrough，
    /// 不带 `--append-system-prompt-file`），**不参与判定**——否则一条斜杠命令会把常驻进程
    /// 重启一次、紧跟的普通消息再重启一次，来回抖。
    pub instructions: Option<String>,
}

impl LaunchConfig {
    /// 已建立的会话（`self`，注册时的配置）能否服务配置为 `incoming` 的这一轮。
    pub fn accepts(&self, incoming: &LaunchConfig) -> bool {
        if self.flags != incoming.flags {
            return false;
        }
        match incoming.instructions.as_deref() {
            // 本轮不注入指令 ⇒ 不据此判定（见字段注释）。
            None => true,
            // 本轮要注入 ⇒ 会话必须是带着**同一份**指令启动的。注册时为 `None`
            // （例如会话是被一条斜杠命令拉起来的）同样算不匹配，否则用户配置的系统提示
            // 与 Memory 会在这个会话里静默失效。
            Some(hash) => self.instructions.as_deref() == Some(hash),
        }
    }
}

/// Registry entry: the control channel plus metadata used to decide reuse.
pub struct LiveSession {
    pub control: mpsc::Sender<SessionCommand>,
    pub agent_id: String,
    pub cwd: String,
    /// 会话建立时生效的启动配置。与本轮的指纹不符 ⇒ 不可复用（见 `LaunchConfig`）。
    pub launch_config: LaunchConfig,
    /// Last time a turn was sent/started; drives idle reclamation + LRU eviction.
    pub last_activity: Instant,
    /// 常驻子进程的 pid。**纯元数据**，注册表不拿它做任何决策（关停一律走 actor 的
    /// `Close`，绝不按 pid 杀）。存它是因为「这两轮是不是同一个进程」在别处根本没有
    /// 可观测信号——不记就只能去数系统进程表。
    pub child_pid: Option<u32>,
    /// 这个进程已经服过几轮（注册即 1，之后每次被复用 +1）。同样是纯元数据。
    pub turns_served: u32,
}

impl LiveSession {
    /// A session is reusable only if its actor is still listening, it targets the same
    /// agent + working directory as the incoming turn, and it was launched with a
    /// configuration that still matches what the UI currently shows.
    pub fn is_reusable(&self, agent_id: &str, cwd: &str, launch_config: &LaunchConfig) -> bool {
        !self.control.is_closed()
            && self.agent_id == agent_id
            && self.cwd == cwd
            && self.launch_config.accepts(launch_config)
    }

    /// Reclaimable: the actor already exited, or the session has been idle past `ttl`.
    pub fn is_idle(&self, ttl: Duration) -> bool {
        self.control.is_closed()
            || Instant::now().saturating_duration_since(self.last_activity) >= ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(agent: &str, cwd: &str) -> (LiveSession, mpsc::Receiver<SessionCommand>) {
        let (tx, rx) = mpsc::channel(1);
        (
            LiveSession {
                control: tx,
                agent_id: agent.to_string(),
                cwd: cwd.to_string(),
                launch_config: LaunchConfig::default(),
                last_activity: Instant::now(),
                child_pid: None,
                turns_served: 1,
            },
            rx,
        )
    }

    #[test]
    fn reusable_when_agent_and_cwd_match_and_actor_alive() {
        let (session, _rx) = make("codex", "/proj");
        let any = LaunchConfig::default();
        assert!(session.is_reusable("codex", "/proj", &any));
        assert!(!session.is_reusable("codex", "/other", &any));
        assert!(!session.is_reusable("claude", "/proj", &any));
    }

    #[test]
    fn not_reusable_when_actor_dropped() {
        let (session, rx) = make("codex", "/proj");
        drop(rx); // actor gone → control channel closed
        assert!(!session.is_reusable("codex", "/proj", &LaunchConfig::default()));
    }

    // ---- B1: 启动配置变更必须换进程（spec 第 8 条）----

    fn cfg(flags: &str, instructions: Option<&str>) -> LaunchConfig {
        LaunchConfig {
            flags: flags.to_string(),
            instructions: instructions.map(str::to_string),
        }
    }

    /// 换模型 / 换档位 / 换 sandbox：这些是启动 flag，常驻进程只能靠重连生效。
    #[test]
    fn a_flag_change_rejects_the_existing_session() {
        let established = cfg("opus|high|bypassPermissions", Some("h1"));
        assert!(established.accepts(&cfg("opus|high|bypassPermissions", Some("h1"))));
        assert!(!established.accepts(&cfg("sonnet|high|bypassPermissions", Some("h1"))));
        assert!(!established.accepts(&cfg("opus|low|bypassPermissions", Some("h1"))));
        assert!(!established.accepts(&cfg("opus|high|plan", Some("h1"))));
    }

    /// 改系统提示 / Memory：`--append-system-prompt-file` 的内容变了，而常驻进程只在启动时
    /// 读一遍那个文件 ⇒ 不重连就静默失效，无任何可观测信号。
    #[test]
    fn changed_instructions_reject_the_existing_session() {
        let established = cfg("opus||", Some("hash-old"));
        assert!(!established.accepts(&cfg("opus||", Some("hash-new"))));
    }

    /// 斜杠命令那一轮不注入指令 ⇒ 不据此重连（否则「斜杠 → 普通消息」会来回重启两次）。
    #[test]
    fn a_slash_turn_reuses_the_session_regardless_of_instructions() {
        let established = cfg("opus||", Some("hash-old"));
        assert!(established.accepts(&cfg("opus||", None)));
        // 但 flag 变了仍要重连——斜杠命令也是在这个进程里跑的。
        assert!(!established.accepts(&cfg("sonnet||", None)));
    }

    /// 会话是被一条斜杠命令拉起来的（注册时 instructions = None）：紧跟的普通消息必须重连，
    /// 否则用户配置的系统提示与 Memory 在这个会话里永远不生效。
    #[test]
    fn a_session_launched_without_instructions_cannot_serve_a_turn_that_needs_them() {
        let established = cfg("opus||", None);
        assert!(!established.accepts(&cfg("opus||", Some("hash-1"))));
        assert!(established.accepts(&cfg("opus||", None)));
    }

    /// 非 claude 协议指纹恒为默认值 ⇒ 永不触发重连，既有行为不变。
    #[test]
    fn default_launch_config_always_accepts() {
        assert!(LaunchConfig::default().accepts(&LaunchConfig::default()));
    }

    #[test]
    fn is_idle_on_age_or_closed_channel() {
        // Fresh + open → not idle.
        let (session, _rx) = make("codex", "/proj");
        assert!(!session.is_idle(Duration::from_secs(600)));

        // Aged past ttl → idle.
        let (mut aged, _rx2) = make("codex", "/proj");
        aged.last_activity = Instant::now()
            .checked_sub(Duration::from_secs(700))
            .expect("instant in range");
        assert!(aged.is_idle(Duration::from_secs(600)));
        assert!(!aged.is_idle(Duration::from_secs(3600)));

        // Closed actor → idle regardless of age.
        let (closed, rx3) = make("codex", "/proj");
        drop(rx3);
        assert!(closed.is_idle(Duration::from_secs(3600)));
    }
}
