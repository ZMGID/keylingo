//! HTTP 客户端、provider 凭据解析、retry / failover、OpenAI 兼容 chat completion 调用与 SSE 流。
//!
//! 本模块对外暴露：
//! - `ProviderConnectionInput` / `resolve_provider_credentials` —— 来自前端的 provider 临时配置或 settings.json 的解析。
//! - `build_http_client` —— 共享 reqwest Client 构造；只设置连接/读空闲超时。
//! - `with_standard_request_timeout` —— 为非流式请求显式加总超时。
//! - `effective_retry_attempts` —— 把 settings.retry_enabled + retry_attempts 折成实际尝试次数。
//! - `extract_status_code` / `is_failover_error` —— failover 判定（401/402/403 立即换 key；429 阈值化换 key）。
//! - `send_with_retry` —— 网络抖动 / 5xx / 429 退避重试。
//! - `send_with_failover` —— 在 api_keys 列表上轮换；401/402/403 立即换 key，429 达阈值且有备用 key 才换。
//! - `call_openai_text` / `call_openai_ocr` / `call_vision_api` —— 翻译/OCR/Lens 三类调用，
//!   内部组 `GenerateRequest` 走 `chat/model/` 的多协议适配器（尊重 provider.api_format）。
//! - `stream_chat_call` / `stream_translate_combined` —— 流式调用；`LensEventSink` /
//!   `CombinedTranslateEventSink` 把适配器的 `StreamPart` 翻译成现有 Tauri 事件。
//! - `ocr_image_message` —— 视觉请求的 image+text 用户消息构造。

use std::{
    collections::HashSet,
    fs,
    future::Future,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use base64::{engine::general_purpose, Engine as _};
use reqwest::{header::HeaderMap, Client, RequestBuilder, StatusCode};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use crate::chat::model::{
    generate_with_chat_provider, stream_with_chat_provider, GenerateOptions, GenerateRequest,
    MessagePart, ModelError, ModelErrorKind, ModelMessage, ModelRole, RequestMetadata, StreamPart,
    StreamSink,
};
use crate::lens_commands::resolve_explain_image_path;
use crate::prompts::COMBINED_TRANSLATE_SEPARATOR;
use crate::settings::{
    self, default_lens_system_prompt, no_think_instruction, ExplainMessage, Settings,
};
use crate::state::AppState;

// ===== Provider 凭据 =====

/// 供应商连接输入参数，用于测试连接或获取模型列表时临时传入
/// api_keys 优先；api_key 为兼容旧前端发的单 key 字段（v2.3.x 时的 ProviderConnectionInput）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionInput {
    pub id: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub api_keys: Vec<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// 连接测试用的模型：有则发一条极小对话请求（比 /models 更能反映“能不能调模型”，
    /// 且不依赖供应商支持 /models）；无则回退到 /models 探测。
    #[serde(default)]
    pub model: Option<String>,
    /// 供应商接口协议（openai_chat / anthropic_messages / openai_responses / gemini），
    /// 测试请求按它选 URL 和鉴权方式；缺省时从 settings 里的供应商配置读取。
    #[serde(default)]
    pub api_format: Option<String>,
    /// 「请求配置」（自定义头 / 代理 / CLI 身份）。设置窗口是手动保存的，测试连接必须用
    /// **正在编辑**的这份，否则「测试通过、聊天 403」，用户完全查不出原因。
    #[serde(default)]
    pub request: Option<crate::settings::ProviderRequestConfig>,
}

impl ProviderConnectionInput {
    /// 整理出非空 key 列表：优先 api_keys，回退到 api_key。
    pub fn merged_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .api_keys
            .iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        if keys.is_empty() {
            if let Some(legacy) = self.api_key.as_deref() {
                let trimmed = legacy.trim().to_string();
                if !trimmed.is_empty() {
                    keys.push(trimmed);
                }
            }
        }
        keys
    }
}

/// 解析供应商的凭据信息（base_url + 多 key 列表）
/// 优先使用传入的 ProviderConnectionInput（如测试连接时），否则从 settings 中查找对应的供应商
pub fn resolve_provider_credentials(
    settings: &Settings,
    provider_id: &str,
    provider: Option<ProviderConnectionInput>,
) -> Result<(String, Vec<String>), String> {
    if let Some(input) = provider {
        let id_matches = input
            .id
            .as_ref()
            .map(|id| id.is_empty() || id == provider_id)
            .unwrap_or(true);

        if id_matches {
            return Ok((input.base_url.clone(), input.merged_keys()));
        }
    }

    let provider = settings
        .get_provider(provider_id)
        .ok_or_else(|| "Provider not found".to_string())?;
    Ok((provider.base_url.clone(), provider.api_keys.clone()))
}

/// 普通非流式 API 请求的总超时。
pub const STANDARD_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// 非流式**对话补全**的总超时。
///
/// 为什么不能沿用 60 秒：一次 high reasoning + 十万 token 输入的补全，光思考就要两三分钟，
/// 非流式又是「憋完整个回答再一次性返回」——60 秒结构性必然超时，三次重试白烧 195 秒。
/// 实测就踩过：流式跑了 135 秒断包后回落非流式，3×60 秒全超，用户等了三分多钟拿到
/// 一句"模型调用失败"。流式那条路本来就只有 300 秒读空闲、没有总时长上限，回落到更紧的
/// 硬顶是反的。
pub const CHAT_COMPLETION_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// 只限制 TCP/TLS 建连阶段，避免 DNS/握手长期卡住。
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 流式响应的读空闲超时：持续有 SSE chunk 到达时不会触发。
const HTTP_READ_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// 空闲连接在池中最多保留多久后被淘汰。默认(reqwest 90s)偏长；缩短以更快丢弃可能已被
/// 服务端/NAT 静默关闭的连接，降低长时间运行后复用陈旧连接的概率。
const HTTP_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// TCP keepalive：让内核定期探活，及时发现对端已消失的连接。
const HTTP_TCP_KEEPALIVE: Duration = Duration::from_secs(30);
/// HTTP/2 keepalive PING 间隔：h2 单连接多路复用，一条半死连接会拖垮该 host 的全部请求；
/// 定期 PING 让 hyper 主动探测并丢弃死连接（配合 while_idle 覆盖空闲期）。
const HTTP2_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
/// HTTP/2 keepalive PING 超时：PING 无响应超过此值即判定连接死亡。
const HTTP2_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(15);

/// 为非流式请求设置总超时。流式请求不要用它，否则长回答会被总时长切断。
pub fn with_standard_request_timeout(request: RequestBuilder) -> RequestBuilder {
    request.timeout(STANDARD_HTTP_REQUEST_TIMEOUT)
}

/// 为非流式**对话补全**设置总超时（[`CHAT_COMPLETION_REQUEST_TIMEOUT`]）。
/// 四个适配器（openai / anthropic / gemini / responses）的非流式路径都用它，
/// 别再退回 [`with_standard_request_timeout`]——那是给 embedding / rerank / 文档解析
/// 这类「秒级就该回」的请求用的。
pub fn with_chat_request_timeout(request: RequestBuilder) -> RequestBuilder {
    request.timeout(CHAT_COMPLETION_REQUEST_TIMEOUT)
}

/// 把 JSON body 挂到请求上：`gzip=false` 走普通 `.json()`；`gzip=true` 则序列化后
/// gzip 压缩并设置 `Content-Encoding: gzip`。用于绕开个别供应商前置 WAF 对明文请求体的
/// 误拦（详见 `ModelProvider::compress_request_body`）。压缩任一步失败都安全退回明文。
pub fn attach_json_body(
    request: RequestBuilder,
    body: &serde_json::Value,
    gzip: bool,
) -> RequestBuilder {
    use std::io::Write as _;
    if gzip {
        if let Ok(raw) = serde_json::to_vec(body) {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            if enc.write_all(&raw).is_ok() {
                if let Ok(gz) = enc.finish() {
                    return request
                        .header(reqwest::header::CONTENT_TYPE, "application/json")
                        .header(reqwest::header::CONTENT_ENCODING, "gzip")
                        .body(gz);
                }
            }
        }
    }
    request.json(body)
}

/// 构建 HTTP 客户端：不设置 total timeout，避免活跃 SSE 流在 60 秒处被砍掉。
/// 连接池加保活 + 缩短空闲淘汰：长时间运行后不再复用被对端静默关闭的陈旧连接
/// （见任务 07-24-http-connection-pool-diagnosis）。
pub fn build_http_client() -> Client {
    build_client(false)
}

/// 直连客户端：同样的连接池调优，但忽略系统/环境代理。供关掉「跟随系统代理」的供应商使用。
pub fn build_direct_http_client() -> Client {
    build_client(true)
}

fn build_client(no_proxy: bool) -> Client {
    let builder = Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .read_timeout(HTTP_READ_IDLE_TIMEOUT)
        .pool_idle_timeout(HTTP_POOL_IDLE_TIMEOUT)
        .tcp_keepalive(HTTP_TCP_KEEPALIVE)
        .http2_keep_alive_interval(HTTP2_KEEPALIVE_INTERVAL)
        .http2_keep_alive_timeout(HTTP2_KEEPALIVE_TIMEOUT)
        .http2_keep_alive_while_idle(true);
    let builder = if no_proxy {
        builder.no_proxy()
    } else {
        builder
    };
    builder.build().unwrap_or_else(|err| {
        eprintln!("Failed to build HTTP client: {err}");
        Client::new()
    })
}

/// 流式 UTF-8 增量解码器：SSE 分片可能在多字节字符（如中文 3 字节）中间切开，
/// 逐片 `from_utf8_lossy` 会把半个字符变成替换符。此解码器把不完整的尾字节留到下一片。
#[derive(Default)]
pub struct Utf8StreamDecoder {
    tail: Vec<u8>,
}

impl Utf8StreamDecoder {
    /// 喂入一片原始字节，返回可安全解码的前缀；未构成完整字符的尾字节暂存到下次。
    pub fn push(&mut self, chunk: &[u8]) -> String {
        self.tail.extend_from_slice(chunk);
        let valid = match std::str::from_utf8(&self.tail) {
            Ok(s) => s.len(),
            Err(err) => err.valid_up_to(),
        };
        let out = String::from_utf8_lossy(&self.tail[..valid]).into_owned();
        self.tail.drain(..valid);
        // 单个 UTF-8 字符最多 4 字节：残留超过 4 字节说明是真·非法序列而非跨片切割，
        // 按 lossy 冲掉以免永久卡住。
        if self.tail.len() > 4 {
            let flushed = String::from_utf8_lossy(&self.tail).into_owned();
            self.tail.clear();
            return out + &flushed;
        }
        out
    }
}

