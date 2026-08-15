use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use chrono::Local;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::chat::agent::AgentRunEntry;
use crate::chat::commands::{
    emit_chat_stream_delta, emit_chat_tool_record, push_assistant_message,
};
use crate::chat::memory::l1_prompt_block;
use crate::chat::model::ModelUsage;
use crate::chat::types::{
    AgentTodoState, ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase,
    CompactionBoundaryRecord, ToolCallRecord, ToolCallStatus,
};
use crate::chat::Conversation;
use crate::external_agents::defs::claude::append_system_prompt_file_args;
use crate::external_agents::prompt::{
    compose_external_prompt, compose_external_prompt_passthrough, cwd_hint,
    instructions_via_launch_flag, is_cli_slash_input,
};
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::session::acp::AcpMcpServer;
use crate::external_agents::session::live::LaunchConfig;
use crate::external_agents::session::pi_rpc::run_pi_rpc_session;
use crate::external_agents::session::{
    persist_delivered_session, resolve_agent_resume_context, stable_prompt_hash,
};
use crate::external_agents::skill_stage::{skill_cwd_alias_segment, stage_active_skill};
use crate::external_agents::slash::{self};
use crate::external_agents::spawn::{
    drain_stderr, kill_agent_process_tree, resolve_binary, spawn_agent, tail_chars,
};
use crate::external_agents::types::{
    RuntimeBuildOptions, RuntimeContext, StreamFormat, UnifiedAgentEvent,
};
use crate::external_agents::workspace::{ensure_effective_cwd, extra_allowed_dirs_for_agent};
use crate::skills::read_skill_detail;
use crate::state::AppState;

/// Emitted (as a leading text banner) when a persistent-session turn expected to resume a native
/// session but had to reconnect fresh — the CLI's prior context is gone (R4 "resume 失败降级：
/// 提示上下文已丢失而非静默重放"). Rendered as a markdown blockquote so it reads as a system notice
/// and stays visually separate from the answer. TextDelta is chosen over Raw because Raw is only
/// surfaced when the turn produces no other output (see `apply_unified_event`), which would make
/// the notice silently vanish on the common case where the fresh turn does answer — defeating the
/// "不静默" goal. This uses the existing TextDelta variant, so no event/payload shape changes.
const CONTEXT_RESET_NOTICE: &str =
    "> ⚠️ 会话上下文已重置：原生会话无法恢复，本轮之前的对话历史对该 CLI 不可见。\n\n";

fn context_reset_notice_event() -> UnifiedAgentEvent {
    UnifiedAgentEvent::TextDelta {
        delta: CONTEXT_RESET_NOTICE.to_string(),
    }
}

/// 系统提示落盘文件的前缀。启动 GC 也认这个前缀（`screenshot::cleanup_orphan_temp_files`），
/// 崩溃留下的残渣 24h 后被回收。
const SYSTEM_PROMPT_FILE_PREFIX: &str = "kivio-extsys-";

/// 把会话级系统指令（用户系统提示 + Memory + cwd 提示）写到一个文件，供 CLI 用
/// `--append-system-prompt-file` 读取（A1）。
///
/// **为什么是 file 而不是内联字符串**：Windows 命令行有 32767 字符上限，而含 Memory 块的
/// instructions 可能超；npm 安装的用户拿到的是 `claude.cmd`，长参数在批处理转义那层还有风险。
///
/// 路径按 conversation_id 固定 ⇒ **每轮覆写同一个文件**，不会随轮次累积（每个会话最多一个）。
fn write_system_prompt_file(conversation_id: &str, instructions: &str) -> Result<PathBuf, String> {
    // conversation_id 来自内部 uuid，但仍做一次保守净化——它要拼进文件名。
    let safe_id: String = conversation_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let path = std::env::temp_dir().join(format!("{SYSTEM_PROMPT_FILE_PREFIX}{safe_id}.md"));
    std::fs::write(&path, instructions).map_err(|e| format!("write system prompt file: {e}"))?;
    Ok(path)
}

pub async fn run_external_cli_slash_command(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation: &mut Conversation,
    slash_command: &str,
) -> Result<(), String> {
    if !is_cli_slash_input(slash_command) {
        return Err("外部 CLI slash 命令必须以 / 开头".to_string());
    }
    run_external_cli_reply(
        app,
        state,
        conversation,
        None,
        slash_command,
        &[],
        &[],
        None,
        AgentRunEntry::Send,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_external_cli_reply(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation: &mut Conversation,
    title_from_first_user: Option<&str>,
    latest_user_message: &str,
    image_paths: &[std::path::PathBuf],
    file_paths: &[std::path::PathBuf],
    active_skill_id: Option<&str>,
    entry: AgentRunEntry,
) -> Result<(), String> {
    let settings = state.settings_read().clone();
    let agent_id = conversation
        .agent_runtime
        .external_agent_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "未选择外部 Agent".to_string())?;

    let def = get_agent_def(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;

    // CLI 要在这个目录里真的跑起来 ⇒ 必须确保存在（唯一需要建目录的路径之一）。
    let cwd = ensure_effective_cwd(app, &conversation.id, conversation.project_id.as_deref())?;
    // N2：回复路径不再跑完整检测（version/auth/模型探测可达 10-25s）。可用性/auth 的展示
    // 交给列表阶段；这里只解析二进制（唯一必需项），把第 2+ 轮的前置开销压到 <500ms。
    let probe_start = Instant::now();
    let resolved_bin = resolve_binary(def)
        .await
        .ok_or_else(|| format!("{} 未安装或不可用，请确认 CLI 在 PATH 中。", def.name))?;
    // 计时日志仅 debug 构建输出（供 <500ms 验收测量），release 不刷 stderr。
    if cfg!(debug_assertions) {
        eprintln!(
            "[external-agent] {} 前置二进制解析耗时 {}ms",
            def.id,
            probe_start.elapsed().as_millis()
        );
    }

    let is_slash = is_cli_slash_input(latest_user_message);

    let skill_detail = if is_slash {
        None
    } else if let Some(skill_id) = active_skill_id.filter(|s| !s.is_empty()) {
        read_skill_detail(app, &settings.chat_tools.skill_scan_paths, skill_id).ok()
    } else {
        None
    };

    let memory_body = if is_slash || !settings.chat_memory.enabled {
        String::new()
    } else {
        l1_prompt_block(app).unwrap_or(None).unwrap_or_default()
    };

    let mut daemon_instructions = String::new();
    if !is_slash {
        if !settings.chat.system_prompt.trim().is_empty() {
            daemon_instructions.push_str(settings.chat.system_prompt.trim());
            daemon_instructions.push_str("\n\n");
        }
        if !memory_body.trim().is_empty() {
            daemon_instructions.push_str("## Memory\n\n");
            daemon_instructions.push_str(memory_body.trim());
            daemon_instructions.push('\n');
        }
    }
    daemon_instructions.push_str(&cwd_hint(cwd.to_string_lossy().as_ref()));

    // A1：部分 CLI（目前只有 claude）的系统指令走**启动 flag** 而不是 prompt 正文。
    //
    // 为什么改：塞进正文的那条消息会被 CLI 自己的上下文压缩摘要掉甚至丢弃，而
    // `skip_instructions`（内容没变就不重发）保证了**永远不会补发** ⇒ 长会话跑一阵子后
    // 用户配置的系统提示与 Memory 静默失效，没有任何可观测信号。
    // 启动 flag 每次进程启动都重新注入，与对话历史无关，压缩影响不到。
    let instructions_via_flag = instructions_via_launch_flag(def.id);
    let system_prompt_file = if instructions_via_flag && !is_slash {
        match write_system_prompt_file(&conversation.id, daemon_instructions.trim()) {
            Ok(path) => Some(path),
            Err(err) => {
                // 写不出文件不该让整轮失败：退回正文注入（旧行为）而不是没有系统提示。
                eprintln!("[external-agent] {err}，本轮退回正文注入");
                None
            }
        }
    } else {
        None
    };

    let resume_ctx = resolve_agent_resume_context(
        app,
        &conversation.id,
        def.id,
        def.resumes_session_via_cli,
        &daemon_instructions,
        conversation.agent_runtime.external_model.as_deref(),
    );

    let skill_dir = skill_detail.as_ref().and_then(|d| d.meta.path.clone());
    let skill_body = skill_detail.as_ref().map(|d| d.body.clone());
    let skill_folder = skill_dir.as_deref().map(skill_cwd_alias_segment);

    if !is_slash {
        if let (Some(dir), Some(folder)) = (skill_dir.as_deref(), skill_folder.as_deref()) {
            let _ = stage_active_skill(&cwd, folder, std::path::Path::new(dir));
        }
    }

    let composed = if is_slash {
        compose_external_prompt_passthrough(latest_user_message)
    } else {
        compose_external_prompt(
            // 走启动 flag 的 CLI 不再把 instructions 拼进正文（否则同一份内容发两遍）。
            // 其余 8 个 CLI 仍是正文注入 + `skip_instructions` 去重（spec 第 1 条）。
            if system_prompt_file.is_some() {
                ""
            } else {
                &daemon_instructions
            },
            skill_body.as_deref(),
            skill_dir.as_deref(),
            skill_folder.as_deref(),
            resume_ctx.skip_instructions,
            latest_user_message,
        )
    };
    let mut composed = composed;

    // 附件（slash 命令不带附件，保持 passthrough 语义）。图片：支持原生图片块的协议按白名单
    // 加载为 base64 块，其余（不支持 / 超白名单 / 读失败）降级为路径文本；文件：一律路径说明块。
    let (image_blocks, degraded_image_paths): (
        Vec<crate::external_agents::attachments::ImageBlock>,
        Vec<std::path::PathBuf>,
    ) = if is_slash {
        (Vec::new(), Vec::new())
    } else if def.supports_native_image {
        crate::external_agents::attachments::load_image_blocks(
            image_paths,
            def.image_mime_whitelist,
        )
    } else {
        (Vec::new(), image_paths.to_vec())
    };
    if !is_slash {
        composed
            .full_prompt
            .push_str(&crate::external_agents::attachments::image_paths_note(
                &degraded_image_paths,
            ));
        composed
            .full_prompt
            .push_str(&crate::external_agents::attachments::file_attachments_note(
                file_paths,
            ));
    }

    let mut extra_dirs = extra_allowed_dirs_for_agent(def, &settings.chat_tools.skill_scan_paths);
    // 降级图片 / 文件需要 CLI 自己从磁盘读 → 把本会话附件目录加进 allowed-dir。
    if !is_slash && (!degraded_image_paths.is_empty() || !file_paths.is_empty()) {
        if let Ok(dir) = crate::chat::storage::conversation_attachments_dir(app, &conversation.id) {
            extra_dirs.push(dir.to_string_lossy().to_string());
        }
    }
    let runtime_ctx = RuntimeContext {
        extra_allowed_dirs: extra_dirs,
        resume_session_id: resume_ctx.resume_session_id.clone(),
        new_session_id: resume_ctx.new_session_id.clone(),
        include_partial_messages: true,
    };

    // Claude: external_model is catalog id (picker); wire value is settings-mapped runtime.
    // Resolve once at the boundary so launch argv and mid-session set_model share one id space.
    let wire_model = if agent_id == "claude" {
        crate::external_agents::session::claude_init::claude_wire_model(
            conversation.agent_runtime.external_model.as_deref(),
        )
    } else {
        conversation
            .agent_runtime
            .external_model
            .clone()
            .filter(|m| !m.is_empty() && m != "default")
    };
    let build_options = RuntimeBuildOptions {
        model: wire_model.clone(),
        reasoning: conversation.agent_runtime.external_reasoning.clone(),
        sandbox: conversation.agent_runtime.external_sandbox.clone(),
    };

    if let Some(max_bytes) = def.max_prompt_arg_bytes {
        if composed.full_prompt.len() > max_bytes {
            return Err(format!(
                "Prompt 过长（{} 字节），超过 {} 的上限（{} 字节）。请缩短消息或改用 stdin 模式的 Agent。",
                composed.full_prompt.len(),
                def.name,
                max_bytes
            ));
        }
    }

    let prompt_for_args = if def.prompt_via_stdin {
        None
    } else {
        Some(composed.full_prompt.as_str())
    };
    let args = (def.build_args)(&runtime_ctx, &build_options, prompt_for_args);
    // A1：系统指令以 flag 追加（不改 `build_args` 的形状，也不动 `RuntimeContext` ——
    // 那两处是所有 CLI 共用的，为一个 claude 专属 flag 加字段要牵动全部 def 与其单测）。
    let args = match system_prompt_file.as_deref() {
        Some(path) => {
            let mut args = args;
            args.extend(append_system_prompt_file_args(path));
            args
        }
        None => args,
    };

    let extra_env: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let run_generation = state.next_chat_generation(&conversation.id);
    let run_id = format!("ext-run-{}-{}", run_generation, Uuid::new_v4());
    let assistant_message_id = format!("msg_{}", Uuid::new_v4());
    crate::chat::protocol::register_run(
        app,
        &conversation.id,
        &run_id,
        &assistant_message_id,
        conversation.revision,
    );
    let _protocol_guard =
        crate::chat::protocol::RegisteredRunGuard::new(app, &run_id, conversation.revision);

    // Phase 2 / B1: claude、codex app-server、ACP 家族与 dsh SDK JSON-RPC 都通过
    // live-session 注册表把进程跨轮保活。只剩 `PiRpc` 每轮起一个新子进程（见下面
    // `_ =>` 分支的注释）。
    let persistent = matches!(
        def.stream_format,
        StreamFormat::ClaudeStreamJson
            | StreamFormat::CodexAppServer
            | StreamFormat::AcpJsonRpc
            | StreamFormat::DshJsonRpc
    );
    let mut spawned_opt = if persistent {
        None
    } else {
        Some(spawn_agent(def, &resolved_bin, &args, &cwd, &extra_env).await?)
    };
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut raw_output = String::new();
    let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
    let mut tool_map: HashMap<String, usize> = HashMap::new();
    let mut usage: Option<ModelUsage> = None;
    // 协议层自报的失败（claude `result.is_error` / codex `turn/completed failed` / pi
    // `stopReason:error` …）。读流本身常常**正常** Ok 返回，失败只体现在这条消息里，
    // 故单独记下来在出口与 `read_result` 一起判（见 `resolve_turn_error`）。
    let mut stream_error: Option<String> = None;
    let mut stream_outcome = "completed".to_string();
    // A7：CLI **自己**压缩上下文时产生的边界记录。不能在事件回调里直接改 `conversation`
    // （闭包已可变借走了一批局部变量，而 `conversation` 在读流之后还要用），
    // 所以先攒起来，读流结束后一次性落到 `context_state`。
    let mut cli_compactions: Vec<CompactionBoundaryRecord> = Vec::new();
    // 分隔线的时间线锚点 = 触发时刻的最后一条消息（与内置路径 `compaction.rs` 同语义）。
    // **必须是个能解析到的 id**：前端 `resolveCompactionBoundaries` 对空锚点直接 `continue`
    // ——此前这里发的是空串，于是 CLI 自压的分隔线一次都没渲染过。
    let compaction_anchor_id = conversation
        .messages
        .last()
        .map(|message| message.id.clone())
        .unwrap_or_else(|| assistant_message_id.clone());
    // 协议层完成标志：本轮是否读到了 CLI 明确的「本轮结束」帧（claude 的 `result`）。
    // 用于豁免出口的「非零退出码 = 失败」规则（spec 第 8b 条）——杀整棵进程树后
    // 拿到非零退出码的路径变多（Windows `TerminateProcess` 退出码恒为 1），
    // 不豁免会凭空造出失败气泡。
    let protocol_completed = false;
    let mut segment_order = 0u32;
    let mut segments: Vec<ChatMessageSegment> = Vec::new();
    let mut segment_tracker = StreamSegmentTracker::default();
    // Claude Task* 是补丁，不是 DSH 那种整表快照。流上按顺序累加，立刻发 TodoUpdated；
    // 落盘仍走 mutate 补丁，避免并发 spawn 用过期整表把后写的条目盖掉。
    let mut todo_state = conversation.agent_todo_state.clone();
    let conversation_id = conversation.id.clone();
    let started_at = Instant::now();
    // 缓存 key 用探测 cwd（resolve_detection_cwd，非项目会话 = __global__），与斜杠探测的
    // 读取 key 一致——运行时从 CLI init 学到的真实命令列表才能覆盖探测缓存（含空负缓存）。
    // 执行 cwd（上面的 `cwd`）保持每会话独立，仅缓存 key 用全局 scope。
    let slash_cache_key =
        crate::external_agents::workspace::resolve_detection_cwd(app, Some(&conversation.id))
            .map(|detection_cwd| slash::cache_key(&agent_id, &detection_cwd.to_string_lossy()))
            .unwrap_or_else(|_| slash::cache_key(&agent_id, &cwd.to_string_lossy()));
    // 实时用量推送：**只推分子**。分母不实时——它在一轮内是常量，且唯一权威来源是 CLI 轮末
    // 实报的 `result.modelUsage[model].contextWindow`（claude 的 `[1m]` beta / 环境覆盖 /
    // 第三方 router 都只有它知道）。这里若拿静态表算个兜底分母下发，就会出现「回答中
    // `claude-sonnet-5` ⇒ 200K、答完跳回 1M」。前端 `contextPanel.ts::applyLiveContextUsage`
    // 对 `null` 分母保留已知旧窗口，所以不下发就是「沿用上一轮的分母」——正是要的语义。

    let mut emit_event = |event: UnifiedAgentEvent| {
        if let Some(commands) = slash::slash_commands_from_event(&event) {
            state.set_cached_external_slash_commands(slash_cache_key.clone(), commands);
        }
        apply_unified_event(
            app,
            &run_id,
            &conversation_id,
            &compaction_anchor_id,
            &mut content,
            &mut reasoning,
            &mut raw_output,
            &mut tool_calls,
            &mut tool_map,
            &mut usage,
            &mut stream_error,
            &mut segments,
            &mut segment_order,
            &mut segment_tracker,
            &mut cli_compactions,
            &mut todo_state,
            event,
        );
    };

    let cancel_check = || !state.is_chat_generation_active(&conversation_id, run_generation);

    // 本轮接不接工具审批 / 问用户。claude 的判据取自即将启动的 argv 本身
    // （`--permission-prompt-tool stdio`），从 argv 读回来就不可能与 `build_args` 分叉。
    // 没有这条 flag 的 CLI（dsh 的 `session/ask`）靠 `ask_user::needs_host` 开通道，
    // 否则问用户会卡在 `NO_PROVIDER`。
    let approval_host = turn_needs_approval_host(&args, &agent_id).then(|| ApprovalHost {
        app,
        state,
        conversation_id: &conversation_id,
        run_id: &run_id,
        generation: run_generation,
        agent_id: &agent_id,
        auto_allow_tools: std::sync::atomic::AtomicBool::new(
            permission_mode_from_args(&args)
                .is_some_and(crate::external_agents::defs::claude::claude_mode_auto_allows_tools)
                || crate::external_agents::ask_user::auto_allow_ordinary_tools(&agent_id),
        ),
    });

    // Drain stderr concurrently with the stdout read below: keeps a full stderr pipe from
    // blocking the child, and captures failure text a silent (non-JSON, empty-stdout) run would
    // otherwise lose. Persistent protocols manage their own process, so there's no child here.
    let stderr_task = spawned_opt
        .as_mut()
        .map(|spawned| drain_stderr(&mut spawned.child));

    let read_result = if persistent {
        let persistent_mcp: Vec<AcpMcpServer> = vec![];
        // 本轮的启动配置指纹：变了就换进程（spec 第 8 条）。指令哈希只在**真的注入了**
        // `--append-system-prompt-file` 时才有值——斜杠命令那一轮不注入，不参与判定。
        let launch_config = launch_config_for_turn(
            def.stream_format,
            conversation.agent_runtime.external_model.as_deref(),
            conversation.agent_runtime.external_reasoning.as_deref(),
            conversation.agent_runtime.external_sandbox.as_deref(),
            conversation.agent_runtime.external_agent_preset.as_deref(),
            system_prompt_file
                .as_ref()
                .map(|_| stable_prompt_hash(daemon_instructions.trim()))
                .as_deref(),
        );
        run_persistent_turn(
            app,
            state,
            &conversation_id,
            &agent_id,
            def.stream_format,
            &resolved_bin,
            &args,
            &cwd,
            // Claude: already resolved to runtime id above; other agents keep external_model.
            wire_model.clone(),
            conversation.agent_runtime.external_reasoning.clone(),
            conversation.agent_runtime.external_sandbox.clone(),
            conversation.agent_runtime.external_agent_preset.clone(),
            persistent_mcp,
            &launch_config,
            &composed.full_prompt,
            persistent_turn_prompt(
                def.stream_format,
                &composed.full_prompt,
                latest_user_message,
            ),
            &image_blocks,
            &mut emit_event,
            &cancel_check,
            approval_host.as_ref(),
        )
        .await
    } else {
        // 常驻改造（B1）之后，非常驻路径**只剩 `PiRpc`** —— 上面的 `persistent` 谓词把
        // claude / codex / ACP 全收走了，而 `StreamFormat` 一共就这四个变体。此前这里还留着
        // `CodexAppServer` / `AcpJsonRpc` / `_` 三条臂（连带 `run_acp_session` 与
        // `run_codex_app_server_session` 两个一次性驱动，共约 470 行），全部不可达 ——
        // 那正是 rmcp 那次重构刚在 MCP 上消灭掉的「同一个协议两份实现」。
        debug_assert_eq!(def.stream_format, StreamFormat::PiRpc);
        let spawned = spawned_opt
            .as_mut()
            .expect("non-persistent path spawns a child");
        let model = conversation.agent_runtime.external_model.as_deref();
        run_pi_rpc_session(
            &mut spawned.child,
            &composed.full_prompt,
            model,
            |event| emit_event(event),
            cancel_check,
        )
        .await
    };

    // Non-persistent path waits on (and drops/kills) the per-turn child. Persistent sessions
    // keep their process alive in the registry, so there is nothing to wait on here.
    let exit_code: Option<i32> = match spawned_opt {
        Some(mut spawned) => {
            // A6: on a read error the child may still be running (e.g. an I/O error that didn't
            // kill it) — kill first so `wait()` can't block on a live process. 杀**整棵树**
            // 而不只是直接子进程：CLI 会按用户配置拉起自己的 MCP 服务器作为子进程。
            if read_result.is_err() {
                kill_agent_process_tree(&mut spawned.child);
            }
            let status = spawned.child.wait().await.map_err(|e| e.to_string())?;
            status.code()
        }
        None => None,
    };
    let stderr_output = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    };
    // 出口诊断（仅 debug 构建）：定位「生成异常结束」这类 outcome 误判时的第一手数据。
    if cfg!(debug_assertions) {
        eprintln!(
            "[external-agent] {} turn done: read_result={:?} exit_code={:?} stderr_len={} stderr_tail={:?}",
            def.id,
            read_result.as_ref().err(),
            exit_code,
            stderr_output.len(),
            tail_chars(stderr_output.trim(), 200),
        );
    }
    // R2: a read error (non-cancel) becomes a classified, actionable bubble — the raw error goes
    // into a collapsible `<details>` rather than being shown verbatim as the bubble body.
    let turn_error = resolve_turn_error(read_result.as_ref().err(), stream_error.as_ref());
    let mut error_rendered = false;
    if let Some(err) = turn_error {
        if is_cancellation(err) {
            stream_outcome = "cancelled".to_string();
        } else {
            stream_outcome = "error".to_string();
            let classified =
                crate::external_agents::errors::classify(err, exit_code, &stderr_output, &agent_id);
            let bubble = classified.render_bubble();
            append_final_text(
                &mut content,
                &mut segments,
                &mut segment_order,
                tool_calls.len(),
                &bubble,
            );
            error_rendered = true;
        }
    } else if nonzero_exit_is_a_failure(exit_code, protocol_completed) {
        if content.trim().is_empty() {
            stream_outcome = "error".to_string();
        }
    }

    // Fill empty content from the richest available fallback: captured raw stdout lines first,
    // then stderr (as an explicit failure), then the slash / no-output placeholders.
    if !error_rendered && content.trim().is_empty() {
        let fallback = if !raw_output.trim().is_empty() {
            raw_output.trim().to_string()
        } else if !stderr_output.trim().is_empty() {
            stream_outcome = "error".to_string();
            format!(
                "{} 执行失败：\n\n{}",
                def.name,
                truncate_for_preview(stderr_output.trim(), 4000)
            )
        } else if stream_outcome == "completed" {
            if is_slash && tool_calls.is_empty() {
                format!("{} 命令已执行", def.name)
            } else if is_slash {
                String::new()
            } else {
                stream_outcome = "error".to_string();
                format!(
                    "{} 未产生输出（exit={:?}，耗时 {}ms）",
                    def.name,
                    exit_code,
                    started_at.elapsed().as_millis()
                )
            }
        } else {
            String::new()
        };
        append_final_text(
            &mut content,
            &mut segments,
            &mut segment_order,
            tool_calls.len(),
            &fallback,
        );
    }

    // A nonzero exit with stderr is a failure even if the CLI also produced some stdout — append
    // the stderr (unless it's already the content) so the error is visible, not swallowed. Skipped
    // when a classified error bubble already folded the stderr into its `<details>`, and when the
    // protocol already said this turn completed (spec 8b).
    if !error_rendered
        && nonzero_exit_is_a_failure(exit_code, protocol_completed)
        && !stderr_output.trim().is_empty()
    {
        stream_outcome = "error".to_string();
        if !content.contains(stderr_output.trim()) {
            let tail = format!(
                "{} stderr：\n\n{}",
                def.name,
                truncate_for_preview(stderr_output.trim(), 4000)
            );
            append_final_text(
                &mut content,
                &mut segments,
                &mut segment_order,
                tool_calls.len(),
                &tail,
            );
        }
    }

    persist_delivered_session(
        app,
        &conversation_id,
        def.id,
        &resume_ctx,
        // 哈希只覆盖**会话级常量**（系统提示 + Memory + cwd 提示）。skill 正文是 per-turn 的，
        // 不进哈希——否则换 skill 会被当成「instructions 变了」而重发一遍会话级指令。
        &daemon_instructions,
        is_slash,
    )?;

    // A7：把 CLI 自压的边界落到会话上。此前只发了实时压缩更新、从不落盘，
    // 于是「已压缩 N 次」永远不涨、刷新或重开会话后分隔线消失（那条注释说要记一次压缩，
    // 但代码并没有做）。写在 `push_assistant_message` **之前**：它会用
    // `compute_context_state` 整体重算 `context_state`，而外部路径的重算会把
    // `compression_count` / `compaction_boundaries` / `last_compressed_at` 原样带过去。
    for boundary in &cli_compactions {
        conversation.context_state.compression_count = conversation
            .context_state
            .compression_count
            .saturating_add(1);
        conversation.context_state.last_compressed_at = Some(boundary.created_at);
        conversation
            .context_state
            .compaction_boundaries
            .push(boundary.clone());
    }

    let terminal_content = content.clone();
    let terminal_outcome = stream_outcome.clone();
    push_assistant_message(
        app,
        state,
        &settings,
        conversation,
        assistant_message_id,
        content,
        if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        },
        vec![],
        tool_calls,
        vec![],
        segments,
        active_skill_id,
        title_from_first_user,
        Some(match entry {
            AgentRunEntry::Send => "send",
            AgentRunEntry::Regenerate => "regenerate",
        }),
        Some(&stream_outcome),
        usage,
        None,
        None,
        // 外部 CLI 走自己的协议，没有 agent 循环的降级兜底。
        None,
    )
    .await?;

    crate::chat::protocol::finish_run(
        app,
        &run_id,
        &terminal_outcome,
        &terminal_content,
        conversation.revision,
    );

    Ok(())
}

