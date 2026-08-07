//! Gemini Native API <-> OpenAI Chat Completions conversion.
//!
//! Gemini's native `generateContent` format uses `contents`/`parts` instead of
//! `messages`, and wraps generation params in `generationConfig`. The gateway
//! converts between these when `api_format = "gemini_native"`.

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
}
