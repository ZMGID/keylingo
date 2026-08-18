use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant},
};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::inpainting::InpaintingClient;
#[cfg(target_os = "macos")]
use crate::macos_ocr::MacOcrClient;
use crate::mcp::manager::McpSession;
use crate::mcp::types::{McpTool, PythonRunResult};
use crate::native_tools::SandboxExportContext;
use crate::offline_models::OfflineModelManager;
use crate::rapidocr::RapidOcrClient;
use crate::settings::Settings;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingChatExternalAttachment {
    pub id: String,
    pub r#type: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug)]
pub struct PendingPythonRun {
    pub sender: oneshot::Sender<PythonRunResult>,
    pub export_ctx: SandboxExportContext,
}

/// 一条挂起的会话级授权。run_id 用来在应答/取消/超时时撤掉快照里的授权卡。
#[derive(Debug)]
pub struct PendingSessionConsent {
    pub run_id: String,
    pub sender: oneshot::Sender<bool>,
}

/// 一条挂起的敏感工具审批。除 sender 外还带上 conversation_id + 工具名，因为
/// 「总是允许」是在响应命令（只拿到 tool_call_id）里落表的，得知道往哪条键上记。
#[derive(Debug)]
pub struct PendingToolApproval {
    pub conversation_id: String,
    pub tool_name: String,
    pub sender: oneshot::Sender<ToolApprovalOutcome>,
}

/// 用户对一张审批卡的答复。
///
/// 绝大多数审批只有「允许 / 拒绝」，`permission_mode` 恒为 `None`。它存在的唯一理由是
/// claude 的计划批准（`ExitPlanMode`）：那张卡是**三选一**（批准并自动放行 / 批准但逐步
/// 确认 / 拒绝），选哪一档决定了批准之后要把 CLI 切到哪个权限模式。
#[derive(Debug, Clone, Default)]
pub struct ToolApprovalOutcome {
    pub approved: bool,
    pub permission_mode: Option<String>,
}

#[derive(Debug)]
pub struct TimedCacheEntry<V> {
    created_at: Instant,
    last_accessed: Instant,
    value: V,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingChatExternalMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingChatExternalSend {
    pub id: String,
    pub content: String,
    pub attachments: Vec<PendingChatExternalAttachment>,
    /// 可选的多轮历史。为空 → 旧的「单条消息」交接路径；非空 → 用历史预置一个新会话，
    /// 不触发回复（截图作为首个 user 轮的附件，见 attachments）。
    #[serde(default)]
    pub messages: Vec<PendingChatExternalMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolSnapshot {
    pub config_fingerprint: String,
    pub tools: Vec<McpTool>,
}

fn mcp_tool_snapshot_path(usage_dir: &std::path::Path) -> PathBuf {
    usage_dir.join("mcp-tool-snapshots.json")
}

fn load_mcp_tool_snapshots(usage_dir: &std::path::Path) -> HashMap<String, McpToolSnapshot> {
    let path = mcp_tool_snapshot_path(usage_dir);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    match serde_json::from_str(&content) {
        Ok(snapshots) => snapshots,
        Err(err) => {
            eprintln!(
                "Failed to load MCP tool snapshots from {}: {err}",
                path.display()
            );
            HashMap::new()
        }
    }
}

/// 应用全局状态
/// 使用 RwLock 保护 settings，允许多读单写；
/// Mutex 用于 explain_images 等需要独占访问的数据；
/// AtomicBool 标记 lens 是否正在进行，防止并发热键触发。
pub struct AppState {
    pub settings: RwLock<Settings>,
    pub explain_images: Mutex<HashMap<String, PathBuf>>,
    pub current_explain_image_id: Mutex<Option<String>>,
    pub lens_busy: AtomicBool,
    /// Lens 会话代号：每次 `lens_request_internal` 成功开启一个新浮窗会话就 +1。
    /// 强制关闭 watchdog（`schedule_forced_lens_close`）用它判断"宽限期内是否已开了新会话"，
    /// 避免迟到的 watchdog 误杀用户刚刚重新打开的浮窗。
    pub lens_open_seq: AtomicU64,
    /// 最近一次 Lens 开启的时刻。busy 自愈（busy=true 但无浮窗可见 → 清 busy）必须避开
    /// "正在开启中"的窗口期：开启过程要截冻结帧（200-500ms），此时窗口尚不可见，若立即
    /// 自愈会让快速连按热键并发跑两次 lens_request_internal（take-once 复位载荷被吞，
    /// 前端进入坏状态）。宽限期内（LENS_OPEN_GRACE）不自愈。
    pub lens_opened_at: Mutex<Option<std::time::Instant>>,
    /// macOS：打开浮窗前记下的前台 App PID（0 = 无 / 前台就是 Kivio 自己），关闭浮窗时据此把
    /// 前台交还给原来的 App，避免 Kivio 变成"前台却无窗口"而触发 RunEvent::Reopen 误开 Chat。
    /// lens（含截图/选词翻译）与输入翻译是各自独立、可同时存在的浮窗，各占一个槽，避免相互覆盖。
    /// 详见 spec/backend/window-lifecycle.md。
    pub prev_frontmost_pid_lens: AtomicI32,
    pub prev_frontmost_pid_main: AtomicI32,
    /// 流式取消代号：每开新的流就 +1，跑流的循环检测到代号变了就立即结束。
    pub explain_stream_generation: AtomicU64,
    /// Chat 流式取消代号分配器。仅作**单调递增的 generation 号分配器**（never 重用）：
    /// 每条 run 取一个全局唯一号，不表达「活跃」语义。会话内唯一即够用（号只用于集合成员
    /// 判定），故用一个进程级 `AtomicU64` 计数器即可，无需按 conversation_id 分桶。
    /// 「哪些 generation 当前有效」由 `chat_active_generations` 表达——这样同一会话可同时
    /// 有多条并发 run（多模型一问多答），开新 run 不再作废兄弟 run。
    pub chat_stream_generation: AtomicU64,
    /// 每个 conversation_id 当前**活跃**的 generation 集合。`next_chat_generation` 往里加，
    /// run 结束 `end_chat_generation` 移除，`cancel_chat_generation` 整会话清空（取消全部在跑 run）。
    /// 单 run 时集合恒为单元素，语义与旧「单 u64」等价。
    pub chat_active_generations: Mutex<HashMap<String, HashSet<u64>>>,
    /// 正在进行 assistant 回复生成的 (conversation_id → run_id 集合)，防止同对话同一 run 重复，
    /// 但允许同一会话多条 run 并存（多模型一问多答）。
    pub chat_active_replies: Mutex<HashMap<String, HashSet<String>>>,
    /// Sequenced, replayable realtime chat protocol state keyed by run id.
    pub chat_protocol: Mutex<crate::chat::protocol::ChatProtocolHub>,
    /// 等待用户确认的敏感 Chat tool 调用（key = tool_call_id）。
    pub pending_chat_tool_approvals: Mutex<HashMap<String, PendingToolApproval>>,
    /// 本对话已按工具名授予的「总是允许」集合：`(conversation_id, 小写工具名)`。
    /// 仅内存、不持久化，重启后重新询问（同 `chat_session_consent` 的取舍）。
    pub chat_tool_always_allow: Mutex<HashSet<(String, String)>>,
    /// 本会话(conversation_id)已授予「文件/命令」工具的会话级授权集合。
    /// 仅内存、不持久化:重启后重新授权(也是一道轻量安全属性)。
    pub chat_session_consent: Mutex<HashSet<String>>,
    /// 等待用户响应的会话级授权请求(按 conversation_id,同一会话同时至多一个)。
    pub pending_chat_session_consents: Mutex<HashMap<String, PendingSessionConsent>>,
    /// 串行化会话授权弹窗:同一时刻全局只发一个授权请求。首轮多个并行只读工具
    /// (read/grep/find/ls)同时触发授权时,避免互相覆盖 pending sender 导致「假拒绝」——
    /// 拿到锁后先复查 has_chat_consent,领头者授权后其余直接复用、不再弹窗。
    pub chat_consent_prompt_lock: tokio::sync::Mutex<()>,
    /// 等待用户回答的 Chat ask_user 澄清卡片。
    pub pending_chat_user_prompts:
        Mutex<HashMap<String, crate::chat::ask_user::PendingAskUserPrompt>>,
    /// 外部 CLI 的问用户答完之后的 `askUser` 结构化载荷（键 = 工具调用 id）。
    ///
    /// 为什么要绕一道：那条卡片的记录是 CLI 的流解析层建的（`structured_content` 是 claude
    /// 的原始入参），而答案只有审批宿主那侧知道，两边在不同的任务里。不放进落盘记录的话，
    /// 消息流里那块「问了什么 + 选了什么」刷新一次就没了 —— 只剩一行看不见的灰字。
    ///
    /// ponytail: 只在 `ToolResult` 落地时消费一次并移除；那一轮死在半路的残留会留到进程退出
    /// （一条询问一个小 JSON，量级可忽略）。真要收严就在轮末按 run 清一次。
    pub answered_ask_user_content: Mutex<HashMap<String, serde_json::Value>>,
    /// 等待前端 Pyodide 完成的 run_python 调用。
    pub pending_python_runs: Mutex<HashMap<String, PendingPythonRun>>,
    /// 保护 Chat 空白会话复用的短临界区，避免快速多次新建时并发创建多个空白对话。
    pub chat_create_conversation_lock: tokio::sync::Mutex<()>,
    /// 外部 CLI 斜杠命令探测缓存（agent_id:cwd → 命令列表）。
    pub external_slash_commands_cache: Mutex<
        HashMap<
            String,
            TimedCacheEntry<Vec<crate::external_agents::types::ExternalCliSlashCommand>>,
        >,
    >,
    /// 外部 CLI 模型列表探测缓存（agent_id:cwd → 模型选项 + 来源）。probed 结果长 TTL，
    /// fallback（探测失败降级）短 TTL 负缓存，防止连续失败风暴。
    pub external_agent_models_cache:
        Mutex<HashMap<String, TimedCacheEntry<crate::external_agents::types::CachedAgentModels>>>,
    /// 外部 CLI 全量检测结果缓存（cwd → available/version/auth/models）。避免 RuntimePicker /
    /// 设置页每次打开都重探全部 CLI，同时隔离项目级模型配置。force_refresh 时跳过当前 cwd。
    pub external_detected_agents_cache:
        Mutex<HashMap<String, TimedCacheEntry<Vec<crate::external_agents::types::DetectedAgent>>>>,
    /// single-flight：可用性探测的全局串行锁——并发调用只实跑一次，后到者持锁后复查缓存即命中。
    pub availability_probe_lock: tokio::sync::Mutex<()>,
    /// single-flight：按 (agent:cwd) key 的模型探测锁，避免同一目标并发重探。
    // ponytail: 无上限增长，每 (agent, cwd) 一项（会话×agent 级，量很小）；若日后 key 基数变大，
    // 改成带容量上限的 LRU 或探测完即移除空闲锁。
    pub model_probe_locks: Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    /// Pi tree panel lazy-connect single-flight, keyed by Kivio conversation id.
    pub pi_session_control_locks: Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    /// Phase 2 持久会话注册表：conversation_id → 活会话（仅持有控制通道，不持有 Child）。
    /// 仅在 get/insert/remove 时短暂持锁，绝不跨 turn await 持锁。
    pub external_live_sessions:
        Mutex<HashMap<String, crate::external_agents::session::live::LiveSession>>,
    /// 外部入口（例如 Lens）交给 Chat 前端发送的待处理消息。
    /// 后端只负责保存请求和打开窗口，实际发送必须走 Chat 前端的手动发送状态机。
    pub pending_chat_external_sends: Mutex<Vec<PendingChatExternalSend>>,
    /// 运行中用户插话（steering）的信箱：conversation_id → 待注入的用户消息。
    /// `run_agent_loop` 在每个轮次开头 take 一次（取一次清一次），注入进本轮模型历史。
    /// 仅内存、不持久化：它描述的是「某条正在跑的 run」，进程活着才有意义。
    ///
    /// ponytail: 按 conversation_id 而不是 run_id 建键 —— 同会话多条并发 run（多模型一问多答）时
    /// 无法定向到具体某条臂，所以前端在 `reply_models ≥ 2` 时不给「立刻引导」入口。要支持就把键
    /// 换成 run_id，并让前端把当前 run_id 传进来。
    pub pending_chat_steering: Mutex<HashMap<String, Vec<crate::chat::agent::SteeringMessage>>>,
    /// 运行中原生 follow-up 信箱：conversation_id → 待在终答后续跑的用户消息。
    /// 不在轮首注入（那是 `pending_chat_steering`）。仅内存、不持久化。
    /// 同样按 conversation_id 建键，前端在 `reply_models ≥ 2` 时不给自动 follow-up。
    pub pending_chat_follow_up: Mutex<HashMap<String, Vec<crate::chat::agent::SteeringMessage>>>,
    /// Lens 启动前抓到的选中文本：放在这里等前端 enterSelect 来取走。
    /// 取一次清一次（take 语义）。无选中 / 取过 / translate 模式 = None。
    pub pending_selection: Mutex<Option<String>>,
    /// Windows 冻结帧选择模式的临时截图 id。仅在进入 select 态前预抓屏幕时使用。
    pub lens_freeze_frame_image_id: Mutex<Option<String>>,
    /// Lens 进入 select 态的复位载荷（frame + freezeFrameImageId 的 JSON）。前端冷挂载时
    /// 主动 take 来兜底：Windows 关闭即销毁后,下次冷启的 webview 可能晚于 Rust 的 lens:reset
    /// eval 才挂上监听 → 事件被丢。改放 AppState 供拉取,丢事件也不丢冻结帧。take 语义。
    pub lens_pending_reset: Mutex<Option<String>>,
    /// API Key 多 key failover 状态：(provider_id, key_idx) → 冷却到期时间。
    /// 某个 key 触发 quota/rate-limit/auth 失败时进入冷却，KEY_COOLDOWN 秒内不再选用。
    pub key_cooldowns: Mutex<HashMap<(String, usize), Instant>>,
    /// 每个 provider 当前活跃 key idx：上一次成功的 key 优先继续用。
    pub active_key_idx: Mutex<HashMap<String, usize>>,
    /// 运行时学习到的"该端点拒绝 `prompt_cache_key`"集合（按 base_url）。
    /// 某端点首次因该字段 400 后记入，本会话后续请求不再发，避免重复触发 + 无谓重试。
    pub prompt_cache_key_unsupported: Mutex<HashSet<String>>,
    /// 运行时学习到的"该端点拒绝 `prompt_cache_retention`"集合（按 base_url）。
    /// long 档 24h 被拒时记入，后续只发 key 不发 24h。
    pub prompt_cache_retention_unsupported: Mutex<HashSet<String>>,
    /// 出图端点自愈缓存：(provider_id, normalized_model) → 上次成功的 [`ImageRoute`]。
    /// 首选端点被 provider 判为端点错配后，换端点成功即记入，下次同模型直达正确端点。
    /// 仅内存、不落盘（`ImageRoute` 是运行时枚举，非配置）。
    pub image_route_cache:
        Mutex<HashMap<(String, String), crate::chat::image_generation::ImageRoute>>,
    /// MCP 持久连接池：server_id → 该 server 的长连接会话。
    /// 每会话独立 `Arc<Mutex>`，A 服务器握手不阻塞 B；外层 `tokio::sync::Mutex`
    /// 只在命中判断 / 插入 / 移除时短暂持有，绝不跨握手 await。
    pub mcp_sessions: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<McpSession>>>>,
    /// Last successful MCP tool schemas, independent from transport/session lifetime.
    /// Persisted so startup failures can still expose tools discovered by a previous run.
    pub mcp_tool_snapshots: Mutex<HashMap<String, McpToolSnapshot>>,
    /// Token usage ledger directory under app data. Model providers can append records
    /// without needing an AppHandle threaded through every call path.
    pub usage_dir: PathBuf,
    pub http: Client,
    /// 直连客户端（忽略系统/环境代理）。只有当某个供应商关掉「跟随系统代理」时才构造，
    /// 默认全跟随系统代理的用户不会多出一个连接池。
    http_direct: std::sync::OnceLock<Client>,
    /// macOS Apple Vision OCR sidecar 客户端。只有系统 OCR 路径会拉起。
    #[cfg(target_os = "macos")]
    pub macos_ocr: std::sync::Arc<MacOcrClient>,
    /// RapidOCR 与替换翻译共用的离线模型清单、下载器和 ONNX Runtime 生命周期。
    pub offline_models: std::sync::Arc<OfflineModelManager>,
    /// RapidOCR 离线 OCR 客户端。模型 + onnxruntime dylib 都由用户在设置页面下载到 app data 目录,
    /// 安装包不带任何 ONNX Runtime 二进制。`status()` 检查 4 个文件齐不齐, 不齐让前端引导下载。
    pub rapidocr: std::sync::Arc<RapidOcrClient>,
    /// MI-GAN 惰性 session；只在替换翻译调用且离线包已显式下载后加载。
    pub inpainting: std::sync::Arc<InpaintingClient>,
    /// 多 agent / 子 agent 任务表（P3）：spawn 的子 agent 状态、按名寻址、并发上限。
    pub sub_agents: crate::chat::sub_agent::SubAgentManager,
    /// 后台 run_command 进程注册表：job_id → 跟踪中的后台命令。
    /// 与后台 subagent 不同：这些命令**跨 turn 存活**，只由显式 `kill_background`
    /// 或 app 退出 sweep 清理（对齐 Claude Code background bash，dev-server 友好），
    /// 不随发起的 run 取消。仅在 insert/lookup/sweep 时短暂持锁。
    pub background_commands: Arc<Mutex<HashMap<String, crate::native_tools::BackgroundCommand>>>,
    /// 外部 CLI 自报的后台任务注册表（目前只有 claude：后台 Bash / 后台子代理，
    /// task_id → 条目）。由 `run.rs` 消费 `UnifiedAgentEvent::BackgroundTask` 时 upsert，
    /// Background tasks 面板轮询读取。仅内存：任务活在 CLI 进程里，Kivio 重启即失效。
    pub external_background_tasks: Mutex<HashMap<String, ExternalBackgroundTask>>,
    /// 开发者「请求调试」内存环形缓冲：最近 [`REQUEST_DEBUG_CAPACITY`] 条 provider 调用的
    /// 请求（脱敏 headers + body）+ 响应摘要。默认关闭（`chat_tools.request_debug_enabled`），
    /// 关闭时 adapter 短路、不构造记录。仅内存、不落盘，进程退出即清。
    pub request_debug: Mutex<VecDeque<crate::chat::request_debug::RequestDebugRecord>>,
}

/// 一条外部 CLI 后台任务（claude 的 `system/task_started` / `task_notification`）。
/// 任务本体活在 CLI 进程里，Kivio 只有观测与 `stop_task`，没有 pid、没有日志文件。
#[derive(Debug, Clone)]
pub struct ExternalBackgroundTask {
    pub task_id: String,
    pub conversation_id: String,
    /// claude 的 `task_type`：`local_bash` / `local_agent` / `remote_agent` / `local_workflow`。
    pub kind: String,
    pub description: String,
    /// `running` | `completed` | `failed` | `stopped`。
    pub status: String,
    /// 终态摘要（退出码文案 / 子代理最终回复）。
    pub summary: Option<String>,
    pub started_at: std::time::SystemTime,
    pub ended_at: Option<std::time::SystemTime>,
}

/// 单个 key 触发 failover 后的冷却时长。
pub const KEY_COOLDOWN: Duration = Duration::from_secs(60);
const EXTERNAL_DISCOVERY_CACHE_CAPACITY: usize = 64;
const EXTERNAL_DISCOVERY_CACHE_RETENTION: Duration = Duration::from_secs(300);

/// 共享的 TTL 缓存读取：每次访问全表清理过期项，命中时刷新 LRU 时间。
fn get_cached<V: Clone>(
    cache: &Mutex<HashMap<String, TimedCacheEntry<V>>>,
    key: &str,
    ttl: Duration,
) -> Option<V> {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, entry| entry.created_at.elapsed() <= ttl);
    let entry = cache.get_mut(key)?;
    entry.last_accessed = Instant::now();
    Some(entry.value.clone())
}

/// 共享的有界缓存写入：先清理历史项，再按 LRU 淘汰到容量以内。
fn set_cached<V>(
    cache: &Mutex<HashMap<String, TimedCacheEntry<V>>>,
    key: String,
    value: V,
    retention: Duration,
    capacity: usize,
) {
    let now = Instant::now();
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, entry| entry.created_at.elapsed() <= retention);
    cache.insert(
        key,
        TimedCacheEntry {
            created_at: now,
            last_accessed: now,
            value,
        },
    );
    while cache.len() > capacity.max(1) {
        let Some(oldest_key) = cache
            .iter()
            .min_by(|(key_a, entry_a), (key_b, entry_b)| {
                entry_a
                    .last_accessed
                    .cmp(&entry_b.last_accessed)
                    .then_with(|| entry_a.created_at.cmp(&entry_b.created_at))
                    .then_with(|| key_a.cmp(key_b))
            })
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&oldest_key);
    }
}