// ===== Retry / Failover =====

/// 重试延迟基础值（毫秒）。暂时性错误起步退避 ~5s。
const RETRY_BASE_DELAY_MS: u64 = 5_000;
/// 重试延迟最大值（毫秒）。温和退避封顶 30s（Retry-After 可覆盖更大值）。
const RETRY_MAX_DELAY_MS: u64 = 30_000;
/// 同一个 key 上连续 429 退避重试达到该次数后，若存在未冷却的备用 key，
/// 则交回外层切 key（在新 key 上重新计数 / 重试）；无备用 key 时继续退避到总次数上限。
const RATE_LIMIT_KEY_SWITCH_THRESHOLD: usize = 2;
/// 限流（429）专用的最小重试次数。限流是「等一会就能恢复」的暂时性错误（QPM/配额按时间桶
/// 刷新），值得比通用 `retry_attempts` 更耐心地退避重试——对标 Claude Code 对 rate-limit 的
/// 多次退避。仅在**无备用 key**（无法换 key 规避）时生效；有备用 key 时仍按阈值优先换 key。
const RATE_LIMIT_MAX_ATTEMPTS: usize = 8;

/// 获取实际的重试次数
/// 如果重试功能被禁用，则返回 1（即只尝试一次）
pub fn effective_retry_attempts(settings: &Settings) -> usize {
    if settings.retry_enabled {
        settings.retry_attempts as usize
    } else {
        1
    }
}

/// 从响应头中解析 Retry-After 值（秒），转换为毫秒延迟
fn parse_retry_after(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

/// 判断 HTTP 状态码是否属于"立即换 key"错误（坏 / 失效 key）：
/// - 401 鉴权失败（key 被吊销 / 错误）
/// - 402 需要付费（账户欠费）
/// - 403 权限不足 / 被封禁
/// 这些与 key 直接相关、在同一 key 上重试永远失败 → 内层不重试，立即交外层换 key。
/// 注意：429 不在此列 —— 429 由内层退避重试，仅在达到阈值且有备用 key 时才换 key。
fn is_immediate_failover_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 402 | 403)
}

/// 判断请求错误是否可重试
/// 包括超时和连接错误
fn is_retryable_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

/// 把 reqwest 错误展开成完整 source 链文本。reqwest 的 `Display` 常年只有一句
/// `error sending request`，真正的原因（DNS 解析 / TCP connect / TLS handshake /
/// connection closed / h2 reset）都在 `std::error::Error::source()` 链里。诊断长时间
/// 运行后的 `statusCode=null` 网络故障必须看到这条链（见任务 07-24-http-connection-pool-diagnosis）。
pub fn format_reqwest_error(error: &reqwest::Error) -> String {
    use std::error::Error as _;
    let mut parts = vec![error.to_string()];
    let mut src = error.source();
    while let Some(err) = src {
        let text = err.to_string();
        // 跳过与上一层完全重复的文案，避免链条里堆叠同一句。
        if parts.last().map(|p| p != &text).unwrap_or(true) {
            parts.push(text);
        }
        src = err.source();
    }
    // 附带 reqwest 的错误类别标记，便于区分是 connect / timeout / body / decode。
    let mut kind = Vec::new();
    if error.is_connect() {
        kind.push("connect");
    }
    if error.is_timeout() {
        kind.push("timeout");
    }
    if error.is_request() {
        kind.push("request");
    }
    if error.is_body() {
        kind.push("body");
    }
    if error.is_decode() {
        kind.push("decode");
    }
    let chain = parts.join(" → ");
    if kind.is_empty() {
        chain
    } else {
        format!("{chain} [{}]", kind.join(","))
    }
}

/// 计算重试延迟
/// 优先使用服务器返回的 Retry-After 头；否则使用指数退避策略。
/// Retry-After 同样受 `RETRY_MAX_DELAY_MS` 封顶：上游（或中转）偶尔返回
/// `Retry-After: 86400` 之类的巨值，照睡会把整次请求挂死一天（只能靠取消轮询打断）。
fn retry_delay_ms(attempt: usize, retry_after: Option<u64>) -> u64 {
    if let Some(seconds) = retry_after {
        return seconds.saturating_mul(1000).min(RETRY_MAX_DELAY_MS);
    }

    let delay = RETRY_BASE_DELAY_MS.saturating_mul(2u64.saturating_pow((attempt - 1) as u32));
    delay.min(RETRY_MAX_DELAY_MS)
}

fn parse_leading_status_code(value: &str) -> Option<u16> {
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    if end == 0 {
        return None;
    }
    value[..end].parse().ok()
}

/// 从 HTTP 错误信息中提取状态码
/// 格式约定：`"{label} Error: {status} - {body}"`，
/// status 形如 `"429 Too Many Requests"`，第一段数字即可
/// 兼容少数防御性分支使用的 `"{label} HTTP {status}: {body}"`。
/// 网络错误（reqwest::Error 路径）格式为 `"{label} Error: <reqwest msg>"`，无前导数字 → 返回 None
pub fn extract_status_code(err_msg: &str) -> Option<u16> {
    if let Some(idx) = err_msg.find(" Error: ") {
        let rest = &err_msg[idx + " Error: ".len()..];
        if let Some(code) = parse_leading_status_code(rest) {
            return Some(code);
        }
    }

    if let Some(idx) = err_msg.find(" HTTP ") {
        let rest = &err_msg[idx + " HTTP ".len()..];
        return parse_leading_status_code(rest);
    }

    None
}

/// 判断错误信息是否触发 key failover（外层换 key 决策）
/// 严格按 HTTP 状态码：401/402/403/429 才换 key —— 与 key 直接相关的错误：
/// - 401 鉴权失败（key 被吊销 / 错误）→ 内层不重试，立即换 key
/// - 402 需要付费（账户欠费）→ 立即换 key
/// - 403 权限不足 / 被封禁 → 立即换 key
/// - 429 限流（key 维度配额耗尽）→ 内层先退避重试；达到阈值且有备用 key 时，
///   429 错误冒泡到外层触发换 key（在新 key 上重新计数）
/// 其它 4xx（如 400 malformed body）属于请求本身问题，换 key 也无济于事 → 不触发
/// 5xx 由内层退避重试，正常不会到这里（除非耗尽次数；耗尽后非 key 问题，不换 key）
/// 网络错误（timeout / connect 失败）非 key 问题，extract_status_code 返回 None → 不触发
pub fn is_failover_error(err_msg: &str) -> bool {
    matches!(extract_status_code(err_msg), Some(401 | 402 | 403 | 429))
}

/// 多 key failover 包装：在 api_keys 列表上依次尝试，遇到 failover-eligible 错误自动切下一 key。
///
/// 错误分类（内层 vs 外层换 key）：
/// - **401/402/403（坏 / 失效 key）**：内层不重试，立即冒泡 → 外层换 key。
/// - **429（限流）**：内层在当前 key 退避重试；只有当**同一 key 连续 429 达到阈值**
///   `RATE_LIMIT_KEY_SWITCH_THRESHOLD` **且存在未冷却备用 key** 时，才让 429 冒泡 → 外层换 key
///   （换后在新 key 上重新计数 / 重试）；无备用 key 时继续退避到总次数上限。
/// - **5xx / timeout / connect（暂时性）**：内层退避重试，不换 key（不是 key 的问题）。
/// - **400 / 404 / 422 等确定性客户端错误**：不重试，快速失败。
///
/// 始终优先尊重 Retry-After。所有 key 用尽后返回最后一次错误（最终失败路径不变）。
pub async fn send_with_failover<F, Fut>(
    state: &AppState,
    label: &str,
    attempts: usize,
    provider_id: &str,
    api_keys: &[String],
    send: F,
) -> Result<reqwest::Response, String>
where
    F: Fn(&str) -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    send_with_failover_cancelable(
        state,
        label,
        attempts,
        provider_id,
        api_keys,
        || false,
        send,
    )
    .await
}

async fn send_with_failover_cancelable<F, Fut, C>(
    state: &AppState,
    label: &str,
    attempts: usize,
    provider_id: &str,
    api_keys: &[String],
    is_cancelled: C,
    send: F,
) -> Result<reqwest::Response, String>
where
    F: Fn(&str) -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
    C: Fn() -> bool + Send + Sync,
{
    let total = api_keys.len();
    if total == 0 {
        return Err(format!("{} Error: No API key configured", label));
    }

    let mut tried: HashSet<usize> = HashSet::new();
    let mut last_err: Option<String> = None;

    while tried.len() < total {
        if is_cancelled() {
            return Err(format!("{} cancelled", label));
        }
        let idx = match state.pick_active_key(provider_id, total, &tried) {
            Some(i) => i,
            None => break,
        };
        tried.insert(idx);
        let key = api_keys[idx].as_str();

        // 是否还有未试过的备用 key —— 决定 429 是否在阈值处提前交回外层换 key。
        let has_backup_key = state.pick_active_key(provider_id, total, &tried).is_some();
        let rate_limit_cap = if has_backup_key {
            Some(RATE_LIMIT_KEY_SWITCH_THRESHOLD)
        } else {
            None
        };

        let mut attempt_send = || send(key);
        match send_with_retry_status_policy(
            label,
            attempts,
            &mut attempt_send,
            FailoverRetryPolicy { rate_limit_cap },
            &is_cancelled,
        )
        .await
        {
            Ok(resp) => {
                state.mark_key_ok(provider_id, idx);
                return Ok(resp);
            }
            Err(err_msg) => {
                if is_failover_error(&err_msg) && tried.len() < total {
                    state.mark_key_failed(provider_id, idx);
                    eprintln!(
                        "[failover] {} key #{}/{} failed, switching to next: {}",
                        label,
                        idx + 1,
                        total,
                        err_msg
                    );
                    last_err = Some(err_msg);
                    continue;
                }
                // 非 failover 错误（或已穷举所有 key）→ 直接返回
                if is_failover_error(&err_msg) {
                    state.mark_key_failed(provider_id, idx);
                }
                return Err(err_msg);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| format!("{} Error: all {} keys exhausted", label, total)))
}

/// 带重试机制的 HTTP 发送函数
/// 对可重试的错误（限流、服务器错误、超时、连接失败）进行指数退避重试
///
/// 职责边界:这里只做**传输层重试**(429 / 5xx / 网络超时连接错误的退避;坏 key 换 key)。
/// **语义级恢复**(上下文超长 overflow 的「压缩后重试」、内容审核去敏重试、确定性兜底)
/// 不在此处——归 `chat/agent/recovery.rs`(分类 + 策略中枢)+ `chat/agent/synthesis.rs`
/// (执行)。不要在这里加 overflow / 去敏判定,避免与上层语义恢复重复退避、放大延迟。
pub async fn send_with_retry<F, Fut>(
    label: &str,
    attempts: usize,
    mut send: F,
) -> Result<reqwest::Response, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let never_cancelled = || false;
    send_with_retry_status_policy(
        label,
        attempts,
        &mut send,
        FailoverRetryPolicy {
            rate_limit_cap: None,
        },
        &never_cancelled,
    )
    .await
}