#[derive(Default)]
struct StreamSegmentTracker {
    active_text_idx: Option<usize>,
    active_reasoning_idx: Option<usize>,
}

impl StreamSegmentTracker {
    fn reset_text(&mut self) {
        self.active_text_idx = None;
    }

    fn reset_reasoning(&mut self) {
        self.active_reasoning_idx = None;
    }

    fn append(
        &mut self,
        kind: ChatMessageSegmentKind,
        segments: &mut Vec<ChatMessageSegment>,
        segment_order: &mut u32,
        tool_calls_len: usize,
        delta: &str,
    ) -> ChatMessageSegment {
        let phase = segment_phase_for_tool_count(&kind, tool_calls_len);
        let active = match kind {
            ChatMessageSegmentKind::Reasoning => &mut self.active_reasoning_idx,
            _ => &mut self.active_text_idx,
        };
        if let Some(idx) = *active {
            if let Some(segment) = segments.get_mut(idx) {
                if segment.kind == kind && segment.phase == phase {
                    let merged = format!("{}{}", segment.text.as_deref().unwrap_or(""), delta);
                    segment.text = Some(merged);
                    return segment.clone();
                }
            }
        }

        *segment_order += 1;
        let segment = ChatMessageSegment {
            id: format!("seg_{}", Uuid::new_v4()),
            kind,
            phase,
            order: *segment_order,
            step_number: None,
            round: if tool_calls_len == 0 { None } else { Some(1) },
            text: Some(delta.to_string()),
            tool_call_id: None,
        };
        *active = Some(segments.len());
        segments.push(segment.clone());
        segment
    }
}

/// Phase 2: run one turn against a persistent live session, reusing the conversation's existing
/// session, resuming a persisted one after a restart, or connecting fresh. The CLI process is kept
/// alive in the registry between turns, so a reused/resumed session sends only the latest user
/// message (the server holds prior context), while a fresh session gets the full composed prompt.
#[allow(clippy::too_many_arguments)]
async fn run_persistent_turn<E, C>(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation_id: &str,
    agent_id: &str,
    protocol: StreamFormat,
    resolved_bin: &std::path::Path,
    args: &[String],
    cwd: &std::path::Path,
    model: Option<String>,
    reasoning: Option<String>,
    sandbox: Option<String>,
    preset: Option<String>,
    mcp_servers: Vec<AcpMcpServer>,
    launch_config: &LaunchConfig,
    first_prompt: &str,
    reuse_prompt: &str,
    images: &[crate::external_agents::attachments::ImageBlock],
    emit: &mut E,
    cancel: &C,
    // 本轮的工具审批出口。`None` = 不接（协议不支持 / 用户没选会询问的权限档位）——
    // 会话侧仍对 `can_use_tool` 回 error 兜底，绝不沉默。
    approvals: Option<&ApprovalHost<'_>>,
) -> Result<(), String>
where
    E: FnMut(UnifiedAgentEvent),
    C: Fn() -> bool,
{
    use crate::external_agents::session::live::LiveSession;
    use crate::external_agents::session::{
        clear_live_handle, load_live_handle, save_live_handle, LiveSessionHandle,
    };

    let cwd_str = cwd.to_string_lossy().to_string();
    let protocol_tag = persistent_protocol_tag(protocol);

    // 本轮实际使用的启动参数。resume 失效时会被改写成「开新会话」（见
    // `drop_resume_for_fresh_session`），所以不能直接用调用方那份不可变的 `args`。
    let mut turn_args: Vec<String> = args.to_vec();
    // resume 已经被摘掉过一次了吗。降级恰好一次（第二次仍失败说明原因不是 resume）。
    let mut dropped_resume = false;

    // 能拿来续接的原生会话 id。**轮内重连必须带上它**：ACP（`session/load`）与 codex
    // （thread id）的续接走的是这个参数，不像 claude 在 argv 里。不带 = 空白会话 + 一条
    // 「上下文已重置」，而触发重连的多是上游 503 / 瞬时 RPC 故障，或者用户只是改了个
    // reasoning 档位（grok 的 `--reasoning-effort` 是启动参数，必须换进程）—— 这些都不该
    // 让用户丢掉整段上下文。会话 id 不随进程死亡失效：grok 实测（1.0.3）新进程
    // `session/load` 同一个 id 成功，`initialize` 也声明了 `loadSession: true`。
    let mut resumable_native: Option<String> = load_live_handle(app, conversation_id)
        .filter(|h| h.agent_id == agent_id && h.cwd == cwd_str && h.protocol == protocol_tag)
        .map(|h| h.native_id);

    // Establish the control channel: 1. reuse a live session in the registry; 2. resume a
    // persisted one; 3. connect fresh.
    //
    // 复用判据里含 `launch_config`：模型 / reasoning / sandbox / 系统指令任一变化 ⇒ 不可复用
    // ⇒ 丢弃条目（actor 自行关停旧进程）并走下面的连接分支**带原生 resume**，于是新 flag
    // 生效而上下文不丢（spec 第 8 条：UI 所见必须与会话实际配置一致）。
    let previous_control = state.external_live_session_control_any(conversation_id);
    let reusable_control =
        state.external_live_session_control(conversation_id, agent_id, &cwd_str, launch_config);
    if reusable_control.is_none() {
        if let Some(stale) = previous_control {
            // A new dsh process must not resume while the old process can still write the same
            // native session log. Close the actor and wait for its receiver to disappear first.
            let _ = stale
                .send(crate::external_agents::session::live::SessionCommand::Close)
                .await;
            if tokio::time::timeout(std::time::Duration::from_secs(5), stale.closed())
                .await
                .is_err()
            {
                return Err("旧外部 CLI 会话关闭超时，请重试".to_string());
            }
        }
    }
    let (mut control, mut prompt) = match reusable_control {
        Some(control) => (control, reuse_prompt.to_string()),
        None => {
            let resume_native = resumable_native.clone();
            // We intended to continue an existing native session iff a matching handle was
            // persisted. If the resume then fails and we fall back to fresh, the prior context
            // is lost and the user must be told (R4) rather than silently getting a blank slate.
            let intended_resume = resume_native.is_some();
            let connected = match connect_persistent_session(
                protocol,
                resolved_bin,
                &turn_args,
                cwd,
                model.as_deref(),
                reasoning.as_deref(),
                sandbox.as_deref(),
                preset.as_deref(),
                &mcp_servers,
                resume_native.clone(),
                Some(background_task_sink(app, conversation_id)),
                Some(dsh_idle_sink(app, conversation_id)),
            )
            .await
            {
                Ok(connected) => connected,
                // Resume can fail during connect before the first turn: Claude reports a missing
                // conversation on stderr; dsh returns `session \"...\" not found` from session/open.
                // Clear the stale handle and retry fresh exactly once, with the normal reset notice.
                Err(err)
                    if !dropped_resume
                        && (crate::external_agents::stream::claude::is_missing_session_error(&err)
                            || crate::external_agents::session::dsh_jsonrpc::is_missing_session_error(
                                &err,
                            )) =>
                {
                    dropped_resume = true;
                    turn_args = drop_resume_for_fresh_session(
                        app,
                        conversation_id,
                        agent_id,
                        protocol,
                        &turn_args,
                    );
                    connect_persistent_session(
                        protocol,
                        resolved_bin,
                        &turn_args,
                        cwd,
                        model.as_deref(),
                        reasoning.as_deref(),
                        sandbox.as_deref(),
                        preset.as_deref(),
                        &mcp_servers,
                        None,
                        Some(background_task_sink(app, conversation_id)),
                        Some(dsh_idle_sink(app, conversation_id)),
                    )
                    .await?
                }
                Err(err) => return Err(err),
            };
            let PersistentConnection {
                control,
                native_id,
                resumed,
                child_pid,
            } = connected;
            // 轮内重连要续的是**这个**会话，不是 handle 里那个（首连开了新会话时两者不同）。
            resumable_native = Some(native_id.clone()).filter(|id| !id.trim().is_empty());
            let _ = save_live_handle(
                app,
                conversation_id,
                &LiveSessionHandle {
                    agent_id: agent_id.to_string(),
                    protocol: protocol_tag.to_string(),
                    native_id,
                    cwd: cwd_str.clone(),
                },
            );
            state.register_external_live_session(
                conversation_id.to_string(),
                LiveSession {
                    control: control.clone(),
                    agent_id: agent_id.to_string(),
                    cwd: cwd_str.clone(),
                    launch_config: launch_config.clone(),
                    last_activity: std::time::Instant::now(),
                    child_pid,
                    turns_served: 1,
                    busy: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                },
            );
            // A resumed session already holds history → send only the latest message.
            let prompt = if resumed {
                reuse_prompt.to_string()
            } else {
                first_prompt.to_string()
            };
            // Intended to resume but ended up fresh → warn about the lost context.
            // `dropped_resume` 走的也是这一条：resume 的目标会话被 claude 清理掉了，我们换了个
            // 新会话继续 —— 用户该看到的正是这条**已有的**「上下文已重置」提示，
            // 而不是 claude 那句英文原文（别为它新写一条文案，spec 第 2 条）。
            if !resumed && (intended_resume || dropped_resume) {
                emit(context_reset_notice_event());
            }
            (control, prompt)
        }
    };

    // 标记「有在飞轮次」：一轮可以跑十几分钟（没有轮内超时），而 `last_activity` 只在轮
    // 开始时写一次。不标的话清扫器/LRU 会在轮中把这条会话回收掉 —— 这一轮不会断，但轮末
    // 最后一个 sender 落地就把进程关了，下一轮白付一次冷启动（越重度使用越吃不到常驻）。
    // guard 落地即清，覆盖下面每条 `return`。
    // guard 落地即清，覆盖下面每条 `return`。**轮内重连之后必须重挂**：`reconnect_fresh`
    // 会往注册表塞一条全新的 `LiveSession`（`busy: false`），旧 guard 还指着旧会话的 Arc
    // —— 不重挂的话，恰好是刚付过冷启动的那条路反而不受保护。
    // 下划线名字是必要的：这个值只靠 Drop 起作用，没有任何读取点。
    let mut _busy = state.mark_external_live_session_busy(conversation_id);

    // At most one automatic fresh reconnect after a non-cancel / non-auth failure (R3), plus one
    // reconnect for a config change that only a relaunch can apply (R4 NeedsReconnect). Each is
    // gated by its own bool so a persistently-failing session can't loop.
    let mut retried_after_failure = false;
    let mut reconnected_for_config = false;
    loop {
        let outcome = drive_persistent_turn(
            &control,
            prompt.clone(),
            model.clone(),
            reasoning.clone(),
            images,
            emit,
            cancel,
            approvals,
        )
        .await;

        let err = match outcome {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };

        // 会话是否还能留在注册表里。默认丢弃（entry 落地 ⇒ control sender 关闭 ⇒ actor 关停
        // 子进程）；只有「协议级取消之后会话仍可用」的协议例外 —— 否则用户点一次「停止」
        // 就把常驻进程连带杀掉，上下文延续与 0.1s 冷启动全部作废（见 `cancel_keeps_live_session`）。
        if !cancel_keeps_live_session(&err, protocol) {
            state.remove_external_live_session(conversation_id);
        }

        match persistent_failure_action(
            &err,
            agent_id,
            retried_after_failure,
            reconnected_for_config,
            dropped_resume,
        ) {
            // Cancelled keeps the persisted handle so a later turn can resume the native session.
            PersistentFailureAction::Cancelled => return Err(err),
            // Auth / exhausted retries → drop the handle (process likely dead) and surface the error.
            PersistentFailureAction::Fatal => {
                clear_live_handle(app, conversation_id);
                return Err(err);
            }
            // Launch-flag config change (reasoning) → relaunch fresh with the new `args`.
            PersistentFailureAction::ReconnectConfig => {
                reconnected_for_config = true;
            }
            // resume 的目标会话在 CLI 那边没了 → 换个新会话 id、不带 resume 重连一次。
            // 这里**不重置** `retried_after_failure`：降级本身有独立闸门（`dropped_resume`）。
            PersistentFailureAction::ReconnectWithoutResume => {
                dropped_resume = true;
                // 这条路的**全部意义**就是别再续那个死会话了（argv 与协议参数两条都要摘）。
                resumable_native = None;
                turn_args = drop_resume_for_fresh_session(
                    app,
                    conversation_id,
                    agent_id,
                    protocol,
                    &turn_args,
                );
            }
            // Transient failure → drop the stale handle and reconnect fresh once.
            PersistentFailureAction::RetryFresh => {
                retried_after_failure = true;
                clear_live_handle(app, conversation_id);
            }
        }

        let (next_control, resumed, reconnect_native) = reconnect_fresh(
            app,
            state,
            conversation_id,
            agent_id,
            protocol,
            protocol_tag,
            resolved_bin,
            &turn_args,
            cwd,
            &cwd_str,
            model.as_deref(),
            reasoning.as_deref(),
            sandbox.as_deref(),
            preset.as_deref(),
            &mcp_servers,
            launch_config,
            resumable_native.clone(),
        )
        .await?;
        // 又重连一次时续的是这条会话（续接成功 ⇒ 同一个 id；失败降级成新会话 ⇒ 新 id）。
        resumable_native = reconnect_native.filter(|id| !id.trim().is_empty());
        // 新会话进了注册表 ⇒ 旧 guard 已经指不到它了，重挂一个（旧的在赋值时落地）。
        _busy = state.mark_external_live_session_busy(conversation_id);
        control = next_control;
        prompt = if resumed {
            reuse_prompt.to_string()
        } else {
            first_prompt.to_string()
        };
        // A fresh reconnect after an in-run session failure drops whatever context that session
        // had accumulated this run — surface it rather than silently continuing on a blank slate.
        // 但**真的续上了**原生会话时不能发这条（claude 的 argv 仍带 `--resume` 就属于这种）：
        // 一条假的「上下文已重置」本身就是 bug。
        if !resumed {
            emit(context_reset_notice_event());
        }
    }
}

