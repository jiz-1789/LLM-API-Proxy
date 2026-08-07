pub mod anthropic;
pub mod capabilities;
pub mod gemini;
pub mod openai_responses;
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
    matches!(api_format, FORMAT_ANTHROPIC | FORMAT_GEMINI_NATIVE)
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
        FORMAT_OPENAI_CHAT | FORMAT_OPENAI_RESPONSES | "" => Ok(body.clone()),
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

/// Normalize a client-facing OpenAI Responses request body into the internal
/// OpenAI Chat format (used by the `/v1/responses` endpoint).
pub fn normalize_responses_input(body: &Value) -> Result<Value, String> {
    openai_responses::responses_to_chat(body)
}

/// Convert an internal OpenAI Chat response body into a client-facing
/// Responses API response body (used by the `/v1/responses` endpoint).
pub fn normalize_responses_output(body: &Value, model_display: &str) -> Value {
    openai_responses::chat_to_responses(body, model_display)
}

// ============================================================================
// Client-native entry conversions
//
// The gateway exposes `POST /v1/messages` (Anthropic) and
// `POST /v1beta/models/{model}:generateContent` (Gemini) so that native
// clients can talk to the proxy directly. Requests are normalized into the
// internal Chat format, processed, then converted back to the client format.
// ============================================================================

/// Normalize an Anthropic Messages request body into internal Chat format.
pub fn normalize_anthropic_input(body: &Value) -> Result<Value, String> {
    anthropic::anthropic_request_to_chat(body)
}

/// Convert an internal Chat response body into an Anthropic Messages response.
pub fn normalize_anthropic_output(body: &Value, model_display: &str) -> Value {
    anthropic::chat_to_anthropic_client_response(body, model_display)
}

/// Convert a Chat SSE chunk into Anthropic Messages SSE events.
pub fn chat_sse_to_anthropic(
    json_str: &str,
    display_name: &str,
) -> (Vec<String>, Option<(i64, i64, i64)>, Option<String>) {
    anthropic::chat_sse_chunk_to_anthropic(json_str, display_name)
}

/// Normalize a Gemini Native request body into internal Chat format.
pub fn normalize_gemini_input(body: &Value) -> Result<Value, String> {
    gemini::gemini_request_to_chat(body)
}

/// Convert an internal Chat response body into a Gemini Native response.
pub fn normalize_gemini_output(body: &Value, model_display: &str) -> Value {
    gemini::chat_to_gemini_client_response(body, model_display)
}

/// Convert a Chat SSE chunk into a Gemini Native SSE chunk.
pub fn chat_sse_to_gemini(
    json_str: &str,
    display_name: &str,
) -> (Vec<String>, Option<(i64, i64, i64)>, Option<String>) {
    gemini::chat_sse_chunk_to_gemini(json_str, display_name)
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

    #[test]
    fn test_normalize_anthropic_input_and_output() {
        let body = json!({
            "model": "claude-sonnet-4",
            "system": "You are helpful",
            "messages": [{"role": "user", "content": "Hi"}]
        });
        let chat = normalize_anthropic_input(&body).unwrap();
        assert_eq!(chat["model"], "claude-sonnet-4");
        assert_eq!(chat["messages"][0]["role"], "system");

        let resp = json!({
            "choices": [{"message": {"content": "answer"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        });
        let anth = normalize_anthropic_output(&resp, "Display");
        assert_eq!(anth["type"], "message");
        assert_eq!(anth["content"][0]["text"], "answer");
        assert_eq!(anth["stop_reason"], "end_turn");
    }

    #[test]
    fn test_normalize_gemini_input_and_output() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "Hello"}]}],
            "generationConfig": {"temperature": 0.5}
        });
        let chat = normalize_gemini_input(&body).unwrap();
        assert_eq!(chat["messages"][0]["role"], "user");
        assert_eq!(chat["messages"][0]["content"], "Hello");
        assert_eq!(chat["temperature"], 0.5);

        let resp = json!({
            "choices": [{"message": {"content": "answer"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        });
        let gem = normalize_gemini_output(&resp, "Display");
        assert_eq!(gem["candidates"][0]["content"]["parts"][0]["text"], "answer");
        assert_eq!(gem["candidates"][0]["finishReason"], "STOP");
    }
}