/// failover 内层重试策略：
/// - 401/402/403：不重试，立即冒泡（外层换 key）。
/// - 429：退避重试；若 `rate_limit_cap` 为 Some(N)（有备用 key），同一 key 上第 N 次 429
///   后冒泡（外层换 key）；为 None（无备用 key）则退避到总次数上限。
/// - 5xx / timeout / connect：退避重试，不换 key。
/// - 其它确定性 4xx：不重试，快速失败。
///
/// 内层重试的状态分类策略。
#[derive(Clone, Copy)]
struct FailoverRetryPolicy {
    /// 429 退避重试的次数上限：Some(N) 表示有备用 key，同一 key 上第 N 次 429 后停止重试
    /// 并冒泡（让外层换 key）；None 表示无备用 key，429 与 5xx 一样退避到总次数上限。
    rate_limit_cap: Option<usize>,
}

impl FailoverRetryPolicy {
    /// 判断在第 `rate_limit_attempts` 次 429（含本次）后，是否还应继续在当前 key 上退避重试。
    /// - 有备用 key 且已达阈值 → false（停止重试 → 冒泡换 key）。
    /// - 无备用 key → true（继续退避，受总次数上限约束）。
    fn should_retry_rate_limit(&self, rate_limit_attempts: usize) -> bool {
        match self.rate_limit_cap {
            Some(cap) => rate_limit_attempts < cap,
            None => true,
        }
    }
}

async fn send_with_retry_status_policy<F, Fut>(
    label: &str,
    attempts: usize,
    send: &mut F,
    policy: FailoverRetryPolicy,
    is_cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<reqwest::Response, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let attempts = attempts.max(1);
    // 限流（429）在无备用 key 时用更高的重试上限耐心退避；有备用 key 时维持通用上限
    // （阈值处会冒泡换 key，换 key 比干等更优）。其它错误（5xx/网络）仍走通用 `attempts`。
    let rate_limit_max = if policy.rate_limit_cap.is_none() {
        attempts.max(RATE_LIMIT_MAX_ATTEMPTS)
    } else {
        attempts
    };
    let loop_max = attempts.max(rate_limit_max);
    let mut last_error: Option<String> = None;
    // 同一 key 上累计的 429 次数，用于阈值化换 key。
    let mut rate_limit_attempts: usize = 0;

    for attempt in 1..=loop_max {
        if is_cancelled() {
            return Err(format!("{} cancelled", label));
        }
        match send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    return Ok(response);
                }

                // 401/402/403：坏 / 失效 key，内层不重试，立即冒泡（外层换 key）。
                if is_immediate_failover_status(status) {
                    let text = response.text().await.unwrap_or_default();
                    let err_msg = format!("{} Error: {} - {}", label, status, text);
                    return Err(format!("{} (attempt {}/{})", err_msg, attempt, attempts));
                }

                let is_rate_limit = status == StatusCode::TOO_MANY_REQUESTS;
                if is_rate_limit {
                    rate_limit_attempts += 1;
                }

                let retry_after = parse_retry_after(response.headers());
                let text = response.text().await.unwrap_or_default();
                let err_msg = format!("{} Error: {} - {}", label, status, text);

                // 429：受 key-switch 阈值约束 + 限流专用上限耐心退避；
                // 5xx：退避重试到通用次数；其它确定性 4xx：不重试快速失败。
                let (should_retry, shown_max) = if is_rate_limit {
                    (
                        policy.should_retry_rate_limit(rate_limit_attempts)
                            && attempt < rate_limit_max,
                        rate_limit_max,
                    )
                } else {
                    (status.is_server_error() && attempt < attempts, attempts)
                };

                if should_retry {
                    last_error = Some(err_msg);
                    let delay = retry_delay_ms(attempt, retry_after);
                    eprintln!(
                        "{} retrying in {}ms (attempt {}/{})",
                        label, delay, attempt, shown_max
                    );
                    if is_cancelled() {
                        return Err(format!("{} cancelled", label));
                    }
                    sleep_with_cancel(label, delay, is_cancelled).await?;
                    continue;
                }

                return Err(format!("{} (attempt {}/{})", err_msg, attempt, shown_max));
            }
            Err(err) => {
                let err_msg = format!("{} Error: {}", label, format_reqwest_error(&err));
                if is_retryable_error(&err) && attempt < attempts {
                    last_error = Some(err_msg);
                    let delay = retry_delay_ms(attempt, None);
                    eprintln!(
                        "{} retrying in {}ms (attempt {}/{})",
                        label, delay, attempt, attempts
                    );
                    if is_cancelled() {
                        return Err(format!("{} cancelled", label));
                    }
                    sleep_with_cancel(label, delay, is_cancelled).await?;
                    continue;
                }
                return Err(format!("{} (attempt {}/{})", err_msg, attempt, attempts));
            }
        }
    }

    Err(last_error
        .map(|msg| format!("{} (attempt {}/{})", msg, loop_max, loop_max))
        .unwrap_or_else(|| format!("{} Error: exceeded retry attempts ({})", label, loop_max)))
}

