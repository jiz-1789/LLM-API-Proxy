use serde::{Deserialize, Serialize};

/// Thinking intensity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Low,
    Medium,
    High,
    Max,
    Custom,
}

impl ThinkingLevel {
    /// Parse a level from its string representation. Unknown values fall back to `Off`.
    pub fn parse(s: &str) -> Self {
        match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "max" => Self::Max,
            "custom" => Self::Custom,
            _ => Self::Off,
        }
    }

    /// Whether this level enables thinking.
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Thinking parameter injection by vendor type and intensity level.
///
/// Mapping table (see PLAN 3.3):
/// - deepseek/ds:   `{"reasoning": true, "reasoning_effort": <level>}`
/// - openai/gpt:    `{"reasoning_effort": <level>}`
/// - claude/anthropic: `{"thinking": {"type": "enabled", "budget_tokens": N}}`
/// - gemini/google: `{"generationConfig": {"thinkingConfig": {"thinkingBudget": N}}}`
/// - custom:        raw JSON injected verbatim
/// - other vendors: no injection
pub fn get_thinking_params(
    vendor: &str,
    level: &ThinkingLevel,
    custom_params: &str,
) -> Option<serde_json::Value> {
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Custom => {
            if custom_params.trim().is_empty() {
                None
            } else {
                serde_json::from_str(custom_params).ok()
            }
        }
        _ => {
            let vendor_lower = vendor.to_lowercase();
            if vendor_lower.contains("deepseek") || vendor_lower.contains("ds") {
                Some(serde_json::json!({
                    "reasoning": true,
                    "reasoning_effort": level_str(level)
                }))
            } else if vendor_lower.contains("openai") || vendor_lower.contains("gpt") {
                Some(serde_json::json!({ "reasoning_effort": level_str(level) }))
            } else if vendor_lower.contains("claude") || vendor_lower.contains("anthropic") {
                Some(serde_json::json!({
                    "thinking": { "type": "enabled", "budget_tokens": claude_budget(level) }
                }))
            } else if vendor_lower.contains("gemini") || vendor_lower.contains("google") {
                Some(serde_json::json!({
                    "generationConfig": {
                        "thinkingConfig": { "thinkingBudget": gemini_budget(level) }
                    }
                }))
            } else {
                None
            }
        }
    }
}

fn level_str(level: &ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Max => "max",
        _ => "high",
    }
}

fn claude_budget(level: &ThinkingLevel) -> i64 {
    match level {
        ThinkingLevel::Low => 5_000,
        ThinkingLevel::Medium => 16_000,
        ThinkingLevel::High => 32_000,
        ThinkingLevel::Max => 64_000,
        _ => 16_000,
    }
}

fn gemini_budget(level: &ThinkingLevel) -> i64 {
    match level {
        ThinkingLevel::Low => 1_000,
        ThinkingLevel::Medium => 8_000,
        ThinkingLevel::High => 24_000,
        ThinkingLevel::Max => 32_000,
        _ => 8_000,
    }
}

