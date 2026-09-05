//! Anonymous OpenCode Zen free tier. No CLI installation or credential import.
pub fn is_endpoint(base: &str) -> bool {
    base.trim().trim_end_matches('/') == "https://opencode.ai/zen/v1"
}

pub fn is_free_model(model: &str) -> bool {
    model == "big-pickle" || (model.ends_with("-free") && model != "ox-alpha-free")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opencode_free_is_scoped_to_official_zen_and_free_models() {
        assert!(is_endpoint("https://opencode.ai/zen/v1/"));
        for url in ["https://opencode.ai/zen/go/v1", "https://opencode.ai.evil/zen/v1", "http://opencode.ai/zen/v1"] {
            assert!(!is_endpoint(url));
        }
        assert!(is_free_model("deepseek-v4-flash-free"));
        assert!(is_free_model("big-pickle"));
        assert!(!is_free_model("ox-alpha-free"));
        assert!(!is_free_model("gpt-6-astra"));
    }
}
