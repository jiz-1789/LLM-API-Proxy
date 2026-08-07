//! OpenAI Responses API <-> OpenAI Chat Completions conversion.
//!
//! The Responses API is the newer OpenAI format used by Codex CLI and friends.
//! The gateway converts it into the internal Chat Completions canonical format.

use serde_json::{json, Map, Value};

/// Convert an OpenAI Responses request body into a Chat Completions request body.
///
/// - `input` (string) -> `messages[]` user message
/// - `input` (array of items) -> `messages[]` (role/type mapping)
/// - `instructions` -> system message at the start
/// - `reasoning.effort` -> `reasoning_effort`
/// - `max_output_tokens` -> `max_tokens`
/// - `tools` (responses `function` type) -> `tools` (chat `function` type)
pub fn responses_to_chat(body: &Value) -> Result<Value, String> {
    let obj = body.as_object().ok_or("request body must be a JSON object")?;

    let mut messages: Vec<Value> = Vec::new();

    // instructions -> system message
    if let Some(instr) = obj.get("instructions")
        && !instr.is_null()
    {
        messages.push(json!({"role": "system", "content": instr}));
    }

    // input -> messages
    if let Some(input) = obj.get("input") {
        match input {
            Value::String(s) => {
                messages.push(json!({"role": "user", "content": s}));
            }
            Value::Array(items) => {
                for item in items {
                    if let Some(msg) = responses_item_to_message(item) {
                        messages.push(msg);
                    }
                }
            }
            _ => {}
        }
    }

    let mut out = Map::new();
    if let Some(model) = obj.get("model") {
        out.insert("model".to_string(), model.clone());
    }
    out.insert("messages".to_string(), Value::Array(messages));

    // max_output_tokens -> max_tokens
    if let Some(v) = obj.get("max_output_tokens") {
        out.insert("max_tokens".to_string(), v.clone());
    }

    // scalar passthrough
    for key in ["temperature", "top_p", "stream", "store"] {
        if let Some(v) = obj.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }

    // reasoning.effort
    if let Some(level) = obj
        .get("reasoning")
        .and_then(|r| r.get("effort"))
        .and_then(|e| e.as_str())
    {
        out.insert("reasoning_effort".to_string(), Value::String(level.to_string()));
    }

    // tools: responses function -> chat function
    if let Some(tools) = obj.get("tools").and_then(|t| t.as_array()) {
        let mut chat_tools: Vec<Value> = Vec::new();
        for tool in tools {
            if let Some(func) = tool.get("function") {
                chat_tools.push(json!({
                    "type": "function",
                    "function": func,
                }));
            }
        }
        if !chat_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(chat_tools));
        }
    }

    Ok(Value::Object(out))
}

/// Map a single Responses `input` item to a Chat message.
fn responses_item_to_message(item: &Value) -> Option<Value> {
    if let Some(role) = item.get("role").and_then(|r| r.as_str()) {
        // {role, content}
        let content = item.get("content").cloned().unwrap_or(Value::String(String::new()));
        return Some(json!({"role": role, "content": content}));
    }
    // {type: "message", role, content}
    if let Some(role) = item.get("type").and_then(|t| t.as_str()) {
        match role {
            "message" => {
                let mrole = item.get("role").and_then(|r| r.as_str()).unwrap_or("user");
                let content = item.get("content").cloned().unwrap_or(Value::String(String::new()));
                return Some(json!({"role": mrole, "content": content}));
            }
            "function_call" => {
                // function_call output -> assistant tool message
                return Some(json!({
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": item.get("call_id").cloned().unwrap_or(Value::Null),
                        "type": "function",
                        "function": {
                            "name": item.get("name").cloned().unwrap_or(Value::Null),
                            "arguments": item.get("arguments").cloned().unwrap_or_else(|| Value::String("{}".to_string())),
                        }
                    }]
                }));
            }
            "function_call_output" => {
                return Some(json!({
                    "role": "tool",
                    "tool_call_id": item.get("call_id").cloned().unwrap_or(Value::Null),
                    "content": item.get("output").cloned().unwrap_or(Value::String(String::new())),
                }));
            }
            _ => {}
        }
    }
    None
}

/// Convert a Chat Completions response body into a Responses API response body.
///
/// - `choices[0].message.content` -> `output[0].content[0].text`
/// - `usage` -> `usage.input_tokens/output_tokens/total_tokens`
/// - `finish_reason` -> `status` (`stop`->`completed`, `length`->`incomplete`)
pub fn chat_to_responses(response: &Value, model_display: &str) -> Value {
    let obj = response.as_object().cloned().unwrap_or_default();

    let text = obj
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|ch| ch.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|content| content.as_str())
        .unwrap_or("")
        .to_string();

    let finish_reason = obj
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|ch| ch.get("finish_reason"))
        .and_then(|f| f.as_str())
        .unwrap_or("stop");

    let status = match finish_reason {
        "length" => "incomplete",
        "content_filter" => "incomplete",
        _ => "completed",
    };

    let usage = obj.get("usage").cloned().unwrap_or(Value::Null);
    let prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
    let completion_tokens = usage.get("completion_tokens").and_then(|v| v.as_i64()).unwrap_or(0);

    let id = obj
        .get("id")
        .cloned()
        .unwrap_or_else(|| json!(format!("resp_{}", uuid::Uuid::new_v4().simple())));

    json!({
        "id": id,
        "object": "response",
        "created_at": chrono::Utc::now().timestamp(),
        "status": status,
        "model": model_display,
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text}],
        }],
        "usage": {
            "input_tokens": prompt_tokens,
            "output_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        }
    })
}

