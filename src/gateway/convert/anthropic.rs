//! Anthropic Messages API <-> OpenAI Chat Completions conversion.
//!
//! The gateway uses OpenAI Chat Completions as the internal canonical format.
//! When an upstream is configured with `api_format = "anthropic"`, the
//! request is converted to Anthropic Messages format before forwarding, and
//! the response is converted back to OpenAI Chat format for the client.

use serde_json::{json, Map, Value};

/// Convert an OpenAI Chat Completions request body into an Anthropic Messages
/// request body.
///
/// - `messages[].role` maps 1:1 (`assistant` -> `assistant`, `user` -> `user`)
/// - `system` top-level message(s) are extracted into the Anthropic `system` field
/// - `max_tokens` is required by Anthropic; defaults to 4096 when absent
/// - `stop` -> `stop_sequences`
/// - `tools` (OpenAI function schema) -> `tools` (Anthropic function schema)
pub fn chat_to_anthropic(body: &Value) -> Result<Value, String> {
    let obj = body.as_object().ok_or("request body must be a JSON object")?;

    let mut out = Map::new();
    // model
    if let Some(model) = obj.get("model") {
        out.insert("model".to_string(), model.clone());
    }

    // messages: extract system messages, convert roles, unwrap content blocks
    let mut anthropic_messages: Vec<Value> = Vec::new();
    let mut system_parts: Vec<Value> = Vec::new();
    if let Some(messages) = obj.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");
            if role == "system" {
                // Accumulate system messages into top-level `system`
                if let Some(content) = msg.get("content") {
                    system_parts.push(content.clone());
                }
                continue;
            }
            let content = msg.get("content").cloned().unwrap_or(Value::Null);
            // Convert OpenAI content blocks to Anthropic block format
            let converted = convert_content_blocks(&content);
            anthropic_messages.push(json!({
                "role": role,
                "content": converted,
            }));
        }
    }
    if !anthropic_messages.is_empty() {
        out.insert("messages".to_string(), Value::Array(anthropic_messages));
    }

    // system
    if !system_parts.is_empty() {
        if system_parts.len() == 1 {
            out.insert("system".to_string(), system_parts[0].clone());
        } else {
            out.insert("system".to_string(), Value::Array(system_parts));
        }
    }

    // max_tokens (required)
    let max_tokens = obj
        .get("max_tokens")
        .cloned()
        .unwrap_or_else(|| json!(4096));
    out.insert("max_tokens".to_string(), max_tokens);

    // scalar params passthrough
    for key in [
        "temperature",
        "top_p",
        "stream",
        "top_k",
        "presence_penalty",
        "frequency_penalty",
    ] {
        if let Some(v) = obj.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }

    // stop -> stop_sequences
    if let Some(stop) = obj.get("stop") {
        out.insert("stop_sequences".to_string(), stop.clone());
    }

    // tools: OpenAI function calling -> Anthropic tools
    if let Some(tools) = obj.get("tools").and_then(|t| t.as_array()) {
        let mut anth_tools: Vec<Value> = Vec::new();
        for tool in tools {
            if tool.get("type").and_then(|t| t.as_str()) == Some("function")
                && let Some(func) = tool.get("function")
            {
                anth_tools.push(json!({
                    "name": func.get("name").cloned().unwrap_or(Value::Null),
                    "description": func.get("description").cloned().unwrap_or(Value::Null),
                    "input_schema": func.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
                }));
            }
        }
        if !anth_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(anth_tools));
        }
    }

    // thinking: map OpenAI reasoning_effort into Anthropic thinking budget
    if let Some(level) = obj.get("reasoning_effort").and_then(|v| v.as_str()) {
        let budget = match level {
            "low" => 5000,
            "medium" => 16000,
            "high" => 32000,
            "max" => 64000,
            _ => 16000,
        };
        out.insert(
            "thinking".to_string(),
            json!({"type": "enabled", "budget_tokens": budget}),
        );
    } else if obj.get("reasoning").and_then(|v| v.as_bool()) == Some(true) {
        out.insert(
            "thinking".to_string(),
            json!({"type": "enabled", "budget_tokens": 16000}),
        );
    }

    Ok(Value::Object(out))
}

/// Convert OpenAI content (string or array of blocks) into Anthropic content
/// (string or array of content blocks).
fn convert_content_blocks(content: &Value) -> Value {
    match content {
        Value::String(s) => Value::String(s.clone()),
        Value::Array(blocks) => {
            let mut out: Vec<Value> = Vec::new();
            for block in blocks {
                if let Some(bt) = block.get("type").and_then(|t| t.as_str()) {
                    match bt {
                        "text" => {
                            out.push(json!({
                                "type": "text",
                                "text": block.get("text").cloned().unwrap_or(Value::String(String::new())),
                            }));
                        }
                        "image_url" => {
                            // Extract base64 data or URL
                            if let Some(img) = block.get("image_url") {
                                let url = img
                                    .get("url")
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                if url.starts_with("data:") {
                                    // data:image/png;base64,....
                                    let parts: Vec<&str> = url.splitn(2, ',').collect();
                                    if parts.len() == 2 {
                                        let mime = parts[0]
                                            .split([':', ';'])
                                            .nth(1)
                                            .unwrap_or("image/png");
                                        out.push(json!({
                                            "type": "image",
                                            "source": {
                                                "type": "base64",
                                                "media_type": mime,
                                                "data": parts[1],
                                            },
                                        }));
                                    }
                                } else {
                                    out.push(json!({
                                        "type": "image",
                                        "source": {
                                            "type": "url",
                                            "url": url,
                                        },
                                    }));
                                }
                            }
                        }
                        "text_delta" => {
                            if let Some(text) = block.get("text") {
                                out.push(json!({
                                    "type": "text",
                                    "text": text,
                                }));
                            }
                        }
                        _ => {}
                    }
                }
            }
            if out.len() == 1 && out[0].get("type").and_then(|t| t.as_str()) == Some("text") {
                // Collapse single text block to string
                out[0].get("text").cloned().unwrap_or(Value::Null)
            } else if out.is_empty() {
                Value::String(String::new())
            } else {
                Value::Array(out)
            }
        }
        _ => Value::Null,
    }
}