impl AppState {
    /// 集中构造点：`lib.rs::run` 的 `app.manage`、`new_headless`、以及测试用 `test_app_state`
    /// 三处唯一的差异只有 `settings` / `usage_dir` / `http` 与两个 OCR 客户端；其余字段全是
    /// 同样的空默认值。这里统一构造，三处只提供差异字段，避免同一份 ~40 行字面量重复三次。
    pub(crate) fn base(
        settings: Settings,
        usage_dir: PathBuf,
        http: Client,
        #[cfg(target_os = "macos")] macos_ocr: std::sync::Arc<MacOcrClient>,
        offline_models: std::sync::Arc<OfflineModelManager>,
        rapidocr: std::sync::Arc<RapidOcrClient>,
        inpainting: std::sync::Arc<InpaintingClient>,
    ) -> Self {
        let mcp_tool_snapshots = load_mcp_tool_snapshots(&usage_dir);
        AppState {
            settings: RwLock::new(settings),
            explain_images: Mutex::new(HashMap::new()),
            current_explain_image_id: Mutex::new(None),
            lens_busy: AtomicBool::new(false),
            lens_open_seq: AtomicU64::new(0),
            lens_opened_at: Mutex::new(None),
            prev_frontmost_pid_lens: AtomicI32::new(0),
            prev_frontmost_pid_main: AtomicI32::new(0),
            explain_stream_generation: AtomicU64::new(0),
            chat_stream_generation: AtomicU64::new(0),
            chat_active_generations: Mutex::new(HashMap::new()),
            chat_active_replies: Mutex::new(HashMap::new()),
            chat_protocol: Mutex::new(crate::chat::protocol::ChatProtocolHub::default()),
            pending_chat_tool_approvals: Mutex::new(HashMap::new()),
            chat_tool_always_allow: Mutex::new(HashSet::new()),
            chat_session_consent: Mutex::new(HashSet::new()),
            pending_chat_session_consents: Mutex::new(HashMap::new()),
            chat_consent_prompt_lock: tokio::sync::Mutex::new(()),
            pending_chat_user_prompts: Mutex::new(HashMap::new()),
            answered_ask_user_content: Mutex::new(HashMap::new()),
            pending_python_runs: Mutex::new(HashMap::new()),
            chat_create_conversation_lock: tokio::sync::Mutex::new(()),
            external_slash_commands_cache: Mutex::new(HashMap::new()),
            external_agent_models_cache: Mutex::new(HashMap::new()),
            external_detected_agents_cache: Mutex::new(HashMap::new()),
            availability_probe_lock: tokio::sync::Mutex::new(()),
            model_probe_locks: Mutex::new(HashMap::new()),
            pi_session_control_locks: Mutex::new(HashMap::new()),
            external_live_sessions: Mutex::new(HashMap::new()),
            pending_chat_external_sends: Mutex::new(Vec::new()),
            pending_chat_steering: Mutex::new(HashMap::new()),
            pending_chat_follow_up: Mutex::new(HashMap::new()),
            pending_selection: Mutex::new(None),
            lens_freeze_frame_image_id: Mutex::new(None),
            lens_pending_reset: Mutex::new(None),
            key_cooldowns: Mutex::new(HashMap::new()),
            active_key_idx: Mutex::new(HashMap::new()),
            prompt_cache_key_unsupported: Mutex::new(HashSet::new()),
            prompt_cache_retention_unsupported: Mutex::new(HashSet::new()),
            image_route_cache: Mutex::new(HashMap::new()),
            mcp_sessions: tokio::sync::Mutex::new(HashMap::new()),
            mcp_tool_snapshots: Mutex::new(mcp_tool_snapshots),
            usage_dir,
            http,
            http_direct: std::sync::OnceLock::new(),
            #[cfg(target_os = "macos")]
            macos_ocr,
            offline_models,
            rapidocr,
            inpainting,
            sub_agents: crate::chat::sub_agent::SubAgentManager::default(),
            background_commands: Arc::new(Mutex::new(HashMap::new())),
            external_background_tasks: Mutex::new(HashMap::new()),
            request_debug: Mutex::new(VecDeque::new()),
        }
    }

