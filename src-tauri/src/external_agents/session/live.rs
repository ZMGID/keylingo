//! Persistent cross-turn session registry for external CLI agents (Phase 2).
//!
//! A live session keeps the CLI process alive across user turns so the server holds prior
//! context natively (no full-history replay). Each session is owned by a dedicated actor task
//! reachable only through an `mpsc::Sender<SessionCommand>` — the registry never holds the
//! `Child` or any lock across a turn await, only the cheap clonable control sender.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
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
    /// CLI 标记「这个工具要用户在卡片上直接作答」。问用户经 `ask_user::codec_for`
    /// 转成 Kivio 已有的卡片，答复经 `ApprovalDecision::updated_input` 回去；
    /// claude 的 `ExitPlanMode` 走审批卡、批准时额外经
    /// `ApprovalDecision::set_permission_mode` 切档位。其余（CLI 将来新增的
    /// 交互工具）仍当场拒（见 `claude_stream::APPROVAL_INTERACTIVE_UNSUPPORTED`）。
    pub requires_user_interaction: bool,
}

/// 用户对某条询问的答复。`request_id` 是路由键，与 `ApprovalAsk` 一一对应。
#[derive(Debug, Clone)]
pub struct ApprovalDecision {
    pub request_id: String,
    pub approved: bool,
    /// 批准时要**改写**给 CLI 的工具入参（官方 `PermissionResult.updatedInput`）。
    ///
    /// 存在的唯一理由是 `AskUserQuestion`：官方答法就是从这条通道回
    /// `allow + updatedInput.answers`（见 `claude_stream::APPROVAL_INTERACTIVE_UNSUPPORTED`），
    /// 没有第二条控制请求。`None` = 不改入参，CLI 用原入参。
    pub updated_input: Option<serde_json::Value>,
    /// 回这条答复**之前**要先把 CLI 切到的权限档位（`set_permission_mode` 控制请求）。
    ///
    /// 存在的唯一理由是 `ExitPlanMode`：批准一个计划 = 「同意，去做吧」，而 CLI 不会因为
    /// 这次 `allow` 自己离开计划档 —— 不切档位的话它下一句 `Edit` 又被计划档挡回来，
    /// 用户点了「批准」却什么都没发生。paseo 的做法一样（`await setPermissionMode(...)`
    /// 在 resolve 之前），顺序不能反：先切档、再放行。
    pub set_permission_mode: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageInjectionKind {
    Steer,
    FollowUp,
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
        /// Codex `workspace-write` 默认锁在 cwd：附件目录 / 临时图片必须作为
        /// `sandboxPolicy.writableRoots` 下发，否则 CLI 读不到。其它协议忽略。
        extra_writable_roots: Vec<String>,
        events: mpsc::Sender<UnifiedAgentEvent>,
        done: oneshot::Sender<Result<(), String>>,
        /// 本轮的权限审批通道。`None` = 宿主不接权限询问（未启用 / 协议不支持）⇒ 会话对
        /// `can_use_tool` 仍走那条 fail-closed 的 error 兜底，绝不沉默（spec 第 29 条）。
        approvals: Option<ApprovalBridge>,
    },
    /// Interrupt the in-flight turn without killing the process (protocol-level interrupt).
    Cancel,
    /// 运行中注入一条用户消息：不中断在飞的轮次。
    ///
    /// `kind` 区分立刻引导（当前轮）与 follow-up（下一轮）。`accepted` 回 true 只表示
    /// 协议层受理了（codex `turn/steer`、Pi `steer` / `follow_up`、dsh `session/steer` /
    /// `session/prompt`）。
    /// 回 false 的情形都要让调用方把这条消息留在队列里、按普通消息在轮末发出去：
    /// 轮次之间没有可注入的对象、该 CLI 的协议不支持、或者对端明确拒绝
    /// （codex 的 review / compact 轮次不可 steer）。**绝不能悄悄吞掉**。
    Steer {
        /// 前端生成的 id，原样回到 `user_steer` 卡上供前端对账出队。
        id: String,
        text: String,
        images: Vec<crate::external_agents::attachments::ImageBlock>,
        kind: MessageInjectionKind,
        accepted: oneshot::Sender<bool>,
    },
    /// 停止 CLI 侧的一个后台任务（Background tasks 面板的停止按钮）。
    ///
    /// claude 写 `control_request` 的 `stop_task`；dsh 写 `session/stop-job`
    ///（bridge：`ctx.jobs.kill`，子代理 `childSessionId` 走 `subagents.interrupt`）。
    /// 其余 CLI 的 actor 收到后直接忽略。无 ack —— 发起方已乐观置 stopped，
    /// 注册表由后续任务边沿帧修正。
    StopTask { task_id: String },
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
/// 只有把这些配置放在**启动参数**里的 CLI 需要它（claude：`--model` / `--effort` /
/// `--permission-mode` / `--append-system-prompt-file`；Pi：`--model` / `--thinking`）。
/// ACP / codex 能在会话内改模型与推理档位，指纹恒为 `default()`，永不触发重连，行为不变。
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
    /// Pi receives model/thinking as process launch flags. Empty selection still
    /// fingerprints as `"|"` so it matches chat-turn fingerprints,
    /// not `Default` (`flags == ""`).
    pub fn for_pi(model: Option<&str>, reasoning: Option<&str>) -> Self {
        Self {
            flags: format!(
                "{}|{}",
                model.unwrap_or_default(),
                reasoning.unwrap_or_default()
            ),
            instructions: None,
        }
    }

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
    /// 本会话当前有没有在飞的轮次。
    ///
    /// `last_activity` 只在轮**开始**时写一次，轮内的帧不刷新它 —— 而一轮跑十几分钟是
    /// 合法的（没有轮内超时）。少了这一位，清扫器和 LRU 会把正在干活的会话回收掉：
    /// 这一轮不会断（调用方自己持着 `control` 克隆），但轮末最后一个 sender 落地 ⇒ actor
    /// 关掉进程 ⇒ 下一轮白付一次冷启动。结果是**越重度使用的会话越吃不到常驻**，而且
    /// 没有任何可观测信号。
    pub busy: Arc<AtomicBool>,
}

