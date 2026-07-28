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

/// Try to extract token usage from an SSE JSON chunk.
/// Returns `Some((prompt, completion, total))` if the chunk contains a `usage` object.
/// OpenAI sends usage in the final chunk before [DONE] when `stream_options.include_usage` is set.
/// Some providers include usage in every chunk; we take the last non-null one.
pub fn extract_usage_from_sse_chunk(json_str: &str) -> Option<(i64, i64, i64)> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let usage = v.get("usage")?;
    if usage.is_null() {
        return None;
    }
    let prompt = usage.get("prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
    let completion = usage.get("completion_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
    let total = usage.get("total_tokens").and_then(|v| v.as_i64()).unwrap_or(prompt + completion);
    if prompt == 0 && completion == 0 && total == 0 {
        None
    } else {
        Some((prompt, completion, total))
    }
}

/// Combined: parse JSON once, extract usage (if any), detect errors (if any),
/// and replace model name.
/// Returns `(output_line, Option<usage>, Option<error_message>)`.
/// This avoids double-parsing the same JSON chunk for model replacement,
/// usage extraction, and error detection.
pub fn process_sse_chunk(
    json_str: &str,
    display_name: &str,
) -> (String, Option<(i64, i64, i64)>, Option<String>) {
    match serde_json::from_str::<Value>(json_str) {
        Ok(mut v) => {
            // Detect error field before mutating
            let error_msg = v.get("error").map(|err| {
                if let Some(m) = err.get("message").and_then(|m| m.as_str()) {
                    m.to_string()
                } else {
                    err.to_string()
                }
            });

            if let Some(obj) = v.as_object_mut() {
                obj.insert("model".to_string(), Value::String(display_name.to_string()));
            }
            // Extract usage from the same parsed value (no second parse)
            let usage = v.get("usage").filter(|u| !u.is_null()).and_then(|usage| {
                let prompt = usage.get("prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let completion = usage.get("completion_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let total = usage.get("total_tokens").and_then(|v| v.as_i64()).unwrap_or(prompt + completion);
                if prompt == 0 && completion == 0 && total == 0 {
                    None
                } else {
                    Some((prompt, completion, total))
                }
            });
            (format!("data: {}\n\n", v), usage, error_msg)
        }
        Err(_) => (format!("data: {}\n\n", json_str), None, None),
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