    /// Build a headless `AppState` for the `kivio-code` terminal agent — no
    /// `AppHandle`, no Tauri runtime. Differs from the live construction in
    /// `lib.rs::run` only in the two OCR clients (`headless()` constructors) and
    /// `usage_dir` (passed in). The agent loop only touches `settings`, the
    /// chat-generation state, session-consent set, `http`, and `usage_dir`; the
    /// rest are inert defaults kept for struct completeness.
    pub fn new_headless(settings: Settings, usage_dir: PathBuf) -> Self {
        let offline_models = OfflineModelManager::headless(crate::api::build_http_client());
        Self::base(
            settings,
            usage_dir,
            crate::api::build_http_client(),
            #[cfg(target_os = "macos")]
            MacOcrClient::headless(),
            offline_models.clone(),
            RapidOcrClient::headless(offline_models.clone()),
            InpaintingClient::new(offline_models),
        )
    }
    /// 该供应商应当使用的 HTTP 客户端。默认跟随系统代理（与加这个开关之前一致），
    /// 关掉时用忽略代理的直连客户端。
    pub fn client_for(&self, provider: &crate::settings::ModelProvider) -> &Client {
        if provider.request.use_system_proxy {
            &self.http
        } else {
            self.http_direct
                .get_or_init(crate::api::build_direct_http_client)
        }
    }