/// Connect a persistent session for the reconnect paths, persist its handle, and register it.
/// Returns the control channel, **whether the CLI actually continued its native session** — for
/// claude that happens when `args` still carry `--resume`, and in that case the caller must NOT
/// claim the context was reset (a false alarm is its own bug) — and the native session id the
/// connection ended up on, so a further reconnect continues *that* session.
///
/// `resume_native` 是 ACP / codex 的续接凭据（claude 的在 argv 里）。**不传 = 保证开新会话**，
/// 也就保证了一条「上下文已重置」；而走到这里的原因常常只是上游 503 或改了 reasoning 档位。
#[allow(clippy::too_many_arguments)]
async fn reconnect_fresh(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation_id: &str,
    agent_id: &str,
    protocol: StreamFormat,
    protocol_tag: &str,
    resolved_bin: &std::path::Path,
    args: &[String],
    cwd: &std::path::Path,
    cwd_str: &str,
    model: Option<&str>,
    reasoning: Option<&str>,
    sandbox: Option<&str>,
    preset: Option<&str>,
    mcp_servers: &[AcpMcpServer],
    launch_config: &LaunchConfig,
    resume_native: Option<String>,
) -> Result<
    (
        tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>,
        bool,
        Option<String>,
    ),
    String,
> {
    use crate::external_agents::session::live::LiveSession;
    use crate::external_agents::session::{save_live_handle, LiveSessionHandle};

    let PersistentConnection {
        control,
        native_id,
        resumed,
        child_pid,
    } = connect_persistent_session(
        protocol,
        resolved_bin,
        args,
        cwd,
        model,
        reasoning,
        sandbox,
        preset,
        mcp_servers,
        resume_native,
        Some(background_task_sink(app, conversation_id)),
        Some(dsh_idle_sink(app, conversation_id)),
    )
    .await?;
    let _ = save_live_handle(
        app,
        conversation_id,
        &LiveSessionHandle {
            agent_id: agent_id.to_string(),
            protocol: protocol_tag.to_string(),
            native_id: native_id.clone(),
            cwd: cwd_str.to_string(),
        },
    );
    state.register_external_live_session(
        conversation_id.to_string(),
        LiveSession {
            control: control.clone(),
            agent_id: agent_id.to_string(),
            cwd: cwd_str.to_string(),
            launch_config: launch_config.clone(),
            last_activity: std::time::Instant::now(),
            child_pid,
            turns_served: 1,
            busy: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
    );
    Ok((control, resumed, Some(native_id)))
}

/// 本轮的启动配置指纹。
///
/// Claude and dsh both have process-bound settings. Claude fingerprints launch flags/system prompt;
/// dsh fingerprints initialize model, profile reasoning/provider, sandbox, and agent preset. Without
/// this, the UI can show a new configuration while the resident process keeps running the old one.
///
/// ACP / codex 能在会话内改模型与推理档位（`session/set_config_option` / 每轮 `turn/start`
/// 带 model），指纹恒为 `default()` ⇒ 永不触发重连，既有行为不变。
///
/// dsh 相反：model 是进程级 `initialize` 后创建 agent 时固定的，reasoning 是 profile patch，
/// sandbox 是进程环境变量；三者都没有 session 级修改 RPC。任一变化都必须换进程，但 Kivio
/// bridge 会用同一个 native session id 调 `agents.resume()`，所以历史上下文继续保留。
fn dsh_provider_fingerprint_for(provider: Option<&crate::settings::ExternalCliProvider>) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match provider {
        Some(provider) => serde_json::to_string(provider)
            .unwrap_or_else(|_| provider.id.clone())
            .hash(&mut hasher),
        None => "cli-default".hash(&mut hasher),
    }
    format!("{:016x}", hasher.finish())
}

fn dsh_provider_fingerprint() -> String {
    let provider = crate::external_agents::overrides::active_provider("dsh");
    dsh_provider_fingerprint_for(provider.as_ref())
}

fn launch_config_for_turn(
    protocol: StreamFormat,
    model: Option<&str>,
    reasoning: Option<&str>,
    sandbox: Option<&str>,
    preset: Option<&str>,
    instructions_hash: Option<&str>,
) -> LaunchConfig {
    if matches!(protocol, StreamFormat::DshJsonRpc) {
        return LaunchConfig {
            flags: format!(
                "{}|{}|{}|{}|{}",
                model.unwrap_or_default(),
                reasoning.unwrap_or_default(),
                sandbox.unwrap_or_default(),
                crate::external_agents::dsh_profile::normalize_agent_preset(preset),
                dsh_provider_fingerprint()
            ),
            // dsh 的会话级指令在首轮正文里，不是启动配置；指令变化不需要为了它单独重连。
            instructions: None,
        };
    }
    if !matches!(protocol, StreamFormat::ClaudeStreamJson) {
        return LaunchConfig::default();
    }
    LaunchConfig {
        // **model 不在指纹里**：它能在会话内换（`claude_stream::apply_model_change` 发
        // `set_model` 控制请求，官方 `Query.setModel()` 在 streaming input mode 下可用）。
        // 换模型是用户最频繁的操作，把它留在指纹里等于每换一次就付一次 3.2s 冷启动 +
        // 一整段历史重放。会话内切换失败时 `run_turn` 返回 `NEEDS_RECONNECT`，走既有的
        // ReconnectConfig 那条路 —— 也就是退回本次改动之前的行为，没有静默失效的可能。
        //
        // reasoning / sandbox 仍在指纹里：`--effort` 对应的官方入口是
        // `applyFlagSettings({effortLevel})`，wire 形状**没有核实过**；`--permission-mode`
        // 更换还会改变要不要带 `--permission-prompt-tool stdio`（见
        // `defs::claude::claude_permission_prompt_args`），那是启动参数的事，会话内切不了。
        flags: format!(
            "{}|{}",
            reasoning.unwrap_or_default(),
            sandbox.unwrap_or_default()
        ),
        instructions: instructions_hash.map(str::to_string),
    }
}

/// 持久会话「复用 / resume 轮」要发的正文。
///
/// claude 的 `composed.full_prompt` **本身就不含**会话级指令（它们走
/// `--append-system-prompt-file` 启动 flag，spec 第 1 条），剩下的全是 **per-turn** 内容：
/// active skill 正文 + 降级附件说明 + 用户消息。这些每轮都必须整份发出去 —— 只发最新用户
/// 消息会让 skill 正文与附件说明从第 2 轮起静默消失（active skill 是用户可以中途换的
/// per-turn 选择）。
///
/// 其余持久协议（codex / ACP）的 full_prompt 首轮**含**指令，复用轮只发最新用户消息，
/// 保持现有行为。
fn persistent_turn_prompt<'a>(
    protocol: StreamFormat,
    composed_prompt: &'a str,
    latest_user_message: &'a str,
) -> &'a str {
    match protocol {
        StreamFormat::ClaudeStreamJson => composed_prompt,
        _ => latest_user_message,
    }
}

/// 本轮错误是否代表「用户取消」——出口走 cancelled（不弹错误气泡、不发上下文重置提示、
/// 更不会重发这一轮 prompt）。
fn is_cancellation(err: &str) -> bool {
    err == "cancelled" || err == crate::external_agents::session::live::CANCELLED_SESSION_LOST
}

/// 这次失败之后，常驻会话能不能留在注册表里继续服下一轮。
///
/// **claude / dsh**：协议级取消会一直读到当前活动完全回到 idle，流位置停在轮次边界、
/// 进程与原生 session 完好，可以直接继续下一轮。
///
/// **ACP / codex**：`session/cancel` / `turn/interrupt` 发出后立刻返回，reader 停在流中间
/// （未消费的 prompt 响应 + 后续 update），复用会读到上一轮的残帧，因此丢弃 live 进程，
/// 下一轮从落盘 handle 原生 resume。
///
/// `CANCELLED_SESSION_LOST`（进程死了 / 取消超时被硬 Close）任何协议都不保留。
fn cancel_keeps_live_session(err: &str, protocol: StreamFormat) -> bool {
    err == "cancelled"
        && matches!(
            protocol,
            StreamFormat::ClaudeStreamJson | StreamFormat::DshJsonRpc
        )
}

/// What `run_persistent_turn` should do after a turn fails. Pure so the retry policy is unit
/// testable without a Tauri context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistentFailureAction {
    /// User cancellation — surface as-is, keep the persisted handle for a later resume.
    Cancelled,
    /// Relaunch fresh to apply a launch-flag config change (reasoning without a config option).
    ReconnectConfig,
    /// `--resume` 的目标会话在 CLI 那边已经不存在了 —— 摘掉 resume、换个新会话 id 重连一次。
    ///
    /// 与 `RetryFresh` 的区别不是「重不重连」而是**带不带 resume**：claude 的会话 flag 在
    /// argv 里，`RetryFresh` 会拿同一份 argv 再连一次 ⇒ 同一个死 id 再 `--resume` 一次 ⇒
    /// 必然再失败一次，然后用户拿到一句英文原文。
    ReconnectWithoutResume,
    /// Transient failure — reconnect fresh once and re-send the prompt.
    RetryFresh,
    /// Auth failure or exhausted retries — give up and surface the error.
    Fatal,
}

fn persistent_failure_action(
    err: &str,
    agent_id: &str,
    retried_after_failure: bool,
    reconnected_for_config: bool,
    dropped_resume: bool,
) -> PersistentFailureAction {
    if is_cancellation(err) {
        return PersistentFailureAction::Cancelled;
    }
    if err == crate::external_agents::session::acp::NEEDS_RECONNECT {
        // Only relaunch once for a config change; a repeat means the relaunch didn't help.
        return if reconnected_for_config {
            PersistentFailureAction::Fatal
        } else {
            PersistentFailureAction::ReconnectConfig
        };
    }
    // resume 失效：**必须排在下面那条 auth / retried 之前**。它不是瞬时故障（重连同一份 argv
    // 一定再失败），也不是认证问题，而是一个有确定处置的状态：换个新会话继续。
    // 同样只降级一次 —— 摘掉 resume 之后还失败说明是别的原因。
    if crate::external_agents::stream::claude::is_missing_session_error(err)
        || crate::external_agents::session::dsh_jsonrpc::is_missing_session_error(err)
    {
        return if dropped_resume {
            PersistentFailureAction::Fatal
        } else {
            PersistentFailureAction::ReconnectWithoutResume
        };
    }
    // Auth is never auto-retried (a doomed retry could trigger a login storm).
    if crate::external_agents::errors::is_auth_error(err, agent_id) {
        return PersistentFailureAction::Fatal;
    }
    if retried_after_failure {
        PersistentFailureAction::Fatal
    } else {
        PersistentFailureAction::RetryFresh
    }
}

/// resume 失效的降级：换一个新的原生会话 id，把它写进落盘记录，并返回改写后的启动参数。
///
/// 三件事必须一起做，少一件就会留一个坑：
/// 1. **argv**：`--resume <死 id>` → `--session-id <新 id>`（`claude_args_fresh_session`）；
/// 2. **会话记录**：`--resume` 的来源是它，不改的话下一轮又拿死 id 去 resume（每轮降级一次）；
/// 3. **live handle**：它的 `native_id` 也指着那个死会话，留着会在别处兜底成 `--resume`。
///
/// Claude must also rewrite argv and its stored native id. dsh carries resume only in the live
/// handle, so clearing that handle is enough; the fresh connection returns and persists a new id.
fn drop_resume_for_fresh_session(
    app: &AppHandle,
    conversation_id: &str,
    agent_id: &str,
    protocol: StreamFormat,
    args: &[String],
) -> Vec<String> {
    use crate::external_agents::session::{clear_live_handle, replace_stored_session_id};

    if matches!(protocol, StreamFormat::DshJsonRpc) {
        clear_live_handle(app, conversation_id);
        return args.to_vec();
    }
    if !matches!(protocol, StreamFormat::ClaudeStreamJson) {
        return args.to_vec();
    }
    let fresh_id = uuid::Uuid::new_v4().to_string();
    replace_stored_session_id(app, conversation_id, agent_id, &fresh_id);
    clear_live_handle(app, conversation_id);
    crate::external_agents::defs::claude::claude_args_fresh_session(args, &fresh_id)
}

/// True when a cancel was requested (`cancel_at` set) and the grace period has elapsed without the
/// turn winding down — the caller escalates to `Close` (A5). Pure for unit testing.
fn cancel_should_escalate(
    cancel_at: Option<std::time::Instant>,
    now: std::time::Instant,
    grace: std::time::Duration,
) -> bool {
    matches!(cancel_at, Some(t) if now.saturating_duration_since(t) >= grace)
}

/// 本轮的 argv 是否让 CLI 走「问宿主要权限」那条路。
///
/// 判据只有一条：argv 里带 `--permission-prompt-tool`。这是**真正**决定 CLI 会不会发
/// `can_use_tool` 的那个开关（本机 2.1.220 对照实测：带 ⇒ 收到询问；不带 ⇒ 一条都没有、
/// 权限被 CLI 直接拒），所以从 argv 读回来比在这里重抄一份「哪些权限档位会询问」的规则
/// 更不容易分叉 —— 那份规则的唯一副本在 `defs::claude::claude_permission_prompt_args`。
fn turn_asks_for_permission(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--permission-prompt-tool")
}

/// 本轮要不要建审批 / 问用户宿主。claude 看 argv 上的 `--permission-prompt-tool`；
/// 没有这条 flag 的 CLI（dsh 的 `session/ask`）靠 `ask_user::needs_host` —— 加了
/// codec 就会开通道。
fn turn_needs_approval_host(args: &[String], agent_id: &str) -> bool {
    turn_asks_for_permission(args) || crate::external_agents::ask_user::needs_host(agent_id)
}

/// 本轮 argv 里的权限档位（`--permission-mode` 的值）。
///
/// 与 `turn_asks_for_permission` 同一个理由从 argv 读回来而不是再抄一份规则：决定 CLI 行为
/// 的就是 argv 本身，读它就不可能与 `build_args` 分叉。
fn permission_mode_from_args(args: &[String]) -> Option<&str> {
    args.windows(2)
        .find(|w| w[0] == "--permission-mode")
        .map(|w| w[1].as_str())
}

