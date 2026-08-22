use std::path::PathBuf;

/// Strip the Windows `\\?\` / `\\?\UNC\` prefix that `fs::canonicalize` adds.
///
/// Rust file APIs accept verbatim paths. Node (`dsh`, other CLIs) does not:
/// `fs.realpath('\\?\E:\foo')` throws `EISDIR ... lstat 'E:'`, so Host Workspace
/// attach fails and the session never joins the folder the user opened in Kivio.
pub fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        const VERBATIM_UNC: &str = r"\\?\UNC\";
        const VERBATIM: &str = r"\\?\";
        let raw = path.to_string_lossy();
        if let Some(rest) = raw.strip_prefix(VERBATIM_UNC) {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = raw.strip_prefix(VERBATIM) {
            return PathBuf::from(rest);
        }
    }
    path
}

/// 判断 provider 是否支持 `thinking` 字段。
/// 目前只有 DeepSeek 官方 API 和 Kimi 支持该字段；
/// 第三方代理（OpenRouter / 反代）做严格校验时会以 400 拒绝整个请求。
pub fn provider_supports_thinking_field(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    lower.contains("deepseek.com") || lower.contains("moonshot.cn")
}

/// 是否官方 DeepSeek API 主机（`api.deepseek.com`）。
/// 中转 / 文档站不算：hosted `web_search` 只在这条线上可靠。
pub fn is_official_deepseek_api(base_url: &str) -> bool {
    let lower = base_url.trim().to_ascii_lowercase();
    let host = lower
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(lower.as_str());
    let host = host.split('/').next().unwrap_or(host);
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host == "api.deepseek.com"
}

/// 官方 DeepSeek 的 Anthropic / Claude 协议端点（`https://api.deepseek.com/anthropic`）。
pub fn is_official_deepseek_anthropic_api(base_url: &str) -> bool {
    if !is_official_deepseek_api(base_url) {
        return false;
    }
    let lower = base_url.trim().to_ascii_lowercase();
    let rest = lower
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(lower.as_str());
    let path = rest.split_once('/').map(|(_, path)| path).unwrap_or("");
    path == "anthropic" || path.starts_with("anthropic/")
}

/**
 * 解析目标语言
 * 当设置为 "auto" 时，根据文本内容自动判断：
 * - 如果文本包含中文，则目标语言为英文
 * - 否则目标语言为中文
 */
pub fn resolve_target_lang(target: &str, text: &str) -> String {
    if target == "auto" {
        if has_chinese(text) {
            "en".to_string()
        } else {
            "zh".to_string()
        }
    } else {
        target.to_string()
    }
}

/**
 * 判断文本中是否包含中文字符
 */
pub fn has_chinese(text: &str) -> bool {
    text.chars().any(|c| ('\u{4e00}'..'\u{9fff}').contains(&c))
}

/**
 * 获取语言代码对应的显示名称
 */
pub fn language_name(code: &str) -> &'static str {
    match code {
        "zh" | "zh-Hans" => "Simplified Chinese",
        "zh-Hant" => "Traditional Chinese",
        "en" => "English",
        "ja" => "Japanese",
        "ko" => "Korean",
        "fr" => "French",
        "de" => "German",
        _ => "English",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_official_deepseek_anthropic_api, is_official_deepseek_api, strip_windows_verbatim_prefix,
    };
    use std::path::PathBuf;

    #[test]
    fn official_deepseek_api_matches_host_only() {
        assert!(is_official_deepseek_api("https://api.deepseek.com"));
        assert!(is_official_deepseek_api("https://api.deepseek.com/v1"));
        assert!(is_official_deepseek_api("https://api.deepseek.com/anthropic"));
        assert!(!is_official_deepseek_api("https://docs.deepseek.com"));
        assert!(!is_official_deepseek_api("https://relay.example/deepseek.com/v1"));
        assert!(!is_official_deepseek_api("https://api.openai.com/v1"));
    }

    #[test]
    fn official_deepseek_anthropic_api_matches_path() {
        assert!(is_official_deepseek_anthropic_api(
            "https://api.deepseek.com/anthropic"
        ));
        assert!(is_official_deepseek_anthropic_api(
            "https://api.deepseek.com/anthropic/v1"
        ));
        assert!(!is_official_deepseek_anthropic_api(
            "https://api.deepseek.com/v1"
        ));
        assert!(!is_official_deepseek_anthropic_api(
            "https://api.anthropic.com"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn strip_windows_verbatim_prefix_unwraps_drive_and_unc() {
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"\\?\E:\ZM database\kivioC")),
            PathBuf::from(r"E:\ZM database\kivioC")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\dir")),
            PathBuf::from(r"\\server\share\dir")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"E:\already\normal")),
            PathBuf::from(r"E:\already\normal")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn strip_windows_verbatim_prefix_is_a_noop_off_windows() {
        let path = PathBuf::from(r"\\?\E:\ZM database\kivioC");
        assert_eq!(strip_windows_verbatim_prefix(path.clone()), path);
    }
}
