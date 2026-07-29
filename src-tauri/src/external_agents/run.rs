use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use chrono::Local;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::chat::agent::AgentRunEntry;
use crate::chat::commands::{
    emit_chat_stream_delta, emit_chat_stream_done, emit_chat_tool_record, push_assistant_message,
};
use crate::chat::memory::l1_prompt_block;
use crate::chat::model::ModelUsage;
use crate::chat::storage::save_conversation;
use crate::chat::types::{
    ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase, CompactionBoundaryRecord,
    ToolCallRecord, ToolCallStatus,
};
use crate::chat::Conversation;
use crate::external_agents::defs::claude::append_system_prompt_file_args;
use crate::external_agents::prompt::{
    compose_external_prompt, compose_external_prompt_passthrough, cwd_hint, is_cli_slash_input,
    instructions_via_launch_flag,
};
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::session::acp::{run_acp_session, AcpMcpServer};
use crate::external_agents::session::codex_app_server::run_codex_app_server_session;
use crate::external_agents::session::live::LaunchConfig;
use crate::external_agents::session::pi_rpc::run_pi_rpc_session;
use crate::external_agents::session::{
    persist_delivered_session, resolve_agent_resume_context, stable_prompt_hash,
};
use crate::external_agents::skill_stage::{skill_cwd_alias_segment, stage_active_skill};
use crate::external_agents::slash::{self};
use crate::external_agents::spawn::{
    drain_stderr, kill_agent_process_tree, read_stdout_lines, resolve_binary, spawn_agent,
    tail_chars, write_prompt_stdin,
};
use crate::external_agents::stream::create_stream_handler;
use crate::external_agents::types::{
    RuntimeBuildOptions, RuntimeContext, StreamFormat, UnifiedAgentEvent,
};
use crate::external_agents::workspace::{extra_allowed_dirs_for_agent, resolve_effective_cwd};
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
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
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

    let cwd = resolve_effective_cwd(app, &conversation.id, conversation.project_id.as_deref())?;
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
        is_slash,
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

    let build_options = RuntimeBuildOptions {
        model: conversation.agent_runtime.external_model.clone(),
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

    // Phase 2 / B1: claude、codex app-server 与 ACP 家族都通过 live-session 注册表把进程跨轮
    // 保活。只剩 `PiRpc` 每轮起一个新子进程（见下面 `_ =>` 分支的注释）。
    let persistent = matches!(
        def.stream_format,
        StreamFormat::ClaudeStreamJson | StreamFormat::CodexAppServer | StreamFormat::AcpJsonRpc
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
    let mut protocol_completed = false;
    let mut segment_order = 0u32;
    let mut segments: Vec<ChatMessageSegment> = Vec::new();
    let mut segment_tracker = StreamSegmentTracker::default();
    let conversation_id = conversation.id.clone();
    let started_at = Instant::now();
    // 缓存 key 用探测 cwd（resolve_detection_cwd，非项目会话 = __global__），与斜杠探测的
    // 读取 key 一致——运行时从 CLI init 学到的真实命令列表才能覆盖探测缓存（含空负缓存）。
    // 执行 cwd（上面的 `cwd`）保持每会话独立，仅缓存 key 用全局 scope。
    let slash_cache_key =
        crate::external_agents::workspace::resolve_detection_cwd(app, Some(&conversation.id))
            .map(|detection_cwd| slash::cache_key(&agent_id, &detection_cwd.to_string_lossy()))
            .unwrap_or_else(|_| slash::cache_key(&agent_id, &cwd.to_string_lossy()));

    let mut emit_event = |event: UnifiedAgentEvent| {
        if let Some(commands) = slash::slash_commands_from_event(&event) {
            state.set_cached_external_slash_commands(slash_cache_key.clone(), commands);
        }
        apply_unified_event(
            app,
            &conversation_id,
            &run_id,
            &assistant_message_id,
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
            event,
        );
    };

    let cancel_check = || !state.is_chat_generation_active(&conversation_id, run_generation);

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
            conversation.agent_runtime.external_model.clone(),
            conversation.agent_runtime.external_reasoning.clone(),
            conversation.agent_runtime.external_sandbox.clone(),
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
        )
        .await
    } else {
        let spawned = spawned_opt
            .as_mut()
            .expect("non-persistent path spawns a child");
        match def.stream_format {
            StreamFormat::PiRpc => {
                let model = conversation.agent_runtime.external_model.as_deref();
                run_pi_rpc_session(
                    &mut spawned.child,
                    &composed.full_prompt,
                    model,
                    |event| emit_event(event),
                    cancel_check,
                )
                .await
            }
            StreamFormat::CodexAppServer => {
                let model = conversation.agent_runtime.external_model.as_deref();
                let reasoning = conversation.agent_runtime.external_reasoning.as_deref();
                run_codex_app_server_session(
                    &mut spawned.child,
                    &composed.full_prompt,
                    model,
                    reasoning,
                    &cwd,
                    |event| emit_event(event),
                    cancel_check,
                )
                .await
            }
            StreamFormat::AcpJsonRpc => {
                let model = conversation.agent_runtime.external_model.as_deref();
                let mcp_servers: Vec<AcpMcpServer> = vec![];
                run_acp_session(
                    &mut spawned.child,
                    &composed.full_prompt,
                    &cwd,
                    model,
                    &mcp_servers,
                    |event| emit_event(event),
                    cancel_check,
                )
                .await
            }
            // 现状（B1 之后）：这条「非持久 + `write_prompt_stdin` + `read_stdout_lines` +
            // `create_stream_handler`」的路**已经没有 CLI 走了** —— claude 是最后一个，
            // 现在也常驻了；`PiRpc` 走上面的 `run_pi_rpc_session`，codex / ACP 走持久分支。
            //
            // **刻意保留**：`create_stream_handler` / `write_probe_stdin` / `read_stdout_lines`
            // 还有探测路径在用（`slash.rs`、`session/claude_init.rs`），且 claude 的解析器
            // 单测与真机测试也从这里进入；删掉这条分支等于把「逐行喂解析器」这个入口一起删了。
            // 新增走行式 stdout 协议的 CLI 时，这里是现成的落点。
            _ => {
                if def.prompt_via_stdin {
                    write_prompt_stdin(
                        &mut spawned.child,
                        def,
                        &composed.full_prompt,
                        &image_blocks,
                    )
                    .await?;
                }
                let mut handler = create_stream_handler(def.stream_format);
                let outcome = read_stdout_lines(
                    &mut spawned.child,
                    |line| {
                        handler.handle_line(line, &mut |event| emit_event(event));
                        Ok(())
                    },
                    cancel_check,
                )
                .await;
                protocol_completed = handler.saw_protocol_completion();
                outcome
            }
        }
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
            if content.trim().is_empty() {
                content = bubble;
            } else {
                content.push_str("\n\n");
                content.push_str(&bubble);
            }
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
        if !raw_output.trim().is_empty() {
            content = raw_output.trim().to_string();
        } else if !stderr_output.trim().is_empty() {
            stream_outcome = "error".to_string();
            content = format!(
                "{} 执行失败：\n\n{}",
                def.name,
                truncate_for_preview(stderr_output.trim(), 4000)
            );
        } else if stream_outcome == "completed" {
            if is_slash {
                content = format!("{} 命令已执行", def.name);
            } else {
                stream_outcome = "error".to_string();
                content = format!(
                    "{} 未产生输出（exit={:?}，耗时 {}ms）",
                    def.name,
                    exit_code,
                    started_at.elapsed().as_millis()
                );
            }
        }
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
            if !content.trim().is_empty() {
                content.push_str("\n\n");
            }
            content.push_str(&format!(
                "{} stderr：\n\n{}",
                def.name,
                truncate_for_preview(stderr_output.trim(), 4000)
            ));
        }
    }

    emit_chat_stream_done(
        app,
        &conversation_id,
        &run_id,
        &assistant_message_id,
        &stream_outcome,
        &content,
    );

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

    // A7：把 CLI 自压的边界落到会话上。此前只发了 `chat-compaction` 事件、从不落盘，
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

    save_conversation(app, conversation)?;
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
        let phase = text_phase_for_tool_count(tool_calls_len);
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
    mcp_servers: Vec<AcpMcpServer>,
    launch_config: &LaunchConfig,
    first_prompt: &str,
    reuse_prompt: &str,
    images: &[crate::external_agents::attachments::ImageBlock],
    emit: &mut E,
    cancel: &C,
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

    // Establish the control channel: 1. reuse a live session in the registry; 2. resume a
    // persisted one; 3. connect fresh.
    //
    // 复用判据里含 `launch_config`：模型 / reasoning / sandbox / 系统指令任一变化 ⇒ 不可复用
    // ⇒ 丢弃条目（actor 自行关停旧进程）并走下面的连接分支**带原生 resume**，于是新 flag
    // 生效而上下文不丢（spec 第 8 条：UI 所见必须与会话实际配置一致）。
    let (mut control, mut prompt) = match state.external_live_session_control(
        conversation_id,
        agent_id,
        &cwd_str,
        launch_config,
    ) {
        Some(control) => (control, reuse_prompt.to_string()),
        None => {
            let resume_native = load_live_handle(app, conversation_id)
                .filter(|h| h.agent_id == agent_id && h.cwd == cwd_str && h.protocol == protocol_tag)
                .map(|h| h.native_id);
            // We intended to continue an existing native session iff a matching handle was
            // persisted. If the resume then fails and we fall back to fresh, the prior context
            // is lost and the user must be told (R4) rather than silently getting a blank slate.
            let intended_resume = resume_native.is_some();
            let (control, native_id, resumed) = connect_persistent_session(
                protocol,
                resolved_bin,
                args,
                cwd,
                model.as_deref(),
                reasoning.as_deref(),
                sandbox.as_deref(),
                &mcp_servers,
                resume_native,
            )
            .await?;
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
                },
            );
            // A resumed session already holds history → send only the latest message.
            let prompt = if resumed {
                reuse_prompt.to_string()
            } else {
                first_prompt.to_string()
            };
            // Intended to resume but ended up fresh → warn about the lost context.
            if intended_resume && !resumed {
                emit(context_reset_notice_event());
            }
            (control, prompt)
        }
    };

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
            // Transient failure → drop the stale handle and reconnect fresh once.
            PersistentFailureAction::RetryFresh => {
                retried_after_failure = true;
                clear_live_handle(app, conversation_id);
            }
        }

        let (next_control, resumed) = reconnect_fresh(
            app,
            state,
            conversation_id,
            agent_id,
            protocol,
            protocol_tag,
            resolved_bin,
            args,
            cwd,
            &cwd_str,
            model.as_deref(),
            reasoning.as_deref(),
            sandbox.as_deref(),
            &mcp_servers,
            launch_config,
        )
        .await?;
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