/// Convert an Anthropic Messages response body into an OpenAI Chat Completions
/// response body.
///
/// - `content[0].text` (or joined text) -> `choices[0].message.content`
/// - `stop_reason` -> `finish_reason` (`end_turn`->`stop`, `max_tokens`->`length`)
/// - `usage.input_tokens/output_tokens` -> `prompt_tokens/completion_tokens`
pub fn anthropic_to_chat(response: &Value, model_display: &str) -> Value {
    let obj = response.as_object().cloned().unwrap_or_default();

    // Extract text content from Anthropic content blocks
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut thinking_text = String::new();
    if let Some(blocks) = obj.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match btype {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
                "thinking" => {
                    if let Some(t) = block.get("thinking").and_then(|t| t.as_str()) {
                        thinking_text.push_str(t);
                    }
                }
                "tool_use" => {
                    tool_calls.push(json!({
                        "id": block.get("id").cloned().unwrap_or(Value::Null),
                        "type": "function",
                        "function": {
                            "name": block.get("name").cloned().unwrap_or(Value::Null),
                            "arguments": serde_json::to_string(
                                block.get("input").unwrap_or(&Value::Null)
                            ).unwrap_or_else(|_| "{}".to_string()),
                        },
                    }));
                }
                _ => {}
            }
        }
    }

    // stop_reason mapping
    let finish_reason = match obj
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("end_turn")
    {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        _ => "stop",
    };

    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(text));
    if !thinking_text.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(thinking_text),
        );
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }

    // usage mapping
    let usage = obj.get("usage").cloned().unwrap_or(Value::Null);
    let prompt_tokens = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let completion_tokens = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let id = obj
        .get("id")
        .cloned()
        .unwrap_or_else(|| json!(format!("msg_{}", uuid::Uuid::new_v4().simple())));

    json!({
        "id": id,
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model_display,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_to_anthropic_basic() {
        let body = json!({
            "model": "claude-sonnet-4",
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hi"}
            ],
            "max_tokens": 100,
            "temperature": 0.5,
            "stop": ["\n\n"],
            "stream": false
        });
        let out = chat_to_anthropic(&body).unwrap();
        assert_eq!(out["model"], "claude-sonnet-4");
        assert_eq!(out["system"], "You are helpful");
        assert_eq!(out["max_tokens"], 100);
        assert_eq!(out["stop_sequences"], json!(["\n\n"]));
        assert_eq!(out["temperature"], 0.5);
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "Hi");
    }

    #[test]
    fn test_chat_to_anthropic_default_max_tokens() {
        let body = json!({
            "model": "claude-sonnet-4",
            "messages": [{"role": "user", "content": "Hi"}]
        });
        let out = chat_to_anthropic(&body).unwrap();
        assert_eq!(out["max_tokens"], 4096);
    }

    #[test]
    fn test_chat_to_anthropic_reasoning_effort() {
        let body = json!({
            "model": "claude-sonnet-4",
            "messages": [{"role": "user", "content": "Hi"}],
            "reasoning_effort": "high"
        });
        let out = chat_to_anthropic(&body).unwrap();
        assert_eq!(out["thinking"]["budget_tokens"], 32000);
    }

    #[test]
    fn test_chat_to_anthropic_tools() {
        let body = json!({
            "model": "claude-sonnet-4",
            "messages": [{"role": "user", "content": "Hi"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather",
                    "parameters": {"type": "object", "properties": {}}
                }
            }]
        });
        let out = chat_to_anthropic(&body).unwrap();
        let tools = out["tools"].as_array().unwrap();
        assert_eq!(tools[0]["name"], "get_weather");
        assert_eq!(tools[0]["input_schema"]["type"], "object");
    }

    #[test]
    fn test_anthropic_to_chat_basic() {
        let resp = json!({
            "id": "msg_123",
            "content": [
                {"type": "text", "text": "Hello world"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let out = anthropic_to_chat(&resp, "Display Model");
        assert_eq!(out["object"], "chat.completion");
        assert_eq!(out["model"], "Display Model");
        assert_eq!(out["choices"][0]["message"]["content"], "Hello world");
        assert_eq!(out["choices"][0]["finish_reason"], "stop");
        assert_eq!(out["usage"]["prompt_tokens"], 10);
        assert_eq!(out["usage"]["completion_tokens"], 5);
        assert_eq!(out["usage"]["total_tokens"], 15);
    }

    #[test]
    fn test_anthropic_to_chat_max_tokens_reason() {
        let resp = json!({
            "content": [{"type": "text", "text": "abc"}],
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        });
        let out = anthropic_to_chat(&resp, "M");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
    }

    #[test]
    fn test_anthropic_to_chat_thinking_and_tools() {
        let resp = json!({
            "content": [
                {"type": "thinking", "thinking": "let me think"},
                {"type": "text", "text": "answer"},
                {"type": "tool_use", "id": "tu_1", "name": "get_weather", "input": {"city": "SF"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let out = anthropic_to_chat(&resp, "M");
        assert_eq!(out["choices"][0]["message"]["reasoning_content"], "let me think");
        assert_eq!(out["choices"][0]["message"]["content"], "answer");
        assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
        let tool_calls = out["choices"][0]["message"]["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
    }
}
