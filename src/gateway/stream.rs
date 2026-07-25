// SSE streaming support for gateway responses.
//
// The main streaming logic lives in `super::handle_chat_completions`, which:
// - Detects `stream: true` in the request body
// - Calls `UpstreamClient::forward_stream_request` to get a streaming response
// - Spawns a tokio task that reads SSE lines, replaces the model field in each
//   `data: {...}` chunk with the pool's display_name, and pipes them through an
//   mpsc channel to the client as a streaming SSE response.
//
// This module provides shared SSE utility functions.

use serde_json::Value;

/// Replace the `model` field in an SSE JSON chunk with the given display name.
/// Returns the reformatted SSE line: `data: {json}\n\n`.
/// If parsing fails, returns the original line unchanged.
pub fn replace_model_in_sse_chunk(json_str: &str, display_name: &str) -> String {
    if let Ok(mut v) = serde_json::from_str::<Value>(json_str) {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("model".to_string(), Value::String(display_name.to_string()));
        }
        format!("data: {}\n\n", v)
    } else {
        format!("data: {}\n\n", json_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_model_in_sse_chunk() {
        let chunk = r#"{"id":"chatcmpl-123","model":"gpt-4","choices":[{"delta":{"content":"Hi"}}]}"#;
        let result = replace_model_in_sse_chunk(chunk, "my-pool");
        assert!(result.starts_with("data: "));
        assert!(result.ends_with("\n\n"));
        assert!(result.contains(r#""model":"my-pool""#));
        assert!(!result.contains(r#""model":"gpt-4""#));
    }

    #[test]
    fn test_replace_model_invalid_json() {
        let chunk = "not-valid-json";
        let result = replace_model_in_sse_chunk(chunk, "my-pool");
        assert_eq!(result, "data: not-valid-json\n\n");
    }
}