/// Result of converting an OpenAI Chat SSE chunk to Responses events.
type SseConvertResult = (
    Vec<String>,
    Option<(i64, i64, i64)>,
    Option<String>,
);

/// Convert a single OpenAI Chat SSE JSON chunk into Responses API SSE data.
///
/// Returns `(output_lines, usage, error)` where `output_lines` is empty when
/// the chunk should be swallowed (e.g. role announcements) and contains
/// `data: {...}\n\n` lines otherwise.
pub fn chat_sse_chunk_to_responses(
    json_str: &str,
    display_name: &str,
) -> SseConvertResult {
    let Ok(v) = serde_json::from_str::<Value>(json_str) else {
        return (vec![], None, None);
    };

    // Error detection (OpenAI error chunk shape)
    let error = v.get("error").map(|err| {
        err.get("message")
            .and_then(|m| m.as_str())
            .map(String::from)
            .unwrap_or_else(|| err.to_string())
    });
    if error.is_some() {
        return (vec![], None, error);
    }

    // Usage extraction
    let usage = v.get("usage").filter(|u| !u.is_null()).and_then(|u| {
        let prompt = u.get("prompt_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
        let completion = u.get("completion_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
        if prompt == 0 && completion == 0 {
            None
        } else {
            Some((prompt, completion, prompt + completion))
        }
    });

    let Some(choice) = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
    else {
        return (vec![], usage, None);
    };

    // Extract delta text (and reasoning if present)
    let delta = choice.get("delta").unwrap_or(&Value::Null);
    let text = delta.get("content").and_then(|t| t.as_str()).unwrap_or("");
    let reasoning = delta
        .get("reasoning_content")
        .and_then(|t| t.as_str())
        .unwrap_or("");

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("");

    let mut lines = Vec::new();

    // Emit reasoning delta if present (Responses: `reasoning_summary_text.delta`)
    if !reasoning.is_empty() {
        let event = json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": format!("item_{}", uuid::Uuid::new_v4().simple()),
            "output_index": 0,
            "content_index": 0,
            "delta": reasoning,
            "sequence_number": 0,
        });
        lines.push(format!("event: response.reasoning_summary_text.delta\ndata: {}\n\n", event));
    }

    // Emit text delta
    if !text.is_empty() {
        let event = json!({
            "type": "response.output_text.delta",
            "item_id": format!("item_{}", uuid::Uuid::new_v4().simple()),
            "output_index": 0,
            "content_index": 0,
            "delta": text,
            "sequence_number": 0,
        });
        lines.push(format!("event: response.output_text.delta\ndata: {}\n\n", event));
    }

    // Emit completion event on finish
    if !finish_reason.is_empty() && finish_reason != "null" {
        let status = if finish_reason == "length" || finish_reason == "content_filter" {
            "incomplete"
        } else {
            "completed"
        };
        let (p, c, t) = usage.unwrap_or((0, 0, 0));
        let event = json!({
            "type": "response.completed",
            "response": {
                "id": format!("resp_{}", uuid::Uuid::new_v4().simple()),
                "object": "response",
                "created_at": chrono::Utc::now().timestamp(),
                "status": status,
                "model": display_name,
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": ""}]
                }],
                "usage": {
                    "input_tokens": p,
                    "output_tokens": c,
                    "total_tokens": t,
                }
            }
        });
        lines.push(format!("event: response.completed\ndata: {}\n\n", event));
        lines.push("data: [DONE]\n\n".to_string());
    }

    (lines, usage, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_responses_to_chat_basic() {
        let body = json!({
            "model": "gpt-4o",
            "input": "Hello there",
            "instructions": "Be concise",
            "max_output_tokens": 200,
            "reasoning": {"effort": "high"}
        });
        let out = responses_to_chat(&body).unwrap();
        assert_eq!(out["model"], "gpt-4o");
        assert_eq!(out["max_tokens"], 200);
        assert_eq!(out["reasoning_effort"], "high");
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "Be concise");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello there");
    }

    #[test]
    fn test_responses_to_chat_item_array() {
        let body = json!({
            "model": "gpt-4o",
            "input": [
                {"role": "user", "content": "Hi"},
                {"type": "message", "role": "assistant", "content": "Hello"},
                {"type": "function_call", "call_id": "fc_1", "name": "get_weather", "arguments": "{\"city\":\"SF\"}"},
                {"type": "function_call_output", "call_id": "fc_1", "output": "72F"}
            ]
        });
        let out = responses_to_chat(&body).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "fc_1");
        assert_eq!(messages[3]["content"], "72F");
    }

    #[test]
    fn test_chat_to_responses_basic() {
        let resp = json!({
            "id": "chatcmpl-1",
            "choices": [{
                "message": {"role": "assistant", "content": "Hello"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        let out = chat_to_responses(&resp, "My Pool");
        assert_eq!(out["object"], "response");
        assert_eq!(out["status"], "completed");
        assert_eq!(out["output"][0]["content"][0]["text"], "Hello");
        assert_eq!(out["usage"]["input_tokens"], 10);
        assert_eq!(out["usage"]["output_tokens"], 5);
    }

    #[test]
    fn test_chat_to_responses_length_status() {
        let resp = json!({
            "choices": [{"message": {"content": "abc"}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        });
        let out = chat_to_responses(&resp, "M");
        assert_eq!(out["status"], "incomplete");
    }
}