    /// 安全读取设置（锁中毒时返回内部数据，不 panic）
    pub fn settings_read(&self) -> std::sync::RwLockReadGuard<'_, Settings> {
        self.settings.read().unwrap_or_else(|e| e.into_inner())
    }
    /// 安全写入设置（锁中毒时返回内部数据，不 panic）
    pub fn settings_write(&self) -> std::sync::RwLockWriteGuard<'_, Settings> {
        self.settings.write().unwrap_or_else(|e| e.into_inner())
    }
    /// 开发者「请求调试」开关。关时 adapter 短路，不构造任何记录（零开销）。
    pub fn request_debug_enabled(&self) -> bool {
        self.settings_read().chat_tools.request_debug_enabled
    }
    /// 安全获取解释图片映射锁
    pub fn images_lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, PathBuf>> {
        self.explain_images
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
    /// 安全获取当前解释图片 ID 锁
    pub fn current_id_lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.current_explain_image_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
    /// 标记一次 Lens 浮窗会话开启：会话代号 +1 并记录开启时刻。
    /// 返回新代号，供强制关闭 watchdog 快照比对。
    pub fn mark_lens_opened(&self) -> u64 {
        let seq = self
            .lens_open_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        *self
            .lens_opened_at
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
        seq
    }
    /// Lens 是否处于"刚开启"宽限期内（窗口可能还没来得及可见）。
    /// busy 自愈逻辑在宽限期内不得清 busy，否则快速连按热键会并发双开。
    pub fn lens_open_in_grace(&self) -> bool {
        const LENS_OPEN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
        self.lens_opened_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|at| at.elapsed() < LENS_OPEN_GRACE)
            .unwrap_or(false)
    }

    /// 选择一个可用的 API Key 索引：
    /// 优先返回 active_key_idx 记录的 idx；若它在冷却中或已被试过，退回到下一个非冷却 idx；
    /// 全部冷却或 tried 已穷举时返回 None（调用方决定是否报错）。
    pub fn pick_active_key(
        &self,
        provider_id: &str,
        total: usize,
        tried: &HashSet<usize>,
    ) -> Option<usize> {
        if total == 0 {
            return None;
        }
        let now = Instant::now();
        let cooldowns = self.key_cooldowns.lock().unwrap_or_else(|e| e.into_inner());
        let active = self
            .active_key_idx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(provider_id)
            .copied()
            .unwrap_or(0)
            .min(total.saturating_sub(1));

        let in_cooldown = |idx: usize| {
            cooldowns
                .get(&(provider_id.to_string(), idx))
                .map(|until| *until > now)
                .unwrap_or(false)
        };

        // 1) 优先 active idx（未试过 + 未冷却）
        if !tried.contains(&active) && !in_cooldown(active) {
            return Some(active);
        }
        // 2) 从 active+1 开始环绕扫描
        for offset in 1..total {
            let idx = (active + offset) % total;
            if !tried.contains(&idx) && !in_cooldown(idx) {
                return Some(idx);
            }
        }
        // 3) 全部冷却 → 兜底找一个未试过的（无视冷却，避免完全无 key 可用）
        for offset in 0..total {
            let idx = (active + offset) % total;
            if !tried.contains(&idx) {
                return Some(idx);
            }
        }
        None
    }

    /// 为某个 Chat conversation 开启一轮新的可取消运行，返回本轮 generation。
    /// 分配一个进程内从未用过的 generation 号（全局单调递增），并登记到活跃集合。
    /// **不**作废同会话其它在跑 run —— 多模型一问多答时 N 条 run 各持自己的 generation 并存。
    pub fn next_chat_generation(&self, conversation_id: &str) -> u64 {
        let next = self.chat_stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.chat_active_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(conversation_id.to_string())
            .or_default()
            .insert(next);
        next
    }

    /// 取消指定 conversation 的**所有**当前 Chat 运行：清空其活跃 generation 集合，
    /// 使任何持旧 generation 的 run（含同会话并发的多模型 run）在下一个检查点判失效。
    /// 直接移除键：空集合与无键语义等价（is_active 都判 false），且 sub-agent 每次
    /// spawn 用一次性合成 conversation_id，留空集合会无界累积。
    pub fn cancel_chat_generation(&self, conversation_id: &str) {
        self.chat_active_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id);
    }

    /// 单条 run 自然结束时退役其 generation（不影响同会话其它在跑 run）。
    /// 单模型路径下集合恒只含本 run 的一个号，移除后即变空，与旧「cancel 推代号」等价。
    /// 集合变空时移除键，防止一次性 conversation_id（sub-agent 合成会话）留下空条目。
    pub fn end_chat_generation(&self, conversation_id: &str, generation: u64) {
        let mut map = self
            .chat_active_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(active) = map.get_mut(conversation_id) {
            active.remove(&generation);
            if active.is_empty() {
                map.remove(conversation_id);
            }
        }
    }

    /// 该会话是否已授予文件/命令工具的会话级授权。
    pub fn has_chat_consent(&self, conversation_id: &str) -> bool {
        self.chat_session_consent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(conversation_id)
    }

    /// 记录该会话已授予文件/命令工具的会话级授权(本进程内有效)。
    pub fn grant_chat_consent(&self, conversation_id: &str) {
        self.chat_session_consent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(conversation_id.to_string());
    }

    /// 该对话是否已对某个工具按下过「总是允许」。工具名统一小写后比较：内置 agent 报的是
    /// `write`，外部 CLI 报的是自己的原名（claude 的 `Write`），不归一化两边对不上。
    pub fn has_tool_always_allow(&self, conversation_id: &str, tool_name: &str) -> bool {
        self.chat_tool_always_allow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(conversation_id.to_string(), tool_name.to_ascii_lowercase()))
    }

    /// 记录「本对话内该工具不再询问」(本进程内有效)。
    pub fn grant_tool_always_allow(&self, conversation_id: &str, tool_name: &str) {
        self.chat_tool_always_allow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((conversation_id.to_string(), tool_name.to_ascii_lowercase()));
    }

    /// 判断指定 conversation 的某条 Chat 运行是否仍然有效（其 generation 仍在活跃集合内）。
    pub fn is_chat_generation_active(&self, conversation_id: &str, generation: u64) -> bool {
        self.chat_active_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(conversation_id)
            .map(|active| active.contains(&generation))
            .unwrap_or(false)
    }

    /// 用户在运行中插话：把消息放进该会话的 steering 信箱，等 `run_agent_loop` 轮首来取。
    /// 返回 false = 该会话当前没有活跃 run，调用方应改走普通发送（前端队列）。
    pub fn push_chat_steering(
        &self,
        conversation_id: &str,
        message: crate::chat::agent::SteeringMessage,
    ) -> bool {
        if self
            .chat_active_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(conversation_id)
            .map(|active| active.is_empty())
            .unwrap_or(true)
        {
            return false;
        }
        self.pending_chat_steering
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(conversation_id.to_string())
            .or_default()
            .push(message);
        true
    }

    /// 取走该会话待注入的插话（取一次清一次）。空集合时移除键，避免无界累积。
    pub fn take_chat_steering(
        &self,
        conversation_id: &str,
    ) -> Vec<crate::chat::agent::SteeringMessage> {
        self.pending_chat_steering
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id)
            .unwrap_or_default()
    }

    /// run 结束时丢掉没来得及消费的插话——否则它会漏进**下一条** run 的第一轮。
    /// 不丢消息：前端队列里那条要等卡片事件才出队，收不到就按普通消息重发。
    pub fn clear_chat_steering(&self, conversation_id: &str) {
        self.pending_chat_steering
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id);
    }

    /// 用户在运行中排 follow-up：放进终答后续跑的信箱。没有活跃 run 则 false。
    pub fn push_chat_follow_up(
        &self,
        conversation_id: &str,
        message: crate::chat::agent::SteeringMessage,
    ) -> bool {
        if self
            .chat_active_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(conversation_id)
            .map(|active| active.is_empty())
            .unwrap_or(true)
        {
            return false;
        }
        self.pending_chat_follow_up
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(conversation_id.to_string())
            .or_default()
            .push(message);
        true
    }

    pub fn take_chat_follow_up(
        &self,
        conversation_id: &str,
    ) -> Vec<crate::chat::agent::SteeringMessage> {
        self.pending_chat_follow_up
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id)
            .unwrap_or_default()
    }

    pub fn clear_chat_follow_up(&self, conversation_id: &str) {
        self.pending_chat_follow_up
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id);
    }

    /// 对话被删除时清理其按 conversation_id 累积的运行态痕迹：活跃 generation 集合、
    /// 会话级工具同意标记、按工具名的「总是允许」集合。三者都严格按 conversation_id 取键，对话删除后再不会被
    /// 引用，是最无歧义的有界清理点（不影响其它活跃对话）。generation 号本身来自进程级
    /// 全局计数器（不分桶），无需在此清理。
    pub fn forget_chat_conversation_runtime(&self, conversation_id: &str) {
        self.chat_active_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id);
        self.chat_session_consent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id);
        self.chat_tool_always_allow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(conv, _)| conv != conversation_id);
        self.clear_chat_steering(conversation_id);
        self.clear_chat_follow_up(conversation_id);
    }

    /// 尝试占用某个对话的某条 run 回复槽位。同会话允许多条 run 并存（多模型一问多答）；
    /// 仅当同一 (conversation_id, run_id) 已在进行中时返回 false（防同一 run 重复进入）。
    pub fn try_begin_chat_reply(&self, conversation_id: &str, run_id: &str) -> bool {
        let mut active = self
            .chat_active_replies
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let runs = active.entry(conversation_id.to_string()).or_default();
        if runs.contains(run_id) {
            return false;
        }
        runs.insert(run_id.to_string());
        true
    }

    /// 原子地「检查 busy + 占用一个哨兵槽位」：在同一把锁内，若该会话已有任意 run 在跑则返回
    /// false，否则注册 `run_id` 哨兵槽位并返回 true。命令入口用它替代「先 check 后 register」
    /// 的两步，关闭 busy 判定与槽位注册之间的 TOCTOU 窗口（防止同会话并发发送同时通过 busy
    /// 检查）。哨兵只占 `chat_active_replies`，不碰 `chat_active_generations`（不参与取消），
    /// 由命令退出时 `end_chat_reply` 释放。
    pub fn try_reserve_chat_send(&self, conversation_id: &str, run_id: &str) -> bool {
        let mut active = self
            .chat_active_replies
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let runs = active.entry(conversation_id.to_string()).or_default();
        if !runs.is_empty() {
            return false;
        }
        runs.insert(run_id.to_string());
        true
    }

    /// 该会话当前是否有任意一条 run 正在回复（用于「生成中拒绝新发送」的 busy 判定）。
    pub fn conversation_has_active_reply(&self, conversation_id: &str) -> bool {
        self.chat_active_replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(conversation_id)
            .map(|runs| !runs.is_empty())
            .unwrap_or(false)
    }

    /// 释放某个对话某条 run 的回复槽位（run 集合空了则移除该会话条目）。
    pub fn end_chat_reply(&self, conversation_id: &str, run_id: &str) {
        let mut active = self
            .chat_active_replies
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(runs) = active.get_mut(conversation_id) {
            runs.remove(run_id);
            if runs.is_empty() {
                active.remove(conversation_id);
            }
        }
    }

    pub fn get_mcp_tool_snapshot(
        &self,
        server_id: &str,
        config_fingerprint: &str,
    ) -> Option<Vec<McpTool>> {
        let snapshots = self
            .mcp_tool_snapshots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        snapshots
            .get(server_id)
            .filter(|snapshot| snapshot.config_fingerprint == config_fingerprint)
            .map(|snapshot| snapshot.tools.clone())
            .filter(|tools| !tools.is_empty())
    }

    pub fn set_mcp_tool_snapshot(
        &self,
        server_id: String,
        config_fingerprint: String,
        tools: Vec<McpTool>,
    ) {
        if tools.is_empty() {
            return;
        }
        let mut snapshots = self
            .mcp_tool_snapshots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        snapshots.insert(
            server_id,
            McpToolSnapshot {
                config_fingerprint,
                tools,
            },
        );
        let content = match serde_json::to_string_pretty(&*snapshots) {
            Ok(content) => content,
            Err(err) => {
                eprintln!("Failed to serialize MCP tool snapshots: {err}");
                return;
            }
        };
        let path = mcp_tool_snapshot_path(&self.usage_dir);
        if let Err(err) = crate::chat::storage::atomic_write(&path, &content, "MCP tool snapshots")
        {
            eprintln!("Failed to persist MCP tool snapshots: {err}");
        }
    }

    /// 斜杠命令缓存读取，TTL 随结果空/非空区分：非空命令列表用长 TTL（`full_ttl`），
    /// 空列表（CLI 不上报任何斜杠命令 / 探测超时降级为空）用短 TTL（`empty_ttl`）做**负缓存**，
    /// 避免切会话/切 agent 每次 useEffect 都重探（kimi 侧每探测一次即落一个空壳会话）。
    pub fn get_cached_external_slash_commands(
        &self,
        cache_key: &str,
        full_ttl: Duration,
        empty_ttl: Duration,
    ) -> Option<Vec<crate::external_agents::types::ExternalCliSlashCommand>> {
        let mut cache = self
            .external_slash_commands_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let max_ttl = full_ttl.max(empty_ttl);
        cache.retain(|_, entry| entry.created_at.elapsed() <= max_ttl);
        let entry = cache.get_mut(cache_key)?;
        let ttl = if entry.value.is_empty() {
            empty_ttl
        } else {
            full_ttl
        };
        if entry.created_at.elapsed() > ttl {
            return None;
        }
        entry.last_accessed = Instant::now();
        Some(entry.value.clone())
    }

    pub fn set_cached_external_slash_commands(
        &self,
        cache_key: String,
        commands: Vec<crate::external_agents::types::ExternalCliSlashCommand>,
    ) {
        set_cached(
            &self.external_slash_commands_cache,
            cache_key,
            commands,
            EXTERNAL_DISCOVERY_CACHE_RETENTION,
            EXTERNAL_DISCOVERY_CACHE_CAPACITY,
        );
    }

    /// 读模型探测缓存：按条目来源应用不同 TTL——probed 用 `probed_ttl`，fallback 用
    /// `fallback_ttl`（短负缓存）。任一超时视为未命中。
    pub fn get_cached_external_agent_models(
        &self,
        cache_key: &str,
        probed_ttl: Duration,
        fallback_ttl: Duration,
    ) -> Option<crate::external_agents::types::CachedAgentModels> {
        use crate::external_agents::types::ModelSource;
        let mut cache = self
            .external_agent_models_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let max_ttl = probed_ttl.max(fallback_ttl);
        cache.retain(|_, entry| entry.created_at.elapsed() <= max_ttl);
        let entry = cache.get_mut(cache_key)?;
        let ttl = match entry.value.source {
            ModelSource::Probed => probed_ttl,
            ModelSource::Fallback => fallback_ttl,
        };
        if entry.created_at.elapsed() > ttl {
            return None;
        }
        entry.last_accessed = Instant::now();
        Some(entry.value.clone())
    }

    pub fn set_cached_external_agent_models(
        &self,
        cache_key: String,
        models: crate::external_agents::types::CachedAgentModels,
    ) {
        set_cached(
            &self.external_agent_models_cache,
            cache_key,
            models,
            EXTERNAL_DISCOVERY_CACHE_RETENTION,
            EXTERNAL_DISCOVERY_CACHE_CAPACITY,
        );
    }

    /// 切换第三方供应商后作废该 agent 的模型探测缓存。key 形如 `agent:cwd`，
    /// 同一个 agent 在不同工作目录下各有一条，全部要清。
    pub fn clear_external_agent_models_cache(&self, agent_id: &str) {
        let prefix = format!("{agent_id}:");
        self.external_agent_models_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|key, _| !key.starts_with(&prefix));
    }

    /// 保存设置后整表作废（供应商可能变了，而设置保存不频繁，不值得逐 agent 比对）。
    pub fn clear_all_external_agent_models_cache(&self) {
        self.external_agent_models_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// 作废可用性缓存（含落盘快照之外的内存副本）。切供应商后版本/认证状态都可能变了。
    pub fn clear_detected_agents_cache(&self) {
        self.external_detected_agents_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn get_cached_detected_agents(
        &self,
        cache_key: &str,
        ttl: Duration,
    ) -> Option<Vec<crate::external_agents::types::DetectedAgent>> {
        get_cached(&self.external_detected_agents_cache, cache_key, ttl)
    }

    pub fn set_cached_detected_agents(
        &self,
        cache_key: String,
        agents: Vec<crate::external_agents::types::DetectedAgent>,
    ) {
        set_cached(
            &self.external_detected_agents_cache,
            cache_key,
            agents,
            EXTERNAL_DISCOVERY_CACHE_RETENTION,
            EXTERNAL_DISCOVERY_CACHE_CAPACITY,
        );
    }

    /// single-flight：取（或创建）某 (agent:cwd) key 的模型探测锁。持锁期间探测，
    /// 释放后并发者复查缓存即命中，避免同一目标并发重探。
    pub fn model_probe_lock_for(&self, key: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .model_probe_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        locks
            .entry(key.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub fn pi_session_control_lock_for(
        &self,
        conversation_id: &str,
    ) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .pi_session_control_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        locks
            .entry(conversation_id.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Phase 2: return the control channel of a reusable live session for this conversation
    /// (same agent + cwd + launch configuration, actor still alive). Removes a stale/mismatched
    /// entry as a side effect.
    ///
    /// `launch_config` 让「配置变了 ⇒ 换进程」不需要额外的控制通道：不匹配就当成不可复用，
    /// 丢弃条目（actor 收到通道关闭后自行关停子进程），调用方走连接分支并原生 resume
    /// ⇒ 新 flag 生效且上下文不丢（spec 第 8 条）。
    pub fn external_live_session_control(
        &self,
        conversation_id: &str,
        agent_id: &str,
        cwd: &str,
        launch_config: &crate::external_agents::session::live::LaunchConfig,
    ) -> Option<tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>>
    {
        let mut map = self
            .external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(session) = map.get_mut(conversation_id) {
            if session.is_reusable(agent_id, cwd, launch_config) {
                session.last_activity = Instant::now();
                session.turns_served = session.turns_served.saturating_add(1);
                return Some(session.control.clone());
            }
        }
        // Dropping the removed entry closes its control channel → the actor shuts the child down.
        map.remove(conversation_id);
        None
    }

    /// 把这条会话标成「有在飞轮次」，返回的 guard 落地时自动清掉。    ///
    /// 轮次开始后调一次即可（复用与新建两条路都走得到），清扫器与 LRU 在此期间会跳过它。
    /// 不存在（刚被回收）时返回 `None` —— 那种情况下这一轮自己持着 `control`，照样跑完。
    pub fn mark_external_live_session_busy(
        &self,
        conversation_id: &str,
    ) -> Option<crate::external_agents::session::live::TurnBusyGuard> {
        let map = self
            .external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.get(conversation_id).map(|session| {
            crate::external_agents::session::live::TurnBusyGuard::new(session.busy.clone())
        })
    }

    /// 控制面操作只在会话空闲时独占 busy 标志；不能覆盖正在生成的 guard。
    pub fn try_mark_external_live_session_busy(
        &self,
        conversation_id: &str,
    ) -> Result<crate::external_agents::session::live::TurnBusyGuard, String> {
        let map = self
            .external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let session = map
            .get(conversation_id)
            .ok_or_else(|| "external live session is unavailable".to_string())?;
        crate::external_agents::session::live::TurnBusyGuard::try_new(session.busy.clone())
            .ok_or_else(|| "Pi session is busy; wait for the current run to finish".to_string())
    }
    /// 取出该会话常驻 CLI 的控制通道（若有）。给「运行中插话」用：外部 CLI 那条路不走
    /// `pending_chat_steering` 信箱（那是内置 agent 循环的轮首注入），而是把命令直接送进
    /// 会话 actor，由各协议自己决定能不能注入。不存在常驻会话 = 这条对话没在跑 CLI。
    pub fn external_live_session_control_any(
        &self,
        conversation_id: &str,
    ) -> Option<tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>>
    {
        self.external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(conversation_id)
            .map(|session| session.control.clone())
    }

    /// 取出宣称 follow-up 能力的常驻会话控制通道，以及该 CLI 的图片 MIME 白名单。
    /// 没有常驻会话、或该协议不支持 follow-up，都回 `None`（前端按普通轮末发送）。
    pub fn external_follow_up_live_session(
        &self,
        conversation_id: &str,
    ) -> Option<(
        tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>,
        &'static [&'static str],
    )> {
        let map = self
            .external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let session = map.get(conversation_id)?;
        let def = crate::external_agents::registry::get_agent_def(&session.agent_id)?;
        if !def.supports_follow_up {
            return None;
        }
        Some((session.control.clone(), def.image_mime_whitelist))
    }

    /// 取出 Pi 常驻会话控制通道（session tree / fork / switch）。
    pub fn external_pi_live_session_control(
        &self,
        conversation_id: &str,
    ) -> Option<tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>>
    {
        self.external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(conversation_id)
            .filter(|session| session.agent_id == "pi")
            .map(|session| session.control.clone())
    }

    pub fn register_external_live_session(
        &self,
        conversation_id: String,
        session: crate::external_agents::session::live::LiveSession,
    ) {
        const IDLE_TTL: Duration = Duration::from_secs(600);
        const MAX_LIVE_SESSIONS: usize = 6;
        let mut map = self
            .external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Reclaim idle sessions (dropping each entry closes its actor + child) and any whose
        // actor already exited. 有在飞轮次的会话不参与回收（见 `LiveSession::busy`）。
        map.retain(|_, s| !s.is_idle(IDLE_TTL));
        // Bound concurrent live processes: evict least-recently-used until under the cap.
        // 同样跳过在飞的 —— 正在跑长轮次的那条恰好 `last_activity` 最旧，不排除就一定被它选中。
        while map.len() >= MAX_LIVE_SESSIONS {
            let Some(oldest) = map
                .iter()
                .filter(|(_, s)| !s.busy.load(std::sync::atomic::Ordering::Acquire))
                .min_by_key(|(_, s)| s.last_activity)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            map.remove(&oldest);
        }
        map.insert(conversation_id, session);
    }

    pub fn remove_external_live_session(&self, conversation_id: &str) {
        self.external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id);
    }

    pub fn move_external_live_session(
        &self,
        source_conversation_id: &str,
        destination_conversation_id: &str,
    ) -> Result<(), String> {
        let mut map = self
            .external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if map.contains_key(destination_conversation_id) {
            return Err("destination conversation already has a live session".to_string());
        }
        let mut session = map
            .remove(source_conversation_id)
            .ok_or_else(|| "source live session disappeared".to_string())?;
        session.last_activity = Instant::now();
        map.insert(destination_conversation_id.to_string(), session);
        Ok(())
    }
    /// Reclaim every idle/dead live session (e.g. from a periodic sweeper). Returns how many
    /// were dropped. Dropping each entry closes its actor + child process.
    pub fn sweep_idle_external_live_sessions(&self, idle_ttl: Duration) -> usize {
        let mut map = self
            .external_live_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let before = map.len();
        map.retain(|_, s| !s.is_idle(idle_ttl));
        before - map.len()
    }

    /// 关停所有活的外部 CLI 会话（退出钩子用），**等它们真的退出**。
    ///
    /// 只 `clear()` 掉 sender 是不够的：那只是让 actor 在**下一次被 poll 时**才走
    /// `close()`，而退出钩子后面运行时就随进程走了，actor 永远等不到那一次 poll。
    /// `kill_on_drop(true)` 同理不触发 —— `Child` 就在那个永不 drop 的 actor 栈帧里。
    /// 结果是每次退出留下最多 `MAX_LIVE_SESSIONS` 个 CLI 进程，各自还挂着自己拉起的
    /// MCP stdio 子进程（`kill_on_drop` 也只杀直接子进程，够不到孙子）。
    ///
    /// 所以这里显式发 `Close` 并等通道关闭；超时没退的按 pid 杀进程树。按 pid 杀是
    /// `LiveSession::child_pid` 那条「绝不按 pid 杀」规则的**唯一例外** —— 那条规则防的是
    /// 把进程从一个还活着的 actor 底下抽走，而进程退出时这个顾虑已经不存在了。
    pub async fn close_all_external_live_sessions(&self) {
        const PER_SESSION_CLOSE_TIMEOUT: Duration = Duration::from_millis(1_500);
        let sessions: Vec<crate::external_agents::session::live::LiveSession> = {
            let mut map = self
                .external_live_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            map.drain().map(|(_, session)| session).collect()
        };
        futures::future::join_all(sessions.into_iter().map(|session| async move {
            let _ = session
                .control
                .send(crate::external_agents::session::live::SessionCommand::Close)
                .await;
            if tokio::time::timeout(PER_SESSION_CLOSE_TIMEOUT, session.control.closed())
                .await
                .is_err()
            {
                if let Some(pid) = session.child_pid {
                    crate::native_tools::kill_process_group(pid);
                }
            }
        }))
        .await;
    }

    /// Shared handle to the background-command registry. Returned as a cloned
    /// `Arc` so a detached waiter task can update job status after the spawning
    /// stack frame is gone (background commands survive across turns).
    pub fn background_commands_handle(
        &self,
    ) -> Arc<Mutex<HashMap<String, crate::native_tools::BackgroundCommand>>> {
        Arc::clone(&self.background_commands)
    }

    /// upsert 一条外部 CLI 后台任务。started 帧建条目；终态帧只补 status/summary/ended_at，
    /// 不覆盖已知的 kind/description（notification 帧不带它们）。超额时先清最老的终态条目。
    pub fn upsert_external_background_task(
        &self,
        conversation_id: &str,
        task_id: &str,
        status: &str,
        kind: Option<&str>,
        description: Option<&str>,
        summary: Option<&str>,
    ) {
        const MAX_TRACKED_EXTERNAL_TASKS: usize = 64;
        let terminal = status != "running";
        let mut map = self
            .external_background_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(task_id) {
            entry.status = status.to_string();
            if let Some(kind) = kind {
                entry.kind = kind.to_string();
            }
            if let Some(description) = description {
                entry.description = description.to_string();
            }
            if summary.is_some() {
                entry.summary = summary.map(str::to_string);
            }
            if terminal && entry.ended_at.is_none() {
                entry.ended_at = Some(std::time::SystemTime::now());
            }
            return;
        }
        while map.len() >= MAX_TRACKED_EXTERNAL_TASKS {
            let oldest_terminal = map
                .iter()
                .filter(|(_, t)| t.status != "running")
                .min_by_key(|(_, t)| t.started_at)
                .map(|(id, _)| id.clone());
            match oldest_terminal {
                Some(id) => map.remove(&id),
                // 全在跑（几乎不可能）：不淘汰运行中的，接受超额。
                None => break,
            };
        }
        let now = std::time::SystemTime::now();
        map.insert(
            task_id.to_string(),
            ExternalBackgroundTask {
                task_id: task_id.to_string(),
                conversation_id: conversation_id.to_string(),
                kind: kind.unwrap_or("local_bash").to_string(),
                description: description.unwrap_or_default().to_string(),
                status: status.to_string(),
                summary: summary.map(str::to_string),
                started_at: now,
                ended_at: terminal.then_some(now),
            },
        );
    }

    /// Register a tracked background command. Reaps already-terminated entries
    /// opportunistically so the map does not grow unbounded across a long
    /// session, but keeps the most recent terminal jobs so `bash_output` can
    /// still return their final output + exit code right after they finish.
    pub fn register_background_command(&self, job: crate::native_tools::BackgroundCommand) {
        const MAX_TRACKED_BACKGROUND_COMMANDS: usize = 64;
        let mut map = self
            .background_commands
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Evict oldest terminal jobs first if we are over the cap; never evict a
        // still-running job (it owns a live process group).
        while map.len() >= MAX_TRACKED_BACKGROUND_COMMANDS {
            let oldest_terminal = map
                .values()
                .filter(|j| {
                    !matches!(
                        j.status,
                        crate::native_tools::BackgroundCommandStatus::Running
                    )
                })
                .min_by_key(|j| j.started_at)
                .map(|j| j.job_id.clone());
            match oldest_terminal {
                Some(id) => {
                    // Remove the evicted job's per-job log too, otherwise long
                    // sessions that churn >64 short-lived background commands
                    // leak one small (now-unreachable) log file per eviction.
                    if let Some(job) = map.remove(&id) {
                        let _ = std::fs::remove_file(&job.log_path);
                    }
                }
                // All remaining jobs are still running; stop evicting.
                None => break,
            }
        }
        map.insert(job.job_id.clone(), job);
    }

    /// Kill all tracked background command process groups and clear the registry
    /// (e.g. on app shutdown). Each running job's process group is SIGKILLed /
    /// taskkill'd; their log files are best-effort removed. Returns how many
    /// process groups were killed.
    pub fn kill_all_background_commands(&self) -> usize {
        let mut map = self
            .background_commands
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut killed = 0;
        for job in map.values_mut() {
            if matches!(
                job.status,
                crate::native_tools::BackgroundCommandStatus::Running
            ) {
                // Prefer signaling the owning waiter task (which still holds the
                // live Child) so the kill targets the live process group and
                // never a reaped/reused pid (TOCTOU). Fall back to a direct
                // process-group kill only when there is no waiter (seeded test
                // jobs).
                match job.kill_tx.take() {
                    Some(kill_tx) => {
                        let _ = kill_tx.send(());
                        killed += 1;
                    }
                    None => {
                        if let Some(pid) = job.pid {
                            crate::native_tools::kill_process_group(pid);
                            killed += 1;
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&job.log_path);
        }
        map.clear();
        killed
    }

    /// Kill the background jobs a single conversation started, and drop them from
    /// the registry. Used when deleting a conversation: a still-running dev server
    /// keeps its cwd inside `chat-workspaces/<id>`, and on Windows an in-use
    /// directory refuses to be removed — which used to abort the whole delete.
    /// Returns how many process groups were killed.
    ///
    /// Jobs with no `conversation_id` (test seeds / non-agent paths) are left
    /// alone: unowned is not the same as owned-by-this-conversation.
    pub fn kill_background_commands_for_conversation(&self, conversation_id: &str) -> usize {
        let mut map = self
            .background_commands
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let owned: Vec<String> = map
            .values()
            .filter(|job| job.conversation_id.as_deref() == Some(conversation_id))
            .map(|job| job.job_id.clone())
            .collect();
        let mut killed = 0;
        for job_id in &owned {
            let Some(job) = map.get_mut(job_id) else {
                continue;
            };
            if matches!(
                job.status,
                crate::native_tools::BackgroundCommandStatus::Running
            ) {
                // 同 kill_all：优先叫醒持有 Child 的 waiter，避免直接按已回收的 pid 杀
                // （pid 复用 TOCTOU）；没有 waiter 才回落到进程组直杀。
                match job.kill_tx.take() {
                    Some(kill_tx) => {
                        let _ = kill_tx.send(());
                        killed += 1;
                    }
                    None => {
                        if let Some(pid) = job.pid {
                            crate::native_tools::kill_process_group(pid);
                            killed += 1;
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&job.log_path);
            map.remove(job_id);
        }
        killed
    }

    /// 该 base_url 是否已被学习为"拒绝 prompt_cache_key"。
    pub fn prompt_cache_key_unsupported(&self, base_url: &str) -> bool {
        self.prompt_cache_key_unsupported
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(base_url)
    }

    /// 记住该 base_url 拒绝 prompt_cache_key（首次 400 后调用）。
    pub fn mark_prompt_cache_key_unsupported(&self, base_url: &str) {
        self.prompt_cache_key_unsupported
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(base_url.to_string());
    }

    /// 该 base_url 是否已被学习为"拒绝 prompt_cache_retention"。
    pub fn prompt_cache_retention_unsupported(&self, base_url: &str) -> bool {
        self.prompt_cache_retention_unsupported
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(base_url)
    }

    /// 记住该 base_url 拒绝 prompt_cache_retention（首次 400 后调用）。
    pub fn mark_prompt_cache_retention_unsupported(&self, base_url: &str) {
        self.prompt_cache_retention_unsupported
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(base_url.to_string());
    }

    /// 标记某个 key 失败：进入冷却 + 不变更 active_key_idx
    pub fn mark_key_failed(&self, provider_id: &str, idx: usize) {
        let mut cooldowns = self.key_cooldowns.lock().unwrap_or_else(|e| e.into_inner());
        cooldowns.insert(
            (provider_id.to_string(), idx),
            Instant::now() + KEY_COOLDOWN,
        );
    }

    /// 标记某个 key 成功：清除该 idx 的冷却 + 设为 active
    pub fn mark_key_ok(&self, provider_id: &str, idx: usize) {
        let mut cooldowns = self.key_cooldowns.lock().unwrap_or_else(|e| e.into_inner());
        cooldowns.remove(&(provider_id.to_string(), idx));
        drop(cooldowns);
        let mut active = self
            .active_key_idx
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        active.insert(provider_id.to_string(), idx);
    }
}

#[cfg(test)]
/// 构造一个最小可用的 AppState 用于单测（cooldown / MCP 连接池等）。
/// 不涉及网络，Client::new() 即可（不会发请求）。供 state / mcp::manager 测试复用。
pub(crate) fn test_app_state() -> AppState {
    let offline_models = OfflineModelManager::headless(Client::new());
    AppState::base(
        Settings::default(),
        std::env::temp_dir().join(format!("kivio-test-usage-{}", uuid::Uuid::new_v4())),
        Client::new(),
        #[cfg(target_os = "macos")]
        MacOcrClient::disabled(),
        offline_models.clone(),
        RapidOcrClient::new(offline_models.clone()),
        InpaintingClient::new(offline_models),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        test_app_state()
    }

    #[test]
    fn pick_active_key_returns_none_when_total_zero() {
        let st = test_state();
        assert_eq!(st.pick_active_key("p", 0, &HashSet::new()), None);
    }

    #[test]
    fn external_agent_detection_cache_is_scoped_by_cwd() {
        let st = test_state();
        st.set_cached_detected_agents("/project-a".to_string(), Vec::new());

        assert!(st
            .get_cached_detected_agents("/project-a", Duration::from_secs(60))
            .is_some());
        assert!(st
            .get_cached_detected_agents("/project-b", Duration::from_secs(60))
            .is_none());
    }

    #[test]
    fn model_probe_lock_is_shared_per_key_and_distinct_across_keys() {
        let st = test_state();
        let a1 = st.model_probe_lock_for("claude:/proj");
        let a2 = st.model_probe_lock_for("claude:/proj");
        let b = st.model_probe_lock_for("codex:/proj");
        // 同 key 复用同一把锁（single-flight 生效），不同 key 各自独立。
        assert!(std::sync::Arc::ptr_eq(&a1, &a2));
        assert!(!std::sync::Arc::ptr_eq(&a1, &b));
    }

    #[test]
    fn external_agent_models_cache_applies_source_aware_ttl() {
        use crate::external_agents::types::{CachedAgentModels, ModelSource, RuntimeModelOption};
        let st = test_state();
        let one = |id: &str| RuntimeModelOption {
            id: id.to_string(),
            label: id.to_string(),
            context_window_tokens: None,
        };

        // probed 条目在长 TTL 内命中，短 fallback TTL 不影响它。
        st.set_cached_external_agent_models(
            "claude:/p".to_string(),
            CachedAgentModels {
                models: vec![one("gpt-5")],
                source: ModelSource::Probed,
                reasoning_options: vec![],
                reasoning_by_model: Default::default(),
                current_model: None,
                current_reasoning: None,
            },
        );
        assert!(st
            .get_cached_external_agent_models("claude:/p", Duration::from_secs(300), Duration::ZERO)
            .is_some());

        // fallback 条目按短 TTL 裁定：TTL=0 立即视为过期（负缓存到点即重探）。
        st.set_cached_external_agent_models(
            "codex:/p".to_string(),
            CachedAgentModels {
                models: vec![one("default")],
                source: ModelSource::Fallback,
                reasoning_options: vec![],
                reasoning_by_model: Default::default(),
                current_model: None,
                current_reasoning: None,
            },
        );
        assert!(st
            .get_cached_external_agent_models("codex:/p", Duration::from_secs(300), Duration::ZERO)
            .is_none());
        // 同一 fallback 条目在足够长的 fallback TTL 内仍命中。
        st.set_cached_external_agent_models(
            "codex:/p".to_string(),
            CachedAgentModels {
                models: vec![one("default")],
                source: ModelSource::Fallback,
                reasoning_options: vec![],
                reasoning_by_model: Default::default(),
                current_model: None,
                current_reasoning: None,
            },
        );
        let hit = st.get_cached_external_agent_models(
            "codex:/p",
            Duration::from_secs(300),
            Duration::from_secs(30),
        );
        assert!(matches!(hit.map(|c| c.source), Some(ModelSource::Fallback)));
    }

    #[test]
    fn external_slash_commands_cache_negative_caches_empty_with_short_ttl() {
        use crate::external_agents::types::ExternalCliSlashCommand;
        let st = test_state();
        let cmd = |name: &str| ExternalCliSlashCommand {
            slash: format!("/{name}"),
            name: name.to_string(),
            description: None,
            argument_hint: None,
        };

        // 非空命令列表走长 TTL：短(empty) TTL=0 不影响它，仍命中。
        st.set_cached_external_slash_commands("kimi:/g".to_string(), vec![cmd("compact")]);
        assert!(st
            .get_cached_external_slash_commands("kimi:/g", Duration::from_secs(300), Duration::ZERO)
            .is_some());

        // 空列表（负缓存）按短 TTL 裁定：empty TTL=0 立即过期 → 到点重探。
        st.set_cached_external_slash_commands("grok:/g".to_string(), Vec::new());
        assert!(st
            .get_cached_external_slash_commands("grok:/g", Duration::from_secs(300), Duration::ZERO)
            .is_none());
        // 但空列表在足够长的 empty TTL 内仍命中（TTL 内不重探）。
        let hit = st.get_cached_external_slash_commands(
            "grok:/g",
            Duration::from_secs(300),
            Duration::from_secs(30),
        );
        assert!(matches!(hit, Some(ref v) if v.is_empty()));
    }

    #[test]
    fn bounded_cache_sweeps_all_expired_entries_on_read() {
        let cache = Mutex::new(HashMap::from([
            (
                "expired".to_string(),
                TimedCacheEntry {
                    created_at: Instant::now() - Duration::from_secs(10),
                    last_accessed: Instant::now() - Duration::from_secs(10),
                    value: 1u32,
                },
            ),
            (
                "fresh".to_string(),
                TimedCacheEntry {
                    created_at: Instant::now(),
                    last_accessed: Instant::now() - Duration::from_secs(1),
                    value: 2u32,
                },
            ),
        ]));

        assert_eq!(get_cached(&cache, "fresh", Duration::from_secs(5)), Some(2));
        let guard = cache.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert!(!guard.contains_key("expired"));
    }

    #[test]
    fn bounded_cache_evicts_least_recently_used_entry() {
        let cache = Mutex::new(HashMap::new());
        set_cached(&cache, "a".into(), 1u32, Duration::from_secs(60), 2);
        set_cached(&cache, "b".into(), 2u32, Duration::from_secs(60), 2);
        assert_eq!(get_cached(&cache, "a", Duration::from_secs(60)), Some(1));
        set_cached(&cache, "c".into(), 3u32, Duration::from_secs(60), 2);

        let guard = cache.lock().unwrap();
        assert!(guard.contains_key("a"));
        assert!(guard.contains_key("c"));
        assert!(!guard.contains_key("b"));
    }

    #[test]
    fn bounded_cache_update_replaces_value_and_refreshes_ttl() {
        let cache = Mutex::new(HashMap::new());
        set_cached(&cache, "same".into(), 1u32, Duration::from_secs(60), 2);
        {
            let mut guard = cache.lock().unwrap();
            let entry = guard.get_mut("same").unwrap();
            entry.created_at = Instant::now() - Duration::from_secs(10);
            entry.last_accessed = entry.created_at;
        }

        set_cached(&cache, "same".into(), 2u32, Duration::from_secs(5), 2);

        assert_eq!(get_cached(&cache, "same", Duration::from_secs(5)), Some(2));
        assert_eq!(cache.lock().unwrap().len(), 1);
    }

    #[test]
    fn bounded_cache_remains_within_capacity_under_concurrent_writes() {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let threads: Vec<_> = (0..8)
            .map(|worker| {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || {
                    for item in 0..32 {
                        set_cached(
                            &cache,
                            format!("{worker}:{item}"),
                            item,
                            Duration::from_secs(60),
                            16,
                        );
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(cache.lock().unwrap().len() <= 16);
    }

    fn sample_mcp_tool(name: &str) -> McpTool {
        McpTool {
            name: name.to_string(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({ "type": "object" }),
            output_schema: None,
            annotations: None,
        }
    }

    #[test]
    fn mcp_tool_snapshot_persists_across_state_restart() {
        let st = test_state();
        let usage_dir = st.usage_dir.clone();
        st.set_mcp_tool_snapshot("srv".into(), "fp-1".into(), vec![sample_mcp_tool("echo")]);

        // 内存命中
        let hit = st
            .get_mcp_tool_snapshot("srv", "fp-1")
            .expect("in-memory hit");
        assert_eq!(hit[0].name, "echo");

        // 模拟重启：新 AppState 从同一 usage_dir 灌入落盘快照
        let restarted = AppState::new_headless(Settings::default(), usage_dir.clone());
        let reloaded = restarted
            .get_mcp_tool_snapshot("srv", "fp-1")
            .expect("snapshot reloads from disk after restart");
        assert_eq!(reloaded[0].name, "echo");
        assert_eq!(reloaded[0].description, "echo tool");

        let _ = std::fs::remove_dir_all(&usage_dir);
    }

    #[test]
    fn mcp_tool_snapshot_fingerprint_mismatch_misses() {
        let st = test_state();
        let usage_dir = st.usage_dir.clone();
        st.set_mcp_tool_snapshot("srv".into(), "fp-old".into(), vec![sample_mcp_tool("echo")]);

        assert!(st.get_mcp_tool_snapshot("srv", "fp-new").is_none());
        // 落盘后重启同样不命中改配置的旧快照
        let restarted = AppState::new_headless(Settings::default(), usage_dir.clone());
        assert!(restarted.get_mcp_tool_snapshot("srv", "fp-new").is_none());
        assert!(restarted.get_mcp_tool_snapshot("srv", "fp-old").is_some());

        let _ = std::fs::remove_dir_all(&usage_dir);
    }

    #[test]
    fn mcp_tool_snapshot_ignores_corrupt_disk_file() {
        let usage_dir =
            std::env::temp_dir().join(format!("kivio-test-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&usage_dir).expect("create usage dir");
        std::fs::write(mcp_tool_snapshot_path(&usage_dir), "{ not json !!")
            .expect("write corrupt snapshot file");

        // 损坏文件 = 视为无缓存，不 panic
        let st = AppState::new_headless(Settings::default(), usage_dir.clone());
        assert!(st.get_mcp_tool_snapshot("srv", "fp").is_none());

        // 后续写入照常覆盖损坏文件
        st.set_mcp_tool_snapshot("srv".into(), "fp".into(), vec![sample_mcp_tool("echo")]);
        let restarted = AppState::new_headless(Settings::default(), usage_dir.clone());
        assert!(restarted.get_mcp_tool_snapshot("srv", "fp").is_some());

        let _ = std::fs::remove_dir_all(&usage_dir);
    }

    #[test]
    fn mcp_tool_snapshot_file_contains_no_secrets_and_empty_tools_are_not_stored() {
        let st = test_state();
        let usage_dir = st.usage_dir.clone();
        // 空工具列表不入缓存（也不落盘）
        st.set_mcp_tool_snapshot("empty".into(), "fp".into(), Vec::new());
        assert!(st.get_mcp_tool_snapshot("empty", "fp").is_none());

        st.set_mcp_tool_snapshot("srv".into(), "fp".into(), vec![sample_mcp_tool("echo")]);
        let raw = std::fs::read_to_string(mcp_tool_snapshot_path(&usage_dir))
            .expect("snapshot file exists");
        // 落盘内容只有工具 schema + 指纹哈希：不该出现 headers/env/token 之类的键
        assert!(raw.contains("config_fingerprint"));
        assert!(raw.contains("echo"));
        for secret_marker in ["Authorization", "Bearer", "headers", "api_key"] {
            assert!(
                !raw.contains(secret_marker),
                "snapshot file must not contain {secret_marker}: {raw}"
            );
        }

        let _ = std::fs::remove_dir_all(&usage_dir);
    }

    #[test]
    fn pick_active_key_starts_at_idx_zero_when_no_active_recorded() {
        let st = test_state();
        assert_eq!(st.pick_active_key("p", 3, &HashSet::new()), Some(0));
    }

    #[test]
    fn pick_active_key_prefers_last_known_good_idx() {
        let st = test_state();
        st.mark_key_ok("p", 2);
        assert_eq!(st.pick_active_key("p", 3, &HashSet::new()), Some(2));
    }

    #[test]
    fn pick_active_key_skips_tried_indices() {
        let st = test_state();
        let mut tried = HashSet::new();
        tried.insert(0);
        // active 是 0（没记录过 ok），但 0 已 tried → 应返回 1（环绕扫描下一个）
        assert_eq!(st.pick_active_key("p", 3, &tried), Some(1));
    }

    #[test]
    fn pick_active_key_skips_cooled_down_indices() {
        let st = test_state();
        st.mark_key_failed("p", 0); // 0 进入冷却
                                    // active 默认 0；0 在冷却 → 应跳到 1
        assert_eq!(st.pick_active_key("p", 3, &HashSet::new()), Some(1));
    }

    #[test]
    fn pick_active_key_falls_back_to_cooled_when_all_cooled_but_untried() {
        let st = test_state();
        // 三个 key 全部冷却
        st.mark_key_failed("p", 0);
        st.mark_key_failed("p", 1);
        st.mark_key_failed("p", 2);
        // 但都没试过 → 兜底返回某个 idx（不是 None），让用户至少有 key 用
        assert!(st.pick_active_key("p", 3, &HashSet::new()).is_some());
    }

    #[test]
    fn pick_active_key_returns_none_when_all_tried() {
        let st = test_state();
        let mut tried = HashSet::new();
        tried.insert(0);
        tried.insert(1);
        tried.insert(2);
        assert_eq!(st.pick_active_key("p", 3, &tried), None);
    }

    #[test]
    fn mark_key_ok_clears_cooldown() {
        let st = test_state();
        st.mark_key_failed("p", 0);
        // 此时 0 在冷却
        assert_ne!(st.pick_active_key("p", 2, &HashSet::new()), Some(0));
        // 标记成功后冷却被清除 + active 设为 0
        st.mark_key_ok("p", 0);
        assert_eq!(st.pick_active_key("p", 2, &HashSet::new()), Some(0));
    }

    #[test]
    fn cooldowns_are_per_provider() {
        let st = test_state();
        st.mark_key_failed("p1", 0);
        // p1 idx 0 冷却不影响 p2 idx 0
        assert_eq!(st.pick_active_key("p2", 2, &HashSet::new()), Some(0));
    }

    #[test]
    fn pick_active_key_handles_active_idx_out_of_bounds() {
        // 用户原来有 5 个 key，active=4；删了 3 个，现在 total=2
        // pick_active_key 应该 clamp 到 total-1，不 panic
        let st = test_state();
        st.mark_key_ok("p", 4);
        let result = st.pick_active_key("p", 2, &HashSet::new());
        assert!(result.is_some());
        assert!(result.unwrap() < 2);
    }

    #[test]
    fn chat_session_consent_is_per_conversation() {
        let st = test_state();
        assert!(!st.has_chat_consent("conv-1"));
        st.grant_chat_consent("conv-1");
        assert!(st.has_chat_consent("conv-1"));
        // Consent is scoped to a single conversation, not global.
        assert!(!st.has_chat_consent("conv-2"));
    }

    #[test]
    fn chat_tool_always_allow_is_per_conversation_and_tool() {
        let st = test_state();
        assert!(!st.has_tool_always_allow("conv-1", "write"));
        st.grant_tool_always_allow("conv-1", "write");
        assert!(st.has_tool_always_allow("conv-1", "write"));
        // 外部 CLI 报 PascalCase，必须命中同一条。
        assert!(st.has_tool_always_allow("conv-1", "Write"));
        // 只放行按下的那个工具，不是整会话放行。
        assert!(!st.has_tool_always_allow("conv-1", "read"));
        // 不跨对话。
        assert!(!st.has_tool_always_allow("conv-2", "write"));
        st.forget_chat_conversation_runtime("conv-1");
        assert!(!st.has_tool_always_allow("conv-1", "write"));
    }

    // --- 多模型一问多答：并发护栏 per-run 化（任务 06-30 步骤 1） ---

    #[test]
    fn single_run_generation_equivalence() {
        // 单 run（单模型）行为必须与改前等价：分配 → 活跃 → 取消 → 失活。
        let st = test_state();
        let gen = st.next_chat_generation("conv");
        assert!(st.is_chat_generation_active("conv", gen));
        st.cancel_chat_generation("conv");
        assert!(!st.is_chat_generation_active("conv", gen));
    }

    #[test]
    fn single_run_end_generation_retires_only_self() {
        let st = test_state();
        let gen = st.next_chat_generation("conv");
        assert!(st.is_chat_generation_active("conv", gen));
        st.end_chat_generation("conv", gen);
        assert!(!st.is_chat_generation_active("conv", gen));
    }

    #[test]
    fn new_run_does_not_invalidate_sibling_run() {
        // 同会话开第二条 run（多模型并发）不得作废第一条。
        let st = test_state();
        let gen_a = st.next_chat_generation("conv");
        let gen_b = st.next_chat_generation("conv");
        assert_ne!(gen_a, gen_b);
        assert!(st.is_chat_generation_active("conv", gen_a));
        assert!(st.is_chat_generation_active("conv", gen_b));
    }

    #[test]
    fn cancel_kills_all_runs_in_conversation() {
        // R4：cancel 一刀切该会话所有在跑 run。
        let st = test_state();
        let gen_a = st.next_chat_generation("conv");
        let gen_b = st.next_chat_generation("conv");
        let gen_c = st.next_chat_generation("conv");
        st.cancel_chat_generation("conv");
        assert!(!st.is_chat_generation_active("conv", gen_a));
        assert!(!st.is_chat_generation_active("conv", gen_b));
        assert!(!st.is_chat_generation_active("conv", gen_c));
    }

    #[test]
    fn cancel_is_per_conversation() {
        // 取消 conv-1 不影响 conv-2（含 sub-agent 用独立合成 conversation_id 的级联语义）。
        let st = test_state();
        let gen1 = st.next_chat_generation("conv-1");
        let gen2 = st.next_chat_generation("conv-2");
        st.cancel_chat_generation("conv-1");
        assert!(!st.is_chat_generation_active("conv-1", gen1));
        assert!(st.is_chat_generation_active("conv-2", gen2));
    }

    #[test]
    fn end_one_run_keeps_sibling_active() {
        let st = test_state();
        let gen_a = st.next_chat_generation("conv");
        let gen_b = st.next_chat_generation("conv");
        st.end_chat_generation("conv", gen_a);
        assert!(!st.is_chat_generation_active("conv", gen_a));
        assert!(st.is_chat_generation_active("conv", gen_b));
    }

    #[test]
    fn reply_slot_allows_multiple_runs_same_conversation() {
        // 同会话允许多条 run 并存；同一 (conv, run) 重复进入才拒绝。
        let st = test_state();
        assert!(!st.conversation_has_active_reply("conv"));
        assert!(st.try_begin_chat_reply("conv", "run-1"));
        assert!(st.try_begin_chat_reply("conv", "run-2"));
        // 同一 run 重复注册被拒。
        assert!(!st.try_begin_chat_reply("conv", "run-1"));
        assert!(st.conversation_has_active_reply("conv"));
    }

    #[test]
    fn reply_slot_release_is_per_run() {
        let st = test_state();
        st.try_begin_chat_reply("conv", "run-1");
        st.try_begin_chat_reply("conv", "run-2");
        st.end_chat_reply("conv", "run-1");
        // 仍有 run-2 在跑 → 会话仍 busy。
        assert!(st.conversation_has_active_reply("conv"));
        st.end_chat_reply("conv", "run-2");
        // 全部释放 → 会话不再 busy，且可重新注册同名 run。
        assert!(!st.conversation_has_active_reply("conv"));
        assert!(st.try_begin_chat_reply("conv", "run-1"));
    }

    #[test]
    fn forget_conversation_clears_active_generations() {
        let st = test_state();
        let gen = st.next_chat_generation("conv");
        assert!(st.is_chat_generation_active("conv", gen));
        st.forget_chat_conversation_runtime("conv");
        assert!(!st.is_chat_generation_active("conv", gen));
    }

    #[test]
    fn reserve_send_is_atomic_busy_check_and_reserve() {
        // 命令入口哨兵：首个预留成功并占槽；同会话第二个预留（哨兵或真实 run 在跑）被拒。
        let st = test_state();
        assert!(st.try_reserve_chat_send("conv", "send-1"));
        assert!(st.conversation_has_active_reply("conv"));
        // 任意第二个预留在哨兵存活期间被拒（关闭并发发送的 TOCTOU）。
        assert!(!st.try_reserve_chat_send("conv", "send-2"));
        // 哨兵存活期间，真实 per-run 槽位仍可与之共存（fan-out 各臂注册自己的 run）。
        assert!(st.try_begin_chat_reply("conv", "run-arm-1"));
        assert!(st.try_begin_chat_reply("conv", "run-arm-2"));
        // 释放哨兵后仍有 run 在跑 → 仍 busy；新预留仍被拒。
        st.end_chat_reply("conv", "send-1");
        assert!(st.conversation_has_active_reply("conv"));
        assert!(!st.try_reserve_chat_send("conv", "send-3"));
        // 全部 run 释放后才能再次预留。
        st.end_chat_reply("conv", "run-arm-1");
        st.end_chat_reply("conv", "run-arm-2");
        assert!(!st.conversation_has_active_reply("conv"));
        assert!(st.try_reserve_chat_send("conv", "send-4"));
    }
}