/// 轮次期间把会话标成 busy，任何退出路径（含 `?` 与 panic）都会清掉。
pub struct TurnBusyGuard(Arc<AtomicBool>);

impl TurnBusyGuard {
    pub fn new(busy: Arc<AtomicBool>) -> Self {
        busy.store(true, Ordering::Release);
        Self(busy)
    }

    pub fn try_new(busy: Arc<AtomicBool>) -> Option<Self> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(busy))
    }
}

impl Drop for TurnBusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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
    ///
    /// 有在飞轮次时永不可回收 —— 见 `busy`。
    pub fn is_idle(&self, ttl: Duration) -> bool {
        if self.control.is_closed() {
            return true;
        }
        if self.busy.load(Ordering::Acquire) {
            return false;
        }
        Instant::now().saturating_duration_since(self.last_activity) >= ttl
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
                busy: Arc::new(AtomicBool::new(false)),
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

    /// 非 claude / Pi 协议指纹恒为默认值 ⇒ 永不触发重连，既有行为不变。
    #[test]
    fn default_launch_config_always_accepts() {
        assert!(LaunchConfig::default().accepts(&LaunchConfig::default()));
    }

    #[test]
    fn pi_launch_config_matches_chat_turns_even_with_empty_selection() {
        let idle = LaunchConfig::for_pi(None, None);
        assert_eq!(idle.flags, "|");
        assert_ne!(idle, LaunchConfig::default());
        assert!(idle.accepts(&LaunchConfig::for_pi(None, None)));
        assert!(!LaunchConfig::default().accepts(&idle));
        assert!(LaunchConfig::for_pi(Some("opus"), Some("high"))
            .accepts(&LaunchConfig::for_pi(Some("opus"), Some("high"))));
        assert!(!LaunchConfig::for_pi(Some("opus"), Some("high"))
            .accepts(&LaunchConfig::for_pi(Some("opus"), Some("low"))));
    }

    /// 轮内重连之后，guard 必须重新挂到**新**会话上。
    ///
    /// `reconnect_fresh` 会往注册表塞一条全新的 `LiveSession`（`busy: false`），旧 guard 指着
    /// 旧会话的 Arc。不重挂的话，恰好是刚付过一次冷启动的那条路反而会被清扫器/LRU 拆掉。
    #[test]
    fn a_new_session_needs_its_own_busy_guard() {
        let (old_session, _rx1) = make("claude", "/proj");
        let (new_session, _rx2) = make("claude", "/proj");

        let old_guard = TurnBusyGuard::new(old_session.busy.clone());
        assert!(!old_session.is_idle(Duration::from_secs(0)), "旧会话在飞");
        // 新会话没被标记 ⇒ 立刻可回收。这正是重连后的真实状态。
        assert!(
            new_session.is_idle(Duration::from_secs(0)),
            "新会话默认不 busy"
        );

        // 重挂之后新会话受保护；旧 guard 在赋值时落地，旧会话回到可回收。
        let new_guard = TurnBusyGuard::new(new_session.busy.clone());
        drop(old_guard);
        assert!(
            old_session.is_idle(Duration::from_secs(0)),
            "旧 guard 落地即清"
        );
        assert!(
            !new_session.is_idle(Duration::from_secs(0)),
            "新会话现在在飞"
        );
        drop(new_guard);
        assert!(
            new_session.is_idle(Duration::from_secs(0)),
            "guard 落地即清"
        );
    }

    #[test]
    fn a_control_guard_cannot_clear_an_existing_turn_guard() {
        let (session, _rx) = make("pi", "/proj");
        let turn_guard = TurnBusyGuard::new(session.busy.clone());
        assert!(TurnBusyGuard::try_new(session.busy.clone()).is_none());
        assert!(session.busy.load(Ordering::Acquire));
        drop(turn_guard);
        let control_guard = TurnBusyGuard::try_new(session.busy.clone()).expect("idle session");
        assert!(session.busy.load(Ordering::Acquire));
        drop(control_guard);
        assert!(!session.busy.load(Ordering::Acquire));
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