async fn sleep_with_cancel(
    label: &str,
    delay_ms: u64,
    is_cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(), String> {
    const CANCEL_POLL_MS: u64 = 250;
    let mut remaining = delay_ms;
    while remaining > 0 {
        if is_cancelled() {
            return Err(format!("{} cancelled", label));
        }
        let step = remaining.min(CANCEL_POLL_MS);
        tokio::time::sleep(Duration::from_millis(step)).await;
        remaining -= step;
    }
    if is_cancelled() {
        return Err(format!("{} cancelled", label));
    }
    Ok(())
}

// ===== Chat completion 调用 =====
// 翻译/OCR/Lens 的模型调用统一组 `GenerateRequest` 走 `chat/model/` 的多协议适配器
// （openai_chat / anthropic_messages / openai_responses / gemini），尊重 provider.api_format。
// usage 记录由适配器完成（source/operation 通过 `RequestMetadata` 显式传入，面板维度不变）；
// 本层不再重复记录。

/// 构造翻译/OCR/Lens 调用的 `RequestMetadata`：label 进错误文案，
/// usage_source/usage_operation 显式传给适配器的 usage 记录（不靠 label 推断）。
fn legacy_request_metadata(
    label: &str,
    usage_source: &str,
    usage_operation: &str,
) -> RequestMetadata {
    RequestMetadata {
        label: label.to_string(),
        usage_source: Some(usage_source.to_string()),
        usage_operation: Some(usage_operation.to_string()),
        ..RequestMetadata::default()
    }
}

/// 视觉请求的 image+text 用户消息（image 在 text 前，与 lens/截图翻译一贯顺序一致）。
/// 各协议的图像编码由对应适配器完成。
pub fn ocr_image_message(image_path: &Path, prompt: &str) -> Result<ModelMessage, String> {
    let bytes = fs::read(image_path).map_err(|e| e.to_string())?;
    let base64 = general_purpose::STANDARD.encode(bytes);
    Ok(ModelMessage {
        role: ModelRole::User,
        content: vec![
            MessagePart::Image {
                mime_type: "image/png".to_string(),
                data: base64,
                path: None,
            },
            MessagePart::Text {
                text: prompt.to_string(),
            },
        ],
    })
}

/// 纯文本模型调用（翻译器 / 选中文本翻译 / 本地 OCR 后翻译的非流式路径）。
/// temperature / thinking 禁用字段由适配器按 provider/model 元数据处理。
pub async fn call_openai_text(
    state: &State<'_, AppState>,
    config: &settings::ModelProvider,
    model: &str,
    prompt: String,
    retry_attempts: usize,
    thinking_enabled: bool,
    usage_source: &str,
    usage_operation: &str,
) -> Result<String, String> {
    if model.trim().is_empty() {
        return Err("Please select a model first".to_string());
    }
    let request = GenerateRequest {
        model: model.to_string(),
        system: String::new(),
        messages: vec![ModelMessage::text(ModelRole::User, prompt)],
        tools: Vec::new(),
        options: GenerateOptions {
            thinking_enabled,
            ..GenerateOptions::default()
        },
        metadata: legacy_request_metadata("Text API", usage_source, usage_operation),
    };
    let output = generate_with_chat_provider(state.inner(), config, retry_attempts, request)
        .await
        .map_err(|err| err.to_string())?;
    Ok(output.text.trim().to_string())
}

/// 带图的 OCR/视觉调用（截图翻译非流式路径）。
/// 将图片作为 image part 发送，各协议的图像编码走对应适配器的 message 组装。
pub async fn call_openai_ocr(
    state: &State<'_, AppState>,
    config: &settings::ModelProvider,
    model: &str,
    image_path: &Path,
    prompt: &str,
    retry_attempts: usize,
    thinking_enabled: bool,
    usage_source: &str,
    usage_operation: &str,
) -> Result<String, String> {
    if model.trim().is_empty() {
        return Err("Please select a model first".to_string());
    }
    let request = GenerateRequest {
        model: model.to_string(),
        system: String::new(),
        messages: vec![ocr_image_message(image_path, prompt)?],
        tools: Vec::new(),
        options: GenerateOptions {
            thinking_enabled,
            ..GenerateOptions::default()
        },
        metadata: legacy_request_metadata("OCR API", usage_source, usage_operation),
    };
    let output = generate_with_chat_provider(state.inner(), config, retry_attempts, request)
        .await
        .map_err(|err| err.to_string())?;
    Ok(output.text.trim().to_string())
}

/// 调用视觉 API（截图解释 / Lens 共用）
/// 支持流式输出：如果 stream 为 true，通过 `LensEventSink` 逐段 emit `event_name` 事件。
/// `provider_id_override` 非空时使用指定 provider/model（用于 lens 选择独立模型）；空则走 explain 配置。
#[allow(clippy::too_many_arguments)]
pub async fn call_vision_api(
    app: &AppHandle,
    state: &State<'_, AppState>,
    image_id: &str,
    messages: Vec<ExplainMessage>,
    language: &str,
    retry_attempts: usize,
    stream: bool,
    stream_kind: &str,
    event_name: &str,
    provider_id_override: Option<&str>,
    model_override: Option<&str>,
    system_prompt_override: Option<&str>,
    thinking_enabled: bool,
    usage_source: &str,
    usage_operation: &str,
) -> Result<String, String> {
    let settings = state.settings_read().clone();
    let provider_id = provider_id_override
        .filter(|s| !s.is_empty())
        .unwrap_or(&settings.translator_provider_id);
    let provider = settings
        .get_provider(provider_id)
        .ok_or_else(|| "Vision provider not found".to_string())?;

    // image_id 为空 → 走纯文本对话路径（不附图）
    let has_image = !image_id.is_empty();

    // 优先用调用方传入的 system_prompt_override；否则用默认模板（区分有/无图片）
    // 关闭思考时在 system 末尾追加显式禁止指令，作为参数层不生效时的兜底
    // （适配器层会按 provider 元数据补 thinking 禁用字段）
    let system_prompt_to_use = {
        let base = match system_prompt_override.filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => default_lens_system_prompt(language, has_image),
        };
        if !thinking_enabled {
            format!("{}{}", base, no_think_instruction(language))
        } else {
            base
        }
    };

    let explain_role = |role: &str| -> ModelRole {
        if role == "assistant" {
            ModelRole::Assistant
        } else {
            ModelRole::User
        }
    };
    let mut api_messages: Vec<ModelMessage> = Vec::new();
    if has_image {
        let image_path = resolve_explain_image_path(app, state, image_id)?;
        if let Some(first) = messages.first() {
            api_messages.push(ocr_image_message(&image_path, &first.content)?);
            for message in messages.iter().skip(1) {
                api_messages.push(ModelMessage::text(
                    explain_role(&message.role),
                    message.content.clone(),
                ));
            }
        }
    } else {
        // 纯文本：每条 message 直接 push（无图）
        for message in messages.iter() {
            api_messages.push(ModelMessage::text(
                explain_role(&message.role),
                message.content.clone(),
            ));
        }
    }

    let model = model_override
        .filter(|s| !s.is_empty())
        .unwrap_or(&settings.translator_model);
    if model.trim().is_empty() {
        return Err("Please select a model first".to_string());
    }
    let request = GenerateRequest {
        model: model.to_string(),
        system: system_prompt_to_use,
        messages: api_messages,
        tools: Vec::new(),
        options: GenerateOptions {
            thinking_enabled,
            ..GenerateOptions::default()
        },
        metadata: legacy_request_metadata("Vision API", usage_source, usage_operation),
    };

    if stream {
        // 启动新流：递增代号，存到本流持有的快照里；sink 每次 emit 只要发现全局代号 != 自己的快照
        // 就返回 Cancelled 错，适配器沿 `?` 上抛回来。
        let generation = state
            .explain_stream_generation
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        let mut sink = LensEventSink::new(
            |payload| {
                let _ = app.emit(event_name, payload);
            },
            image_id,
            stream_kind,
            &state.explain_stream_generation,
            generation,
        );
        let result =
            stream_with_chat_provider(state.inner(), provider, retry_attempts, request, &mut sink)
                .await;
        return match result {
            // 正常结束：done 事件已由 sink 的 Finish 分支发出。
            Ok(output) => Ok(output.text.trim().to_string()),
            // 取消不是错误：emit done("cancelled")，返回已累积的部分文本。
            Err(err) if err.is_cancelled() => {
                sink.emit_done("cancelled");
                Ok(sink.full_text().trim().to_string())
            }
            Err(err) => {
                sink.emit_done("error");
                Err(err.to_string())
            }
        };
    }

    let output = generate_with_chat_provider(state.inner(), provider, retry_attempts, request)
        .await
        .map_err(|err| err.to_string())?;
    Ok(output.text.trim().to_string())
}

// ===== 流式调用 =====

/// 通用流式 chat 调用：组 `GenerateRequest` → 适配器流式 → `LensEventSink` emit 事件。
/// 复用 explain_stream_generation 作取消代号（lens-stream / lens-translate-stream 都共用）。
/// 取消不是错误：emit done("cancelled") 并返回已累积的部分文本。
#[allow(clippy::too_many_arguments)]
pub async fn stream_chat_call(
    app: &AppHandle,
    state: &State<'_, AppState>,
    provider: &settings::ModelProvider,
    model: &str,
    system: String,
    messages: Vec<ModelMessage>,
    retry_attempts: usize,
    thinking_enabled: bool,
    image_id: &str,
    kind: &str,
    event_name: &str,
    usage_source: &str,
    usage_operation: &str,
) -> Result<String, String> {
    if model.trim().is_empty() {
        return Err("Please select a model first".to_string());
    }
    let request = GenerateRequest {
        model: model.to_string(),
        system,
        messages,
        tools: Vec::new(),
        options: GenerateOptions {
            thinking_enabled,
            ..GenerateOptions::default()
        },
        metadata: legacy_request_metadata("Stream chat", usage_source, usage_operation),
    };
    let generation = state
        .explain_stream_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    let mut sink = LensEventSink::new(
        |payload| {
            let _ = app.emit(event_name, payload);
        },
        image_id,
        kind,
        &state.explain_stream_generation,
        generation,
    );
    let result =
        stream_with_chat_provider(state.inner(), provider, retry_attempts, request, &mut sink)
            .await;
    match result {
        // 正常结束：done 事件已由 sink 的 Finish 分支发出。
        Ok(output) => Ok(output.text.trim().to_string()),
        Err(err) if err.is_cancelled() => {
            sink.emit_done("cancelled");
            Ok(sink.full_text().trim().to_string())
        }
        Err(err) => {
            sink.emit_done("error");
            Err(err.to_string())
        }
    }
}

/// 截图翻译合并模式流：单次调用模型，按 `<<<ORIGINAL>>>` 分隔符把 SSE delta 拆成两段。
/// 分隔符前的 chunk emit kind="translated"；分隔符后的 chunk emit kind="original"。
/// 返回 (translated, original) 完整文本。
///
/// 关键点：
/// - 分隔符可能跨 SSE chunk 边界 → 用 tail 缓冲住末尾 (SEPARATOR.len()-1) 字节防止把分隔符前缀当成译文 emit 出去
/// - tail 切片必须落在 UTF-8 char boundary，否则 String::drain 会 panic（用户截图常含 CJK，每字 3 字节）
#[allow(clippy::too_many_arguments)]
pub async fn stream_translate_combined(
    app: &AppHandle,
    state: &State<'_, AppState>,
    provider: &settings::ModelProvider,
    model: &str,
    system: String,
    messages: Vec<ModelMessage>,
    retry_attempts: usize,
    thinking_enabled: bool,
    image_id: &str,
    event_name: &str,
    usage_source: &str,
    usage_operation: &str,
) -> Result<(String, String), String> {
    if model.trim().is_empty() {
        return Err("Please select a model first".to_string());
    }
    let request = GenerateRequest {
        model: model.to_string(),
        system,
        messages,
        tools: Vec::new(),
        options: GenerateOptions {
            thinking_enabled,
            ..GenerateOptions::default()
        },
        metadata: legacy_request_metadata(
            "Stream translate combined",
            usage_source,
            usage_operation,
        ),
    };
    let my_gen = state
        .explain_stream_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    let mut sink = CombinedTranslateEventSink::new(
        |payload| {
            let _ = app.emit(event_name, payload);
        },
        image_id,
        &state.explain_stream_generation,
        my_gen,
    );
    let result =
        stream_with_chat_provider(state.inner(), provider, retry_attempts, request, &mut sink)
            .await;
    match result {
        // 正常结束：Finish 分支已 flush tail + emit done("done")。
        Ok(_) => Ok((sink.translated().to_string(), sink.original().to_string())),
        // 取消不是错误：flush tail 当译文 + emit done("cancelled")，返回部分结果。
        Err(err) if err.is_cancelled() => {
            sink.flush_tail_emit();
            sink.emit_done("cancelled");
            Ok((sink.translated().to_string(), sink.original().to_string()))
        }
        Err(err) => {
            sink.emit_done("error");
            Err(err.to_string())
        }
    }
}

// ===== StreamPart → Lens 事件桥（多协议适配器接入旧调用路径的 sink 层）=====

/// 取消哨兵文案：包含 "cancelled"，使 usage 记录层（`error_kind_from_message` /
/// `failure_status_from_message`）把该错误归类为 cancelled 而非 failure。
pub(crate) const STREAM_CANCELLED_SENTINEL: &str =
    "stream cancelled: superseded by a newer generation";

/// 代际失配时 sink 返回的取消错误。适配器的 `?` 会把它一路抛回调用方；
/// 调用方捕获 `is_cancelled()` 后按取消（正常收尾）而非错误处理。
pub(crate) fn stream_cancelled_error() -> ModelError {
    ModelError::with_kind(STREAM_CANCELLED_SENTINEL, ModelErrorKind::Cancelled)
}