/// Send one `RunTurn` on `control` and pump its events/terminal result. On user cancel, send a
/// protocol-level `Cancel`; if the turn doesn't wind down within `CANCEL_ESCALATE_GRACE`, escalate
/// to `Close` (A5) so a hung session can't block cancellation indefinitely.
async fn drive_persistent_turn<E, C>(
    control: &tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>,
    prompt: String,
    model: Option<String>,
    reasoning: Option<String>,
    images: &[crate::external_agents::attachments::ImageBlock],
    emit: &mut E,
    cancel: &C,
    approvals: Option<&ApprovalHost<'_>>,
) -> Result<(), String>
where
    E: FnMut(UnifiedAgentEvent),
    C: Fn() -> bool,
{
    use crate::external_agents::session::live::{ApprovalBridge, SessionCommand};
    use futures::stream::{FuturesUnordered, StreamExt};
    use tokio::sync::{mpsc, oneshot};

    const CANCEL_ESCALATE_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

    let (events_tx, mut events_rx) = mpsc::channel::<UnifiedAgentEvent>(64);
    let (done_tx, done_rx) = oneshot::channel::<Result<(), String>>();
    // 审批通道只在宿主接得住时才建。会话侧据此决定 `can_use_tool` 是「问用户」还是走那条
    // fail-closed 的 error 兜底 —— 两种都绝不沉默（沉默 = 那一轮永久挂死）。
    let (bridge, mut ask_rx, decision_tx) = match approvals {
        Some(_) => {
            let (ask_tx, ask_rx) = mpsc::channel(8);
            let (decision_tx, decision_rx) = mpsc::channel(8);
            (
                Some(ApprovalBridge {
                    requests: ask_tx,
                    decisions: decision_rx,
                }),
                Some(ask_rx),
                Some(decision_tx),
            )
        }
        None => (None, None, None),
    };
    if control
        .send(SessionCommand::RunTurn {
            prompt,
            model,
            reasoning,
            images: images.to_vec(),
            events: events_tx,
            done: done_tx,
            approvals: bridge,
        })
        .await
        .is_err()
    {
        return Err("外部 CLI 会话已结束，请重试".to_string());
    }

    // 在飞的审批（可以有多条：claude 会并行调工具）。放 `FuturesUnordered` 而不是 spawn：
    // 借用 `app`/`state` 就够，不需要 `'static`，而且本函数返回时它们自然被丢弃。
    let mut in_flight = FuturesUnordered::new();
    // 本轮问过的 toolCallId。返回前必须把 `pending_chat_tool_approvals` 里可能残留的条目扫掉
    // ——那张表按 id 存 oneshot sender，走了异常出口（EOF / 硬 Close）不扫就是一条永久泄漏。
    let mut asked_ids: Vec<String> = Vec::new();

    let mut done_rx = done_rx;
    let mut events_open = true;
    let mut cancel_sent = false;
    let mut cancel_at: Option<std::time::Instant> = None;
    let outcome = loop {
        tokio::select! {
            biased;
            result = &mut done_rx => {
                // Invariant (A4): the actor sends every `event` before `done`, and mpsc preserves
                // order, so all remaining events are already queued — drain them before returning.
                while let Ok(event) = events_rx.try_recv() {
                    emit(event);
                }
                break result.unwrap_or_else(|_| Err("session actor dropped".to_string()));
            }
            maybe_event = events_rx.recv(), if events_open => {
                match maybe_event {
                    Some(event) => emit(event),
                    None => events_open = false,
                }
            }
            // 会话侧送来一条「这个工具能用吗」⇒ 复用内置 agent 那条审批链路去问用户
            // （typed approval event + `chat_confirm_tool_call` 命令 + 同一张挂起表），
            // 不另建一套 UI（spec 第 2 条）。
            Some(ask) = async {
                match ask_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => None,
                }
            }, if ask_rx.is_some() => {
                if let Some(host) = approvals {
                    asked_ids.push(ask.tool_call_id.clone());
                    in_flight.push(host.ask(ask));
                }
            }
            // 用户答了 ⇒ 把答复送回会话，由它写 `control_response`。
            Some(decision) = in_flight.next(), if !in_flight.is_empty() => {
                if let Some(tx) = decision_tx.as_ref() {
                    let _: Result<_, _> = tx.send(decision).await;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
        }
        if !cancel_sent && cancel() {
            cancel_sent = true;
            cancel_at = Some(std::time::Instant::now());
            let _ = control.send(SessionCommand::Cancel).await;
        }
        // A5: protocol-level cancel didn't wind the turn down in time → escalate to a hard Close.
        if cancel_should_escalate(cancel_at, std::time::Instant::now(), CANCEL_ESCALATE_GRACE) {
            let _ = control.send(SessionCommand::Close).await;
            // 用 `CANCELLED_SESSION_LOST` 而不是 `"cancelled"`：出口仍按取消呈现，但会话
            // 已经被硬 Close 掉了，注册表条目**必须**丢弃 —— 否则 claude 的「取消后保留常驻
            // 会话」会把一个死 actor 留到下一轮才发现。
            break Err(crate::external_agents::session::live::CANCELLED_SESSION_LOST.to_string());
        }
    };
    // 会话侧那张挂起表由 `claude_stream::reject_pending` 负责整批拒掉（它才是欠 CLI 响应的
    // 一方）；这里只扫宿主侧的残留条目 + 让在飞的 future 随本函数一起丢弃。
    if let Some(host) = approvals {
        host.forget(&asked_ids);
    }
    // 未消费的答复对应的询问已经被会话侧拒掉了，丢弃即可。
    drop(decision_tx);
    outcome
}

/// 批准计划后把 claude 切到的档位 —— **仅当前端没给**（老 UI / 「总是允许」直通）时的兜底。
///
/// `acceptEdits` 而不是 `bypassPermissions`：批了计划的意思是「这些改动我同意」，不是
/// 「以后任何命令都别问我」。正常路径下这一档由用户在卡片上三选一决定
/// （见 `Chat.tsx` 的 `PLAN_APPROVAL_ACTIONS`）。
const PLAN_APPROVED_PERMISSION_MODE: &str = "acceptEdits";

/// 「批准并自动放行」那一档的值。与 `defs::claude::DEFAULT_PERMISSION_MODE` 同值，
/// 但语义不同（那个是「新会话的默认档」，这个是「计划批准后要不要继续弹卡」的判据）。
const FULL_ACCESS_PERMISSION_MODE: &str = "bypassPermissions";

/// 宿主侧回答工具审批询问所需的一切。
///
/// 存在的意义只有一个：**把外部 CLI 的权限询问接到 Kivio 已有的那条审批链路上**
/// （typed approval event → 前端确认卡 → `chat_confirm_tool_call` → 同一张
/// `pending_chat_tool_approvals` 挂起表）。内置 agent 用的就是这一套，不许再造第二套
/// （spec 第 2 条）。唯一需要的适配是 **id 映射**：Kivio 那侧按「工具调用 id」寻址，
/// 而 CLI 给的是它自己的 `request_id` —— 所以卡片用 claude 的 `tool_use_id`（与工具卡同一个
/// id），回程按 `request_id` 路由，两者在 `ApprovalAsk` 里成对携带。
struct ApprovalHost<'a> {
    app: &'a AppHandle,
    state: &'a AppState,
    conversation_id: &'a str,
    run_id: &'a str,
    generation: u64,
    /// 用来找 `ask_user::codec_for`。问用户按 agent 分发，不按折叠后的工具名
    /// （`AskUserQuestion` 和 `ask_user_question` 折叠后一样）。
    agent_id: &'a str,
    /// 普通工具就地放行、不弹卡（「完全」档；见 `claude_mode_auto_allows_tools`）。
    /// 这一档接上询问通道**只为了**问用户 / 计划卡，不是为了开始审批。
    ///
    /// 可变（`AtomicBool`）是因为用户可以在**轮中**用「批准并自动放行」把整轮切进这一档。
    auto_allow_tools: std::sync::atomic::AtomicBool,
}

impl ApprovalHost<'_> {
    /// 问用户一次。返回的 `ApprovalDecision` 带回 `request_id`，会话据它回 `control_response`。
    ///
    /// 超时 / 用户点停止时 `request_tool_approval` 自己会返回 false 并清掉挂起条目
    /// —— 也就是**默认拒**，这正是 fail-closed 想要的。
    async fn ask(
        &self,
        ask: crate::external_agents::session::live::ApprovalAsk,
    ) -> crate::external_agents::session::live::ApprovalDecision {
        // 复用 `ToolCallRecord` 作为审批卡的载体：`request_tool_approval` 的入参就是它，
        // 而它的 `format_tool_approval_summary` 已经会把 claude 的 `Write`/`file_path`
        // 摘成人看得懂的一两行。只填审批需要的字段，其余留默认。
        let record = ToolCallRecord {
            id: ask.tool_call_id.clone(),
            name: ask.tool_name.clone(),
            source: "external_cli".to_string(),
            server_id: None,
            arguments: serde_json::to_string(&ask.input).unwrap_or_else(|_| "{}".to_string()),
            status: ToolCallStatus::Running,
            result_preview: None,
            error: None,
            duration_ms: None,
            started_at: None,
            completed_at: None,
            round: 1,
            sensitive: true,
            artifacts: vec![],
            trace_id: None,
            span_id: None,
            structured_content: None,
        };
        // 问用户不是「要不要放行这个工具」，而是 CLI 在问用户一个问题。
        // 官方答法走各 CLI 自己的线（claude: 同一条 `can_use_tool` 回
        // `allow + updatedInput`；dsh: `session/ask` 回 `{ answers }`），
        // 卡片是 Kivio 已有的那张 —— 不该为每条 CLI 再造一套（spec 第 2 条）。
        if let Some(codec) =
            crate::external_agents::ask_user::codec_for(self.agent_id, &ask.tool_name)
        {
            if let Some(prompt) = (codec.parse)(&ask.input) {
                return self
                    .present_ask_user(&ask, record, prompt, |prompt, answered| {
                        (codec.encode)(&ask.input, prompt, answered)
                    })
                    .await;
            }
            if matches!(
                codec.unknown_shape,
                crate::external_agents::ask_user::UnknownAskShape::Reject
            ) {
                // 形状不认识：回拒。退审批卡没有意义（这条 CLI 等的不是 allow/deny）。
                return crate::external_agents::session::live::ApprovalDecision {
                    request_id: ask.request_id,
                    approved: false,
                    updated_input: None,
                    set_permission_mode: None,
                };
            }
            // FallbackApproval：退回普通审批卡，别静默吞掉这次询问。
        }
        // `ExitPlanMode` = 「计划写完了，批准我照着做」。走普通审批卡（卡片上就是计划正文，
        // 见 `format_tool_approval_summary`），但批准时**必须额外切档位** —— CLI 不会因为这次
        // `allow` 自己离开计划档，不切的话它下一句 `Edit` 又被挡回来，用户点了「批准」却什么
        // 都没发生。目标档位取 `acceptEdits`（paseo 同款默认：批了计划就别再为每次编辑弹卡）。
        //
        // 这一条**排在 `auto_allow_tools` 之前**：批准一个计划是真正的用户决定，
        // 「完全」档也不该替他点头。
        //
        // ponytail: 只切**本次会话**的档位，不改会话配置里的 `external_sandbox`
        // （底栏胶囊仍显示「计划」）。够用是因为常驻会话不会因此重启；进程真的重启时会退回
        // 计划档、需要重新批准。要让胶囊跟着变，得在跑轮途中改会话配置 —— 那是另一件事。
        if crate::external_agents::session::claude_stream::is_exit_plan_mode(&ask.tool_name) {
            let outcome = crate::chat::commands::interaction::request_tool_approval_outcome(
                self.app,
                self.state,
                self.conversation_id,
                self.run_id,
                self.generation,
                &record,
            )
            .await;
            let mode = outcome.approved.then(|| {
                outcome
                    .permission_mode
                    .clone()
                    .unwrap_or_else(|| PLAN_APPROVED_PERMISSION_MODE.to_string())
            });
            // 「批准并自动放行」= 这一轮剩下的工具也别再弹卡了。`auto_allow_tools` 是开轮时
            // 从 argv 读的（那时还是计划档），不就地翻掉的话用户选了「自动放行」却还要一个个
            // 点 —— 正是他刚刚选的那一档要消灭的东西。
            if mode.as_deref() == Some(FULL_ACCESS_PERMISSION_MODE) {
                self.auto_allow_tools
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return crate::external_agents::session::live::ApprovalDecision {
                request_id: ask.request_id,
                approved: outcome.approved,
                updated_input: None,
                set_permission_mode: mode,
            };
        }
        // `EnterPlanMode` = claude 自己要求「先探索、出方案，别急着改」。**放行就够** ——
        // 它的实现里自己就把会话切进了计划档，宿主不用发切档帧（与 `ExitPlanMode` 不对称，
        // 见 `claude_stream::is_enter_plan_mode`）。
        //
        // 同样排在 `auto_allow_tools` 之前：把一次生成中途变成只读是用户的决定，
        // 「完全」档也不该替他点头。底栏胶囊由前端在批准后写成「计划」（同计划批准那条路）。
        if crate::external_agents::session::claude_stream::is_enter_plan_mode(&ask.tool_name) {
            let approved = crate::chat::commands::interaction::request_tool_approval(
                self.app,
                self.state,
                self.conversation_id,
                self.run_id,
                self.generation,
                &record,
            )
            .await;
            return crate::external_agents::session::live::ApprovalDecision {
                request_id: ask.request_id,
                approved,
                updated_input: None,
                set_permission_mode: None,
            };
        }
        // 「完全」档：通道之所以接上只为了上面那两张卡，普通工具原地放行。
        // 少了这一条，选了「全自动放行」的用户会突然开始每个工具都被问一次。
        if self
            .auto_allow_tools
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return crate::external_agents::session::live::ApprovalDecision {
                request_id: ask.request_id,
                approved: true,
                updated_input: None,
                set_permission_mode: None,
            };
        }
        let approved = crate::chat::commands::interaction::request_tool_approval(
            self.app,
            self.state,
            self.conversation_id,
            self.run_id,
            self.generation,
            &record,
        )
        .await;
        crate::external_agents::session::live::ApprovalDecision {
            request_id: ask.request_id,
            approved,
            updated_input: None,
            set_permission_mode: None,
        }
    }

    /// 弹 Kivio 已有的问用户卡，答完补发带答案的工具记录并落盘。
    ///
    /// 答完把**带答案**的记录补发一次，消息流里才留得下「问了什么 + 选了什么」。
    /// 少了这一步，那条工具卡的载荷永远停在「等待作答」，而 CLI 随后回的
    /// `tool_result` 又不带结构化内容 ⇒ 看着像「答完什么都没留下」。
    /// 内置 agent 那条路早就这么做了（`agent/execute.rs` 的 ask_user 分支）。
    /// 同一份载荷再留一份给**落盘记录**：流解析层建的 tool 记录只有原始入参，
    /// 答案只有这里知道。不留的话刷新一次「问了什么 + 选了什么」就没了。
    async fn present_ask_user(
        &self,
        ask: &crate::external_agents::session::live::ApprovalAsk,
        record: ToolCallRecord,
        prompt: crate::chat::ask_user::AskUserPromptPayload,
        encode: impl FnOnce(
            &crate::chat::ask_user::AskUserPromptPayload,
            &crate::chat::ask_user::AskUserResponseResult,
        ) -> serde_json::Value,
    ) -> crate::external_agents::session::live::ApprovalDecision {
        let answered = crate::chat::commands::interaction::request_user_response(
            self.app,
            self.state,
            self.conversation_id,
            self.run_id,
            self.generation,
            &record,
            prompt.clone(),
        )
        .await;
        let mut answered_record = record;
        answered_record.status = ToolCallStatus::Success;
        answered_record.completed_at = Some(chrono::Local::now().timestamp());
        answered_record.sensitive = false;
        answered_record.structured_content = Some(crate::chat::ask_user::structured_content(
            &prompt,
            &answered.phase,
            &answered.answers,
        ));
        crate::chat::commands::interaction::emit_chat_tool_record(
            self.app,
            self.run_id,
            &answered_record,
        );
        if let Some(content) = answered_record.structured_content.clone() {
            self.state
                .answered_ask_user_content
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .insert(answered_record.id.clone(), content);
        }
        let approved = answered.phase == crate::chat::ask_user::ASK_USER_PHASE_ANSWERED;
        crate::external_agents::session::live::ApprovalDecision {
            request_id: ask.request_id.clone(),
            approved,
            updated_input: approved.then(|| encode(&prompt, &answered)),
            set_permission_mode: None,
        }
    }

    /// 扫掉本轮问过的挂起条目。异常出口（EOF / 硬 Close）下 `ask` 的 future 会被直接丢弃，
    /// 它没机会自己清理，不扫就在这张进程级的表里永久留一条。
    ///
    /// **同时要撤掉前端卡片**：`request_tool_approval` 只在自己的超时 / 取消分支里发撤回，
    /// 而 future 被丢弃时那两条分支根本不执行。少了这一手，卡片会留在屏幕上，用户点
    /// 「允许」是静默空操作（挂起条目已经被下面删掉了）—— 正是撤回事件本来要消灭的那个现象。
    fn forget(&self, tool_call_ids: &[String]) {
        if tool_call_ids.is_empty() {
            return;
        }
        {
            let mut pending = self
                .state
                .pending_chat_tool_approvals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for id in tool_call_ids {
                pending.remove(id);
            }
        }
        {
            let mut pending = self
                .state
                .pending_chat_user_prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for id in tool_call_ids {
                pending.remove(id);
            }
        }
        for id in tool_call_ids {
            crate::chat::commands::interaction::withdraw_tool_confirm(self.app, id);
            crate::chat::protocol::resolve_user_prompt(self.app, self.run_id, id);
        }
    }
}

fn persistent_protocol_tag(protocol: StreamFormat) -> &'static str {
    match protocol {
        StreamFormat::ClaudeStreamJson => "claude_stream_json",
        StreamFormat::CodexAppServer => "codex_app_server",
        StreamFormat::AcpJsonRpc => "acp_json_rpc",
        StreamFormat::PiRpc => "pi_rpc",
        StreamFormat::DshJsonRpc => "dsh_json_rpc",
    }
}

/// 一次「建立常驻会话」的产物。
///
/// `child_pid` 是纯元数据（写进注册表条目供诊断 / 判定「两轮是不是同一个进程」），
/// 任何关停路径都走 actor 的 `Close`，绝不按 pid 杀。
struct PersistentConnection {
    control: tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>,
    native_id: String,
    resumed: bool,
    child_pid: Option<u32>,
}

/// claude 轮间空闲读的副作用旁路：会话层没有（也不该有）`AppHandle`，
/// 这里把「upsert 注册表 / 追加唤醒消息」包成闭包递给 actor。轮内的同名任务事件走
/// `apply_unified_event` 的正常事件流，两条路对同一注册表做幂等 upsert。
fn background_task_sink(
    app: &AppHandle,
    conversation_id: &str,
) -> crate::external_agents::session::claude_stream::BackgroundTaskSink {
    use crate::external_agents::session::claude_stream::IdleSideEffect;
    let app = app.clone();
    let conversation_id = conversation_id.to_string();
    std::sync::Arc::new(move |effect| match effect {
        IdleSideEffect::Task(UnifiedAgentEvent::BackgroundTask {
            task_id,
            status,
            kind,
            description,
            summary,
        }) => {
            app.state::<AppState>().upsert_external_background_task(
                &conversation_id,
                &task_id,
                &status,
                kind.as_deref(),
                description.as_deref(),
                summary.as_deref(),
            );
        }
        IdleSideEffect::Task(_) => {}
        // sink 是同步闭包，持久化是 async —— 甩给运行时，失败只打日志（唤醒消息
        // 是锦上添花，不能因为它把会话 actor 拖死）。
        IdleSideEffect::WakeMessage { text, model, usage } => {
            let app = app.clone();
            let conversation_id = conversation_id.clone();
            tauri::async_runtime::spawn(async move {
                append_wake_turn_message(app, conversation_id, text, model, usage).await;
            });
        }
    })
}

fn dsh_idle_sink(
    app: &AppHandle,
    conversation_id: &str,
) -> crate::external_agents::session::dsh_jsonrpc::DshIdleSink {
    use crate::external_agents::session::dsh_jsonrpc::DshIdleEffect;
    let app = app.clone();
    let conversation_id = conversation_id.to_string();
    std::sync::Arc::new(move |effect| match effect {
        DshIdleEffect::Event(event) => apply_idle_dsh_event(&app, &conversation_id, event),
        DshIdleEffect::Wake { text, model, usage } => {
            let app = app.clone();
            let conversation_id = conversation_id.clone();
            tauri::async_runtime::spawn(async move {
                append_wake_turn_message(app, conversation_id, text, model, usage).await;
            });
        }
    })
}

