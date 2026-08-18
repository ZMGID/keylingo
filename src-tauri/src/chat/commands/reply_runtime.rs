use uuid::Uuid;

use crate::settings::Settings;
use crate::state::AppState;

use super::ChatMessage;

pub(super) const CHAT_REPLY_BUSY_ERROR: &str = "该对话正在生成中，请稍后再试";
/// 多模型一问多答的并排上限（决策 D4）。超过此数不允许发送。
pub(super) const MAX_REPLY_MODELS: usize = 4;

/// 由会话级 `reply_models` 解析出本次发送要 fan-out 的「臂」列表。
/// 返回去重后（按 provider_id+model）、保序的 `(provider_id, model)`。
/// - 0 或 1 个有效臂 → 返回长度 ≤1（调用方走单模型现状路径，行为不变）。
/// - ≥2 个 → 多模型 fan-out。
/// 校验：上限 `MAX_REPLY_MODELS`（超出 `Err`）；provider 必须存在（不存在的臂跳过）；
/// 空 model 跳过。
pub(super) fn resolve_reply_arms(
    settings: &Settings,
    reply_models: &[crate::chat::ModelRef],
) -> Result<Vec<(String, String)>, String> {
    if reply_models.len() > MAX_REPLY_MODELS {
        return Err(format!(
            "多模型并行回答最多同时选择 {MAX_REPLY_MODELS} 个模型（当前 {}）。",
            reply_models.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut arms = Vec::new();
    for model_ref in reply_models {
        let provider_id = model_ref.provider_id.trim();
        let model = model_ref.model.trim();
        if provider_id.is_empty() || model.is_empty() {
            continue;
        }
        if settings.get_provider(provider_id).is_none() {
            continue;
        }
        let key = format!("{provider_id}\u{0}{model}");
        if seen.insert(key) {
            arms.push((provider_id.to_string(), model.to_string()));
        }
    }
    Ok(arms)
}

/// 命令入口的哨兵预留守卫：原子地「busy 检查 + 占一个哨兵槽位」，关闭 busy 判定与真实
/// per-run 槽位注册之间的 TOCTOU 窗口（防同会话并发发送同时通过 busy 检查）。哨兵槽位只占
/// `chat_active_replies`、不参与 generation/取消，命令任意退出路径 drop 时释放。
/// 真实 per-run 槽位（`ChatReplyGuard`）在哨兵存活期间额外注册，二者按不同 run_id 共存。
pub(super) struct ChatSendReservation<'a> {
    state: &'a AppState,
    conversation_id: String,
    run_id: String,
}

impl<'a> ChatSendReservation<'a> {
    /// 尝试预留某会话的发送哨兵。返回 None 表示该会话已有 run 在跑（busy）。
    pub(super) fn try_acquire(state: &'a AppState, conversation_id: &str) -> Option<Self> {
        let run_id = format!("chat-send-reservation-{}", Uuid::new_v4());
        if !state.try_reserve_chat_send(conversation_id, &run_id) {
            return None;
        }
        Some(Self {
            state,
            conversation_id: conversation_id.to_string(),
            run_id,
        })
    }
}

impl Drop for ChatSendReservation<'_> {
    fn drop(&mut self) {
        self.state
            .end_chat_reply(&self.conversation_id, &self.run_id);
    }
}

/// RAII 守卫：占住某条 run 的回复槽位与活跃 generation，函数任意退出路径都释放。
/// 同一会话允许多条 run 并存（多模型一问多答），每条 run 各持一个守卫。
pub(super) struct ChatReplyGuard<'a> {
    state: &'a AppState,
    conversation_id: String,
    run_id: String,
    generation: u64,
}

impl<'a> ChatReplyGuard<'a> {
    /// 注册一条 run 的回复槽位。返回 None 表示同一 (conversation_id, run_id) 已在进行中。
    /// `generation` 一并登记，drop 时随槽位一起退役（不影响同会话其它在跑 run）。
    pub(super) fn try_new(
        state: &'a AppState,
        conversation_id: &str,
        run_id: &str,
        generation: u64,
    ) -> Option<Self> {
        if !state.try_begin_chat_reply(conversation_id, run_id) {
            return None;
        }
        Some(Self {
            state,
            conversation_id: conversation_id.to_string(),
            run_id: run_id.to_string(),
            generation,
        })
    }
}

impl Drop for ChatReplyGuard<'_> {
    fn drop(&mut self) {
        self.state
            .end_chat_reply(&self.conversation_id, &self.run_id);
        self.state
            .end_chat_generation(&self.conversation_id, self.generation);
        // 没来得及消费的插话不能留给**下一条** run（用户是在对这一轮说话）。
        // 丢在这里不等于丢消息：前端队列里那条要等 `user_steer` 卡才出队，收不到就按普通消息重发。
        self.state.clear_chat_steering(&self.conversation_id);
        self.state.clear_chat_follow_up(&self.conversation_id);
    }
}

/// 多模型一问多答（任务 06-30）单条「臂」的覆盖配置。`complete_assistant_reply`
/// 收到 `Some(arm)` 时：用该臂自己的 provider/model（而非会话级），把 `group_id`/
/// provider/model 写进 assistant 消息，**自动批准工具**（避免 N 个并发 run 各弹一次审批），
/// 并且 **不直接落盘**——产出的 assistant `ChatMessage` 由协调者（`chat_send_message`）回收后
/// 统一 upsert + 一次性 save，避开 N 条并发 run 同写一个 `conversations/{id}.json` 的竞态。
/// 单模型路径传 `None`，行为与改造前完全一致。
pub(super) struct ReplyArm {
    pub(super) group_id: String,
    pub(super) group_size: usize,
    pub(super) arm_index: usize,
    pub(super) provider_id: String,
    pub(super) model: String,
}

/// 多模型臂运行后回收的结果。协调者据此把 assistant 消息合并进真正的会话并落盘。
/// 单模型路径（`arm = None`）`message` 为 None（已在函数内自行落盘）。
pub(super) struct ArmReplyOutcome {
    pub(super) message: Option<ChatMessage>,
    pub(super) run_id: Option<String>,
    pub(super) error: Option<String>,
}
