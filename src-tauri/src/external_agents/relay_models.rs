//! 从中转站拉模型列表：给供应商弹窗的「获取模型」用。
//!
//! 中转站的 base_url 千奇百怪（`…/anthropic`、`…/v1`、裸域名都有），而 `/v1/models` 这个
//! 端点又不一定挂在同一层，所以按 ccgui 的做法**派生一串候选依次试**，first-hit 即返回。
//! 拉不到不是错误——它只是给四个档位输入框做建议，用户照样可以手填。
use std::time::Duration;

/// 由 base_url 派生出的候选 models 端点，按尝试顺序。
fn candidates(base_url: &str) -> Vec<String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!("{base}/v1/models")];
    if base.ends_with("/v1") {
        out.push(format!("{base}/models"));
    }
    // `https://relay.example/anthropic` 这种：模型列表通常挂在剥掉后缀的那一层。
    if let Some(stripped) = base.strip_suffix("/anthropic") {
        out.push(format!("{stripped}/v1/models"));
    }
    if let Ok(url) = url::Url::parse(base) {
        if let Some(host) = url.host_str() {
            let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
            let origin = format!("{}://{host}{port}/v1/models", url.scheme());
            out.push(origin);
        }
    }
    out.dedup();
    out
}

/// 从一份响应 JSON 里抽模型 id。兼容 `{data:[…]}` / 顶层数组 / `{models:[…]}`，
/// 元素可以是字符串，也可以是 `{id}` / `{name}` 对象。保序去重。
fn extract_ids(value: &serde_json::Value) -> Vec<String> {
    let array = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array());
    let Some(array) = array else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for item in array {
        let id = match item {
            serde_json::Value::String(s) => Some(s.trim().to_string()),
            serde_json::Value::Object(_) => item
                .get("id")
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string()),
            _ => None,
        };
        if let Some(id) = id.filter(|s| !s.is_empty()) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// 依次试候选端点，第一个能解析出模型的就返回。全都不行 → 返回最后一次的错误说明。
pub async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, String> {
    let urls = candidates(base_url);
    if urls.is_empty() {
        return Err("请先填 API URL".to_string());
    }
    let key = api_key.trim();
    let mut last_error = String::new();
    for url in urls {
        let mut req = client.get(&url).timeout(Duration::from_secs(15));
        if !key.is_empty() {
            // 两个头都带：中转站有的认 Bearer（Anthropic 系），有的认 x-api-key。
            req = req.bearer_auth(key).header("x-api-key", key);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if !status.is_success() {
                    last_error = format!("{url} → HTTP {status}");
                    continue;
                }
                match resp.json::<serde_json::Value>().await {
                    Ok(value) => {
                        let ids = extract_ids(&value);
                        if !ids.is_empty() {
                            return Ok(ids);
                        }
                        last_error = format!("{url} → 响应里没有模型列表");
                    }
                    Err(err) => last_error = format!("{url} → 响应不是 JSON：{err}"),
                }
            }
            Err(err) => last_error = format!("{url} → {err}"),
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn candidates_cover_the_common_relay_shapes() {
        let anthropic = candidates("https://relay.example/anthropic");
        assert_eq!(anthropic[0], "https://relay.example/anthropic/v1/models");
        // 剥掉 `/anthropic` 之后那一层是中转站最常见的挂法。
        assert!(anthropic.contains(&"https://relay.example/v1/models".to_string()));

        let v1 = candidates("https://relay.example/v1");
        assert!(v1.contains(&"https://relay.example/v1/models".to_string()));

        assert!(candidates("  ").is_empty());
    }

    #[test]
    fn extract_handles_the_three_response_shapes() {
        assert_eq!(
            extract_ids(&json!({"data": [{"id": "a"}, {"id": "b"}]})),
            vec!["a", "b"]
        );
        assert_eq!(extract_ids(&json!(["a", "a", "b"])), vec!["a", "b"]);
        assert_eq!(extract_ids(&json!({"models": [{"name": "c"}]})), vec!["c"]);
        assert!(extract_ids(&json!({"foo": 1})).is_empty());
    }
}