fn apply_idle_dsh_event(app: &AppHandle, conversation_id: &str, event: UnifiedAgentEvent) {
    match event {
        UnifiedAgentEvent::BackgroundTask {
            task_id,
            status,
            kind,
            description,
            summary,
        } => {
            app.state::<AppState>().upsert_external_background_task(
                conversation_id,
                &task_id,
                &status,
                kind.as_deref(),
                description.as_deref(),
                summary.as_deref(),
            );
            if status == "running" && summary.is_none() {
                return;
            }
            let preview = summary.clone().unwrap_or_default();
            emit_live_subagent_progress(
                app,
                conversation_id,
                &task_id,
                &status,
                preview.clone(),
                Vec::new(),
            );
            persist_dsh_subagent_card(
                app.clone(),
                conversation_id.to_string(),
                task_id,
                status,
                preview,
                Vec::new(),
                summary,
            );
        }
        UnifiedAgentEvent::SubagentProgress {
            task_id,
            status,
            preview,
            steps,
        } => {
            emit_live_subagent_progress(
                app,
                conversation_id,
                &task_id,
                &status,
                preview.clone(),
                steps.clone(),
            );
            persist_dsh_subagent_card(
                app.clone(),
                conversation_id.to_string(),
                task_id,
                status,
                preview,
                steps,
                None,
            );
        }
        _ => {}
    }
}

fn emit_live_subagent_progress(
    app: &AppHandle,
    conversation_id: &str,
    task_id: &str,
    status: &str,
    preview: String,
    steps: Vec<String>,
) {
    crate::chat::protocol::emit_live_run_event(
        app,
        conversation_id,
        crate::chat::protocol::ChatRunEvent::SubagentUpdated {
            parent_tool_call_id: String::new(),
            task_id: task_id.to_string(),
            name: "subagent".to_string(),
            model: None,
            depth: 1,
            status: status.to_string(),
            preview: (!preview.is_empty()).then_some(preview),
            steps,
        },
    );
}

fn persist_dsh_subagent_card(
    app: AppHandle,
    conversation_id: String,
    task_id: String,
    status: String,
    preview: String,
    steps: Vec<String>,
    summary: Option<String>,
) {
    tauri::async_runtime::spawn(async move {
        let now = Local::now().timestamp();
        if let Err(err) = crate::chat::repository::repository(&app)
            .mutate(&app, &conversation_id, move |conversation| {
                for message in conversation.messages.iter_mut().rev() {
                    let Some(record) = find_dsh_subagent_record(&mut message.tool_calls, &task_id)
                    else {
                        continue;
                    };
                    if !steps.is_empty() || !preview.is_empty() {
                        merge_subagent_progress(record, &preview, &steps, &status, &task_id);
                    } else {
                        attach_child_session_id(record, &task_id);
                    }
                    if status != "running" {
                        record.status = match status.as_str() {
                            "failed" => ToolCallStatus::Error,
                            "stopped" | "cancelled" => ToolCallStatus::Cancelled,
                            _ => ToolCallStatus::Success,
                        };
                        if let Some(summary) = summary.as_ref().filter(|text| !text.is_empty()) {
                            record.result_preview = Some(truncate_for_preview(summary, 800));
                        }
                        record.completed_at = Some(now);
                    }
                    return Ok(());
                }
                Ok(())
            })
            .await
        {
            eprintln!("[external-agent] persist dsh subagent card failed: {err}");
        }
    });
}

/// 把唤醒轮（后台任务完成后 CLI 自起的一轮）的正文落成一条真正的助手消息，
/// 并走标准 run 协议事件让**打开着的窗口实时看到**。
///
/// 合成 run 可行的依据：前端对 `run_started` 的处理是窗口无关的（恢复 / 多窗口场景
/// 本来就要求它渲染「不是自己发起的 run」，见 Chat.tsx 的 onChatStream——不核对
/// 发起方，收到就建流快照），`run_completed` 触发它按 `conversation_revision`
/// 重新拉会话，读到的就是这里持久化的消息。先落盘再发事件：落盘失败时不留幽灵 run。
async fn append_wake_turn_message(
    app: AppHandle,
    conversation_id: String,
    text: String,
    model: Option<String>,
    usage: Option<ModelUsage>,
) {
    let message_id = format!("msg_{}", Uuid::new_v4());
    // ChatMessage 字段全有 serde 默认值，走最小 JSON 构造（与 repository 单测同款），
    // 免得每加一个字段这里就要跟着改。model/usage 是出处证明：让消息元信息栏显示
    // 真实模型与用量，而不是估算的「~N tokens」。
    let message: crate::chat::types::ChatMessage = match serde_json::from_value(serde_json::json!({
        "id": message_id,
        "role": "assistant",
        "content": text,
        "model": model,
        "usage": usage,
        "timestamp": Local::now().timestamp(),
    })) {
        Ok(message) => message,
        Err(err) => {
            eprintln!("wake-turn message build failed: {err}");
            return;
        }
    };
    let persisted = match crate::chat::repository::repository(&app)
        .append_message(&app, &conversation_id, message)
        .await
    {
        Ok(conversation) => conversation,
        Err(err) => {
            eprintln!("wake-turn message persist failed: {err:?}");
            return;
        }
    };
    let run_id = format!("ext-wake-{}", Uuid::new_v4());
    crate::chat::protocol::register_run(
        &app,
        &conversation_id,
        &run_id,
        &message_id,
        persisted.revision.saturating_sub(1),
    );
    crate::chat::protocol::emit_run_event(
        &app,
        &run_id,
        crate::chat::protocol::ChatRunEvent::TextDelta {
            delta: text.clone(),
            segment: None,
        },
    );
    crate::chat::protocol::finish_run(&app, &run_id, "done", &text, persisted.revision);
}

/// Connect (or resume) a persistent protocol session, returning its control channel, native id,
/// and whether a resume actually succeeded. Falls back to a fresh session if resume fails.
///
/// `background_task_sink`：claude 轮间空闲读到的后台任务事件旁路（→ AppState 注册表）。
/// `dsh_idle_sink`：dsh 轮间的任务边沿 + 子代理进度 + 唤醒轮正文。
#[allow(clippy::too_many_arguments)]
async fn connect_persistent_session(
    protocol: StreamFormat,
    resolved_bin: &std::path::Path,
    args: &[String],
    cwd: &std::path::Path,
    model: Option<&str>,
    reasoning: Option<&str>,
    sandbox: Option<&str>,
    preset: Option<&str>,
    mcp_servers: &[AcpMcpServer],
    resume_native: Option<String>,
    background_task_sink: Option<
        crate::external_agents::session::claude_stream::BackgroundTaskSink,
    >,
    dsh_idle_sink: Option<crate::external_agents::session::dsh_jsonrpc::DshIdleSink>,
) -> Result<PersistentConnection, String> {
    use crate::external_agents::session::acp::{spawn_acp_session_actor, AcpSession};
    use crate::external_agents::session::claude_stream::{
        spawn_claude_stream_session_actor_with_sink, ClaudeStreamJsonSession,
    };
    use crate::external_agents::session::codex_app_server::{
        spawn_codex_session_actor, CodexAppServerSession,
    };
    use crate::external_agents::session::dsh_jsonrpc::{
        spawn_dsh_session_actor_with_sink, DshJsonRpcSession,
    };

    match protocol {
        StreamFormat::ClaudeStreamJson => {
            // claude 的会话 id 走**启动参数**，不像 codex / ACP 在握手 RPC 里传 ——
            // 所以这里要看/改 argv 而不是传参。
            //
            // **参数说了算**：`build_claude_args` 已按 `resolve_agent_resume_context` 放好
            // `--resume <id>`（续接已有会话）或 `--session-id <new>`（这个对话还没有会话）。
            // 参数里的会话标记是**本轮的真实决定**；live handle 只是「上一个进程最后报的那个
            // id」。让 handle 压过参数，会把一个全新对话接到某个旧会话上 —— 用户会在新对话里
            // 看到别人的上下文。
            //
            // （历史：这里曾解释成「换模型要开新会话」。2026-07-29 本机实测 claude 2.1.220
            // 推翻了那个前提 —— `--resume` 带另一个 `--model` 既切得动模型、也保住上下文，
            // 所以换模型现在照常续接。优先级本身不变，只是理由换了。见 spec 第 8 / 25 条。）
            //
            // live handle 只在参数里**没有**任何会话 flag 时兜底，且必须改写成 `--resume`：
            // 同一个 id 再 `--session-id` 一次会被 claude 以「id 已存在」拒绝启动。
            let (effective_args, resumed) = if args.iter().any(|arg| arg == "--resume") {
                (args.to_vec(), true)
            } else if args.iter().any(|arg| arg == "--session-id") {
                (args.to_vec(), false)
            } else if let Some(id) = resume_native
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                (
                    crate::external_agents::defs::claude::claude_args_resuming(args, id),
                    true,
                )
            } else {
                (args.to_vec(), false)
            };
            let session =
                ClaudeStreamJsonSession::connect(resolved_bin, &effective_args, cwd).await?;
            let id = session.session_id().to_string();
            let child_pid = session.child_pid();
            Ok(PersistentConnection {
                control: spawn_claude_stream_session_actor_with_sink(session, background_task_sink),
                native_id: id,
                resumed,
                child_pid,
            })
        }
        StreamFormat::CodexAppServer => {
            if let Some(tid) = resume_native.as_deref() {
                if let Ok(session) = CodexAppServerSession::connect(
                    resolved_bin,
                    args,
                    cwd,
                    model,
                    sandbox,
                    Some(tid),
                )
                .await
                {
                    let id = session.thread_id().to_string();
                    let child_pid = session.child_pid();
                    return Ok(PersistentConnection {
                        control: spawn_codex_session_actor(session),
                        native_id: id,
                        resumed: true,
                        child_pid,
                    });
                }
                // C3: resume failed → fall through to fresh so the caller overwrites the stale
                // live handle (whose native_id is dead) instead of retrying a doomed resume.
                // 同 ACP 那条：原因要留在日志里，否则「上下文已重置」提示查不到根因。
                eprintln!("[external-agent] codex resume failed (thread {tid}), connecting fresh");
            }
            let session =
                CodexAppServerSession::connect(resolved_bin, args, cwd, model, sandbox, None)
                    .await?;
            let id = session.thread_id().to_string();
            let child_pid = session.child_pid();
            Ok(PersistentConnection {
                control: spawn_codex_session_actor(session),
                native_id: id,
                resumed: false,
                child_pid,
            })
        }
        StreamFormat::AcpJsonRpc => {
            if let Some(sid) = resume_native.as_deref() {
                match AcpSession::connect(
                    resolved_bin,
                    args,
                    cwd,
                    model,
                    reasoning,
                    mcp_servers,
                    Some(sid),
                )
                .await
                {
                    Ok(session) => {
                        let id = session.session_id().to_string();
                        let child_pid = session.child_pid();
                        return Ok(PersistentConnection {
                            control: spawn_acp_session_actor(session),
                            native_id: id,
                            resumed: true,
                            child_pid,
                        });
                    }
                    // C3: resume failed → connect fresh; the caller's save_live_handle overwrites
                    // the stale handle so the next turn won't attempt the dead native_id again.
                    // **原因必须打出来**：这条路的下游是一条「上下文已重置」提示，而用户看到
                    // 提示时唯一能查的就是日志。之前这里是 `if let Ok(..)`，`session/load` 的
                    // 报错被整个丢掉，只剩一句「失败了」。
                    Err(err) => {
                        eprintln!(
                            "[external-agent] acp resume failed (session {sid}), connecting fresh: {err}"
                        );
                    }
                }
            }
            let session =
                AcpSession::connect(resolved_bin, args, cwd, model, reasoning, mcp_servers, None)
                    .await?;
            let id = session.session_id().to_string();
            let child_pid = session.child_pid();
            Ok(PersistentConnection {
                control: spawn_acp_session_actor(session),
                native_id: id,
                resumed: false,
                child_pid,
            })
        }
        StreamFormat::DshJsonRpc => {
            // 与 ACP / codex 同口径：续接失败且目标原生会话已经不在，降级开新会话，
            // 而不是把 session/open 的原文甩成一轮硬失败。resume mismatch / 其它握手错误
            // 仍 fail-loud（那不是「会话没了」）。
            let session = match DshJsonRpcSession::connect(
                resolved_bin,
                args,
                cwd,
                resume_native.as_deref(),
                model,
                reasoning,
                sandbox,
                preset,
            )
            .await
            {
                Ok(session) => session,
                Err(err)
                    if resume_native.as_deref().is_some_and(|id| !id.trim().is_empty())
                        && crate::external_agents::session::dsh_jsonrpc::is_missing_session_error(
                            &err,
                        ) =>
                {
                    eprintln!(
                        "[external-agent] dsh resume failed (session {}), connecting fresh: {err}",
                        resume_native.as_deref().unwrap_or("")
                    );
                    DshJsonRpcSession::connect(
                        resolved_bin,
                        args,
                        cwd,
                        None,
                        model,
                        reasoning,
                        sandbox,
                        preset,
                    )
                    .await?
                }
                Err(err) => return Err(err),
            };
            let id = session.session_id().to_string();
            let resumed = session.resumed();
            let child_pid = session.child_pid();
            Ok(PersistentConnection {
                control: spawn_dsh_session_actor_with_sink(session, dsh_idle_sink),
                native_id: id,
                resumed,
                child_pid,
            })
        }
        StreamFormat::PiRpc => Err("protocol does not support persistent sessions".to_string()),
    }
}

/// 文本 / 推理分段的 phase。
///
/// **文本段绝不能标 ToolLoop。** 落库时 `message.content` 是由
/// `chat::commands::messages::content_from_segments` 从分段反算出来的，而那把尺只认
/// `Plain | Synthesis` 的文本段；外部 CLI 这边的 `content` 却是**所有** `TextDelta` 的累加。
/// 一旦工具之后的正文被标成 ToolLoop，那把尺就看不见它 → `normalize_assistant_segments`
/// 判定「有正文但没有任何分段覆盖」→ 再补一条同文案的 Synthesis 段 → 气泡里正文渲染两遍。
/// 所以调过工具之后的正文标 `Synthesis`（语义也对：那就是工具跑完后的汇报），与内置路径
/// `chat/agent/planning.rs` 里「正文段必须是 Plain|Synthesis」的约定一致。
///
/// 推理段不受这条约束（`reasoning_from_segments` 不看 phase），保持原有 ToolLoop 语义。
pub(crate) fn segment_phase_for_tool_count(
    kind: &ChatMessageSegmentKind,
    tool_calls_len: usize,
) -> ChatMessageSegmentPhase {
    if tool_calls_len == 0 {
        return ChatMessageSegmentPhase::Plain;
    }
    match kind {
        ChatMessageSegmentKind::Reasoning => ChatMessageSegmentPhase::ToolLoop,
        _ => ChatMessageSegmentPhase::Synthesis,
    }
}

/// 把一段「轮末补充文案」（分类错误气泡 / stderr / 占位文案）**同时**写进 `content` 和
/// `segments`。两者必须一起长：读流结束后单独往 `content` 上拼字符串，会让
/// 「正文」与「被 `content_from_segments` 认可的分段文字」对不上——落库时要么正文被
/// 渲染两遍（正文一条分段都没覆盖时），要么这段补充文案被整段丢掉（已有分段覆盖时）。
fn append_final_text(
    content: &mut String,
    segments: &mut Vec<ChatMessageSegment>,
    segment_order: &mut u32,
    tool_calls_len: usize,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }
    if !content.trim().is_empty() {
        content.push_str("\n\n");
    }
    content.push_str(text);
    *segment_order += 1;
    segments.push(ChatMessageSegment {
        id: format!("seg_{}", Uuid::new_v4()),
        kind: ChatMessageSegmentKind::Text,
        phase: segment_phase_for_tool_count(&ChatMessageSegmentKind::Text, tool_calls_len),
        order: *segment_order,
        step_number: None,
        round: None,
        text: Some(text.to_string()),
        tool_call_id: None,
    });
}

fn push_tool_segment(
    segments: &mut Vec<ChatMessageSegment>,
    segment_order: &mut u32,
    tool_call_id: &str,
) -> ChatMessageSegment {
    *segment_order += 1;
    let segment = ChatMessageSegment {
        id: format!("seg_{}", Uuid::new_v4()),
        kind: ChatMessageSegmentKind::Tool,
        phase: ChatMessageSegmentPhase::ToolLoop,
        order: *segment_order,
        step_number: None,
        round: Some(1),
        text: None,
        tool_call_id: Some(tool_call_id.to_string()),
    };
    segments.push(segment.clone());
    segment
}

/// 构造一条 CLI 自压的边界记录，并发协议压缩更新通知前端插分隔线。
///
/// payload 与内置路径（`chat/commands/context.rs::emit_chat_compaction_state`）同形，
/// 且**直接复用 `CompactionBoundaryRecord`** 而不是手写一份 json——两份形状迟早分叉
/// （spec 第 2 条）。
///
/// 返回记录供调用方落盘：live 事件与持久化必须是**同一条记录**（同一个 id），
/// 否则刷新后会出现两条分隔线。
///
/// `token_estimate_after` 取 `compact_metadata.post_tokens`（claude 2.1.220 确实有这个字段）；
/// 缺失时为 0，前端此时不显示「→ N」。
fn emit_cli_compaction(
    app: &AppHandle,
    run_id: &str,
    anchor_message_id: &str,
    trigger: &str,
    pre_tokens: Option<u64>,
    post_tokens: Option<u64>,
    now: i64,
) -> CompactionBoundaryRecord {
    let boundary = CompactionBoundaryRecord {
        id: format!("ctxbd_{}", Uuid::new_v4()),
        // CLI 内部压缩，Kivio 拿不到「摘要覆盖到哪条消息」——这是 CLI 自己的上下文切分点，
        // 协议里也不上报。留空并靠 `display_after_message_id` 做时间线锚点。
        source_until_message_id: String::new(),
        display_after_message_id: Some(anchor_message_id.to_string()),
        token_estimate_before: pre_tokens.unwrap_or(0) as usize,
        token_estimate_after: post_tokens.unwrap_or(0) as usize,
        // 摘要正文只存在于 CLI 自己的会话里，协议不上报。
        summary_content: String::new(),
        // CLI 自压沿用 CLI 自报的 trigger（`auto` / `manual`），与内置的 `agent_loop`
        // 区分开——排查时能看出压缩是谁触发的。
        trigger: trigger.to_string(),
        created_at: now,
    };
    crate::chat::protocol::emit_run_event(
        app,
        run_id,
        crate::chat::protocol::ChatRunEvent::CompactionUpdated {
            phase: "completed".to_string(),
            trigger: Some(trigger.to_string()),
            boundary: Some((&boundary).into()),
        },
    );
    boundary
}

