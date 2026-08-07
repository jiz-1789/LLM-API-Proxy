use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 模型能力标记，v14。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelCapabilities {
    /// 支持的输入模态 (e.g. text, image, audio)
    pub input_modalities: Vec<String>,
    /// 支持的输出模态 (e.g. text, image, audio)
    pub output_modalities: Vec<String>,
    /// 是否支持函数调用
    pub supports_function_calling: bool,
    /// 是否支持流式输出
    pub supports_streaming: bool,
    /// 上下文窗口大小（tokens）
    pub context_window: Option<i32>,
    /// 最大输出 tokens
    pub max_output_tokens: Option<i32>,
}

impl ModelCapabilities {
    /// 仅支持文本的默认能力（用于未知模型）。
    pub fn default_text_only() -> Self {
        Self {
            input_modalities: vec!["text".to_string()],
            output_modalities: vec!["text".to_string()],
            supports_function_calling: false,
            supports_streaming: true,
            context_window: None,
            max_output_tokens: None,
        }
    }

    /// 判断是否"未知"（即序列化后为空默认值）。
    pub fn is_unknown(&self) -> bool {
        self.input_modalities.is_empty()
            && self.output_modalities.is_empty()
            && !self.supports_function_calling
            && !self.supports_streaming
            && self.context_window.is_none()
            && self.max_output_tokens.is_none()
    }

    /// 两个能力标记求并集（池级聚合）。
    pub fn union(&self, other: &Self) -> Self {
        Self {
            input_modalities: union_vec(&self.input_modalities, &other.input_modalities),
            output_modalities: union_vec(&self.output_modalities, &other.output_modalities),
            supports_function_calling: self.supports_function_calling
                || other.supports_function_calling,
            supports_streaming: self.supports_streaming || other.supports_streaming,
            context_window: match (self.context_window, other.context_window) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
            max_output_tokens: match (self.max_output_tokens, other.max_output_tokens) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
        }
    }

    /// 从 JSON 字符串解析；空字符串或解析失败时返回 None（未知）。
    pub fn from_json_str(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        serde_json::from_str::<Self>(trimmed).ok()
    }