/// Connect a persistent session for the reconnect paths (no live handle to resume from), persist
/// its handle, and register it. Returns the control channel plus **whether the CLI actually
/// continued its native session** — for claude that happens when `args` still carry `--resume`,
/// and in that case the caller must NOT claim the context was reset (a false alarm is its own bug).
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
    mcp_servers: &[AcpMcpServer],
    launch_config: &LaunchConfig,
) -> Result<
    (
        tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>,
        bool,
    ),
    String,
> {
    use crate::external_agents::session::live::LiveSession;
    use crate::external_agents::session::{save_live_handle, LiveSessionHandle};

    let (control, native_id, resumed) = connect_persistent_session(
        protocol,
        resolved_bin,
        args,
        cwd,
        model,
        reasoning,
        sandbox,
        mcp_servers,
        None,
    )
    .await?;
    let _ = save_live_handle(
        app,
        conversation_id,
        &LiveSessionHandle {
            agent_id: agent_id.to_string(),
            protocol: protocol_tag.to_string(),
            native_id,
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
        },
    );
    Ok((control, resumed))
}

/// 本轮的启动配置指纹。
///
/// 只有 claude 需要它：它的 `--model` / `--effort` / `--permission-mode` /
/// `--append-system-prompt-file` **全是启动参数**，常驻之后只能靠换进程生效。改动前每轮
/// spawn 新进程，换配置是「下一轮自动带上新 flag」白捡的；常驻打破了这个便宜，不补上就会
/// 出现「界面显示一套、会话实际跑另一套」（违反 spec 第 8 条，是功能退步而非缺功能）。
///
/// ACP / codex 能在会话内改模型与推理档位（`session/set_config_option` / 每轮 `turn/start`
/// 带 model），指纹恒为 `default()` ⇒ 永不触发重连，既有行为不变。
fn launch_config_for_turn(
    protocol: StreamFormat,
    model: Option<&str>,
    reasoning: Option<&str>,
    sandbox: Option<&str>,
    instructions_hash: Option<&str>,
) -> LaunchConfig {
    if !matches!(protocol, StreamFormat::ClaudeStreamJson) {
        return LaunchConfig::default();
    }
    LaunchConfig {
        flags: format!(
            "{}|{}|{}",
            model.unwrap_or_default(),
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
/// **claude**：`run_turn` 发出协议级 `interrupt` 后**一直读到本轮的 `result` 才返回**
/// （实测被中断的轮次一定有 result），流位置回到轮次边界、进程完好 ⇒ 可以直接继续用。
/// 这是常驻改造的核心收益：点一次「停止」不该让用户丢掉整个会话上下文，也不该再花 3.2 秒
/// 重新拉起进程。
///
/// **ACP / codex**：`session/cancel` / `turn/interrupt` 发出后立刻返回，reader 停在流中间
/// （未消费的 prompt 响应 + 后续 update），复用会读到上一轮的残帧 ⇒ 保持原行为（丢弃会话，
/// 下一轮从落盘 handle 原生 resume）。
///
/// `CANCELLED_SESSION_LOST`（进程死了 / 取消超时被硬 Close）任何协议都不保留。
fn cancel_keeps_live_session(err: &str, protocol: StreamFormat) -> bool {
    err == "cancelled" && matches!(protocol, StreamFormat::ClaudeStreamJson)
}

/// What `run_persistent_turn` should do after a turn fails. Pure so the retry policy is unit
/// testable without a Tauri context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistentFailureAction {
    /// User cancellation — surface as-is, keep the persisted handle for a later resume.
    Cancelled,
    /// Relaunch fresh to apply a launch-flag config change (reasoning without a config option).
    ReconnectConfig,
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

/// True when a cancel was requested (`cancel_at` set) and the grace period has elapsed without the
/// turn winding down — the caller escalates to `Close` (A5). Pure for unit testing.
fn cancel_should_escalate(
    cancel_at: Option<std::time::Instant>,
    now: std::time::Instant,
    grace: std::time::Duration,
) -> bool {
    matches!(cancel_at, Some(t) if now.saturating_duration_since(t) >= grace)
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
) -> Result<(), String>
where
    E: FnMut(UnifiedAgentEvent),
    C: Fn() -> bool,
{
    use crate::external_agents::session::live::SessionCommand;
    use tokio::sync::{mpsc, oneshot};

    const CANCEL_ESCALATE_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

    let (events_tx, mut events_rx) = mpsc::channel::<UnifiedAgentEvent>(64);
    let (done_tx, done_rx) = oneshot::channel::<Result<(), String>>();
    if control
        .send(SessionCommand::RunTurn {
            prompt,
            model,
            reasoning,
            images: images.to_vec(),
            events: events_tx,
            done: done_tx,
        })
        .await
        .is_err()
    {
        return Err("外部 CLI 会话已结束，请重试".to_string());
    }

    let mut done_rx = done_rx;
    let mut events_open = true;
    let mut cancel_sent = false;
    let mut cancel_at: Option<std::time::Instant> = None;
    loop {
        tokio::select! {
            biased;
            result = &mut done_rx => {
                // Invariant (A4): the actor sends every `event` before `done`, and mpsc preserves
                // order, so all remaining events are already queued — drain them before returning.
                while let Ok(event) = events_rx.try_recv() {
                    emit(event);
                }
                return result.unwrap_or_else(|_| Err("session actor dropped".to_string()));
            }
            maybe_event = events_rx.recv(), if events_open => {
                match maybe_event {
                    Some(event) => emit(event),
                    None => events_open = false,
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
            return Err(
                crate::external_agents::session::live::CANCELLED_SESSION_LOST.to_string(),
            );
        }
    }
}

fn persistent_protocol_tag(protocol: StreamFormat) -> &'static str {
    match protocol {
        StreamFormat::ClaudeStreamJson => "claude_stream_json",
        StreamFormat::CodexAppServer => "codex_app_server",
        StreamFormat::AcpJsonRpc => "acp_json_rpc",
        _ => "unknown",
    }
}

/// Connect (or resume) a persistent protocol session, returning its control channel, native id,
/// and whether a resume actually succeeded. Falls back to a fresh session if resume fails.
#[allow(clippy::too_many_arguments)]
async fn connect_persistent_session(
    protocol: StreamFormat,
    resolved_bin: &std::path::Path,
    args: &[String],
    cwd: &std::path::Path,
    model: Option<&str>,
    reasoning: Option<&str>,
    sandbox: Option<&str>,
    mcp_servers: &[AcpMcpServer],
    resume_native: Option<String>,
) -> Result<
    (
        tokio::sync::mpsc::Sender<crate::external_agents::session::live::SessionCommand>,
        String,
        bool,
    ),
    String,
> {
    use crate::external_agents::session::acp::{spawn_acp_session_actor, AcpSession};
    use crate::external_agents::session::claude_stream::{
        spawn_claude_stream_session_actor, ClaudeStreamJsonSession,
    };
    use crate::external_agents::session::codex_app_server::{
        spawn_codex_session_actor, CodexAppServerSession,
    };

    match protocol {
        StreamFormat::ClaudeStreamJson => {
            // claude 的会话 id 走**启动参数**，不像 codex / ACP 在握手 RPC 里传 ——
            // 所以这里要看/改 argv 而不是传参。
            //
            // **参数说了算**：`build_claude_args` 已按 `resolve_agent_resume_context` 放好
            // `--resume <id>`（续接）或 `--session-id <new>`（开新会话）。后者的典型场景是
            // **换了模型** —— claude 的 resume 会话钉死在旧模型上，所以那条既有契约刻意开新会话
            // （代价是丢上下文，由 `intended_resume && !resumed` 的可见提示交代）。用 live
            // handle 的 native id 去覆盖它会让「换模型要生效」这条契约静默失效。
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
            Ok((spawn_claude_stream_session_actor(session), id, resumed))
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
                    return Ok((spawn_codex_session_actor(session), id, true));
                }
                // C3: resume failed → fall through to fresh so the caller overwrites the stale
                // live handle (whose native_id is dead) instead of retrying a doomed resume.
                eprintln!("[external-agent] codex resume failed, connecting fresh");
            }
            let session =
                CodexAppServerSession::connect(resolved_bin, args, cwd, model, sandbox, None)
                    .await?;
            let id = session.thread_id().to_string();
            Ok((spawn_codex_session_actor(session), id, false))
        }
        StreamFormat::AcpJsonRpc => {
            if let Some(sid) = resume_native.as_deref() {
                if let Ok(session) = AcpSession::connect(
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
                    let id = session.session_id().to_string();
                    return Ok((spawn_acp_session_actor(session), id, true));
                }
                // C3: resume failed → connect fresh; the caller's save_live_handle overwrites the
                // stale handle so the next turn won't attempt the dead native_id again.
                eprintln!("[external-agent] acp resume failed, connecting fresh");
            }
            let session =
                AcpSession::connect(resolved_bin, args, cwd, model, reasoning, mcp_servers, None)
                    .await?;
            let id = session.session_id().to_string();
            Ok((spawn_acp_session_actor(session), id, false))
        }
        _ => Err("protocol does not support persistent sessions".to_string()),
    }
}

fn text_phase_for_tool_count(tool_calls_len: usize) -> ChatMessageSegmentPhase {
    if tool_calls_len == 0 {
        ChatMessageSegmentPhase::Plain
    } else {
        ChatMessageSegmentPhase::ToolLoop
    }
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

/// 构造一条 CLI 自压的边界记录，并发 `chat-compaction` 通知前端插分隔线。
///
/// payload 与内置路径（`chat/agent/compaction.rs::emit_compaction_event` /
/// `chat/commands/context.rs::emit_chat_compaction_state`）同形，且**直接复用
/// `CompactionBoundaryRecord`** 而不是手写一份 json——两份形状迟早分叉（spec 第 2 条）。
///
/// 返回记录供调用方落盘：live 事件与持久化必须是**同一条记录**（同一个 id），
/// 否则刷新后会出现两条分隔线。
///
/// `token_estimate_after` 取 `compact_metadata.post_tokens`（claude 2.1.220 确实有这个字段）；
/// 缺失时为 0，前端此时不显示「→ N」。
fn emit_cli_compaction(
    app: &AppHandle,
    conversation_id: &str,
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
    let _ = app.emit(
        "chat-compaction",
        serde_json::json!({
            "conversationId": conversation_id,
            "phase": "completed",
            "trigger": trigger,
            "boundary": &boundary,
        }),
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

#[allow(clippy::too_many_arguments)]
fn apply_unified_event(
    app: &AppHandle,
    conversation_id: &str,
    run_id: &str,
    message_id: &str,
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
            emit_chat_stream_delta(
                app,
                conversation_id,
                run_id,
                message_id,
                &delta,
                None,
                Some(&segment),
            );
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
            emit_chat_stream_delta(
                app,
                conversation_id,
                run_id,
                message_id,
                "",
                Some(&delta),
                Some(&segment),
            );
        }
        UnifiedAgentEvent::ToolUse { id, name, input } => {
            segment_tracker.reset_text();
            segment_tracker.reset_reasoning();
            let segment = push_tool_segment(segments, segment_order, &id);
            emit_chat_stream_delta(
                app,
                conversation_id,
                run_id,
                message_id,
                "",
                None,
                Some(&segment),
            );
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
            emit_chat_tool_record(app, conversation_id, run_id, message_id, &record);
        }
        UnifiedAgentEvent::ToolResult {
            tool_use_id,
            content: result_content,
            is_error,
        } => {
            if let Some(idx) = tool_map.get(&tool_use_id).copied() {
                if let Some(record) = tool_calls.get_mut(idx) {
                    record.status = if is_error {
                        ToolCallStatus::Error
                    } else {
                        ToolCallStatus::Success
                    };
                    record.result_preview = Some(truncate_for_preview(&result_content, 800));
                    record.completed_at = Some(now);
                    emit_chat_tool_record(app, conversation_id, run_id, message_id, record);
                }
            }
        }
        UnifiedAgentEvent::Usage { usage: u } => {
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
            //   1. 发 `chat-compaction` 让前端**立刻**插入分隔线——否则用户只会看到
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
                conversation_id,
                compaction_anchor_id,
                &trigger,
                pre_tokens,
                post_tokens,
                now,
            ));
        }
        _ => {}
    }
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
fn usage_tokens_all_zero(usage: &ModelUsage) -> bool {
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
        assert_eq!(after.phase, ChatMessageSegmentPhase::ToolLoop);
    }

    // ---- Persistent-session retry policy (R3 / R4) ----

    use crate::external_agents::session::acp::NEEDS_RECONNECT;

    #[test]
    fn cancelled_failure_is_surfaced_as_is() {
        assert_eq!(
            persistent_failure_action("cancelled", "grok", false, false),
            PersistentFailureAction::Cancelled
        );
    }

    #[test]
    fn auth_failure_is_never_retried() {
        assert_eq!(
            persistent_failure_action("Authentication required", "grok", false, false),
            PersistentFailureAction::Fatal
        );
    }

    #[test]
    fn transient_failure_retries_fresh_once() {
        assert_eq!(
            persistent_failure_action("ACP session exited mid-turn", "cursor-agent", false, false),
            PersistentFailureAction::RetryFresh
        );
        // Already retried → give up.
        assert_eq!(
            persistent_failure_action("ACP session exited mid-turn", "cursor-agent", true, false),
            PersistentFailureAction::Fatal
        );
    }

    #[test]
    fn needs_reconnect_relaunches_once_then_gives_up() {
        assert_eq!(
            persistent_failure_action(NEEDS_RECONNECT, "grok", false, false),
            PersistentFailureAction::ReconnectConfig
        );
        assert_eq!(
            persistent_failure_action(NEEDS_RECONNECT, "grok", false, true),
            PersistentFailureAction::Fatal
        );
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
            persistent_failure_action(CANCELLED_SESSION_LOST, "claude", false, false),
            PersistentFailureAction::Cancelled
        );
    }

    /// **整个改造的验收点之一**：claude 协议级取消之后常驻会话必须留在注册表里。
    /// 丢弃条目 ⇒ control sender 落地 ⇒ actor 收到通道关闭 ⇒ 子进程被关停，
    /// 于是点一次「停止」就等于把会话上下文和 0.1s 冷启动一起扔掉。
    #[test]
    fn a_claude_cancel_keeps_the_live_session() {
        assert!(cancel_keeps_live_session(
            "cancelled",
            StreamFormat::ClaudeStreamJson
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

    /// 指纹只对 claude 生效（它的 model/effort/permission-mode/系统提示全是启动 flag）；
    /// ACP / codex 恒为默认值 ⇒ 永不触发重连，既有行为不变。
    #[test]
    fn launch_config_only_fingerprints_claude() {
        let claude = launch_config_for_turn(
            StreamFormat::ClaudeStreamJson,
            Some("opus"),
            Some("high"),
            Some("plan"),
            Some("hash-1"),
        );
        assert_eq!(claude.flags, "opus|high|plan");
        assert_eq!(claude.instructions.as_deref(), Some("hash-1"));

        for protocol in [
            StreamFormat::AcpJsonRpc,
            StreamFormat::CodexAppServer,
            StreamFormat::PiRpc,
        ] {
            assert_eq!(
                launch_config_for_turn(protocol, Some("opus"), Some("high"), Some("plan"), Some("h")),
                LaunchConfig::default(),
                "{protocol:?} 不该参与启动指纹判定"
            );
        }
    }

    /// 四项任一变化都要触发重连（`accepts` 为 false）；全都没变则复用。
    #[test]
    fn every_launch_flag_change_forces_a_reconnect() {
        let base = |model, reasoning, sandbox, hash| {
            launch_config_for_turn(
                StreamFormat::ClaudeStreamJson,
                model,
                reasoning,
                sandbox,
                hash,
            )
        };
        let established = base(Some("opus"), Some("high"), Some("plan"), Some("h1"));
        assert!(established.accepts(&base(Some("opus"), Some("high"), Some("plan"), Some("h1"))));
        assert!(!established.accepts(&base(Some("sonnet"), Some("high"), Some("plan"), Some("h1"))));
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