/// 非零退出码是否应判定为本轮失败。
///
/// `protocol_completed`（读到了 CLI 明确的本轮结束帧）时**一律豁免**：spec 第 8b 条记的
/// 已知坑是 Windows `TerminateProcess` 退出码恒为 1，配合「非零退出 + 有 stderr = error」
/// 会把正常收尾的轮次误判成失败；杀整棵进程树后中招面进一步变大。判据改为**协议层完成标志**
/// 而不是退出码形态——真实的协议层失败（`result.is_error` 等）走 `resolve_turn_error`
/// 那条路，不依赖退出码。
///
/// **常驻路径上这条规则天然不适用**（B1 起）：进程不再每轮退出，`exit_code` 恒为 `None`
/// （只有非持久分支才 `wait()` 子进程），所以本函数在 claude / codex / ACP 上恒返回 false。
/// 退出码该归给哪一轮的问题于是不存在了 —— 常驻进程的退出发生在**会话关闭**时（idle 回收 /
/// LRU 淘汰 / 配置变更重连 / 应用退出），与任何单轮都无关，那条路径上也没有气泡可污染。
/// 目前只有 `PiRpc` 还走非持久分支，而它不上报协议层完成标志（`protocol_completed` 恒 false），
/// 所以对它规则照旧生效。
fn nonzero_exit_is_a_failure(exit_code: Option<i32>, protocol_completed: bool) -> bool {
    !protocol_completed && exit_code.map(|code| code != 0).unwrap_or(false)
}

fn apply_unified_event(
    app: &AppHandle,
    run_id: &str,
    conversation_id: &str,
    compaction_anchor_id: &str,
    content: &mut String,
    reasoning: &mut String,
    raw_output: &mut String,
    tool_calls: &mut Vec<ToolCallRecord>,
    tool_map: &mut HashMap<String, usize>,
    usage: &mut Option<ModelUsage>,
    stream_error: &mut Option<String>,
    segments: &mut Vec<ChatMessageSegment>,
    segment_order: &mut u32,
    segment_tracker: &mut StreamSegmentTracker,
    cli_compactions: &mut Vec<CompactionBoundaryRecord>,
    todo_state: &mut AgentTodoState,
    event: UnifiedAgentEvent,
) {
    let now = Local::now().timestamp();
    match event {
        UnifiedAgentEvent::TextDelta { delta } => {
            content.push_str(&delta);
            let segment = segment_tracker.append(
                ChatMessageSegmentKind::Text,
                segments,
                segment_order,
                tool_calls.len(),
                &delta,
            );
            emit_chat_stream_delta(app, run_id, &delta, None, Some(&segment));
        }
        UnifiedAgentEvent::ThinkingDelta { delta } => {
            reasoning.push_str(&delta);
            let segment = segment_tracker.append(
                ChatMessageSegmentKind::Reasoning,
                segments,
                segment_order,
                tool_calls.len(),
                &delta,
            );
            emit_chat_stream_delta(app, run_id, "", Some(&delta), Some(&segment));
        }
        UnifiedAgentEvent::ToolUse { id, name, input } => {
            segment_tracker.reset_text();
            segment_tracker.reset_reasoning();
            let segment = push_tool_segment(segments, segment_order, &id);
            emit_chat_stream_delta(app, run_id, "", None, Some(&segment));
            let record = ToolCallRecord {
                id: id.clone(),
                name: name.clone(),
                source: "external_cli".to_string(),
                server_id: None,
                arguments: input.to_string(),
                status: ToolCallStatus::Running,
                result_preview: None,
                error: None,
                duration_ms: None,
                started_at: Some(now),
                completed_at: None,
                round: 1,
                sensitive: false,
                artifacts: vec![],
                trace_id: None,
                span_id: None,
                structured_content: Some(input),
            };
            tool_map.insert(id.clone(), tool_calls.len());
            tool_calls.push(record.clone());
            emit_chat_tool_record(app, run_id, &record);
        }
        UnifiedAgentEvent::ToolResult {
            tool_use_id,
            content: result_content,
            is_error,
        } => {
            if let Some(idx) = tool_map.get(&tool_use_id).copied() {
                if let Some(record) = tool_calls.get_mut(idx) {
                    apply_external_tool_result(record, &result_content, is_error, now);
                    // 问用户答完时留下的 `askUser` 载荷（问题 + 用户选的答案）在这里落进记录，
                    // 覆盖流解析层塞的原始入参 —— 否则消息流里那块刷新一次就只剩一行灰字。
                    if let Some(answered) = app
                        .state::<AppState>()
                        .answered_ask_user_content
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .remove(&tool_use_id)
                    {
                        record.structured_content = Some(answered);
                    }
                    let claude_todo = if !is_error
                        && crate::external_agents::claude_todo::is_claude_todo_tool(&record.name)
                    {
                        Some((
                            record.name.clone(),
                            record
                                .structured_content
                                .clone()
                                .unwrap_or(serde_json::Value::Null),
                            result_content.clone(),
                        ))
                    } else {
                        None
                    };
                    if let Some((name, input, result)) = claude_todo {
                        if let Some(next) = crate::external_agents::claude_todo::apply_claude_todo_tool(
                            todo_state,
                            &name,
                            &input,
                            &result,
                        ) {
                            *todo_state = next;
                            publish_todo_state(app, run_id, record, todo_state);
                            persist_claude_todo(app, conversation_id, name, input, result);
                        } else {
                            emit_chat_tool_record(app, run_id, record);
                        }
                    } else {
                        emit_chat_tool_record(app, run_id, record);
                    }
                }
            }
        }
        UnifiedAgentEvent::Usage { usage: u } => {
            // 只累积，不下发。上下文占用**一轮更新一次**，由轮末那次权威计算发出
            // （`compute_context_state` → `context_updated`）。
            //
            // 曾经这里有一条 350ms 节流的「实时」通道，删掉的理由是它三分之二不是真值：
            // 分子是**单次请求**的快照（工具循环里每请求一变、压缩后还会掉，看着就是在跳），
            // 分母是上一轮留下的粘滞值（claude 只在轮末 `result` 里报窗口），进度条里的分段
            // 构成则是前端按比例硬缩放出来的。而这个数字唯一能驱动的动作（压缩 / 换会话）
            // 只发生在两轮之间 —— 轮中看到也用不上。要改回来先想清楚这三点。
            *usage = Some(merge_cli_usage(usage.as_ref(), u));
        }
        UnifiedAgentEvent::Error { message, .. } => {
            eprintln!("[external-agent] stream error: {message}");
            // 协议层报的失败必须能走到出口的 `errors::classify`（spec 第 5 条）：只打日志的话，
            // 一条「CLI 明确说本轮失败了」的消息会被整个吞掉——claude 未登录时的
            // `{"subtype":"success","is_error":true}` 正是这样被当成成功轮次的。
            // 首条为准（后续多为同一失败的连带回声），不覆盖。
            if stream_error.is_none() {
                *stream_error = Some(message);
            }
        }
        UnifiedAgentEvent::Raw { line } => {
            // Unparsed stdout line — accumulate (capped) as a fallback surfaced only if the run
            // produced no structured content.
            if !raw_output.is_empty() {
                raw_output.push('\n');
            }
            raw_output.push_str(&line);
            if raw_output.chars().count() > 8192 {
                *raw_output = tail_chars(raw_output, 8192);
            }
        }
        UnifiedAgentEvent::CliCompacted {
            trigger,
            pre_tokens,
            post_tokens,
            dropped_tokens,
            duration_ms,
        } => {
            // CLI **自己**压缩了上下文（claude 的 compact_boundary）。Kivio 并没有发
            // `/compact`，所以不能走 `external_agents::compact` 那条路；这里做两件事：
            //   1. 发协议压缩更新让前端**立刻**插入分隔线——否则用户只会看到
            //      「对话突然变短了」而没有任何解释；
            //   2. 把同一条记录攒起来，读流结束后落到 `context_state`
            //      （计数 + 边界持久化，见调用方注释）。
            // 分子仍由 `message_start.message.usage` / `result` 上报（服务端算的），
            // 这里不推算用量。
            if cfg!(debug_assertions) {
                eprintln!(
                    "[external-agent] cli compaction trigger={trigger} pre={pre_tokens:?} post={post_tokens:?} dropped={dropped_tokens:?} duration_ms={duration_ms:?}"
                );
            }
            cli_compactions.push(emit_cli_compaction(
                app,
                run_id,
                compaction_anchor_id,
                &trigger,
                pre_tokens,
                post_tokens,
                now,
            ));
        }
        UnifiedAgentEvent::UserSteer { id, text } => {
            // 用户在这一轮里插了一句话，且 CLI 已受理注入。卡片走**内置循环那一份构造**
            // （`chat::agent::steering::build_steer_record`）：同一个工具名、同一个
            // structured_content 形状，前端那条 `isUserSteerToolCall` 判据与「收到卡才出队」
            // 的对账逻辑因此两条路共用，不需要为外部 CLI 再写一遍。
            segment_tracker.reset_text();
            segment_tracker.reset_reasoning();
            let Some(message) = crate::chat::agent::SteeringMessage::new(id, &text) else {
                return;
            };
            let record = crate::chat::agent::steering::build_steer_record(&message, 1);
            let segment = push_tool_segment(segments, segment_order, &record.id);
            emit_chat_stream_delta(app, run_id, "", None, Some(&segment));
            tool_map.insert(record.id.clone(), tool_calls.len());
            tool_calls.push(record.clone());
            emit_chat_tool_record(app, run_id, &record);
        }
        // 上游重试等瞬态状态 → 流状态行（StreamStatusLine），不进正文。
        // 前端在下一条正文/思考增量到达时自行清除（重试成功没有显式信号，流恢复即成功）。
        UnifiedAgentEvent::StatusNote { text } => {
            crate::chat::protocol::emit_run_event(
                app,
                run_id,
                crate::chat::protocol::ChatRunEvent::StatusNoteUpdated { note: Some(text) },
            );
        }
        // CLI 侧后台任务（后台 Bash / 后台子代理）→ AppState 注册表，
        // Background tasks 面板轮询读取。不进消息流：任务跨轮存活，run 级事件装不下它。
        UnifiedAgentEvent::BackgroundTask {
            task_id,
            status,
            kind,
            description,
            summary,
        } => {
            app.state::<AppState>().upsert_external_background_task(
                conversation_id,
                &task_id,
                &status,
                kind.as_deref(),
                description.as_deref(),
                summary.as_deref(),
            );
            // dsh 后台子代理的 tool/result 只是派出回执。终态走 BackgroundTask，
            // 同一轮里把对应工具卡从 Running 收掉（跨轮要靠空闲读，目前 dsh 没有）。
            if status != "running" {
                if let Some(record) = find_dsh_subagent_record(tool_calls, &task_id) {
                    record.status = match status.as_str() {
                        "failed" => ToolCallStatus::Error,
                        "stopped" => ToolCallStatus::Cancelled,
                        _ => ToolCallStatus::Success,
                    };
                    if let Some(summary) = summary.filter(|text| !text.is_empty()) {
                        record.result_preview = Some(truncate_for_preview(&summary, 800));
                    }
                    record.completed_at = Some(now);
                    emit_chat_tool_record(app, run_id, record);
                }
            }
        }
        UnifiedAgentEvent::SubagentProgress {
            task_id,
            status,
            preview,
            steps,
        } => {
            if let Some(record) = find_dsh_subagent_record(tool_calls, &task_id) {
                merge_subagent_progress(record, &preview, &steps, &status, &task_id);
                emit_chat_tool_record(app, run_id, record);
                crate::chat::protocol::emit_run_event(
                    app,
                    run_id,
                    crate::chat::protocol::ChatRunEvent::SubagentUpdated {
                        parent_tool_call_id: record.id.clone(),
                        task_id,
                        name: record.name.clone(),
                        model: None,
                        depth: 1,
                        status,
                        preview: (!preview.is_empty()).then_some(preview),
                        steps,
                    },
                );
            }
        }
        UnifiedAgentEvent::TodoWrite { todos } => {
            let Some(state) =
                crate::external_agents::session::dsh_jsonrpc::todo_state_from_write(&todos)
            else {
                return;
            };
            *todo_state = state.clone();
            if let Some(record) = tool_calls
                .iter_mut()
                .rev()
                .find(|record| record.name.eq_ignore_ascii_case("todo_write"))
            {
                publish_todo_state(app, run_id, record, todo_state);
            } else {
                crate::chat::protocol::emit_run_event(
                    app,
                    run_id,
                    crate::chat::protocol::ChatRunEvent::TodoUpdated {
                        todo_state: (&state).into(),
                    },
                );
            }
            let app = app.clone();
            let conversation_id = conversation_id.to_string();
            tauri::async_runtime::spawn(async move {
                match crate::chat::repository::repository(&app)
                    .update_todo(&app, &conversation_id, state)
                    .await
                {
                    Ok(persisted) => crate::chat::todo::emit_chat_todo_state(
                        &app,
                        &conversation_id,
                        persisted.revision,
                        &persisted.agent_todo_state,
                    ),
                    Err(err) => {
                        eprintln!("[external-agent] persist dsh todo failed: {err}");
                    }
                }
            });
        }
        _ => {}
    }
}

fn apply_external_tool_result(
    record: &mut ToolCallRecord,
    result_content: &str,
    is_error: bool,
    now: i64,
) {
    if !is_error {
        if let Some(task_id) = crate::external_agents::session::dsh_jsonrpc::subagent_launch_task_id(
            &record.name,
            result_content,
        ) {
            record.status = ToolCallStatus::Running;
            record.completed_at = None;
            record.result_preview = None;
            attach_background_task_id(record, &task_id);
            return;
        }
    }
    record.status = if is_error {
        ToolCallStatus::Error
    } else {
        ToolCallStatus::Success
    };
    record.result_preview = Some(truncate_for_preview(result_content, 800));
    record.completed_at = Some(now);
}

fn attach_background_task_id(record: &mut ToolCallRecord, task_id: &str) {
    let mut payload = record
        .structured_content
        .clone()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "backgroundTaskId".to_string(),
            serde_json::Value::String(task_id.to_string()),
        );
    }
    record.structured_content = Some(payload);
}

fn merge_subagent_progress(
    record: &mut ToolCallRecord,
    preview: &str,
    steps: &[String],
    status: &str,
    incoming_task_id: &str,
) {
    attach_child_session_id(record, incoming_task_id);
    let mut payload = record
        .structured_content
        .clone()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "subagentProgress".to_string(),
            serde_json::json!({
                "taskId": background_task_id(record)
                    .or_else(|| (!incoming_task_id.is_empty()).then(|| incoming_task_id.to_string())),
                "name": record.name,
                "status": status,
                "preview": preview,
                "steps": steps,
                "depth": 1,
            }),
        );
    }
    record.structured_content = Some(payload);
}

fn structured_string_field(record: &ToolCallRecord, key: &str) -> Option<String> {
    record
        .structured_content
        .as_ref()
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn background_task_id(record: &ToolCallRecord) -> Option<String> {
    structured_string_field(record, "backgroundTaskId")
}

fn child_session_id(record: &ToolCallRecord) -> Option<String> {
    structured_string_field(record, "childSessionId")
}

fn attach_child_session_id(record: &mut ToolCallRecord, task_id: &str) {
    if task_id.is_empty() || background_task_id(record).as_deref() == Some(task_id) {
        return;
    }
    if child_session_id(record).as_deref() == Some(task_id) {
        return;
    }
    let mut payload = record
        .structured_content
        .clone()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "childSessionId".to_string(),
            serde_json::Value::String(task_id.to_string()),
        );
    }
    record.structured_content = Some(payload);
}

/// 派出回执是 `jobs` 的 `jobId`（`subagent-1`），子会话 `session.event` 是另一个
/// `session.id`。对得上就精确绑；对不上且只有一张 Running 子代理卡时绑到那张。
fn find_dsh_subagent_record<'a>(
    tool_calls: &'a mut [ToolCallRecord],
    task_id: &str,
) -> Option<&'a mut ToolCallRecord> {
    if !task_id.is_empty() {
        if let Some(index) = tool_calls.iter().rposition(|record| {
            background_task_id(record).as_deref() == Some(task_id)
                || child_session_id(record).as_deref() == Some(task_id)
        }) {
            return tool_calls.get_mut(index);
        }
    }
    // ponytail: 并行多个子代理时不猜，避免把 A 的步骤写到 B 上。
    let running: Vec<usize> = tool_calls
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            record.status == ToolCallStatus::Running
                && crate::external_agents::session::dsh_jsonrpc::is_subagent_tool_name(&record.name)
        })
        .map(|(index, _)| index)
        .collect();
    if running.len() == 1 {
        return tool_calls.get_mut(running[0]);
    }
    None
}

/// 立刻把对话 Todo 条和工具卡接到同一份快照上（DSH 整表 / Claude 补丁共用）。
fn publish_todo_state(
    app: &AppHandle,
    run_id: &str,
    record: &mut ToolCallRecord,
    state: &AgentTodoState,
) {
    crate::chat::protocol::emit_run_event(
        app,
        run_id,
        crate::chat::protocol::ChatRunEvent::TodoUpdated {
            todo_state: state.into(),
        },
    );
    let mut payload = record
        .structured_content
        .clone()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(object) = payload.as_object_mut() {
        if let Ok(todo) = serde_json::to_value(state) {
            object.insert("todoState".to_string(), todo);
        }
    }
    record.structured_content = Some(payload);
    emit_chat_tool_record(app, run_id, record);
}

/// Claude Code 的 Task / 旧 TodoWrite 成功之后，在对话锁里补丁列表再发协议事件。
///
/// 必须进 `mutate`：`TaskUpdate` 是补丁，不能拿一份过期快照 `update_todo` 整表盖掉。
fn persist_claude_todo(
    app: &AppHandle,
    conversation_id: &str,
    name: String,
    input: serde_json::Value,
    result: String,
) {
    if crate::external_agents::claude_todo::apply_claude_todo_tool(
        &crate::chat::types::AgentTodoState::default(),
        &name,
        &input,
        &result,
    )
    .is_none()
    {
        return;
    }
    let app = app.clone();
    let conversation_id = conversation_id.to_string();
    tauri::async_runtime::spawn(async move {
        match crate::chat::repository::repository(&app)
            .mutate(&app, &conversation_id, move |conversation| {
                if let Some(next) = crate::external_agents::claude_todo::apply_claude_todo_tool(
                    &conversation.agent_todo_state,
                    &name,
                    &input,
                    &result,
                ) {
                    conversation.agent_todo_state = next;
                }
                Ok(())
            })
            .await
        {
            Ok(persisted) => crate::chat::todo::emit_chat_todo_state(
                &app,
                &conversation_id,
                persisted.revision,
                &persisted.agent_todo_state,
            ),
            Err(err) => {
                eprintln!("[external-agent] persist claude todo failed: {err}");
            }
        }
    });
}

