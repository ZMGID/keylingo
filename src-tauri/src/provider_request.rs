//! 供应商级「请求配置」的请求头装配。
//!
//! 这是自定义头 + CLI 身份头的**唯一装配入口**：发送路径（`apply`）与请求调试面板
//! （`header_pairs`）共用同一个函数，杜绝「面板显示的和实际发的不一致」。写法对齐
//! `chat/model/openai.rs::session_header_pairs` 的既有约定。
//!
//! 校验在这里也做一遍（前端已拦过一层）：settings.json 是用户可以手改的文件，
//! 非法头名会让 reqwest 构造失败，头值里的 CR/LF 是 header 注入。

use crate::settings::{ModelProvider, ProviderCustomHeader};

/// 由 Kivio 自己管理、不允许用户覆盖的头。放开会让鉴权/路由错乱。
const RESERVED_HEADER_KEYS: &[&str] = &[
    "authorization",
    "x-api-key",
    "x-goog-api-key",
    "host",
    "content-length",
    "content-encoding",
    "content-type",
    // 适配器已经发了 `Accept-Encoding: identity`，而 reqwest 的 `.header()` 是 append 不是
    // 覆盖 —— 用户再填一条 gzip 会变成 `identity, gzip`，客户端没开 gzip 解码，
    // 认这条头的供应商回来的 SSE 就是一堆二进制垃圾。
    "accept-encoding",
    "anthropic-version",
];

// 内置 CLI 版本号。手填版本为空时用它们。
pub const CLAUDE_CODE_BUILTIN_VERSION: &str = "2.1.71";
pub const CODEX_BUILTIN_VERSION: &str = "0.72.0";
pub const GROK_BUILTIN_VERSION: &str = "0.2.110";

/// RFC 7230 token 字符集。
pub fn is_valid_header_key(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// 只允许可见 ASCII 与水平制表符：CR/LF 是 header 注入，非 ASCII 会让部分网关直接 400。
pub fn is_valid_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
}

pub fn is_reserved_header_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    RESERVED_HEADER_KEYS.contains(&normalized.as_str())
}

/// 一条自定义头是否可用（合法且非保留）。
pub fn is_usable_header(header: &ProviderCustomHeader) -> bool {
    is_valid_header_key(&header.key)
        && is_valid_header_value(&header.value)
        && !is_reserved_header_key(&header.key)
}

/// 版本号会被拼进 User-Agent，所以必须先过头值校验：含 CR/LF 会让 reqwest 构造请求
/// 直接失败（该供应商所有请求报一个看不懂的错），非 ASCII 会被网关 400。非法就退回内置值。
fn identity_version(provider: &ModelProvider, builtin: &str) -> String {
    let configured = provider.request.cli_identity_version.trim();
    if configured.is_empty() || !is_valid_header_value(configured) {
        builtin.to_string()
    } else {
        configured.to_string()
    }
}

/// CLI 身份预设头。命中的网关按 User-Agent 判断客户端类型，只放行特定 CLI。
fn identity_pairs(
    provider: &ModelProvider,
    conversation_id: Option<&str>,
) -> Vec<(String, String)> {
    let p = |k: &str, v: String| (k.to_string(), v);
    match provider.request.cli_identity.trim() {
        "claude_code" => {
            let version = identity_version(provider, CLAUDE_CODE_BUILTIN_VERSION);
            // 整套一起发：按 UA 放行的网关往往同时校验这组 X-Stainless-* 指纹，
            // 少发几条等于没伪装。Content-Type / anthropic-version 由适配器自己带。
            vec![
                p(
                    "User-Agent",
                    format!("claude-cli/{version} (external, cli)"),
                ),
                p("x-app", "cli".to_string()),
                p("X-Stainless-OS", "MacOS".to_string()),
                p("X-Stainless-Arch", "arm64".to_string()),
                p("X-Stainless-Lang", "js".to_string()),
                p("X-Stainless-Runtime", "node".to_string()),
                p("X-Stainless-Runtime-Version", "v22.19.0".to_string()),
                p("X-Stainless-Package-Version", "0.74.0".to_string()),
                p("X-Stainless-Timeout", "600".to_string()),
                p("X-Stainless-Retry-Count", "0".to_string()),
                p(
                    "anthropic-dangerous-direct-browser-access",
                    "true".to_string(),
                ),
            ]
        }
        "codex" => {
            let version = identity_version(provider, CODEX_BUILTIN_VERSION);
            let mut pairs = vec![p(
                "User-Agent",
                format!("codex_cli_rs/{version} (Ubuntu 24.4.0; x86_64) WindowsTerminal"),
            )];
            // session_id / conversation_id 是 Codex CLI 链路的会话身份头；没有会话 id 就不发，
            // 编不出来的假 id 只会让会话亲和型网关串台。
            if let Some(id) = conversation_id.filter(|id| !id.is_empty()) {
                pairs.push(p("session_id", id.to_string()));
                pairs.push(p("conversation_id", id.to_string()));
            }
            pairs
        }
        "grok" => {
            let version = identity_version(provider, GROK_BUILTIN_VERSION);
            vec![p(
                "User-Agent",
                format!("grok-shell/{version} (linux; x86_64)"),
            )]
        }
        _ => Vec::new(),
    }
}

