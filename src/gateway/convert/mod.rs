pub mod anthropic;
pub mod capabilities;
pub mod gemini;
pub mod stream;

pub use stream::NativeStreamConverter;

use serde_json::Value;

/// Supported upstream API formats.
pub const FORMAT_OPENAI_CHAT: &str = "openai_chat";
pub const FORMAT_OPENAI_RESPONSES: &str = "openai_responses";
pub const FORMAT_ANTHROPIC: &str = "anthropic";
pub const FORMAT_GEMINI_NATIVE: &str = "gemini_native";

/// Returns true if the given api_format requires any request conversion
/// (i.e. is not the internal OpenAI Chat canonical format).
pub fn needs_request_conversion(api_format: &str) -> bool {
    !matches!(api_format, FORMAT_OPENAI_CHAT | "")
}

/// Returns true if the given api_format requires response conversion.
pub fn needs_response_conversion(api_format: &str) -> bool {
    needs_request_conversion(api_format)
}

/// Convert an internal OpenAI Chat request body into the upstream's native format.
///
/// Returns an error message if conversion fails. For `openai_chat` the body is
/// returned unchanged.
pub fn convert_request_to_upstream(
    body: &Value,
    api_format: &str,
) -> Result<Value, String> {
    match api_format {
        FORMAT_ANTHROPIC => anthropic::chat_to_anthropic(body),
        FORMAT_GEMINI_NATIVE => gemini::chat_to_gemini(body),
        FORMAT_OPENAI_RESPONSES | FORMAT_OPENAI_CHAT | "" => Ok(body.clone()),
        other => Err(format!("unsupported api_format: {}", other)),
    }
}

/// Convert an upstream native response body back into OpenAI Chat format.
///
/// `model_display` is the pool's display name, substituted into the response.
pub fn convert_response_to_client(
    body: &Value,
    api_format: &str,
    model_display: &str,
) -> Value {
    match api_format {
        FORMAT_ANTHROPIC => anthropic::anthropic_to_chat(body, model_display),
        FORMAT_GEMINI_NATIVE => gemini::gemini_to_chat(body, model_display),
        _ => body.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_needs_conversion_flags() {
        assert!(!needs_request_conversion("openai_chat"));
        assert!(!needs_request_conversion(""));
        assert!(needs_request_conversion("anthropic"));
        assert!(needs_request_conversion("gemini_native"));
        assert!(needs_response_conversion("anthropic"));
    }

    #[test]
    fn test_convert_request_openai_passthrough() {
        let body = json!({"model": "m", "messages": []});
        let out = convert_request_to_upstream(&body, "openai_chat").unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn test_convert_request_unsupported_format() {
        let body = json!({"model": "m"});
        let err = convert_request_to_upstream(&body, "bogus").unwrap_err();
        assert!(err.contains("unsupported api_format"));
    }

    #[test]
    fn test_convert_response_roundtrip_anthropic() {
        let anthropic_resp = json!({
            "id": "msg_1",
            "content": [{"type": "text", "text": "hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let out = convert_response_to_client(&anthropic_resp, "anthropic", "Display");
        assert_eq!(out["choices"][0]["message"]["content"], "hi");
        assert_eq!(out["model"], "Display");
    }

    #[test]
    fn test_convert_response_passthrough_openai() {
        let body = json!({"id": "chatcmpl-1", "choices": []});
        let out = convert_response_to_client(&body, "openai_chat", "M");
        assert_eq!(out, body);
    }
}