/// 把模型适配器的 `StreamPart` 翻译成现有 Lens Tauri 事件 payload 的 sink。
/// 事件形状与 `stream_vision_response` 的 emit 逐字节一致（UI 契约不动）：
/// - `TextDelta`      → `{ imageId, kind, delta }`
/// - `ReasoningDelta` → `{ imageId, kind, delta: "", reasoningDelta }`
/// - `Finish`         → `{ imageId, kind, delta: "", done: true, reason: "done", full }`
///
/// 每次 emit 前检查 `explain_stream_generation` 代际：失配即返回 `Cancelled` 错。
/// 事件发送通过注入闭包完成（调用方包一层 `app.emit(event_name, …)`），便于脱离
/// `AppHandle` 单测。
pub(crate) struct LensEventSink<'a, F: FnMut(serde_json::Value) + Send> {
    emit_event: F,
    image_id: &'a str,
    kind: &'a str,
    generation_atom: &'a AtomicU64,
    my_generation: u64,
    full: String,
}

impl<'a, F: FnMut(serde_json::Value) + Send> LensEventSink<'a, F> {
    pub(crate) fn new(
        emit_event: F,
        image_id: &'a str,
        kind: &'a str,
        generation_atom: &'a AtomicU64,
        my_generation: u64,
    ) -> Self {
        Self {
            emit_event,
            image_id,
            kind,
            generation_atom,
            my_generation,
            full: String::new(),
        }
    }

    /// 已累积的完整文本（取消时调用方取部分结果用）。
    pub(crate) fn full_text(&self) -> &str {
        &self.full
    }

    /// 发出 done 事件（reason: "done"/"cancelled"/"error"）。Finish 分支与调用方收尾共用。
    pub(crate) fn emit_done(&mut self, reason: &str) {
        (self.emit_event)(serde_json::json!({
          "imageId": self.image_id,
          "kind": self.kind,
          "delta": "",
          "done": true,
          "reason": reason,
          "full": self.full.trim(),
        }));
    }

    fn is_stale(&self) -> bool {
        self.generation_atom.load(Ordering::SeqCst) != self.my_generation
    }
}

impl<F: FnMut(serde_json::Value) + Send> StreamSink for LensEventSink<'_, F> {
    fn emit(&mut self, part: StreamPart) -> Result<(), ModelError> {
        if self.is_stale() {
            return Err(stream_cancelled_error());
        }
        match part {
            StreamPart::TextDelta { delta } => {
                if delta.is_empty() {
                    return Ok(());
                }
                self.full.push_str(&delta);
                (self.emit_event)(serde_json::json!({
                  "imageId": self.image_id, "kind": self.kind, "delta": delta,
                }));
            }
            StreamPart::ReasoningDelta { delta } => {
                if delta.is_empty() {
                    return Ok(());
                }
                (self.emit_event)(serde_json::json!({
                  "imageId": self.image_id,
                  "kind": self.kind,
                  "delta": "",
                  "reasoningDelta": delta,
                }));
            }
            StreamPart::Finish { .. } => {
                self.emit_done("done");
            }
            StreamPart::Error { message } => return Err(ModelError::new(message)),
            // 不传 tools，ToolCall* 不会出现。
            _ => {}
        }
        Ok(())
    }
}

/// 合并翻译流的一段拆分产物：分隔符前为译文、后为原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CombinedSplitPiece {
    Translated(String),
    Original(String),
}

/// `<<<ORIGINAL>>>` 拆分状态机（从 `stream_translate_combined` 的内联逻辑提炼，
/// 行为逐条对齐；本体的迁移在后续步骤完成）。
///
/// 关键点（与原实现一致）：
/// - 分隔符可能跨 delta 边界 → tail 缓冲住末尾 (SEPARATOR.len()-1) 字节，防止把分隔符
///   前缀当译文 emit 出去
/// - tail 切片必须落在 UTF-8 char boundary，否则 `String::drain` 会 panic
///   （用户截图常含 CJK，每字 3 字节）
/// - 分隔符命中时：前段 trim 尾部换行、后段 trim 头部换行（仅限同一 delta 内的相邻换行）
#[derive(Debug, Default)]
pub(crate) struct CombinedTranslateSplitter {
    tail: String,
    sep_seen: bool,
    translated: String,
    original: String,
}

impl CombinedTranslateSplitter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 喂入一段文本增量，返回本次可安全发出的拆分片段（可能为空）。
    pub(crate) fn push(&mut self, delta: &str) -> Vec<CombinedSplitPiece> {
        let mut out = Vec::new();
        if delta.is_empty() {
            return out;
        }
        if self.sep_seen {
            self.original.push_str(delta);
            out.push(CombinedSplitPiece::Original(delta.to_string()));
            return out;
        }

        let sep = COMBINED_TRANSLATE_SEPARATOR;
        let sep_len = sep.len();
        self.tail.push_str(delta);
        if let Some(idx) = self.tail.find(sep) {
            // 分隔符命中：拆 before / after，trim 掉分隔符相邻的换行，分别发出
            let before: String = self.tail.drain(..idx).collect();
            // 移除分隔符本身
            self.tail.drain(..sep_len);
            let after: String = std::mem::take(&mut self.tail);

            let before_emit = before.trim_end_matches('\n').to_string();
            if !before_emit.is_empty() {
                self.translated.push_str(&before_emit);
                out.push(CombinedSplitPiece::Translated(before_emit));
            }
            self.sep_seen = true;
            let after_emit = after.trim_start_matches('\n').to_string();
            if !after_emit.is_empty() {
                self.original.push_str(&after_emit);
                out.push(CombinedSplitPiece::Original(after_emit));
            }
        } else {
            // 没命中：emit 安全前缀（保留末尾 sep_len-1 字节防止跨 delta 分隔符被切碎）
            let max_emit = self.tail.len().saturating_sub(sep_len.saturating_sub(1));
            if max_emit == 0 {
                return out;
            }
            // 找一个合法 char boundary（CJK 字符多字节，不能切到字符中间）
            let mut safe = max_emit;
            while safe > 0 && !self.tail.is_char_boundary(safe) {
                safe -= 1;
            }
            if safe == 0 {
                return out;
            }
            let to_emit: String = self.tail.drain(..safe).collect();
            self.translated.push_str(&to_emit);
            out.push(CombinedSplitPiece::Translated(to_emit));
        }
        out
    }

    /// 流结束/取消时把残留 tail 当译文 flush（避免末尾几个字符丢失）。
    /// 与原实现一致：仅在分隔符尚未出现且 tail 非空时有内容。
    pub(crate) fn flush_tail(&mut self) -> Option<String> {
        if self.sep_seen || self.tail.is_empty() {
            return None;
        }
        let tail = std::mem::take(&mut self.tail);
        self.translated.push_str(&tail);
        Some(tail)
    }

    /// 已累积的完整译文（含已 flush 的 tail）。
    pub(crate) fn translated(&self) -> &str {
        &self.translated
    }

    /// 已累积的完整原文。
    pub(crate) fn original(&self) -> &str {
        &self.original
    }
}

/// 合并翻译流的事件桥 sink：`TextDelta` 经 `CombinedTranslateSplitter` 拆成
/// kind="translated"/"original" 两种事件再转发。事件形状与 `stream_translate_combined`
/// 现有 emit 逐字节一致：
/// - 译文段  → `{ imageId, kind: "translated", delta }`
/// - 原文段  → `{ imageId, kind: "original", delta }`
/// - 推理链  → `{ imageId, kind: "translated", delta: "", reasoningDelta }`
/// - done    → `{ imageId, delta: "", done: true, reason }`（`Finish` 时 flush tail 后 reason="done"）
///
/// 与 `LensEventSink` 相同的代际取消语义。
pub(crate) struct CombinedTranslateEventSink<'a, F: FnMut(serde_json::Value) + Send> {
    emit_event: F,
    image_id: &'a str,
    generation_atom: &'a AtomicU64,
    my_generation: u64,
    splitter: CombinedTranslateSplitter,
}

impl<'a, F: FnMut(serde_json::Value) + Send> CombinedTranslateEventSink<'a, F> {
    pub(crate) fn new(
        emit_event: F,
        image_id: &'a str,
        generation_atom: &'a AtomicU64,
        my_generation: u64,
    ) -> Self {
        Self {
            emit_event,
            image_id,
            generation_atom,
            my_generation,
            splitter: CombinedTranslateSplitter::new(),
        }
    }

    pub(crate) fn translated(&self) -> &str {
        self.splitter.translated()
    }

    pub(crate) fn original(&self) -> &str {
        self.splitter.original()
    }

    fn is_stale(&self) -> bool {
        self.generation_atom.load(Ordering::SeqCst) != self.my_generation
    }

    fn emit_piece(&mut self, piece: CombinedSplitPiece) {
        let (kind, delta) = match piece {
            CombinedSplitPiece::Translated(delta) => ("translated", delta),
            CombinedSplitPiece::Original(delta) => ("original", delta),
        };
        (self.emit_event)(serde_json::json!({
          "imageId": self.image_id, "kind": kind, "delta": delta,
        }));
    }

    /// 把残留 tail 当译文 flush 并发出对应事件（done/cancel 收尾共用）。
    pub(crate) fn flush_tail_emit(&mut self) {
        if let Some(tail) = self.splitter.flush_tail() {
            self.emit_piece(CombinedSplitPiece::Translated(tail));
        }
    }

    /// 发出 done 事件（reason: "done" / "cancelled" / "error"，由调用方收尾时定）。
    pub(crate) fn emit_done(&mut self, reason: &str) {
        (self.emit_event)(serde_json::json!({
          "imageId": self.image_id, "delta": "", "done": true, "reason": reason,
        }));
    }
}