/// Merge thinking parameters into the request body.
pub fn merge_thinking_params(body: &mut serde_json::Value, params: &Option<serde_json::Value>) {
    if let (Some(thinking_param), Some(obj)) = (params, body.as_object_mut())
        && let Some(thinking_obj) = thinking_param.as_object()
    {
        for (key, value) in thinking_obj {
            obj.insert(key.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_from_str() {
        assert_eq!(ThinkingLevel::parse("off"), ThinkingLevel::Off);
        assert_eq!(ThinkingLevel::parse("low"), ThinkingLevel::Low);
        assert_eq!(ThinkingLevel::parse("medium"), ThinkingLevel::Medium);
        assert_eq!(ThinkingLevel::parse("high"), ThinkingLevel::High);
        assert_eq!(ThinkingLevel::parse("max"), ThinkingLevel::Max);
        assert_eq!(ThinkingLevel::parse("custom"), ThinkingLevel::Custom);
        assert_eq!(ThinkingLevel::parse("unknown"), ThinkingLevel::Off);
        assert_eq!(ThinkingLevel::parse(""), ThinkingLevel::Off);
    }

    #[test]
    fn test_is_enabled() {
        assert!(!ThinkingLevel::Off.is_enabled());
        assert!(ThinkingLevel::Low.is_enabled());
        assert!(ThinkingLevel::Medium.is_enabled());
        assert!(ThinkingLevel::High.is_enabled());
        assert!(ThinkingLevel::Max.is_enabled());
        assert!(ThinkingLevel::Custom.is_enabled());
    }

    #[test]
    fn test_off_returns_none_for_all_vendors() {
        for vendor in ["DeepSeek", "OpenAI", "Claude", "Gemini", "Unknown"] {
            assert!(get_thinking_params(vendor, &ThinkingLevel::Off, "").is_none());
        }
    }

    #[test]
    fn test_unknown_vendor_returns_none() {
        let param = get_thinking_params("UnknownVendor", &ThinkingLevel::High, "");
        assert!(param.is_none());
    }

    #[test]
    fn test_deepseek_levels() {
        let low = get_thinking_params("DeepSeek", &ThinkingLevel::Low, "").unwrap();
        assert_eq!(low["reasoning"], true);
        assert_eq!(low["reasoning_effort"], "low");

        let max = get_thinking_params("deepseek", &ThinkingLevel::Max, "").unwrap();
        assert_eq!(max["reasoning"], true);
        assert_eq!(max["reasoning_effort"], "max");
    }

    #[test]
    fn test_ds_alias() {
        let param = get_thinking_params("DS-R1", &ThinkingLevel::Medium, "").unwrap();
        assert_eq!(param["reasoning_effort"], "medium");
    }

    #[test]
    fn test_openai_levels() {
        let high = get_thinking_params("OpenAI", &ThinkingLevel::High, "").unwrap();
        assert!(high.get("reasoning").is_none());
        assert_eq!(high["reasoning_effort"], "high");

        let low = get_thinking_params("gpt", &ThinkingLevel::Low, "").unwrap();
        assert_eq!(low["reasoning_effort"], "low");
    }

    #[test]
    fn test_claude_budgets() {
        let low = get_thinking_params("Claude", &ThinkingLevel::Low, "").unwrap();
        assert_eq!(low["thinking"]["type"], "enabled");
        assert_eq!(low["thinking"]["budget_tokens"], 5000);

        let max = get_thinking_params("anthropic", &ThinkingLevel::Max, "").unwrap();
        assert_eq!(max["thinking"]["budget_tokens"], 64000);
    }

    #[test]
    fn test_gemini_budgets() {
        let low = get_thinking_params("Gemini", &ThinkingLevel::Low, "").unwrap();
        assert_eq!(low["generationConfig"]["thinkingConfig"]["thinkingBudget"], 1000);

        let high = get_thinking_params("google", &ThinkingLevel::High, "").unwrap();
        assert_eq!(high["generationConfig"]["thinkingConfig"]["thinkingBudget"], 24000);
    }

    #[test]
    fn test_custom_params() {
        let custom = r#"{"reasoning": true, "reasoning_effort": "medium", "extra": 1}"#;
        let param = get_thinking_params("AnyVendor", &ThinkingLevel::Custom, custom).unwrap();
        assert_eq!(param["reasoning"], true);
        assert_eq!(param["reasoning_effort"], "medium");
        assert_eq!(param["extra"], 1);
    }

    #[test]
    fn test_custom_empty_returns_none() {
        assert!(get_thinking_params("AnyVendor", &ThinkingLevel::Custom, "").is_none());
        assert!(get_thinking_params("AnyVendor", &ThinkingLevel::Custom, "  ").is_none());
        assert!(get_thinking_params("AnyVendor", &ThinkingLevel::Custom, "not json").is_none());
    }

    #[test]
    fn test_merge_params_into_body() {
        let mut body = serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let params = Some(serde_json::json!({ "reasoning": true }));
        merge_thinking_params(&mut body, &params);
        assert_eq!(body["reasoning"], true);
        assert_eq!(body["model"], "test-model");
    }

    #[test]
    fn test_merge_none_leaves_body_unchanged() {
        let mut body = serde_json::json!({ "model": "m", "messages": [] });
        let original = body.clone();
        merge_thinking_params(&mut body, &None);
        assert_eq!(body, original);
    }
}
