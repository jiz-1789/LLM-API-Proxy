//! Gemini Native API <-> OpenAI Chat Completions conversion.
//!
//! Gemini's native `generateContent` format uses `contents`/`parts` instead of
//! `messages`, and wraps generation params in `generationConfig`. Two
//! directions exist:
//!
//! 1. **Upstream adaptation** (`api_format = "gemini_native"`): internal Chat
//!    request -> Gemini (`chat_to_gemini`), Gemini response -> Chat
//!    (`gemini_to_chat`).
//!
//! 2. **Client-native entry** (`POST /v1beta/models/{model}:generateContent`):
//!    a Gemini-format request is normalized to internal Chat
//!    (`gemini_request_to_chat`), then the Chat response is converted back to
//!    Gemini format (`chat_to_gemini_client_response`).

use serde_json::{json, Map, Value};

/// Convert an OpenAI Chat Completions request body into a Gemini Native
/// `generateContent` request body.
///
/// - `messages[]` -> `contents[]` (role `assistant` -> `model`)
/// - `messages[].content` (string) -> `parts[].text`
/// - `messages[].content` (image_url blocks) -> `parts[].inline_data` (base64)
/// - `temperature` / `top_p` / `max_tokens` -> `generationConfig.*`
/// - `max_tokens` -> `generationConfig.maxOutputTokens`
/// - system message -> `systemInstruction`
pub fn chat_to_gemini(body: &Value) -> Result<Value, String> {
    let obj = body.as_object().ok_or("request body must be a JSON object")?;

    let mut out = Map::new();

    // model (Gemini uses path param, but harmless to include in body)
    if let Some(model) = obj.get("model") {
        out.insert("model".to_string(), model.clone());
    }

    // system instruction
    if let Some(messages) = obj.get("messages").and_then(|m| m.as_array()) {
        let mut contents: Vec<Value> = Vec::new();
        let mut system_instruction = Value::Null;
        for msg in messages {
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");
            if role == "system" {
                if system_instruction == Value::Null {
                    system_instruction = json!({
                        "parts": [{"text": extract_text_content(msg.get("content").unwrap_or(&Value::Null))}]
                    });
                }
                continue;
            }
            let gemini_role = if role == "assistant" { "model" } else { "user" };
            let parts = content_to_parts(msg.get("content").unwrap_or(&Value::Null));
            contents.push(json!({
                "role": gemini_role,
                "parts": parts,
            }));
        }
        if !contents.is_empty() {
            out.insert("contents".to_string(), Value::Array(contents));
        }
        if system_instruction != Value::Null {
            out.insert("systemInstruction".to_string(), system_instruction);
        }
    }

    // generationConfig
    let mut gen_config = Map::new();
    if let Some(v) = obj.get("temperature") {
        gen_config.insert("temperature".to_string(), v.clone());
    }
    if let Some(v) = obj.get("top_p") {
        gen_config.insert("topP".to_string(), v.clone());
    }
    if let Some(v) = obj.get("top_k") {
        gen_config.insert("topK".to_string(), v.clone());
    }
    if let Some(v) = obj.get("max_tokens") {
        gen_config.insert("maxOutputTokens".to_string(), v.clone());
    }
    if let Some(v) = obj.get("max_completion_tokens") {
        gen_config.insert("maxOutputTokens".to_string(), v.clone());
    }
    if let Some(v) = obj.get("presence_penalty") {
        gen_config.insert("presencePenalty".to_string(), v.clone());
    }
    if let Some(v) = obj.get("frequency_penalty") {
        gen_config.insert("frequencyPenalty".to_string(), v.clone());
    }
    // thinking budget -> generationConfig.thinkingConfig.thinkingBudget
    if let Some(level) = obj.get("reasoning_effort").and_then(|v| v.as_str()) {
        let budget = match level {
            "low" => 1000,
            "medium" => 8000,
            "high" => 24000,
            "max" => 32000,
            _ => 8000,
        };
        gen_config.insert(
            "thinkingConfig".to_string(),
            json!({"thinkingBudget": budget}),
        );
    }
    if !gen_config.is_empty() {
        out.insert("generationConfig".to_string(), Value::Object(gen_config));
    }

    if let Some(v) = obj.get("stream") {
        out.insert("stream".to_string(), v.clone());
    }

    Ok(Value::Object(out))
}