/// 本轮的失败判据：读流错误与**协议层自报的失败**（`UnifiedAgentEvent::Error`）共用
/// 同一个出口，从而都能走到 `errors::classify`（spec 第 5 条）。
///
/// 为什么不能只看 `read_result`：读流经常**正常** `Ok` 返回（CLI 完整输出后 exit 0），
/// 失败只体现在流里的一条消息里。claude 未登录的真实样本
/// `{"type":"result","subtype":"success","is_error":true,"result":"Not logged in …"}`
/// 就是这样：进程干净退出、stdout 全是合法 JSON，于是整轮被判为「已完成」，
/// 用户拿到一个空气泡且零提示。
///
/// `read_result` 的错误优先：进程级失败带退出码与 stderr，`classify` 能给出更准的分类。
fn resolve_turn_error<'a>(
    read_error: Option<&'a String>,
    stream_error: Option<&'a String>,
) -> Option<&'a String> {
    read_error.or(stream_error)
}

/// 合并一轮内先后到达的多次 CLI 用量上报：**后到覆盖先到**（取最新快照），
/// 但两处例外：
///
/// 1. `context_window_tokens` **粘滞**——新值为 `None` 时保留旧值。
///    ACP 一轮里会到两次用量：`session/update` 的 `usage_update` 带 `size`（窗口）先到，
///    `session/prompt` 的 `PromptResponse.usage` 不带窗口后到；直接整体覆盖会让分母被后一条
///    冲成 `None`，用量条退回「窗口未知」。claude 侧方向相同：`message_start` 的 usage 不带
///    窗口、`result` 的带（`modelUsage.contextWindow`），后到覆盖正好把窗口补上。
/// 2. **全零的 token 数不覆盖非零的**。没有 LLM 往返的 `result`（未登录 / `/help` /
///    未知斜杠命令 / 我们自己发的 `/compact`）四个字段全 0，但它仍可能携带窗口。
///    若让它整体覆盖，本轮 `message_start` 报的真实占用会被清零 ⇒ 用量条从 47K 掉到 0
///    （`context.rs` 挑「最近一条 usage」的判据是 `is_some()`，`Some(0)` 会命中）。
///    这一条与 `stream/claude.rs` 的全零守卫是同一条规则的两道防线。
fn merge_cli_usage(previous: Option<&ModelUsage>, mut incoming: ModelUsage) -> ModelUsage {
    if incoming.context_window_tokens.is_none() {
        incoming.context_window_tokens = previous.and_then(|prev| prev.context_window_tokens);
    }
    if let Some(prev) = previous {
        if usage_tokens_all_zero(&incoming) && !usage_tokens_all_zero(prev) {
            let window = incoming.context_window_tokens;
            incoming = prev.clone();
            incoming.context_window_tokens = window;
        }
    }
    incoming
}

/// 一次上报里所有 token 数是否都是 0 / 缺失（= 这条上报没有任何分子信息）。
/// 这条用量上报有没有任何非零 token。`context.rs::usage_has_token_numbers` 是它的反面，
/// 两处必须共用同一个判据（此前 `reasoning_tokens` 只有这边算，见那里的注释）。
pub(crate) fn usage_tokens_all_zero(usage: &ModelUsage) -> bool {
    usage.input_tokens.unwrap_or(0) == 0
        && usage.output_tokens.unwrap_or(0) == 0
        && usage.total_tokens.unwrap_or(0) == 0
        && usage.cached_input_tokens.unwrap_or(0) == 0
        && usage.cache_creation_input_tokens.unwrap_or(0) == 0
        && usage.reasoning_tokens.unwrap_or(0) == 0
}