impl<F: FnMut(serde_json::Value) + Send> StreamSink for CombinedTranslateEventSink<'_, F> {
    fn emit(&mut self, part: StreamPart) -> Result<(), ModelError> {
        if self.is_stale() {
            return Err(stream_cancelled_error());
        }
        match part {
            StreamPart::TextDelta { delta } => {
                for piece in self.splitter.push(&delta) {
                    self.emit_piece(piece);
                }
            }
            StreamPart::ReasoningDelta { delta } => {
                if delta.is_empty() {
                    return Ok(());
                }
                // 推理链 emit（恒定 kind="translated"，前端在主面板渲染）
                (self.emit_event)(serde_json::json!({
                  "imageId": self.image_id,
                  "kind": "translated",
                  "delta": "",
                  "reasoningDelta": delta,
                }));
            }
            StreamPart::Finish { .. } => {
                self.flush_tail_emit();
                self.emit_done("done");
            }
            StreamPart::Error { message } => return Err(ModelError::new(message)),
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_is_capped_at_max_delay() {
        // 上游/中转返回的巨值 Retry-After 不得被照单全收：否则单次重试会挂死数小时，
        // 只能靠取消轮询打断（B2）。
        assert_eq!(retry_delay_ms(1, Some(86_400)), RETRY_MAX_DELAY_MS);
        assert_eq!(retry_delay_ms(1, Some(u64::MAX)), RETRY_MAX_DELAY_MS);
        // 合理值仍原样生效（未被 clamp 抹平）。
        let small = 2u64;
        assert!(small * 1000 < RETRY_MAX_DELAY_MS, "测试前提：2s 应小于上限");
        assert_eq!(retry_delay_ms(1, Some(small)), small * 1000);
        // 无 Retry-After → 指数退避，同样受上限约束。
        assert!(retry_delay_ms(99, None) <= RETRY_MAX_DELAY_MS);
    }

    #[test]
    fn http_client_builder_config_is_valid() {
        // build_http_client 内部 unwrap_or_else 会在 builder 非法时静默退回 Client::new()，
        // 掩盖配置错误。这里直接断言相同的 builder 链能 build 成功（keepalive/pool 参数合法）。
        let built = Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .read_timeout(HTTP_READ_IDLE_TIMEOUT)
            .pool_idle_timeout(HTTP_POOL_IDLE_TIMEOUT)
            .tcp_keepalive(HTTP_TCP_KEEPALIVE)
            .http2_keep_alive_interval(HTTP2_KEEPALIVE_INTERVAL)
            .http2_keep_alive_timeout(HTTP2_KEEPALIVE_TIMEOUT)
            .http2_keep_alive_while_idle(true)
            .build();
        assert!(
            built.is_ok(),
            "http client builder rejected config: {built:?}"
        );
    }

    #[test]
    fn utf8_decoder_reassembles_split_multibyte() {
        // "温周" 各 3 字节；在字符中间切开分多片喂入，不应产生替换符。
        let bytes = "温周".as_bytes().to_vec();
        let mut dec = Utf8StreamDecoder::default();
        let mut out = String::new();
        // 逐字节喂：最坏情况的边界切割。
        for b in &bytes {
            out.push_str(&dec.push(&[*b]));
        }
        assert_eq!(out, "温周");
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn utf8_decoder_passes_ascii_and_complete_chunks() {
        let mut dec = Utf8StreamDecoder::default();
        assert_eq!(dec.push(b"hello "), "hello ");
        assert_eq!(dec.push("世界".as_bytes()), "世界");
    }

    // ===== attach_json_body (gzip) =====

    #[test]
    fn attach_json_body_plain_when_gzip_off() {
        let client = Client::new();
        let body =
            serde_json::json!({"model": "m", "messages": [{"role": "user", "content": "hi"}]});
        let req = attach_json_body(client.post("http://x.invalid/v1"), &body, false)
            .build()
            .expect("build");
        assert!(req
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .is_none());
        let sent = req
            .body()
            .and_then(|b| b.as_bytes())
            .expect("in-memory body");
        let parsed: serde_json::Value = serde_json::from_slice(sent).expect("plain json");
        assert_eq!(parsed, body);
    }

    #[test]
    fn attach_json_body_gzips_and_round_trips_when_gzip_on() {
        use std::io::Read as _;
        let client = Client::new();
        let body = serde_json::json!({"model": "m", "cmd": "rm -rf /tmp/x && cat /etc/passwd"});
        let req = attach_json_body(client.post("http://x.invalid/v1"), &body, true)
            .build()
            .expect("build");
        assert_eq!(
            req.headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .unwrap(),
            "gzip"
        );
        let gz = req
            .body()
            .and_then(|b| b.as_bytes())
            .expect("in-memory body");
        // 压缩体必须能解回原始 JSON。
        let mut dec = flate2::read::GzDecoder::new(gz);
        let mut raw = Vec::new();
        dec.read_to_end(&mut raw).expect("gunzip");
        let parsed: serde_json::Value = serde_json::from_slice(&raw).expect("round-trip json");
        assert_eq!(parsed, body);
    }

    #[test]
    fn extract_status_code_parses_typical_send_with_retry_format() {
        // send_with_retry 拼出来的标准格式
        let s = "OpenAI API Error: 429 Too Many Requests - {\"error\":\"rate_limit\"}";
        assert_eq!(extract_status_code(s), Some(429));
    }

    #[test]
    fn extract_status_code_handles_each_failover_status() {
        assert_eq!(
            extract_status_code("X Error: 401 Unauthorized - body"),
            Some(401)
        );
        assert_eq!(
            extract_status_code("X Error: 402 Payment Required - body"),
            Some(402)
        );
        assert_eq!(
            extract_status_code("X Error: 403 Forbidden - body"),
            Some(403)
        );
        assert_eq!(
            extract_status_code("X Error: 429 Too Many Requests - body"),
            Some(429)
        );
    }

    #[test]
    fn extract_status_code_handles_defensive_http_format() {
        assert_eq!(
            extract_status_code("Stream HTTP 429: rate limited"),
            Some(429)
        );
        assert_eq!(
            extract_status_code("Stream HTTP 401: unauthorized"),
            Some(401)
        );
        assert_eq!(
            extract_status_code("Vision API HTTP 403: forbidden"),
            Some(403)
        );
    }

    #[test]
    fn extract_status_code_handles_non_failover_status() {
        assert_eq!(
            extract_status_code("X Error: 400 Bad Request - body"),
            Some(400)
        );
        assert_eq!(
            extract_status_code("X Error: 500 Internal Server Error - body"),
            Some(500)
        );
    }

    #[test]
    fn extract_status_code_returns_none_for_network_error() {
        // reqwest::Error 路径无前导数字
        let s = "Stream chat Error: error sending request: connection refused (attempt 3/3)";
        assert_eq!(extract_status_code(s), None);
    }

    #[test]
    fn extract_status_code_returns_none_when_marker_missing() {
        assert_eq!(extract_status_code("just some message"), None);
        assert_eq!(extract_status_code(""), None);
    }

    // ===== is_failover_error =====

    #[test]
    fn is_failover_error_only_triggers_on_auth_quota_codes() {
        assert!(is_failover_error("X Error: 401 - body"));
        assert!(is_failover_error("X Error: 402 - body"));
        assert!(is_failover_error("X Error: 403 - body"));
        assert!(is_failover_error("X Error: 429 - body"));
        assert!(is_failover_error("Stream HTTP 429: rate limited"));
        assert!(is_failover_error("Stream HTTP 401: unauthorized"));
    }

    #[test]
    fn is_failover_error_does_not_trigger_on_400_or_5xx() {
        // 400 是请求 body 问题，不应换 key
        assert!(!is_failover_error("X Error: 400 Bad Request - body"));
        assert!(!is_failover_error("Stream HTTP 400: bad request"));
        // 500 由 send_with_retry 内部退避重试，不应到 failover 层
        assert!(!is_failover_error(
            "X Error: 500 Internal Server Error - body"
        ));
        assert!(!is_failover_error(
            "X Error: 503 Service Unavailable - body"
        ));
    }

    #[test]
    fn is_failover_error_does_not_trigger_on_network_failure() {
        // 网络问题不是 key 的锅
        assert!(!is_failover_error(
            "Stream Error: error sending request: timed out"
        ));
        assert!(!is_failover_error("X Error: connection closed"));
    }

    #[test]
    fn is_failover_error_does_not_trigger_on_body_keywords_alone() {
        // 旧版宽泛匹配 body 含 "billing" / "quota" 会误触发；现版严格按状态码
        assert!(!is_failover_error(
            "X Error: 400 - {\"message\":\"billing issue\"}"
        ));
        assert!(!is_failover_error(
            "X Error: 500 - {\"message\":\"quota exceeded\"}"
        ));
    }

    #[test]
    fn is_failover_error_still_triggers_on_429() {
        // 429 仍是 failover-eligible：内层退避到阈值后冒泡，外层据此换 key。
        assert!(is_failover_error("X Error: 429 Too Many Requests - body"));
    }

    // ===== 错误分类（is_immediate_failover_status / FailoverRetryPolicy） =====

    #[test]
    fn immediate_failover_status_covers_auth_codes_only() {
        assert!(is_immediate_failover_status(StatusCode::UNAUTHORIZED)); // 401
        assert!(is_immediate_failover_status(StatusCode::PAYMENT_REQUIRED)); // 402
        assert!(is_immediate_failover_status(StatusCode::FORBIDDEN)); // 403
                                                                      // 429 不是 immediate failover —— 由内层退避重试。
        assert!(!is_immediate_failover_status(StatusCode::TOO_MANY_REQUESTS));
        // 5xx / 4xx 确定性错误也不是 immediate failover。
        assert!(!is_immediate_failover_status(
            StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(!is_immediate_failover_status(StatusCode::BAD_REQUEST));
        assert!(!is_immediate_failover_status(StatusCode::NOT_FOUND));
    }

    #[test]
    fn rate_limit_policy_caps_at_threshold_when_backup_key_available() {
        let policy = FailoverRetryPolicy {
            rate_limit_cap: Some(RATE_LIMIT_KEY_SWITCH_THRESHOLD),
        };
        // 阈值 N=2：第 1 次 429 后继续重试，第 N 次后停止（冒泡换 key）。
        assert!(policy.should_retry_rate_limit(1));
        assert!(!policy.should_retry_rate_limit(RATE_LIMIT_KEY_SWITCH_THRESHOLD));
        assert!(!policy.should_retry_rate_limit(RATE_LIMIT_KEY_SWITCH_THRESHOLD + 1));
    }

    #[test]
    fn rate_limit_policy_retries_indefinitely_without_backup_key() {
        let policy = FailoverRetryPolicy {
            rate_limit_cap: None,
        };
        // 无备用 key：429 一直可重试（受外层总次数上限约束）。
        assert!(policy.should_retry_rate_limit(1));
        assert!(policy.should_retry_rate_limit(5));
        assert!(policy.should_retry_rate_limit(99));
    }

    // ===== retry_delay_ms / parse_retry_after =====

    #[test]
    fn retry_delay_starts_around_five_seconds_and_caps() {
        // 起步 ~5s（RETRY_BASE_DELAY_MS）；指数退避封顶 RETRY_MAX_DELAY_MS（30s）。
        assert_eq!(retry_delay_ms(1, None), RETRY_BASE_DELAY_MS);
        assert_eq!(retry_delay_ms(2, None), RETRY_BASE_DELAY_MS * 2);
        // 第 4 次本应 5s*8=40s，被 cap 到 30s。
        assert_eq!(retry_delay_ms(4, None), RETRY_MAX_DELAY_MS);
        assert_eq!(retry_delay_ms(10, None), RETRY_MAX_DELAY_MS);
    }

    #[test]
    fn retry_delay_prefers_retry_after_over_backoff() {
        // Retry-After 优先：哪怕退避会算出别的值，也用服务器给的秒数。
        assert_eq!(retry_delay_ms(1, Some(7)), 7_000);
        assert_eq!(retry_delay_ms(5, Some(2)), 2_000);
    }

    #[test]
    fn parse_retry_after_reads_seconds_header() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "12".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(12));

        let empty = HeaderMap::new();
        assert_eq!(parse_retry_after(&empty), None);
    }

    // ===== send_with_retry_status_policy / send_with_failover 行为（mock send 闭包） =====

    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;

    /// 用一个 reqwest::Error 模拟网络层错误（timeout/connect）。
    /// 通过对一个不可路由地址发起极短超时请求来获得真实的 reqwest::Error。
    async fn make_network_error() -> reqwest::Error {
        // 192.0.2.0/24 是 TEST-NET-1，保证不可路由 → connect/timeout 错误。
        Client::builder()
            .connect_timeout(Duration::from_millis(1))
            .build()
            .unwrap()
            .get("http://192.0.2.1:9/")
            .timeout(Duration::from_millis(1))
            .send()
            .await
            .expect_err("expected a network error")
    }

    /// 构造一个带指定状态码与可选 retry-after 的 reqwest::Response（不走网络）。
    fn make_response(status: u16, retry_after: Option<u64>) -> reqwest::Response {
        let mut builder = http::Response::builder().status(status);
        if let Some(secs) = retry_after {
            builder = builder.header("retry-after", secs.to_string());
        }
        let http_resp = builder.body("body").expect("build http response");
        reqwest::Response::from(http_resp)
    }

    fn test_never_cancelled() -> bool {
        false
    }

    #[tokio::test(start_paused = true)]
    async fn server_error_retries_up_to_attempt_limit() {
        let attempts = 5;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_inner = Arc::clone(&calls);

        let result = send_with_retry_status_policy(
            "Test",
            attempts,
            &mut || {
                let calls = Arc::clone(&calls_inner);
                async move {
                    calls.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok(make_response(500, None))
                }
            },
            FailoverRetryPolicy {
                rate_limit_cap: None,
            },
            &test_never_cancelled,
        )
        .await;

        assert!(result.is_err());
        // 5xx 一直重试到 attempts 次。
        assert_eq!(calls.load(AtomicOrdering::SeqCst), attempts);
    }

    #[tokio::test(start_paused = true)]
    async fn network_error_retries_up_to_attempt_limit() {
        let attempts = 5;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_inner = Arc::clone(&calls);

        let result = send_with_retry_status_policy(
            "Test",
            attempts,
            &mut || {
                let calls = Arc::clone(&calls_inner);
                async move {
                    calls.fetch_add(1, AtomicOrdering::SeqCst);
                    Err(make_network_error().await)
                }
            },
            FailoverRetryPolicy {
                rate_limit_cap: None,
            },
            &test_never_cancelled,
        )
        .await;

        assert!(result.is_err());
        // timeout/connect 网络错误也重试到上限。
        assert_eq!(calls.load(AtomicOrdering::SeqCst), attempts);
    }

    #[tokio::test(start_paused = true)]
    async fn deterministic_client_error_does_not_retry() {
        for status in [400u16, 404, 422] {
            let calls = Arc::new(AtomicUsize::new(0));
            let calls_inner = Arc::clone(&calls);

            let result = send_with_retry_status_policy(
                "Test",
                5,
                &mut || {
                    let calls = Arc::clone(&calls_inner);
                    async move {
                        calls.fetch_add(1, AtomicOrdering::SeqCst);
                        Ok(make_response(status, None))
                    }
                },
                FailoverRetryPolicy {
                    rate_limit_cap: None,
                },
                &test_never_cancelled,
            )
            .await;

            assert!(result.is_err());
            // 确定性 4xx 快速失败，只发一次。
            assert_eq!(
                calls.load(AtomicOrdering::SeqCst),
                1,
                "status {status} should not retry"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn immediate_failover_status_does_not_retry_inner() {
        for status in [401u16, 402, 403] {
            let calls = Arc::new(AtomicUsize::new(0));
            let calls_inner = Arc::clone(&calls);

            // 即便给了 rate_limit_cap，401/403 也不重试 —— 立即冒泡换 key。
            let result = send_with_retry_status_policy(
                "Test",
                5,
                &mut || {
                    let calls = Arc::clone(&calls_inner);
                    async move {
                        calls.fetch_add(1, AtomicOrdering::SeqCst);
                        Ok(make_response(status, None))
                    }
                },
                FailoverRetryPolicy {
                    rate_limit_cap: Some(RATE_LIMIT_KEY_SWITCH_THRESHOLD),
                },
                &test_never_cancelled,
            )
            .await;

            let err = result.expect_err("auth error should fail");
            assert!(
                is_failover_error(&err),
                "status {status} should be failover"
            );
            assert_eq!(
                calls.load(AtomicOrdering::SeqCst),
                1,
                "status {status} must not retry inner"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limit_backs_off_on_same_key_when_no_backup() {
        // 无备用 key：429 在同一 key 上退避重试到**限流专用上限**（耐心重试，不受较小的
        // 通用 attempts 限制），不提前停。
        let attempts = 5;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_inner = Arc::clone(&calls);

        let result = send_with_retry_status_policy(
            "Test",
            attempts,
            &mut || {
                let calls = Arc::clone(&calls_inner);
                async move {
                    calls.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok(make_response(429, None))
                }
            },
            FailoverRetryPolicy {
                rate_limit_cap: None,
            },
            &test_never_cancelled,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(calls.load(AtomicOrdering::SeqCst), RATE_LIMIT_MAX_ATTEMPTS);
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limit_bubbles_at_threshold_when_backup_available() {
        // 有备用 key：429 退避到阈值 N 后停止重试并冒泡（让外层换 key）。
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_inner = Arc::clone(&calls);

        let result = send_with_retry_status_policy(
            "Test",
            10, // 总次数远大于阈值，验证是阈值而非总次数封顶
            &mut || {
                let calls = Arc::clone(&calls_inner);
                async move {
                    calls.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok(make_response(429, None))
                }
            },
            FailoverRetryPolicy {
                rate_limit_cap: Some(RATE_LIMIT_KEY_SWITCH_THRESHOLD),
            },
            &test_never_cancelled,
        )
        .await;

        let err = result.expect_err("429 at threshold should bubble");
        assert!(is_failover_error(&err));
        // 第 N 次 429 后停止 → 共发 N 次。
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            RATE_LIMIT_KEY_SWITCH_THRESHOLD
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limit_respects_retry_after_header() {
        // Retry-After 优先：429 带 retry-after，仍退避重试（这里验证不快速失败、能继续）。
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_inner = Arc::clone(&calls);

        let result = send_with_retry_status_policy(
            "Test",
            3,
            &mut || {
                let calls = Arc::clone(&calls_inner);
                async move {
                    calls.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok(make_response(429, Some(2)))
                }
            },
            FailoverRetryPolicy {
                rate_limit_cap: None,
            },
            &test_never_cancelled,
        )
        .await;

        assert!(result.is_err());
        // 退避重试到限流专用上限（paused 时钟让 retry-after 的 sleep 瞬时跳过）。
        assert_eq!(calls.load(AtomicOrdering::SeqCst), RATE_LIMIT_MAX_ATTEMPTS);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_backoff_stops_when_cancelled() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_inner = Arc::clone(&calls);
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancelled_check = Arc::clone(&cancelled);

        let mut task = tokio::spawn(async move {
            send_with_retry_status_policy(
                "Test",
                5,
                &mut || {
                    let calls = Arc::clone(&calls_inner);
                    async move {
                        calls.fetch_add(1, AtomicOrdering::SeqCst);
                        Ok(make_response(500, None))
                    }
                },
                FailoverRetryPolicy {
                    rate_limit_cap: None,
                },
                &move || cancelled_check.load(AtomicOrdering::SeqCst),
            )
            .await
        });

        tokio::task::yield_now().await;
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

        cancelled.store(true, AtomicOrdering::SeqCst);
        tokio::time::advance(Duration::from_millis(250)).await;

        let result = (&mut task).await.expect("retry task should finish");
        assert!(matches!(result, Err(err) if err == "Test cancelled"));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failover_switches_key_after_429_threshold_with_backup() {
        // 两把 key：key#0 一直 429 → 达阈值换 key#1（key#1 成功）。
        let state = crate::state::test_app_state();
        let keys = vec!["key0".to_string(), "key1".to_string()];
        let key0_calls = Arc::new(AtomicUsize::new(0));
        let key1_calls = Arc::new(AtomicUsize::new(0));
        let k0 = Arc::clone(&key0_calls);
        let k1 = Arc::clone(&key1_calls);

        let result = send_with_failover(&state, "Test", 5, "prov", &keys, |key| {
            let k0 = Arc::clone(&k0);
            let k1 = Arc::clone(&k1);
            let key = key.to_string();
            async move {
                if key == "key0" {
                    k0.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok(make_response(429, None))
                } else {
                    k1.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok(make_response(200, None))
                }
            }
        })
        .await;

        assert!(result.is_ok());
        // key#0 退避到阈值 N 次，然后换到 key#1 成功一次。
        assert_eq!(
            key0_calls.load(AtomicOrdering::SeqCst),
            RATE_LIMIT_KEY_SWITCH_THRESHOLD
        );
        assert_eq!(key1_calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failover_switches_immediately_on_auth_error() {
        // key#0 返回 401 → 立即换 key（不重试），key#1 成功。
        let state = crate::state::test_app_state();
        let keys = vec!["key0".to_string(), "key1".to_string()];
        let key0_calls = Arc::new(AtomicUsize::new(0));
        let k0 = Arc::clone(&key0_calls);

        let result = send_with_failover(&state, "Test", 5, "prov", &keys, |key| {
            let k0 = Arc::clone(&k0);
            let key = key.to_string();
            async move {
                if key == "key0" {
                    k0.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok(make_response(401, None))
                } else {
                    Ok(make_response(200, None))
                }
            }
        })
        .await;

        assert!(result.is_ok());
        // 401 不重试，只发一次就换 key。
        assert_eq!(key0_calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failover_with_single_key_429_backs_off_no_switch() {
        // 只有一把 key：429 在该 key 上退避到总次数上限，没有可换的 key。
        let state = crate::state::test_app_state();
        let keys = vec!["only".to_string()];
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);

        let result = send_with_failover(&state, "Test", 5, "prov", &keys, |_key| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(make_response(429, None))
            }
        })
        .await;

        assert!(result.is_err());
        // 无备用 key → 退避到限流专用上限。
        assert_eq!(calls.load(AtomicOrdering::SeqCst), RATE_LIMIT_MAX_ATTEMPTS);
    }

    // ===== LensEventSink / CombinedTranslateSplitter（StreamPart → 事件桥）=====

    const TEST_SEP: &str = "<<<ORIGINAL>>>";

    fn text_delta(delta: &str) -> StreamPart {
        StreamPart::TextDelta {
            delta: delta.to_string(),
        }
    }

    #[test]
    fn lens_event_sink_translates_stream_parts_to_event_payloads() {
        let generation = AtomicU64::new(7);
        let mut events: Vec<serde_json::Value> = Vec::new();
        {
            let mut sink = LensEventSink::new(
                |payload| events.push(payload),
                "img-1",
                "answer",
                &generation,
                7,
            );
            sink.emit(text_delta("Hello")).unwrap();
            sink.emit(StreamPart::ReasoningDelta {
                delta: "thinking...".to_string(),
            })
            .unwrap();
            sink.emit(text_delta(" world")).unwrap();
            sink.emit(StreamPart::Finish {
                reason: "done".to_string(),
                full: "Hello world".to_string(),
            })
            .unwrap();
            assert_eq!(sink.full_text(), "Hello world");
        }
        // 形状与 stream_vision_response 的 emit 逐字节一致。
        assert_eq!(
            events,
            vec![
                serde_json::json!({ "imageId": "img-1", "kind": "answer", "delta": "Hello" }),
                serde_json::json!({
                  "imageId": "img-1", "kind": "answer", "delta": "", "reasoningDelta": "thinking..."
                }),
                serde_json::json!({ "imageId": "img-1", "kind": "answer", "delta": " world" }),
                serde_json::json!({
                  "imageId": "img-1", "kind": "answer", "delta": "", "done": true,
                  "reason": "done", "full": "Hello world"
                }),
            ]
        );
    }

    #[test]
    fn lens_event_sink_cancelled_generation_errors_on_first_emit() {
        let generation = AtomicU64::new(8); // 全局代际已前进（sink 持有的是 7）
        let mut events: Vec<serde_json::Value> = Vec::new();
        let mut sink = LensEventSink::new(
            |payload| events.push(payload),
            "img-1",
            "answer",
            &generation,
            7,
        );
        let err = sink.emit(text_delta("Hello")).unwrap_err();
        assert!(err.is_cancelled());
        assert!(events.is_empty());
        // usage 记录层按同一哨兵归类为 cancelled。
        assert_eq!(
            crate::usage::failure_status_from_message(&err.to_string()),
            "cancelled"
        );
        assert_eq!(
            crate::usage::error_kind_from_message(&err.to_string()),
            "cancelled"
        );
    }

    #[test]
    fn combined_sink_cancelled_generation_errors_on_first_emit() {
        let generation = AtomicU64::new(2);
        let mut events: Vec<serde_json::Value> = Vec::new();
        let mut sink = CombinedTranslateEventSink::new(
            |payload| events.push(payload),
            "img-1",
            &generation,
            1,
        );
        let err = sink.emit(text_delta("你好")).unwrap_err();
        assert!(err.is_cancelled());
        assert!(events.is_empty());
    }

    #[test]
    fn combined_splitter_separator_within_single_delta() {
        let mut splitter = CombinedTranslateSplitter::new();
        let pieces = splitter.push(&format!("你好\n{TEST_SEP}\nHello"));
        assert_eq!(
            pieces,
            vec![
                CombinedSplitPiece::Translated("你好".to_string()),
                CombinedSplitPiece::Original("Hello".to_string()),
            ]
        );
        assert_eq!(splitter.translated(), "你好");
        assert_eq!(splitter.original(), "Hello");
    }

    #[test]
    fn combined_splitter_separator_across_delta_boundary() {
        let mut splitter = CombinedTranslateSplitter::new();
        let mut pieces = Vec::new();
        // 分隔符被拆成三段跨 delta 到达。
        pieces.extend(splitter.push("译文<<<ORI"));
        pieces.extend(splitter.push("GIN"));
        pieces.extend(splitter.push("AL>>>原文"));
        assert_eq!(
            pieces,
            vec![
                CombinedSplitPiece::Translated("译文".to_string()),
                CombinedSplitPiece::Original("原文".to_string()),
            ]
        );
        assert!(splitter.flush_tail().is_none());
    }

    #[test]
    fn combined_splitter_holds_separator_prefix_in_tail() {
        let mut splitter = CombinedTranslateSplitter::new();
        // "<<<ORI" 是分隔符前缀（< sep_len-1 字节），必须整个扣在 tail 里不发出。
        let pieces = splitter.push("<<<ORI");
        assert!(pieces.is_empty());
        // 后续证明它确实是分隔符 → 不应有任何 Translated 泄漏。
        let pieces = splitter.push("GINAL>>>after");
        assert_eq!(
            pieces,
            vec![CombinedSplitPiece::Original("after".to_string())]
        );
        assert_eq!(splitter.translated(), "");
    }

    #[test]
    fn combined_splitter_separator_prefix_turns_out_to_be_text() {
        let mut splitter = CombinedTranslateSplitter::new();
        // 前缀最终不是分隔符 → 之后作为译文正常放行。
        let mut pieces = Vec::new();
        pieces.extend(splitter.push("<<<ORI"));
        pieces.extend(splitter.push("X more text"));
        let flushed = splitter.flush_tail();
        let mut all = String::new();
        for piece in &pieces {
            match piece {
                CombinedSplitPiece::Translated(t) => all.push_str(t),
                CombinedSplitPiece::Original(_) => panic!("unexpected original piece"),
            }
        }
        if let Some(tail) = flushed {
            all.push_str(&tail);
        }
        assert_eq!(all, "<<<ORIX more text");
        assert_eq!(splitter.translated(), "<<<ORIX more text");
    }

    #[test]
    fn combined_splitter_cjk_multibyte_boundary_does_not_panic() {
        // tail 安全前缀切点落在 CJK 多字节字符中间时必须回退到 char boundary。
        // 构造:tail 长度略大于 sep_len-1 且切点在"文"三字节内。
        let mut splitter = CombinedTranslateSplitter::new();
        let mut emitted = String::new();
        for delta in ["这是一段很长的中文译文", "继", "续", "累积文本"] {
            for piece in splitter.push(delta) {
                match piece {
                    CombinedSplitPiece::Translated(t) => emitted.push_str(&t),
                    CombinedSplitPiece::Original(_) => panic!("unexpected original piece"),
                }
            }
        }
        if let Some(tail) = splitter.flush_tail() {
            emitted.push_str(&tail);
        }
        assert_eq!(emitted, "这是一段很长的中文译文继续累积文本");
    }

    #[test]
    fn combined_splitter_separator_exactly_at_delta_end() {
        let mut splitter = CombinedTranslateSplitter::new();
        let mut pieces = Vec::new();
        pieces.extend(splitter.push(&format!("译文\n{TEST_SEP}")));
        pieces.extend(splitter.push("\n原文"));
        // 与旧实现逐字节对齐：trim 换行只发生在"分隔符所在的同一 delta"内；
        // 分隔符恰好收在 delta 末尾时，下一 delta 的开头换行原样透传（前端渲染时无感）。
        assert_eq!(
            pieces,
            vec![
                CombinedSplitPiece::Translated("译文".to_string()),
                CombinedSplitPiece::Original("\n原文".to_string()),
            ]
        );
        assert_eq!(splitter.translated(), "译文");
        assert_eq!(splitter.original(), "\n原文");
    }

    #[test]
    fn combined_splitter_flush_tail_preserves_trailing_chars() {
        // 无分隔符的流：结束时残留 tail 必须 flush 为译文（对齐旧取消/结束语义）。
        let mut splitter = CombinedTranslateSplitter::new();
        let mut emitted = String::new();
        for piece in splitter.push("整段没有分隔符的译文") {
            match piece {
                CombinedSplitPiece::Translated(t) => emitted.push_str(&t),
                CombinedSplitPiece::Original(_) => panic!("unexpected original piece"),
            }
        }
        let tail = splitter.flush_tail().expect("tail should flush");
        emitted.push_str(&tail);
        assert_eq!(emitted, "整段没有分隔符的译文");
        // flush 后再 flush 为空。
        assert!(splitter.flush_tail().is_none());
    }

    #[test]
    fn combined_sink_emits_split_events_and_done() {
        let generation = AtomicU64::new(3);
        let mut events: Vec<serde_json::Value> = Vec::new();
        {
            let mut sink = CombinedTranslateEventSink::new(
                |payload| events.push(payload),
                "img-9",
                &generation,
                3,
            );
            sink.emit(text_delta(&format!("你好\n{TEST_SEP}\nHello")))
                .unwrap();
            sink.emit(StreamPart::Finish {
                reason: "done".to_string(),
                full: String::new(),
            })
            .unwrap();
            assert_eq!(sink.translated(), "你好");
            assert_eq!(sink.original(), "Hello");
        }
        assert_eq!(
            events,
            vec![
                serde_json::json!({ "imageId": "img-9", "kind": "translated", "delta": "你好" }),
                serde_json::json!({ "imageId": "img-9", "kind": "original", "delta": "Hello" }),
                serde_json::json!({ "imageId": "img-9", "delta": "", "done": true, "reason": "done" }),
            ]
        );
    }
}