/// 该供应商本次请求要附加的全部头：先铺 CLI 身份预设，再叠用户自定义头（同名覆盖）。
pub fn header_pairs(
    provider: &ModelProvider,
    conversation_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut pairs = identity_pairs(provider, conversation_id);
    for header in &provider.request.custom_headers {
        if !is_usable_header(header) {
            continue;
        }
        // 同名覆盖，并改用用户自己写的大小写（HTTP 头名大小写不敏感，但发出去的
        // 应该是用户填的那个样子）。
        upsert_pair(&mut pairs, header.key.clone(), header.value.clone());
    }
    for (name, value) in crate::provider_oauth::header_pairs(provider) {
        upsert_pair(&mut pairs, name, value);
    }
    pairs
}

/// 把一条头并进 pairs：同名（大小写不敏感）就整条替换，否则追加。
///
/// reqwest 的 `RequestBuilder::header` 是 **append 不是覆盖**，所以任何「同名只发一条」的
/// 保证都必须在这里做完 —— 一旦两条同名的进了 pairs，上游会收到两行，而请求调试面板用的是
/// BTreeMap，只显示后写的那条，「面板显示的和实际发的」就对不上了。
pub fn upsert_pair(pairs: &mut Vec<(String, String)>, name: String, value: String) {
    match pairs
        .iter_mut()
        .find(|(n, _)| n.eq_ignore_ascii_case(&name))
    {
        Some(existing) => *existing = (name, value),
        None => pairs.push((name, value)),
    }
}