    /// 序列化为 JSON 字符串；未知时为空字符串。
    pub fn to_json_str(&self) -> String {
        if self.is_unknown() {
            return String::new();
        }
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// 内置模型能力推断引擎。根据模型名前缀/关键词推断能力。
pub fn infer_capabilities(model_name: &str) -> ModelCapabilities {
    let name = model_name.to_lowercase();

    // GPT-4o / GPT-5 系列：多模态输入 + 函数调用
    if name.contains("gpt-5") {
        ModelCapabilities {
            input_modalities: vec!["text".into(), "image".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(128000),
            max_output_tokens: Some(32768),
        }
    } else if name.contains("gpt-4o")
        || name.contains("gpt-4.1")
        || name.contains("gpt-4-turbo")
        || name.contains("gpt-4omni")
        || name.contains("omnichat")
    {
        ModelCapabilities {
            input_modalities: vec!["text".into(), "image".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(128000),
            max_output_tokens: Some(16384),
        }
    }
    // Claude 3.5+ / 4 / 5 系列
    else if name.contains("claude-3")
        || name.contains("claude-4")
        || name.contains("claude-5")
        || name.contains("claude-sonnet-4")
    {
        ModelCapabilities {
            input_modalities: vec!["text".into(), "image".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(200000),
            max_output_tokens: Some(8192),
        }
    }
    // Gemini 系列
    else if name.contains("gemini") {
        let is_pro = name.contains("1.5-pro");
        let context_window = if is_pro { Some(2000000) } else { Some(1000000) };
        ModelCapabilities {
            input_modalities: vec![
                "text".into(),
                "image".into(),
                "audio".into(),
                "video".into(),
            ],
            output_modalities: if name.contains("gemini-2.5-spark") || name.contains("gemini-flash-image")
            {
                vec!["text".into(), "image".into()]
            } else {
                vec!["text".into()]
            },
            supports_function_calling: true,
            supports_streaming: true,
            context_window,
            max_output_tokens: Some(8192),
        }
    }
    // DeepSeek 系列
    else if name.contains("deepseek") || name.contains("deep-seek") {
        ModelCapabilities {
            input_modalities: vec!["text".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(64000),
            max_output_tokens: Some(8192),
        }
    }
    // Qwen 系列
    else if name.contains("qwen")
        || name.contains("qwq")
        || name.contains("qvq")
        || name.contains("qwen2")
    {
        let vision = name.contains("vl")
            || name.contains("omni")
            || name.contains("max")
            || name.contains("plus");
        let mut input_modalities = vec!["text".into()];
        if vision {
            input_modalities.push("image".into());
        }
        ModelCapabilities {
            input_modalities,
            output_modalities: vec!["text".into()],
            supports_function_calling: name.contains("qwen2")
                || name.contains("qwq")
                || name.contains("max"),
            supports_streaming: true,
            context_window: Some(131072),
            max_output_tokens: Some(8192),
        }
    }
    // GLM / ChatGLM 系列
    else if name.contains("glm") {
        ModelCapabilities {
            input_modalities: vec!["text".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(128000),
            max_output_tokens: Some(8192),
        }
    }
    // Llama 系列
    else if name.contains("llama3.2-vision") || name.contains("llava") || name.contains("llama-4") {
        ModelCapabilities {
            input_modalities: vec!["text".into(), "image".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: name.contains("llama-4") || name.contains("llama3.3"),
            supports_streaming: true,
            context_window: Some(131072),
            max_output_tokens: Some(8192),
        }
    } else if name.contains("llama") || name.contains("meta") || name.contains("nous-hermes") {
        ModelCapabilities {
            input_modalities: vec!["text".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(8192),
            max_output_tokens: Some(8192),
        }
    }
    // Mistral 系列
    else if name.contains("mistral")
        || name.contains("mixtral")
        || name.contains("codestral")
        || name.contains("ministral")
    {
        ModelCapabilities {
            input_modalities: vec!["text".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(32768),
            max_output_tokens: Some(8192),
        }
    }
    // Kimi / moonshot 系列
    else if name.contains("kimi") || name.contains("moonshot") {
        ModelCapabilities {
            input_modalities: vec!["text".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(128000),
            max_output_tokens: Some(8192),
        }
    }
    // 通义千问官方用 qwen 已覆盖
    // Doubao 系列
    else if name.contains("doubao")
        || name.contains("seed")
        || name.contains("skylark")
        || name.contains("yunxiaobai")
        || name.contains("asia")
    {
        let vision = name.contains("vision") || name.contains("vl") || name.contains("1.5-vl");
        let mut input_modalities = vec!["text".into()];
        if vision {
            input_modalities.push("image".into());
        }
        ModelCapabilities {
            input_modalities,
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(32768),
            max_output_tokens: Some(8192),
        }
    }
    // Grok 系列
    else if name.contains("grok") {
        ModelCapabilities {
            input_modalities: vec!["text".into(), "image".into(), "audio".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(131072),
            max_output_tokens: Some(32768),
        }
    }
    // ERNIE / 文心 系列
    else if name.contains("ernie") || name.contains("wenxin") || name.contains("sun") {
        let has_image = name.contains("vl") || name.contains("vision");
        ModelCapabilities {
            input_modalities: if has_image {
                vec!["text".into(), "image".into()]
            } else {
                vec!["text".into()]
            },
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(128000),
            max_output_tokens: Some(8192),
        }
    }
    // MiniMax 系列
    else if name.contains("minimax")
        || name.contains("abab")
        || name.contains("s1-")
        || name.contains("speech")
        // 语音文本输出（如 speech-tts-0.1、speech-txt2audio 等）
    {
        let speech = name.contains("speech")
            || name.contains("tts")
            || name.contains("txt2audio")
            || name.contains("audio");
        let mut output_modalities = vec!["text".into()];
        if speech {
            output_modalities.push("audio".into());
        }
        ModelCapabilities {
            input_modalities: vec!["text".into()],
            output_modalities,
            supports_function_calling: name.contains("minimax") && !speech,
            supports_streaming: true,
            context_window: Some(8192),
            max_output_tokens: Some(4096),
        }
    }
    // Redream / 智谱
    else if name.contains("redream") {
        ModelCapabilities {
            input_modalities: vec!["text".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: false,
            supports_streaming: true,
            context_window: Some(8192),
            max_output_tokens: None,
        }
    }
    // 默认：仅文本
    else {
        ModelCapabilities::default_text_only()
    }
}

fn union_vec(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = a.to_vec();
    for item in b {
        if !out.iter().any(|x| x == item) {
            out.push(item.clone());
        }
    }
    out
}

/// 治理 JSON 值（用于前端手动填写/DB 兼容），返回规范化的 Value。
pub fn normalize_capabilities_json(v: &Value) -> Result<Value, String> {
    let caps = serde_json::from_value::<ModelCapabilities>(v.clone())
        .map_err(|e| format!("能力字段不合法: {e}"))?;
    Ok(serde_json::to_value(caps).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_gpt4o_has_vision_and_tools() {
        let caps = infer_capabilities("gpt-4o");
        assert!(caps.input_modalities.contains(&"image".to_string()));
        assert!(caps.supports_function_calling);
        assert_eq!(caps.context_window, Some(128000));
    }

    #[test]
    fn test_infer_claude_has_vision() {
        let caps = infer_capabilities("claude-sonnet-4-20250514");
        assert!(caps.input_modalities.contains(&"image".to_string()));
        assert!(caps.supports_function_calling);
    }

    #[test]
    fn test_infer_gemini_has_audio() {
        let caps = infer_capabilities("gemini-1.5-pro");
        assert!(caps.input_modalities.contains(&"audio".to_string()));
        assert_eq!(caps.context_window, Some(2000000));
    }

    #[test]
    fn test_infer_deepseek_text_only_with_tools() {
        let caps = infer_capabilities("deepseek-chat");
        assert_eq!(caps.input_modalities, vec!["text".to_string()]);
        assert!(caps.supports_function_calling);
    }

    #[test]
    fn test_infer_unknown_defaults_text_only() {
        let caps = infer_capabilities("some-unknown-model-2026");
        assert_eq!(caps.input_modalities, vec!["text".to_string()]);
        assert!(!caps.supports_function_calling);
    }

    #[test]
    fn test_union_merges_capabilities() {
        let a = ModelCapabilities {
            input_modalities: vec!["text".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: true,
            supports_streaming: true,
            context_window: Some(64000),
            max_output_tokens: Some(8192),
        };
        let b = ModelCapabilities {
            input_modalities: vec!["text".into(), "image".into()],
            output_modalities: vec!["text".into()],
            supports_function_calling: false,
            supports_streaming: true,
            context_window: Some(200000),
            max_output_tokens: Some(16384),
        };
        let merged = a.union(&b);
        assert_eq!(merged.input_modalities, vec!["text".to_string(), "image".to_string()]);
        assert!(merged.supports_function_calling);
        assert_eq!(merged.context_window, Some(200000));
        assert_eq!(merged.max_output_tokens, Some(16384));
    }

    #[test]
    fn test_json_roundtrip_and_blank() {
        let caps = infer_capabilities("gpt-4o");
        let s = caps.to_json_str();
        assert!(!s.is_empty());
        let back = ModelCapabilities::from_json_str(&s).unwrap();
        assert_eq!(back, caps);
        assert!(ModelCapabilities::from_json_str("").is_none());
        assert!(ModelCapabilities::from_json_str("   ").is_none());
        let text = ModelCapabilities::default_text_only();
        assert!(text.supports_streaming);
        assert_eq!(text.to_json_str().len(), text.to_json_str().len());
        assert!(ModelCapabilities::default().to_json_str().is_empty());
    }
}