fn truncate_for_preview(value: &str, max_chars: usize) -> String {
    let mut out: String = value.chars().take(max_chars).collect();
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 协议层自报的失败必须能到出口——修复前这里只打了一条日志，于是
    /// 「CLI 明确说本轮失败了」被整个吞掉（claude 未登录 ⇒ 空气泡 + 零提示）。
    #[test]
    fn protocol_reported_failure_reaches_the_error_exit() {
        let stream_error = "Not logged in · Please run /login".to_string();
        let resolved = resolve_turn_error(None, Some(&stream_error));
        assert_eq!(
            resolved,
            Some(&stream_error),
            "读流 Ok 时，协议层自报的失败仍必须成为本轮错误"
        );
    }

    /// 读流错误优先于协议层消息：进程级失败带退出码/stderr，`classify` 分得更准。
    #[test]
    fn read_error_wins_over_protocol_reported_failure() {
        let read_error = "session-new: timed out".to_string();
        let stream_error = "some protocol complaint".to_string();
        assert_eq!(
            resolve_turn_error(Some(&read_error), Some(&stream_error)),
            Some(&read_error)
        );
    }

    /// 两者都没有 ⇒ 本轮成功，不得凭空造出错误（正常轮次不能被新逻辑误判）。
    #[test]
    fn clean_turn_has_no_error() {
        assert_eq!(resolve_turn_error(None, None), None);
    }

    #[test]
    fn subagent_launch_receipt_keeps_the_tool_card_running() {
        let mut record = ToolCallRecord {
            id: "c1".into(),
            name: "subagent".into(),
            source: "external_cli".into(),
            server_id: None,
            arguments: "{}".into(),
            status: ToolCallStatus::Running,
            result_preview: None,
            error: None,
            duration_ms: None,
            started_at: Some(1),
            completed_at: None,
            round: 1,
            sensitive: false,
            artifacts: vec![],
            trace_id: None,
            span_id: None,
            structured_content: Some(serde_json::json!({ "description": "搜资讯" })),
        };
        apply_external_tool_result(
            &mut record,
            "started subagent 018b08fc-ee7f-4ea5-b77c-9c5d1c6ecf50",
            false,
            99,
        );
        assert_eq!(record.status, ToolCallStatus::Running);
        assert_eq!(record.completed_at, None);
        assert_eq!(record.result_preview, None);
        assert_eq!(
            background_task_id(&record).as_deref(),
            Some("018b08fc-ee7f-4ea5-b77c-9c5d1c6ecf50")
        );
    }

    #[test]
    fn subagent_progress_lands_on_the_parent_tool_card() {
        let mut record = ToolCallRecord {
            id: "c1".into(),
            name: "subagent".into(),
            source: "external_cli".into(),
            server_id: None,
            arguments: "{}".into(),
            status: ToolCallStatus::Running,
            result_preview: None,
            error: None,
            duration_ms: None,
            started_at: Some(1),
            completed_at: None,
            round: 1,
            sensitive: false,
            artifacts: vec![],
            trace_id: None,
            span_id: None,
            structured_content: Some(serde_json::json!({
                "backgroundTaskId": "child-9",
                "description": "搜资讯"
            })),
        };
        merge_subagent_progress(
            &mut record,
            "正在检索",
            &["web_search 最新AI".to_string()],
            "running",
            "child-9",
        );
        let progress = record
            .structured_content
            .as_ref()
            .and_then(|value| value.get("subagentProgress"))
            .expect("progress");
        assert_eq!(progress["taskId"], "child-9");
        assert_eq!(progress["status"], "running");
        assert_eq!(progress["preview"], "正在检索");
        assert_eq!(progress["steps"][0], "web_search 最新AI");
        assert_eq!(
            record.structured_content.as_ref().unwrap()["backgroundTaskId"],
            "child-9"
        );
    }

    #[test]
    fn subagent_progress_binds_child_session_to_the_launch_receipt_card() {
        let mut tools = vec![ToolCallRecord {
            id: "c1".into(),
            name: "subagent".into(),
            source: "external_cli".into(),
            server_id: None,
            arguments: "{}".into(),
            status: ToolCallStatus::Running,
            result_preview: None,
            error: None,
            duration_ms: None,
            started_at: Some(1),
            completed_at: None,
            round: 1,
            sensitive: false,
            artifacts: vec![],
            trace_id: None,
            span_id: None,
            structured_content: Some(serde_json::json!({
                "backgroundTaskId": "subagent-1",
                "description": "搜资讯"
            })),
        }];
        {
            let record = find_dsh_subagent_record(&mut tools, "018b08fc-session").expect("bind");
            merge_subagent_progress(
                record,
                "正在检索",
                &["web_search 最新AI".to_string()],
                "running",
                "018b08fc-session",
            );
        }
        assert_eq!(
            tools[0].structured_content.as_ref().unwrap()["childSessionId"],
            "018b08fc-session"
        );
        assert_eq!(
            tools[0].structured_content.as_ref().unwrap()["subagentProgress"]["steps"][0],
            "web_search 最新AI"
        );
        assert!(
            find_dsh_subagent_record(&mut tools, "018b08fc-session").is_some(),
            "later events must exact-match the stored child session id"
        );
    }

    /// 端到端：claude 未登录的**真实样本**从流解析一路走到气泡文案。
    /// 这条把 `stream/claude.rs` 的解析与 `run.rs` 的出口接在一起——两边各自正确
    /// 但没接上，正是上一轮 `collect_external_session_usage` 那类空转 bug 的形态。
    #[test]
    fn real_not_logged_in_payload_renders_an_actionable_bubble() {
        let raw = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login","stop_reason":"stop_sequence","total_cost_usd":0,"permission_denials":[],"usage":{"input_tokens":0,"output_tokens":0,"iterations":[]}}"#;
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();

        // 1) 解析：produce 一条 Error。
        let mut stream_error: Option<String> = None;
        crate::external_agents::stream::claude::ClaudeStreamState::default().handle_value(
            &value,
            &mut |event| {
                if let UnifiedAgentEvent::Error { message } = event {
                    if stream_error.is_none() {
                        stream_error = Some(message);
                    }
                }
            },
        );
        assert!(stream_error.is_some(), "未登录样本应产出 Error");

        // 2) 出口：读流 Ok + exit 0（CLI 干净退出）时仍判为本轮失败。
        let turn_error = resolve_turn_error(None, stream_error.as_ref()).expect("应判定为本轮失败");

        // 3) 气泡：可操作的中文提示，裸英文只进 <details>。
        let bubble = crate::external_agents::errors::classify(turn_error, Some(0), "", "claude")
            .render_bubble();
        assert!(
            bubble.contains("claude /login"),
            "气泡应给出可操作的登录命令：{bubble}"
        );
        assert!(
            !bubble.trim().is_empty(),
            "气泡不得为空——这正是修复前的症状"
        );
    }

    // ---- CLI 实报用量的合并口径（一轮一次，轮末权威值用的就是它）----

    fn usage_parts(input: u64, output: u64, cache_read: u64, window: Option<u64>) -> ModelUsage {
        crate::external_agents::stream::usage_from_parts(
            crate::external_agents::stream::CliUsageParts {
                input,
                output,
                cache_read,
                context_window: window,
                ..Default::default()
            },
        )
    }

    /// 零用量的上报不得把已经攒到的分子清零（spec 14h）。`/help` 那种没有 LLM 往返的
    /// `result` 四个字段全 0 却带窗口 —— 采纳它的窗口，但绝不采纳它的 0。
    #[test]
    fn zero_usage_report_does_not_reset_the_numerator() {
        let real = usage_parts(1_200, 800, 45_000, None);
        let zero_with_window = usage_parts(0, 0, 0, Some(1_000_000));
        let merged = merge_cli_usage(Some(&real), zero_with_window);
        assert_eq!(
            crate::external_agents::context::cli_reported_context_tokens(&merged),
            47_000,
            "分子必须保持 47000（只采纳零值上报带来的窗口）"
        );
        assert_eq!(merged.context_window_tokens, Some(1_000_000));
    }

    #[test]
    fn cli_usage_merge_keeps_latest_numbers() {
        let first = ModelUsage {
            input_tokens: Some(100),
            ..Default::default()
        };
        let merged = merge_cli_usage(
            Some(&first),
            ModelUsage {
                input_tokens: Some(250),
                ..Default::default()
            },
        );
        assert_eq!(merged.input_tokens, Some(250));
    }

    #[test]
    fn cli_usage_merge_keeps_window_when_later_report_omits_it() {
        // ACP 实际时序：usage_update(带 size) 先到，PromptResponse.usage(无 size) 后到。
        let with_window = ModelUsage {
            input_tokens: Some(13_477),
            context_window_tokens: Some(200_000),
            ..Default::default()
        };
        let merged = merge_cli_usage(
            Some(&with_window),
            ModelUsage {
                input_tokens: Some(11_685),
                output_tokens: Some(4),
                context_window_tokens: None,
                ..Default::default()
            },
        );
        assert_eq!(merged.context_window_tokens, Some(200_000));
        assert_eq!(merged.input_tokens, Some(11_685));
    }

    #[test]
    fn cli_usage_merge_lets_newer_window_win() {
        let old = ModelUsage {
            context_window_tokens: Some(200_000),
            ..Default::default()
        };
        let merged = merge_cli_usage(
            Some(&old),
            ModelUsage {
                context_window_tokens: Some(1_048_576),
                ..Default::default()
            },
        );
        assert_eq!(merged.context_window_tokens, Some(1_048_576));
    }

    #[test]
    fn cli_usage_merge_without_previous_is_identity() {
        let merged = merge_cli_usage(
            None,
            ModelUsage {
                input_tokens: Some(7),
                ..Default::default()
            },
        );
        assert_eq!(merged.input_tokens, Some(7));
        assert_eq!(merged.context_window_tokens, None);
    }

    /// **零用量的 result 不许把分子清零**（A2）。claude 在没有 LLM 往返的轮次
    /// （未登录 / `/help` / 未知斜杠命令 / Kivio 自己发的 `/compact`）会报一条全 0 的 usage，
    /// 但它仍可能带着 `modelUsage.contextWindow`。直接整体覆盖会把本轮 `message_start`
    /// 报的真实占用清零 ⇒ 用量条从 47K 掉到 0。
    #[test]
    fn cli_usage_merge_keeps_real_numbers_when_a_zero_report_arrives_later() {
        let realtime = ModelUsage {
            input_tokens: Some(1_200),
            output_tokens: Some(800),
            cached_input_tokens: Some(45_000),
            cache_creation_input_tokens: Some(300),
            total_tokens: Some(47_300),
            ..Default::default()
        };
        let merged = merge_cli_usage(
            Some(&realtime),
            ModelUsage {
                input_tokens: Some(0),
                output_tokens: Some(0),
                total_tokens: Some(0),
                context_window_tokens: Some(1_000_000),
                ..Default::default()
            },
        );
        assert_eq!(merged.total_tokens, Some(47_300), "分子被清零了");
        assert_eq!(merged.input_tokens, Some(1_200));
        // 但窗口（分母）要采纳——它是静态属性，与本轮有没有用量无关。
        assert_eq!(merged.context_window_tokens, Some(1_000_000));
    }

    /// 反向不成立：真实数字仍要能覆盖先到的另一份真实数字（取最新快照的语义不变）。
    #[test]
    fn cli_usage_merge_still_takes_the_latest_nonzero_snapshot() {
        let first = ModelUsage {
            input_tokens: Some(100),
            total_tokens: Some(100),
            ..Default::default()
        };
        let merged = merge_cli_usage(
            Some(&first),
            ModelUsage {
                input_tokens: Some(900),
                total_tokens: Some(900),
                ..Default::default()
            },
        );
        assert_eq!(merged.total_tokens, Some(900));
    }

    // ---- 非零退出码的豁免（spec 第 8b 条 + A9 杀整棵进程树）----

    /// 读到协议层完成标志（claude 的 `result` 帧）后，非零退出码不得再把这一轮标成失败。
    /// Windows `TerminateProcess` 退出码恒为 1，而杀整棵进程树让这条路径变常见——
    /// 不豁免就会凭空造出失败气泡。
    #[test]
    fn protocol_completion_exempts_a_nonzero_exit() {
        assert!(!nonzero_exit_is_a_failure(Some(1), true));
        assert!(!nonzero_exit_is_a_failure(Some(-1), true));
        // 没有完成标志时仍按老规则判失败。
        assert!(nonzero_exit_is_a_failure(Some(1), false));
        // 正常退出 / 退出码未知（信号退出，unix 下 code() = None）都不是失败。
        assert!(!nonzero_exit_is_a_failure(Some(0), false));
        assert!(!nonzero_exit_is_a_failure(None, false));
    }

    // ---- 工具审批：本轮接不接询问 ----

    /// 判据取自 argv 本身：带 `--permission-prompt-tool` 才建审批通道。
    /// 这样就不可能与 `defs::claude::claude_permission_prompt_args` 的决定分叉。
    #[test]
    fn the_approval_bridge_follows_the_launch_flag() {
        let asking = crate::external_agents::defs::claude::build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: None,
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: Some("default".to_string()),
            },
            None,
        );
        assert!(turn_asks_for_permission(&asking));

        // 默认档现在也接通道（`AskUserQuestion` 的唯一开关），但普通工具就地放行。
        let silent = crate::external_agents::defs::claude::build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: None,
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        assert!(turn_asks_for_permission(&silent));
        assert!(!turn_asks_for_permission(&[]));
        // 值出现在别处（比如某个 prompt 里）不算 —— 判据只认 flag 本身。
        assert!(!turn_asks_for_permission(&["stdio".to_string()]));
        // dsh 没有 `--permission-prompt-tool`：问用户靠 codec 开通道。
        assert!(turn_needs_approval_host(&[], "dsh"));
        assert!(!turn_needs_approval_host(&[], "cursor"));
        assert!(!turn_needs_approval_host(&[], "claude"));
    }

    /// 「完全」档接上询问通道之后**用户感知不到差别**：普通工具原地放行，只有问用户卡会弹。
    /// 判据同样取自 argv（`--permission-mode` 的值），不在这里重抄一份档位规则。
    #[test]
    fn the_full_access_mode_auto_allows_ordinary_tools() {
        let mk = |sandbox: Option<&str>| {
            crate::external_agents::defs::claude::build_claude_args(
                &RuntimeContext {
                    extra_allowed_dirs: vec![],
                    resume_session_id: None,
                    new_session_id: None,
                    include_partial_messages: true,
                },
                &RuntimeBuildOptions {
                    model: None,
                    reasoning: None,
                    sandbox: sandbox.map(str::to_string),
                },
                None,
            )
        };
        let auto_allows = |args: &[String]| {
            permission_mode_from_args(args)
                .is_some_and(crate::external_agents::defs::claude::claude_mode_auto_allows_tools)
        };
        assert!(auto_allows(&mk(None)), "默认档（完全）必须就地放行");
        assert!(auto_allows(&mk(Some("bypassPermissions"))));
        assert!(
            !auto_allows(&mk(Some("default"))),
            "「每次确认」必须真的弹卡，不能被放行"
        );
        assert_eq!(permission_mode_from_args(&[]), None);
    }

    #[test]
    fn stream_segment_tracker_reuses_text_segment_for_deltas() {
        let mut segments = Vec::new();
        let mut order = 0u32;
        let mut tracker = StreamSegmentTracker::default();

        let first = tracker.append(
            ChatMessageSegmentKind::Text,
            &mut segments,
            &mut order,
            0,
            "你",
        );
        let second = tracker.append(
            ChatMessageSegmentKind::Text,
            &mut segments,
            &mut order,
            0,
            "好",
        );

        assert_eq!(segments.len(), 1);
        assert_eq!(first.id, second.id);
        assert_eq!(segments[0].text.as_deref(), Some("你好"));
        assert_eq!(segments[0].phase, ChatMessageSegmentPhase::Plain);
    }

    #[test]
    fn push_tool_segment_increments_order_and_sets_tool_kind() {
        let mut segments = Vec::new();
        let mut order = 2u32;
        let first = push_tool_segment(&mut segments, &mut order, "tool-1");
        let second = push_tool_segment(&mut segments, &mut order, "tool-2");

        assert_eq!(segments.len(), 2);
        assert_eq!(first.kind, ChatMessageSegmentKind::Tool);
        assert_eq!(first.order, 3);
        assert_eq!(first.tool_call_id.as_deref(), Some("tool-1"));
        assert_eq!(second.order, 4);
        assert_eq!(second.phase, ChatMessageSegmentPhase::ToolLoop);
    }

    #[test]
    fn stream_segment_tracker_starts_new_text_segment_after_tool_use() {
        let mut segments = Vec::new();
        let mut order = 0u32;
        let mut tracker = StreamSegmentTracker::default();

        tracker.append(
            ChatMessageSegmentKind::Text,
            &mut segments,
            &mut order,
            0,
            "before",
        );
        tracker.reset_text();
        let after = tracker.append(
            ChatMessageSegmentKind::Text,
            &mut segments,
            &mut order,
            1,
            "after",
        );

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text.as_deref(), Some("before"));
        assert_eq!(segments[1].text.as_deref(), Some("after"));
        // 工具之后的**正文**必须是 Synthesis：`content` 累加了所有 TextDelta，正文段标
        // ToolLoop 会让落库的 normalize 以为正文没落段而再补一条 → 气泡显示两遍。
        assert_eq!(after.phase, ChatMessageSegmentPhase::Synthesis);
    }

    #[test]
    fn stream_segment_tracker_keeps_reasoning_in_tool_loop_phase() {
        // 推理段不参与正文口径（reasoning_from_segments 不看 phase），保持 ToolLoop 语义。
        let mut segments = Vec::new();
        let mut order = 0u32;
        let mut tracker = StreamSegmentTracker::default();

        let thinking = tracker.append(
            ChatMessageSegmentKind::Reasoning,
            &mut segments,
            &mut order,
            2,
            "还得再看一眼",
        );

        assert_eq!(thinking.phase, ChatMessageSegmentPhase::ToolLoop);
    }

    #[test]
    fn append_final_text_grows_content_and_segments_together() {
        // 轮末补充文案（错误气泡 / stderr / 占位）必须同时进 `content` 和 `segments`：
        // 只拼 `content` 的话，落库时要么正文重复、要么这段补充文案被整段丢掉。
        let mut content = String::from("已创建 dup.md。");
        let mut segments = vec![ChatMessageSegment {
            id: "seg_answer".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::Synthesis,
            order: 3,
            step_number: None,
            round: Some(1),
            text: Some("已创建 dup.md。".to_string()),
            tool_call_id: None,
        }];
        let mut order = 3u32;

        append_final_text(
            &mut content,
            &mut segments,
            &mut order,
            1,
            "claude stderr：boom",
        );

        assert_eq!(content, "已创建 dup.md。\n\nclaude stderr：boom");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].text.as_deref(), Some("claude stderr：boom"));
        assert_eq!(segments[1].kind, ChatMessageSegmentKind::Text);
        assert_eq!(segments[1].phase, ChatMessageSegmentPhase::Synthesis);
        assert_eq!(segments[1].order, 4);
    }

    #[test]
    fn append_final_text_ignores_blank_text() {
        let mut content = String::new();
        let mut segments = Vec::new();
        let mut order = 0u32;

        append_final_text(&mut content, &mut segments, &mut order, 0, "   \n ");

        assert!(content.is_empty());
        assert!(segments.is_empty());
        assert_eq!(order, 0);
    }

    #[test]
    fn append_final_text_on_empty_content_starts_a_plain_segment() {
        // 没有工具的一轮（例如斜杠命令占位文案）：正文段是 Plain，同样被正文口径认可。
        let mut content = String::new();
        let mut segments = Vec::new();
        let mut order = 0u32;

        append_final_text(
            &mut content,
            &mut segments,
            &mut order,
            0,
            "claude 命令已执行",
        );

        assert_eq!(content, "claude 命令已执行");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].phase, ChatMessageSegmentPhase::Plain);
    }

    // ---- Persistent-session retry policy (R3 / R4) ----

    use crate::external_agents::session::acp::NEEDS_RECONNECT;

    #[test]
    fn cancelled_failure_is_surfaced_as_is() {
        assert_eq!(
            persistent_failure_action("cancelled", "grok", false, false, false),
            PersistentFailureAction::Cancelled
        );
    }

    #[test]
    fn auth_failure_is_never_retried() {
        assert_eq!(
            persistent_failure_action("Authentication required", "grok", false, false, false),
            PersistentFailureAction::Fatal
        );
    }

    #[test]
    fn transient_failure_retries_fresh_once() {
        assert_eq!(
            persistent_failure_action(
                "ACP session exited mid-turn",
                "cursor-agent",
                false,
                false,
                false
            ),
            PersistentFailureAction::RetryFresh
        );
        // Already retried → give up.
        assert_eq!(
            persistent_failure_action(
                "ACP session exited mid-turn",
                "cursor-agent",
                true,
                false,
                false
            ),
            PersistentFailureAction::Fatal
        );
    }

    #[test]
    fn needs_reconnect_relaunches_once_then_gives_up() {
        assert_eq!(
            persistent_failure_action(NEEDS_RECONNECT, "grok", false, false, false),
            PersistentFailureAction::ReconnectConfig
        );
        assert_eq!(
            persistent_failure_action(NEEDS_RECONNECT, "grok", false, true, false),
            PersistentFailureAction::Fatal
        );
    }

    // ---- resume 失效必须降级，而不是把 CLI 的英文原句甩给用户 ----

    /// 本机实测原样本（claude 2.1.220，`-p --output-format stream-json --resume <随机 uuid>`）：
    /// stdout 一条 `result`，`errors[]` 里就是这句话；stderr 同一句话，然后 `exit 1`。
    const REAL_MISSING_SESSION_ERROR: &str =
        "No conversation found with session ID: d85724b7-59e4-4690-8984-1f31ca9a3414";

    /// 认出来 ⇒ 摘掉 resume 换新会话重连一次；再失败才算真失败。
    ///
    /// 反面（改动前的行为）：这条文案落进 `RetryFresh`，而 `RetryFresh` 拿的是**同一份 argv**
    /// ——同一个死 id 再 `--resume` 一次，必然再失败一次，然后用户拿到一句看不懂的英文。
    #[test]
    fn a_missing_resume_target_reconnects_without_resume_exactly_once() {
        assert_eq!(
            persistent_failure_action(REAL_MISSING_SESSION_ERROR, "claude", false, false, false),
            PersistentFailureAction::ReconnectWithoutResume
        );
        // 已经降级过一次还失败 ⇒ 原因不是 resume，别再兜圈子。
        assert_eq!(
            persistent_failure_action(REAL_MISSING_SESSION_ERROR, "claude", false, false, true),
            PersistentFailureAction::Fatal
        );
    }

    const REAL_DSH_MISSING_SESSION_ERROR: &str =
        "dsh session/open: session \"kivio-old\" not found";

    #[test]
    fn a_missing_dsh_resume_target_reconnects_without_resume_exactly_once() {
        assert_eq!(
            persistent_failure_action(REAL_DSH_MISSING_SESSION_ERROR, "dsh", false, false, false),
            PersistentFailureAction::ReconnectWithoutResume
        );
        assert_eq!(
            persistent_failure_action(REAL_DSH_MISSING_SESSION_ERROR, "dsh", false, false, true),
            PersistentFailureAction::Fatal
        );
        // 宽到 "Session not found" 会把无关失败拽进降级；dsh 必须带 session/open 前缀。
        assert_ne!(
            persistent_failure_action("Session not found: abc", "dsh", false, false, false),
            PersistentFailureAction::ReconnectWithoutResume
        );
    }

    /// 启动阶段暴露的那条（`connect()` 的 `try_wait` 抓到「立刻退出」+ stderr 尾部）
    /// 必须命中同一条判据 —— 判据是 `contains`，不是全等。
    #[test]
    fn the_startup_flavour_of_the_same_failure_is_recognized_too() {
        let from_connect = format!(
            "claude-init: 进程启动后立刻退出（exit code: 1）\n\n<details>\n{REAL_MISSING_SESSION_ERROR}\n</details>"
        );
        assert_eq!(
            persistent_failure_action(&from_connect, "claude", false, false, false),
            PersistentFailureAction::ReconnectWithoutResume
        );
    }

    /// 判据不能宽到把别的失败也拽进来：那会让真实的认证 / 瞬时故障走错分支。
    #[test]
    fn unrelated_failures_do_not_look_like_a_missing_resume_target() {
        for err in [
            "Not logged in · Please run /login",
            "claude 常驻会话在轮次中退出",
            "No message found with message.uuid of: abc",
            "Session not found: abc",
            "",
        ] {
            assert_ne!(
                persistent_failure_action(err, "claude", false, false, false),
                PersistentFailureAction::ReconnectWithoutResume,
                "{err:?} 被误判成 resume 失效"
            );
        }
    }

    /// 取消永远优先：用户刚点了停止，不该被解读成任何一种重连。
    #[test]
    fn cancellation_still_wins_over_the_resume_downgrade() {
        assert_eq!(
            persistent_failure_action("cancelled", "claude", false, false, false),
            PersistentFailureAction::Cancelled
        );
    }

    /// 降级后的 argv 必须是「开新会话」而不是「换个 id 继续 resume」：
    /// 死会话的 id 不存在，`--resume` 无论换成谁都续不上。
    #[test]
    fn the_downgraded_args_open_a_brand_new_session() {
        use crate::external_agents::defs::claude::{
            claude_args_fresh_session, claude_session_id_from_args,
        };
        let dead = vec![
            "-p".to_string(),
            "--resume".to_string(),
            "dead-id".to_string(),
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ];
        let fresh = claude_args_fresh_session(&dead, "brand-new-id");
        assert!(
            !fresh.contains(&"--resume".to_string()),
            "还带着 --resume ⇒ 必然再失败一次：{fresh:?}"
        );
        assert!(
            !fresh.contains(&"dead-id".to_string()),
            "死 id 的值没被一起摘掉（会变成非法位置参数）：{fresh:?}"
        );
        assert!(fresh
            .windows(2)
            .any(|w| w == ["--session-id", "brand-new-id"]));
        assert_eq!(
            claude_session_id_from_args(&fresh).as_deref(),
            Some("brand-new-id")
        );
        // 其余参数原样保留。
        assert!(fresh
            .windows(2)
            .any(|w| w == ["--permission-mode", "bypassPermissions"]));
    }

    #[test]
    fn cancel_escalates_only_after_grace() {
        let now = std::time::Instant::now();
        let grace = std::time::Duration::from_secs(10);
        // No cancel requested → never escalate.
        assert!(!cancel_should_escalate(None, now, grace));
        // Cancel just now → within grace, don't escalate.
        assert!(!cancel_should_escalate(Some(now), now, grace));
        // Cancel 11s ago → escalate to Close.
        let past = now
            .checked_sub(std::time::Duration::from_secs(11))
            .expect("instant in range");
        assert!(cancel_should_escalate(Some(past), now, grace));
    }

    // ---- B1：常驻 claude（取消存活 / 配置变更重连 / 每轮正文）----

    use crate::external_agents::session::live::CANCELLED_SESSION_LOST;

    /// 两种取消都走 cancelled 出口：不弹错误气泡、不发上下文重置提示、更不重发本轮 prompt。
    #[test]
    fn both_cancel_flavours_are_cancellations() {
        assert!(is_cancellation("cancelled"));
        assert!(is_cancellation(CANCELLED_SESSION_LOST));
        assert!(!is_cancellation("ACP session exited mid-turn"));
        assert!(!is_cancellation(""));
        assert_eq!(
            persistent_failure_action(CANCELLED_SESSION_LOST, "claude", false, false, false),
            PersistentFailureAction::Cancelled
        );
    }

    /// claude / dsh 在协议级取消完整收尾后都必须保留 live session。
    #[test]
    fn settled_protocol_cancel_keeps_supported_live_sessions() {
        assert!(cancel_keeps_live_session(
            "cancelled",
            StreamFormat::ClaudeStreamJson
        ));
        assert!(cancel_keeps_live_session(
            "cancelled",
            StreamFormat::DshJsonRpc
        ));
        // 硬 Close / 进程已死：任何协议都不保留（留着就是个死 actor）。
        assert!(!cancel_keeps_live_session(
            CANCELLED_SESSION_LOST,
            StreamFormat::ClaudeStreamJson
        ));
        // ACP / codex 的 reader 在取消后停在流中间，复用会读到残帧 ⇒ 保持原行为。
        assert!(!cancel_keeps_live_session(
            "cancelled",
            StreamFormat::AcpJsonRpc
        ));
        assert!(!cancel_keeps_live_session(
            "cancelled",
            StreamFormat::CodexAppServer
        ));
        // 真实失败一律丢弃。
        assert!(!cancel_keeps_live_session(
            "claude 常驻会话在轮次中退出",
            StreamFormat::ClaudeStreamJson
        ));
    }

    /// Claude fingerprints launch flags/instructions; dsh fingerprints model/reasoning/sandbox/provider.
    /// ACP / codex / pi can apply their relevant settings without this process-level fingerprint.
    #[test]
    fn launch_config_fingerprints_process_bound_protocols() {
        let claude = launch_config_for_turn(
            StreamFormat::ClaudeStreamJson,
            Some("opus"),
            Some("high"),
            Some("plan"),
            None,
            Some("hash-1"),
        );
        // model **不进指纹**：它能在会话内换（`set_model`），不需要换进程。
        assert_eq!(claude.flags, "high|plan");
        assert_eq!(claude.instructions.as_deref(), Some("hash-1"));

        let dsh = |model, reasoning, sandbox, preset| {
            launch_config_for_turn(
                StreamFormat::DshJsonRpc,
                model,
                reasoning,
                sandbox,
                preset,
                Some("ignored-instructions"),
            )
        };
        let dsh_base = dsh(
            Some("deepseek-v4-flash"),
            Some("off"),
            Some("read-only"),
            None,
        );
        assert!(dsh_base.instructions.is_none());
        assert_ne!(
            dsh_base,
            dsh(
                Some("deepseek-v4-pro"),
                Some("off"),
                Some("read-only"),
                None
            )
        );
        assert_ne!(
            dsh_base,
            dsh(
                Some("deepseek-v4-flash"),
                Some("high"),
                Some("read-only"),
                None
            )
        );
        assert_ne!(
            dsh_base,
            dsh(
                Some("deepseek-v4-flash"),
                Some("off"),
                Some("workspace-write"),
                None
            )
        );
        assert_ne!(
            dsh_base,
            dsh(
                Some("deepseek-v4-flash"),
                Some("off"),
                Some("read-only"),
                Some("minimal")
            )
        );
        let provider_a = crate::settings::ExternalCliProvider {
            id: "provider-a".to_string(),
            config_json: "{\"baseURL\":\"https://a.example/v1\"}".to_string(),
            ..Default::default()
        };
        let provider_b = crate::settings::ExternalCliProvider {
            id: "provider-b".to_string(),
            config_json: "{\"baseURL\":\"https://b.example/v1\"}".to_string(),
            ..Default::default()
        };
        assert_ne!(
            dsh_provider_fingerprint_for(Some(&provider_a)),
            dsh_provider_fingerprint_for(Some(&provider_b))
        );
        assert_ne!(
            dsh_provider_fingerprint_for(None),
            dsh_provider_fingerprint_for(Some(&provider_a))
        );
        for protocol in [
            StreamFormat::AcpJsonRpc,
            StreamFormat::CodexAppServer,
            StreamFormat::PiRpc,
        ] {
            assert_eq!(
                launch_config_for_turn(
                    protocol,
                    Some("opus"),
                    Some("high"),
                    Some("plan"),
                    None,
                    Some("h")
                ),
                LaunchConfig::default(),
                "{protocol:?} 不该参与启动指纹判定"
            );
        }
    }

    /// 真正的启动 flag 任一变化都要触发重连（`accepts` 为 false）；全都没变则复用。
    /// **换模型是例外** —— 见下一条测试。
    #[test]
    fn every_launch_flag_change_forces_a_reconnect() {
        let base = |model, reasoning, sandbox, hash| {
            launch_config_for_turn(
                StreamFormat::ClaudeStreamJson,
                model,
                reasoning,
                sandbox,
                None,
                hash,
            )
        };
        let established = base(Some("opus"), Some("high"), Some("plan"), Some("h1"));
        assert!(established.accepts(&base(Some("opus"), Some("high"), Some("plan"), Some("h1"))));
        assert!(!established.accepts(&base(Some("opus"), Some("low"), Some("plan"), Some("h1"))));
        assert!(!established.accepts(&base(
            Some("opus"),
            Some("high"),
            Some("bypassPermissions"),
            Some("h1")
        )));
        // 系统提示 / Memory 改了：文件内容变了，而常驻进程只在启动时读一遍。
        assert!(!established.accepts(&base(Some("opus"), Some("high"), Some("plan"), Some("h2"))));
    }

    /// **换模型不再换进程**。官方 `Query.setModel()` 在 streaming input mode 下可用，
    /// 所以模型走会话内的 `set_model` 控制请求；把它留在指纹里等于用户每换一次模型都付
    /// 一次冷启动 + 一整段历史重放。会话内切换失败时 `run_turn` 返回 `NEEDS_RECONNECT`，
    /// 退回换进程 —— 所以摘掉它不会让「换了却没生效」变成静默失败。
    #[test]
    fn changing_only_the_model_reuses_the_process() {
        let with = |model| {
            launch_config_for_turn(
                StreamFormat::ClaudeStreamJson,
                model,
                Some("high"),
                Some("plan"),
                None,
                Some("h1"),
            )
        };
        assert!(with(Some("opus")).accepts(&with(Some("sonnet"))));
        assert!(with(Some("sonnet")).accepts(&with(None)));
    }

    /// claude 每轮都发整份 composed prompt：会话级指令走启动 flag、不在正文里，剩下的
    /// skill 正文 + 附件说明是 per-turn 的，只发最新用户消息会让它们从第 2 轮起静默消失。
    #[test]
    fn claude_sends_the_full_composed_prompt_every_turn() {
        let composed = "## Skill: pdf\n<body>\n\n用户消息";
        let latest = "用户消息";
        assert_eq!(
            persistent_turn_prompt(StreamFormat::ClaudeStreamJson, composed, latest),
            composed
        );
        // codex / ACP 的 full_prompt 首轮含指令，复用轮只发最新消息（保持原行为）。
        assert_eq!(
            persistent_turn_prompt(StreamFormat::CodexAppServer, composed, latest),
            latest
        );
        assert_eq!(
            persistent_turn_prompt(StreamFormat::AcpJsonRpc, composed, latest),
            latest
        );
    }

    /// 常驻路径上 `exit_code` 恒为 `None`（只有非持久分支才 `wait()` 子进程），
    /// 所以「非零退出 = 失败」这条规则在常驻会话上天然不触发。
    #[test]
    fn the_nonzero_exit_rule_does_not_apply_to_persistent_sessions() {
        assert!(!nonzero_exit_is_a_failure(None, false));
        assert!(!nonzero_exit_is_a_failure(None, true));
    }
}