/// Extract plain text from an OpenAI message content value.
fn extract_text_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            let mut text = String::new();
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(t) = block.get("text").and_then(|t| t.as_str())
                {
                    text.push_str(t);
                }
            }
            text
        }
        _ => String::new(),
    }
}

/// Convert OpenAI content into Gemini `parts` array.
fn content_to_parts(content: &Value) -> Value {
    match content {
        Value::String(s) => json!([{"text": s}]),
        Value::Array(blocks) => {
            let mut parts: Vec<Value> = Vec::new();
            for block in blocks {
                if let Some(bt) = block.get("type").and_then(|t| t.as_str()) {
                    match bt {
                        "text" => {
                            if let Some(t) = block.get("text") {
                                parts.push(json!({"text": t}));
                            }
                        }
                        "image_url" => {
                            if let Some(img) = block.get("image_url") {
                                let url = img
                                    .get("url")
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                if url.starts_with("data:") {
                                    let raw: Vec<&str> = url.splitn(2, ',').collect();
                                    if raw.len() == 2 {
                                        let mime = raw[0]
                                            .split([':', ';'])
                                            .nth(1)
                                            .unwrap_or("image/png");
                                        parts.push(json!({
                                            "inline_data": {
                                                "mime_type": mime,
                                                "data": raw[1],
                                            }
                                        }));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Value::Array(parts)
        }
        _ => json!([]),
    }
}

/// Convert a Gemini Native response body into an OpenAI Chat Completions
/// response body.
///
/// - `candidates[0].content.parts[].text` -> `choices[0].message.content`
/// - `candidates[0].finishReason` -> `finish_reason` (`STOP`->`stop`, `MAX_TOKENS`->`length`)
/// - `usageMetadata.promptTokenCount/completionTokenCount` -> tokens
/// - `candidates[0].thoughts` -> `reasoning_content`
pub fn gemini_to_chat(response: &Value, model_display: &str) -> Value {
    let obj = response.as_object().cloned().unwrap_or_default();

    // Extract text + thoughts from first candidate
    let mut text = String::new();
    let mut reasoning = String::new();
    if let Some(candidates) = obj.get("candidates").and_then(|c| c.as_array())
        && let Some(candidate) = candidates.first()
    {
        if let Some(content) = candidate.get("content")
            && let Some(parts) = content.get("parts").and_then(|p| p.as_array())
        {
            for part in parts {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
        }
        if let Some(thoughts) = candidate.get("thoughts").and_then(|t| t.as_array()) {
            for thought in thoughts {
                if let Some(t) = thought.get("text").and_then(|t| t.as_str()) {
                    reasoning.push_str(t);
                }
            }
        }
    }

    let finish_reason = match obj
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("finishReason"))
        .and_then(|r| r.as_str())
        .unwrap_or("STOP")
    {
        "MAX_TOKENS" => "length",
        "SAFETY" => "content_filter",
        _ => "stop",
    };

    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(text));
    if !reasoning.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning),
        );
    }

    let usage = obj.get("usageMetadata").cloned().unwrap_or(Value::Null);
    let prompt_tokens = usage
        .get("promptTokenCount")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let completion_tokens = usage
        .get("candidatesTokenCount")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    json!({
        "id": format!("gemini_{}", uuid::Uuid::new_v4().simple()),
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

// ============================================================================
// Client-native entry (`POST /v1beta/models/{model}:generateContent`)
// ============================================================================

/// Convert a Gemini Native `generateContent` **request** body into the
/// internal OpenAI Chat Completions request body (used by the Gemini endpoint).
///
/// - `contents[].parts[].text` -> `messages[].content`
/// - `contents[].role` (`user`/`model`) -> (`user`/`assistant`)
/// - `systemInstruction` -> leading `system` message
/// - `generationConfig.temperature/topP/maxOutputTokens` -> scalar params
/// - `generationConfig.thinkingConfig.thinkingBudget` -> `reasoning_effort` level
pub fn gemini_request_to_chat(body: &Value) -> Result<Value, String> {
    let obj = body.as_object().ok_or("request body must be a JSON object")?;

    let mut messages: Vec<Value> = Vec::new();
    // Map function name -> generated Chat tool_call id so a `functionResponse`
    // part (which references the call by name) can resolve the matching id.
    let mut call_ids: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut call_seq = 0usize;

    // systemInstruction -> system message
    if let Some(si) = obj.get("systemInstruction") {
        let text = extract_gemini_text(si);
        if !text.is_empty() {
            messages.push(json!({"role": "system", "content": text}));
        }
    }

    // contents -> messages
    if let Some(contents) = obj.get("contents").and_then(|c| c.as_array()) {
        for content in contents {
            let role = content
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");
            let chat_role = if role == "model" { "assistant" } else { "user" };
            // Function calls/responses live in `parts`. When present, emit Chat
            // `tool_calls` / `tool` messages; otherwise fall back to plain text.
            if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                let has_function_parts = parts.iter().any(|p| {
                    p.get("functionCall").is_some() || p.get("functionResponse").is_some()
                });
                if has_function_parts {
                    let mut text = String::new();
                    let mut tool_calls: Vec<Value> = Vec::new();
                    let mut tool_results: Vec<Value> = Vec::new();
                    for part in parts {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let args = fc.get("args").cloned().unwrap_or(Value::Null);
                            call_seq += 1;
                            let call_id = format!("call_{}", call_seq);
                            if !name.is_empty() {
                                call_ids.insert(name.to_string(), call_id.clone());
                            }
                            tool_calls.push(json!({
                                "id": call_id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": args.to_string(),
                                },
                            }));
                        }
                        if let Some(fr) = part.get("functionResponse") {
                            let name = fr.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let response = fr.get("response").cloned().unwrap_or(Value::Null);
                            let content_str = match response {
                                Value::String(s) => s,
                                other => other.to_string(),
                            };
                            let call_id = call_ids
                                .get(name)
                                .cloned()
                                .unwrap_or_else(|| format!("call_{}", call_seq));
                            tool_results.push(json!({
                                "tool_call_id": call_id,
                                "content": content_str,
                            }));
                        }
                    }
                    if !tool_calls.is_empty() {
                        let mut m = Map::new();
                        m.insert("role".to_string(), Value::String("assistant".to_string()));
                        m.insert("content".to_string(), Value::String(text));
                        m.insert("tool_calls".to_string(), Value::Array(tool_calls));
                        messages.push(Value::Object(m));
                    } else if !tool_results.is_empty() {
                        for tr in tool_results {
                            let mut m = Map::new();
                            m.insert("role".to_string(), Value::String("tool".to_string()));
                            if let Some(id) = tr.get("tool_call_id").cloned() {
                                m.insert("tool_call_id".to_string(), id);
                            }
                            m.insert("content".to_string(), tr.get("content").cloned().unwrap_or(Value::String(String::new())));
                            messages.push(Value::Object(m));
                        }
                    } else {
                        messages.push(json!({"role": chat_role, "content": text}));
                    }
                    continue;
                }
            }
            let text = extract_gemini_text(content);
            messages.push(json!({"role": chat_role, "content": text}));
        }
    }

    let mut out = Map::new();
    if let Some(model) = obj.get("model") {
        out.insert("model".to_string(), model.clone());
    }
    if !messages.is_empty() {
        out.insert("messages".to_string(), Value::Array(messages));
    }

    // generationConfig -> scalar params
    if let Some(gc) = obj.get("generationConfig").and_then(|g| g.as_object()) {
        if let Some(v) = gc.get("temperature") {
            out.insert("temperature".to_string(), v.clone());
        }
        if let Some(v) = gc.get("topP") {
            out.insert("top_p".to_string(), v.clone());
        }
        if let Some(v) = gc.get("topK") {
            out.insert("top_k".to_string(), v.clone());
        }
        if let Some(v) = gc.get("maxOutputTokens") {
            out.insert("max_tokens".to_string(), v.clone());
        }
        // thinking budget -> reasoning_effort level
        if let Some(budget) = gc
            .get("thinkingConfig")
            .and_then(|t| t.get("thinkingBudget"))
            .and_then(|b| b.as_i64())
        {
            let level = match budget {
                b if b <= 1000 => "low",
                b if b <= 8000 => "medium",
                b if b <= 24000 => "high",
                _ => "max",
            };
            out.insert("reasoning_effort".to_string(), Value::String(level.to_string()));
        }
    }

    if let Some(v) = obj.get("stream") {
        out.insert("stream".to_string(), v.clone());
    }

    Ok(Value::Object(out))
}

/// Extract plain text from a Gemini content / systemInstruction value.
fn extract_gemini_text(value: &Value) -> String {
    let mut text = String::new();
    if let Some(parts) = value.get("parts").and_then(|p| p.as_array()) {
        for part in parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                text.push_str(t);
            }
        }
    }
    text
}

/// Convert an internal OpenAI Chat Completions **response** body into a
/// Gemini Native `generateContent` response body (used by the Gemini endpoint).
///
/// - `choices[0].message.content` -> `candidates[0].content.parts[0].text`
/// - `choices[0].message.reasoning_content` -> `candidates[0].thoughts`
/// - `finish_reason` -> `candidates[0].finishReason` (`stop`->`STOP`, `length`->`MAX_TOKENS`)
/// - `usage.prompt_tokens/completion_tokens` -> `usageMetadata.*TokenCount`
pub fn chat_to_gemini_client_response(response: &Value, model_display: &str) -> Value {
    let obj = response.as_object().cloned().unwrap_or_default();

    let choice = obj
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .cloned()
        .unwrap_or_else(|| json!({}));

    let message = choice.get("message").unwrap_or(&Value::Null);
    let text = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let reasoning = message
        .get("reasoning_content")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("stop");
    let gemini_finish = match finish_reason {
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        _ => "STOP",
    };

    let mut candidate = Map::new();
    candidate.insert("index".to_string(), json!(0));
    candidate.insert(
        "content".to_string(),
        json!({
            "role": "model",
            "parts": [{"text": text}],
        }),
    );
    if !reasoning.is_empty() {
        candidate.insert(
            "thoughts".to_string(),
            json!([{"text": reasoning}]),
        );
    }
    candidate.insert("finishReason".to_string(), Value::String(gemini_finish.to_string()));

    let usage = obj.get("usage").cloned().unwrap_or(Value::Null);
    let prompt_tokens = usage
        .get("prompt_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let completion_tokens = usage
        .get("completion_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    json!({
        "candidates": [Value::Object(candidate)],
        "usageMetadata": {
            "promptTokenCount": prompt_tokens,
            "candidatesTokenCount": completion_tokens,
            "totalTokenCount": prompt_tokens + completion_tokens,
        },
        "modelVersion": model_display,
    })
}

/// Stateful converter turning an OpenAI Chat SSE stream into Gemini SSE
/// chunks, chunk by chunk.
///
/// State is required because token usage arrives in a separate trailing chunk
/// (with `stream_options.include_usage`) and streaming tool calls arrive as
/// `delta.tool_calls` fragments across multiple chunks.
pub struct GeminiStreamConverter {
    last_usage: Option<(i64, i64, i64)>,
    /// Set once `finish_reason` has been seen on a chunk.
    finished: bool,
    /// The deferred final chunk (and its trailing `[DONE]`) was sent.
    completion_sent: bool,
    /// Streaming tool calls, keyed by Chat index.
    tool_calls: Vec<GeminiToolCall>,
}

/// In-progress streaming tool call (Chat `delta.tool_calls` fragment).
#[derive(Clone, Default)]
struct GeminiToolCall {
    name: String,
    args: String,
}

impl GeminiStreamConverter {
    pub fn new(_display_name: &str) -> Self {
        Self {
            last_usage: None,
            finished: false,
            completion_sent: false,
            tool_calls: Vec::new(),
        }
    }

    /// Process one Chat SSE chunk payload (JSON without the `data: ` prefix).
    pub fn process(
        &mut self,
        json_str: &str,
    ) -> crate::gateway::convert::SseChunkResult {
        let Ok(v) = serde_json::from_str::<Value>(json_str) else {
            return (vec![], None, None);
        };

        // Error detection
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .map(String::from)
                .unwrap_or_else(|| err.to_string());
            return (vec![], None, Some(msg));
        }

        // Usage extraction (may arrive in a chunk without choices)
        let usage = v.get("usage").filter(|u| !u.is_null()).and_then(|u| {
            let prompt = u.get("prompt_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
            let completion = u.get("completion_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
            if prompt == 0 && completion == 0 {
                None
            } else {
                Some((prompt, completion, prompt + completion))
            }
        });
        if let Some(u) = usage {
            self.last_usage = Some(u);
        }

        let Some(choice) = v
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
        else {
            // Trailing usage-only chunk: flush a pending completion event.
            let mut lines = Vec::new();
            if self.finished && !self.completion_sent {
                lines.extend(self.emit_completion());
            }
            return (lines, usage, None);
        };

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

        // Reasoning delta -> thoughts
        if !reasoning.is_empty() {
            let event = json!({
                "candidates": [{
                    "index": 0,
                    "content": {"role": "model", "parts": [{"text": ""}]},
                    "thoughts": [{"text": reasoning}],
                }]
            });
            lines.push(format!("data: {}\n\n", event));
        }

        // Text delta
        if !text.is_empty() {
            let event = json!({
                "candidates": [{
                    "index": 0,
                    "content": {"role": "model", "parts": [{"text": text}]},
                }]
            });
            lines.push(format!("data: {}\n\n", event));
        }

        // Streaming tool calls -> accumulate; emitted in the final chunk.
        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let index = tc.get("index").and_then(|i| i.as_i64()).unwrap_or(0) as usize;
                let tc_delta = tc.get("function").unwrap_or(&Value::Null);
                let name = tc_delta.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = tc_delta.get("arguments").and_then(|a| a.as_str()).unwrap_or("");

                if self.tool_calls.len() <= index {
                    self.tool_calls.resize(index + 1, GeminiToolCall::default());
                }
                if !name.is_empty() && self.tool_calls[index].name.is_empty() {
                    self.tool_calls[index].name = name.to_string();
                }
                if !args.is_empty() {
                    self.tool_calls[index].args.push_str(args);
                }
            }
        }

        // Final chunk on finish (deferred until usage is known)
        if !finish_reason.is_empty() && finish_reason != "null" {
            self.finished = true;
            if self.last_usage.is_some() {
                lines.extend(self.emit_completion());
            }
        }

        (lines, usage, None)
    }

    /// Flush the deferred final chunk (with `finishReason` + `usageMetadata`).
    pub fn finish(&mut self) -> Vec<String> {
        if self.completion_sent {
            return Vec::new();
        }
        let mut lines = Vec::new();
        if self.finished {
            lines.extend(self.emit_completion());
        }
        lines
    }

    fn emit_completion(&mut self) -> Vec<String> {
        if self.completion_sent {
            return Vec::new();
        }
        self.completion_sent = true;
        let mut lines = Vec::new();

        // Function calls: one parts array with a functionCall per accumulated
        // tool call, sent in a standalone chunk before the finish chunk.
        let parts: Vec<Value> = self
            .tool_calls
            .iter()
            .filter(|c| !c.name.is_empty())
            .map(|c| {
                let args = serde_json::from_str(&c.args).unwrap_or(Value::String(c.args.clone()));
                json!({"functionCall": {"name": c.name, "args": args}})
            })
            .collect();

        if !parts.is_empty() {
            let event = json!({
                "candidates": [{
                    "index": 0,
                    "content": {"role": "model", "parts": parts},
                }]
            });
            lines.push(format!("data: {}\n\n", event));
        }

        let gemini_finish = if !self.tool_calls.iter().any(|c| !c.name.is_empty()) {
            "STOP"
        } else {
            "FUNCTION_CALL"
        };
        let (p, c, _t) = self.last_usage.unwrap_or((0, 0, 0));
        let event = json!({
            "candidates": [{
                "index": 0,
                "content": {"role": "model", "parts": [{"text": ""}]},
                "finishReason": gemini_finish,
            }],
            "usageMetadata": {
                "promptTokenCount": p,
                "candidatesTokenCount": c,
                "totalTokenCount": p + c,
            },
        });
        lines.push(format!("data: {}\n\n", event));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_to_gemini_basic() {
        let body = json!({
            "model": "gemini-2.0-flash",
            "messages": [
                {"role": "system", "content": "Be brief"},
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi"}
            ],
            "max_tokens": 100,
            "temperature": 0.7
        });
        let out = chat_to_gemini(&body).unwrap();
        assert_eq!(out["systemInstruction"]["parts"][0]["text"], "Be brief");
        let contents = out["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(out["generationConfig"]["maxOutputTokens"], 100);
        assert_eq!(out["generationConfig"]["temperature"], 0.7);
    }

    #[test]
    fn test_chat_to_gemini_reasoning() {
        let body = json!({
            "model": "gemini-2.0-flash",
            "messages": [{"role": "user", "content": "Hi"}],
            "reasoning_effort": "max"
        });
        let out = chat_to_gemini(&body).unwrap();
        assert_eq!(
            out["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            32000
        );
    }

    #[test]
    fn test_gemini_to_chat_basic() {
        let resp = json!({
            "candidates": [{
                "content": {"parts": [{"text": "Hello from Gemini"}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 3
            }
        });
        let out = gemini_to_chat(&resp, "Gemini Display");
        assert_eq!(out["object"], "chat.completion");
        assert_eq!(out["choices"][0]["message"]["content"], "Hello from Gemini");
        assert_eq!(out["choices"][0]["finish_reason"], "stop");
        assert_eq!(out["usage"]["prompt_tokens"], 12);
        assert_eq!(out["usage"]["completion_tokens"], 3);
    }

    #[test]
    fn test_gemini_to_chat_max_tokens() {
        let resp = json!({
            "candidates": [{
                "content": {"parts": [{"text": "abc"}]},
                "finishReason": "MAX_TOKENS"
            }],
            "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 2}
        });
        let out = gemini_to_chat(&resp, "M");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
    }

    #[test]
    fn test_gemini_request_to_chat_basic() {
        let body = json!({
            "contents": [
                {"role": "user", "parts": [{"text": "Hello"}]},
                {"role": "model", "parts": [{"text": "Hi"}]}
            ],
            "systemInstruction": {"parts": [{"text": "Be brief"}]},
            "generationConfig": {
                "temperature": 0.7,
                "maxOutputTokens": 100,
                "topP": 0.9
            }
        });
        let out = gemini_request_to_chat(&body).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "Be brief");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], "Hi");
        assert_eq!(out["temperature"], 0.7);
        assert_eq!(out["max_tokens"], 100);
        assert_eq!(out["top_p"], 0.9);
    }

    #[test]
    fn test_gemini_request_thinking_budget_to_level() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "Hi"}]}],
            "generationConfig": {"thinkingConfig": {"thinkingBudget": 24000}}
        });
        let out = gemini_request_to_chat(&body).unwrap();
        assert_eq!(out["reasoning_effort"], "high");
    }

    #[test]
    fn test_gemini_request_function_calling_preserved() {
        // Multi-turn function calling: functionCall parts become Chat tool_calls,
        // matching functionResponse parts become Chat tool messages.
        let body = json!({
            "contents": [
                {"role": "user", "parts": [{"text": "weather in SF?"}]},
                {"role": "model", "parts": [
                    {"functionCall": {"name": "get_weather", "args": {"city": "SF"}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "get_weather", "response": {"temp": 72}}}
                ]}
            ]
        });
        let out = gemini_request_to_chat(&body).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3, "got: {messages:?}");
        // assistant tool_calls
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "");
        let tool_calls = messages[1]["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls[0]["id"], "call_1");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
        assert_eq!(tool_calls[0]["function"]["arguments"], r#"{"city":"SF"}"#);
        // tool result resolves to the same call id via the name map
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_1");
        assert_eq!(messages[2]["content"], r#"{"temp":72}"#);
    }

    #[test]
    fn test_gemini_request_function_text_mixed() {
        // A model content may mix text and functionCall parts.
        let body = json!({
            "contents": [
                {"role": "model", "parts": [
                    {"text": "Let me check."},
                    {"functionCall": {"name": "get_time", "args": {"tz": "UTC"}}}
                ]}
            ]
        });
        let out = gemini_request_to_chat(&body).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["content"], "Let me check.");
        assert_eq!(messages[0]["tool_calls"][0]["function"]["name"], "get_time");
    }

    #[test]
    fn test_chat_to_gemini_client_response_basic() {
        let resp = json!({
            "choices": [{
                "message": {"role": "assistant", "content": "Hello from proxy", "reasoning_content": "think"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        let out = chat_to_gemini_client_response(&resp, "Gem Pool");
        assert_eq!(out["candidates"][0]["content"]["parts"][0]["text"], "Hello from proxy");
        assert_eq!(out["candidates"][0]["finishReason"], "STOP");
        assert_eq!(out["candidates"][0]["thoughts"][0]["text"], "think");
        assert_eq!(out["usageMetadata"]["promptTokenCount"], 10);
        assert_eq!(out["usageMetadata"]["candidatesTokenCount"], 5);
        assert_eq!(out["modelVersion"], "Gem Pool");
    }

    #[test]
    fn test_chat_to_gemini_client_response_max_tokens() {
        let resp = json!({
            "choices": [{"message": {"content": "abc"}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        });
        let out = chat_to_gemini_client_response(&resp, "M");
        assert_eq!(out["candidates"][0]["finishReason"], "MAX_TOKENS");
    }

    #[test]
    fn test_gemini_stream_converter() {
        let mut conv = GeminiStreamConverter::new("Pool");
        let chunk = r#"{"id":"x","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let (lines, usage, error) = conv.process(chunk);
        assert!(error.is_none());
        assert!(usage.is_none());
        let joined = lines.join("");
        assert!(joined.contains(r#""parts":[{"text":"Hello"}]"#), "got: {joined}");

        // Finish chunk WITHOUT usage: completion must be deferred.
        let done = r#"{"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
        let (lines2, usage2, error2) = conv.process(done);
        assert!(error2.is_none());
        assert!(usage2.is_none());
        assert!(
            lines2.join("").is_empty(),
            "completion should be deferred until usage, got: {}",
            lines2.join("")
        );

        // Trailing usage-only chunk flushes the completion with real usage.
        let usage_chunk = r#"{"id":"x","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#;
        let (lines3, usage3, error3) = conv.process(usage_chunk);
        assert!(error3.is_none());
        assert_eq!(usage3, Some((3, 2, 5)));
        let joined3 = lines3.join("");
        assert!(joined3.contains(r#""finishReason":"STOP""#), "got: {joined3}");
        assert!(joined3.contains("promptTokenCount"), "usage missing: {joined3}");
        assert!(
            !joined3.contains("data: [DONE"),
            "Gemini streams end on the finish chunk, no [DONE]: {joined3}"
        );

        // finish() after completion already sent: no duplicate final chunk.
        let tail = conv.finish();
        assert!(tail.is_empty(), "no duplicate final chunk expected: {}", tail.join(""));
    }

    #[test]
    fn test_gemini_stream_converter_tool_calls() {
        let mut conv = GeminiStreamConverter::new("Pool");
        let start = r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#;
        let (lines, _, error) = conv.process(start);
        assert!(error.is_none());
        assert!(lines.is_empty(), "gemini defers tool calls to final chunk");

        let frag = r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\":\"SF\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let (lines2, _, error2) = conv.process(frag);
        assert!(error2.is_none());
        assert!(
            lines2.join("").is_empty(),
            "completion deferred until usage, got: {}",
            lines2.join("")
        );

        let usage_chunk = r#"{"id":"x","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#;
        let (lines3, _, error3) = conv.process(usage_chunk);
        assert!(error3.is_none());
        let joined3 = lines3.join("");
        assert!(joined3.contains("FUNCTION_CALL"), "got: {joined3}");
        assert!(
            joined3.contains(r#""name":"get_weather""#),
            "functionCall name missing: {joined3}"
        );
        assert!(joined3.contains(r#""args":{"city":"SF"}"#), "args missing: {joined3}");
    }
}