/// 把 `header_pairs` 的结果贴到请求上。
pub fn apply(
    request: reqwest::RequestBuilder,
    provider: &ModelProvider,
    conversation_id: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut request = request;
    for (name, value) in header_pairs(provider, conversation_id) {
        request = request.header(name, value);
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ProviderRequestConfig;

    fn provider_with(request: ProviderRequestConfig) -> ModelProvider {
        ModelProvider {
            id: "p".to_string(),
            name: "P".to_string(),
            api_keys: vec!["k".to_string()],
            api_key_legacy: None,
            base_url: "https://x/v1".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request,
            active_key_index: 0,
        }
    }

    fn header(key: &str, value: &str) -> ProviderCustomHeader {
        ProviderCustomHeader {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn no_config_sends_nothing_extra() {
        let provider = provider_with(ProviderRequestConfig::default());
        assert!(header_pairs(&provider, Some("conv")).is_empty());
    }

    #[test]
    fn custom_headers_pass_through_and_reserved_are_dropped() {
        let provider = provider_with(ProviderRequestConfig {
            custom_headers: vec![
                header("X-Title", "kivio"),
                header("Authorization", "Bearer stolen"),
                header("x-api-key", "nope"),
            ],
            ..Default::default()
        });
        let pairs = header_pairs(&provider, None);
        assert_eq!(pairs, vec![("X-Title".to_string(), "kivio".to_string())]);
    }

    #[test]
    fn malformed_headers_are_dropped() {
        let provider = provider_with(ProviderRequestConfig {
            custom_headers: vec![
                header("Bad Name", "v"),         // 头名里有空格
                header("X-Inject", "a\r\nB: c"), // CRLF 注入
                header("X-Cjk", "中文"),         // 非 ASCII
                header("X-Ok", "fine"),
            ],
            ..Default::default()
        });
        let pairs = header_pairs(&provider, None);
        assert_eq!(pairs, vec![("X-Ok".to_string(), "fine".to_string())]);
    }

    #[test]
    fn custom_header_overrides_identity_preset_case_insensitively() {
        let provider = provider_with(ProviderRequestConfig {
            cli_identity: "claude_code".to_string(),
            custom_headers: vec![header("user-agent", "mine/1.0")],
            ..Default::default()
        });
        let pairs = header_pairs(&provider, None);
        // 覆盖而不是追加：同名头只能有一条，否则上游看到的是哪条全凭运气。
        let uas: Vec<_> = pairs
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
            .collect();
        assert_eq!(uas.len(), 1);
        assert_eq!(uas[0].1, "mine/1.0");
    }

    #[test]
    fn claude_identity_uses_builtin_version_when_unset() {
        let provider = provider_with(ProviderRequestConfig {
            cli_identity: "claude_code".to_string(),
            ..Default::default()
        });
        let pairs = header_pairs(&provider, None);
        assert!(pairs.contains(&(
            "User-Agent".to_string(),
            format!("claude-cli/{CLAUDE_CODE_BUILTIN_VERSION} (external, cli)")
        )));
    }

    #[test]
    fn adapter_owned_headers_cannot_be_overridden() {
        // reqwest 的 .header() 是 append，用户填这几条会变成两行共存：
        // Accept-Encoding 多出 gzip 会让 SSE 流变二进制垃圾，Content-Type 多一条网关 400。
        let provider = provider_with(ProviderRequestConfig {
            custom_headers: vec![
                header("Accept-Encoding", "gzip"),
                header("content-type", "text/plain"),
                header("X-Ok", "1"),
            ],
            ..Default::default()
        });
        assert_eq!(
            header_pairs(&provider, None),
            vec![("X-Ok".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn bad_identity_version_falls_back_to_builtin() {
        // 版本号会被拼进 User-Agent：CR/LF 会让 reqwest 构造请求直接失败（该供应商所有
        // 请求报一个看不懂的错），非 ASCII 会被网关 400。
        const CRLF_VERSION: &str = "2.0\r\n0";
        const TRAILING_LF: &str = "2.0\n";
        let ua_for = |version: &str| {
            let provider = provider_with(ProviderRequestConfig {
                cli_identity: "claude_code".to_string(),
                cli_identity_version: version.to_string(),
                ..Default::default()
            });
            header_pairs(&provider, None)
                .into_iter()
                .find(|(name, _)| name == "User-Agent")
                .map(|(_, value)| value)
                .expect("User-Agent present")
        };

        // 内嵌 CR/LF 与非 ASCII：退回内置版本。
        for bad in [CRLF_VERSION, "中文"] {
            let ua = ua_for(bad);
            assert_eq!(
                ua,
                format!("claude-cli/{CLAUDE_CODE_BUILTIN_VERSION} (external, cli)")
            );
        }
        // 首尾空白（粘贴常带的尾换行）先被 trim 掉，剩下的合法就照用，不必退回。
        assert_eq!(ua_for(TRAILING_LF), "claude-cli/2.0 (external, cli)");

        // 不管走哪条分支，发出去的头值都必须是合法的。
        for version in [CRLF_VERSION, "中文", TRAILING_LF, "", "2.1.71"] {
            assert!(
                is_valid_header_value(&ua_for(version)),
                "version: {version:?}"
            );
        }
    }

    #[test]
    fn claude_identity_sends_the_whole_stainless_fingerprint() {
        // 按 UA 放行的网关往往同时校验这一整组头，少发几条等于没伪装。
        let provider = provider_with(ProviderRequestConfig {
            cli_identity: "claude_code".to_string(),
            ..Default::default()
        });
        let names: Vec<String> = header_pairs(&provider, None)
            .into_iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect();
        for expected in [
            "user-agent",
            "x-app",
            "x-stainless-os",
            "x-stainless-arch",
            "x-stainless-lang",
            "x-stainless-runtime",
            "x-stainless-runtime-version",
            "x-stainless-package-version",
            "x-stainless-timeout",
            "x-stainless-retry-count",
            "anthropic-dangerous-direct-browser-access",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing {expected}: {names:?}"
            );
        }
    }

    #[test]
    fn codex_identity_session_headers_need_a_conversation_id() {
        let provider = provider_with(ProviderRequestConfig {
            cli_identity: "codex".to_string(),
            cli_identity_version: "1.2.3".to_string(),
            ..Default::default()
        });
        let with_id = header_pairs(&provider, Some("conv-1"));
        assert!(with_id.contains(&("session_id".to_string(), "conv-1".to_string())));
        assert!(with_id.contains(&("conversation_id".to_string(), "conv-1".to_string())));
        assert!(with_id
            .iter()
            .any(|(_, v)| v.starts_with("codex_cli_rs/1.2.3")));

        let without_id = header_pairs(&provider, None);
        assert!(!without_id.iter().any(|(k, _)| k == "session_id"));
    }
}
