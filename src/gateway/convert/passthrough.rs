//! Direct passthrough streaming for native client formats.
//!
//! When the client calls a native endpoint (`/v1/responses`, `/v1/messages`,
//! `/v1beta/...generateContent`) whose format matches the upstream's
//! `api_format`, no conversion is needed at all: the upstream SSE stream is
//! forwarded to the client verbatim. This module still parses each `data:`
//! line so it can (a) substitute the pool display name into the `model`
//! field, (b) extract token usage for request logging, and (c) detect
//! mid-stream errors for failover/log status.

use serde_json::Value;

use super::stream::ConvertedLine;

/// Stateful passthrough stream processor for native-format SSE streams.
pub struct PassthroughStreamConverter {
    api_format: String,
    prompt_tokens: i64,
    output_tokens: i64,
}

impl PassthroughStreamConverter {
    pub fn new(api_format: &str) -> Self {
        Self {
            api_format: api_format.to_string(),
            prompt_tokens: 0,
            output_tokens: 0,
        }
    }

    /// Process one raw SSE line and produce the lines to forward.
    pub fn process(&mut self, raw_line: &str, display_name: &str) -> ConvertedLine {
        match self.api_format.as_str() {
            "anthropic" => self.process_anthropic(raw_line, display_name),
            "gemini_native" => self.process_gemini(raw_line, display_name),
            "openai_responses" => self.process_responses(raw_line, display_name),
            _ => ConvertedLine {
                lines: vec![format!("{}\n\n", raw_line)],
                usage: None,
                error: None,
                done: false,
            },
        }
    }

    /// Final accumulated usage after the stream ends.
    pub fn final_usage(&self) -> (i64, i64, i64) {
        (
            self.prompt_tokens,
            self.output_tokens,
            self.prompt_tokens + self.output_tokens,
        )
    }

    // ====================================================================
    // Shared helpers
    // ====================================================================

    fn forward_line(raw_line: &str) -> ConvertedLine {
        ConvertedLine {
            lines: vec![format!("{}\n\n", raw_line)],
            usage: None,
            error: None,
            done: false,
        }
    }

    fn forward_json(v: &Value) -> String {
        format!("data: {}\n\n", v)
    }

    /// Recursively replace any string `model` field with the display name.
    fn replace_model_field(v: &mut Value, display: &str) {
        if let Some(obj) = v.as_object_mut() {
            for (key, val) in obj.iter_mut() {
                if key == "model" && val.is_string() {
                    *val = Value::String(display.to_string());
                } else {
                    Self::replace_model_field(val, display);
                }
            }
        } else if let Some(arr) = v.as_array_mut() {
            for item in arr {
                Self::replace_model_field(item, display);
            }
        }
    }

    // ====================================================================
    // Anthropic
    // ====================================================================

    fn process_anthropic(&mut self, line: &str, display_name: &str) -> ConvertedLine {
        // Anthropic SSE uses `event: <name>` lines interleaved with `data:`.
        let Some(json_str) = line.strip_prefix("data: ") else {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with(':') {
                return ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: None,
                    done: false,
                };
            }
            return Self::forward_line(line);
        };
        let trimmed = json_str.trim();
        if trimmed == "[DONE]" {
            return ConvertedLine {
                lines: vec!["data: [DONE]\n\n".to_string()],
                usage: None,
                error: None,
                done: true,
            };
        }
        let Ok(mut v) = serde_json::from_str::<Value>(trimmed) else {
            return Self::forward_line(json_str);
        };
        let etype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let mut usage = None;
        let mut done = false;
        match etype {
            "message_start" => {
                if let Some(msg) = v.get_mut("message") {
                    Self::replace_model_field(msg, display_name);
                    if let Some(u) = msg.get("usage") {
                        self.prompt_tokens = u
                            .get("input_tokens")
                            .and_then(|t| t.as_i64())
                            .unwrap_or(0);
                    }
                }
            }
            "message_delta" => {
                if let Some(u) = v.get("usage")
                    && let Some(out) = u.get("output_tokens").and_then(|o| o.as_i64())
                {
                    self.output_tokens = out;
                }
                usage = Some((
                    self.prompt_tokens,
                    self.output_tokens,
                    self.prompt_tokens + self.output_tokens,
                ));
            }
            "message_stop" => done = true,
            "error" => {
                let msg = v
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| "upstream error".to_string());
                return ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: Some(msg),
                    done: true,
                };
            }
            _ => {}
        }
        ConvertedLine {
            lines: vec![Self::forward_json(&v)],
            usage,
            error: None,
            done,
        }
    }

    // ====================================================================
    // Gemini
    // ====================================================================

    fn process_gemini(&mut self, line: &str, display_name: &str) -> ConvertedLine {
        let Some(json_str) = line.strip_prefix("data: ") else {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with(':') {
                return ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: None,
                    done: false,
                };
            }
            return Self::forward_line(line);
        };
        let trimmed = json_str.trim();
        if trimmed == "[DONE]" {
            return ConvertedLine {
                lines: vec!["data: [DONE]\n\n".to_string()],
                usage: None,
                error: None,
                done: true,
            };
        }
        let Ok(mut v) = serde_json::from_str::<Value>(trimmed) else {
            return Self::forward_line(json_str);
        };
        let mut usage = None;
        if let Some(meta) = v.get("usageMetadata") {
            let prompt = meta
                .get("promptTokenCount")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            let output = meta
                .get("candidatesTokenCount")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            self.prompt_tokens = prompt;
            self.output_tokens = output;
            usage = Some((prompt, output, prompt + output));
        }
        // Detect an embedded error object.
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .map(String::from)
                .unwrap_or_else(|| "upstream error".to_string());
            return ConvertedLine {
                lines: vec![Self::forward_json(&v)],
                usage: None,
                error: Some(msg),
                done: true,
            };
        }
        // The final chunk carries a finishReason; the stream ends after it.
        let mut done = false;
        if let Some(candidates) = v.get("candidates").and_then(|c| c.as_array())
            && let Some(candidate) = candidates.first()
            && candidate.get("finishReason").is_some()
        {
            done = true;
        }
        Self::replace_model_field(&mut v, display_name);
        ConvertedLine {
            lines: vec![Self::forward_json(&v)],
            usage,
            error: None,
            done,
        }
    }

    // ====================================================================
    // OpenAI Responses
    // ====================================================================

    fn process_responses(&mut self, line: &str, display_name: &str) -> ConvertedLine {
        let Some(json_str) = line.strip_prefix("data: ") else {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with(':') {
                return ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: None,
                    done: false,
                };
            }
            return Self::forward_line(line);
        };
        let trimmed = json_str.trim();
        if trimmed == "[DONE]" {
            return ConvertedLine {
                lines: vec!["data: [DONE]\n\n".to_string()],
                usage: None,
                error: None,
                done: true,
            };
        }
        let Ok(mut v) = serde_json::from_str::<Value>(trimmed) else {
            return Self::forward_line(json_str);
        };
        let etype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let mut usage = None;
        match etype {
            "response.completed" => {
                if let Some(resp) = v.get_mut("response") {
                    Self::replace_model_field(resp, display_name);
                    if let Some(u) = resp.get("usage") {
                        let prompt = u
                            .get("input_tokens")
                            .and_then(|t| t.as_i64())
                            .unwrap_or(0);
                        let output = u
                            .get("output_tokens")
                            .and_then(|t| t.as_i64())
                            .unwrap_or(0);
                        if prompt > 0 || output > 0 {
                            self.prompt_tokens = prompt;
                            self.output_tokens = output;
                        }
                        usage = Some((
                            self.prompt_tokens,
                            self.output_tokens,
                            self.prompt_tokens + self.output_tokens,
                        ));
                    }
                }
                ConvertedLine {
                    lines: vec![Self::forward_json(&v)],
                    usage,
                    error: None,
                    done: true,
                }
            }
            "response.failed" => {
                let msg = v
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| "upstream error".to_string());
                ConvertedLine {
                    lines: vec![Self::forward_json(&v)],
                    usage: None,
                    error: Some(msg),
                    done: true,
                }
            }
            _ => {
                // Other events carry the full response object (model field).
                if let Some(resp) = v.get_mut("response") {
                    Self::replace_model_field(resp, display_name);
                }
                ConvertedLine {
                    lines: vec![Self::forward_json(&v)],
                    usage: None,
                    error: None,
                    done: false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(c: &mut PassthroughStreamConverter, lines: &[&str], display: &str) -> Vec<String> {
        let mut out = Vec::new();
        for l in lines {
            let r = c.process(l, display);
            out.extend(r.lines);
        }
        out
    }

    #[test]
    fn test_anthropic_passthrough_forwards_events() {
        let lines = [
            "event: message_start",
            r#"data: {"type":"message_start","message":{"model":"claude-x","usage":{"input_tokens":12}}}"#,
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            "event: message_delta",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
            "event: message_stop",
            r#"data: {"type":"message_stop"}"#,
        ];
        let mut conv = PassthroughStreamConverter::new("anthropic");
        let out = collect(&mut conv, &lines, "My Pool");
        let joined = out.join("");
        assert!(joined.contains("event: message_start"), "event lines forwarded");
        assert!(joined.contains(r#""model":"My Pool""#), "model replace missing");
        assert!(joined.contains(r#""text":"Hello""#), "content forwarded");
        // usage captured
        let mut conv2 = PassthroughStreamConverter::new("anthropic");
        let mut captured = None;
        for l in &lines {
            let r = conv2.process(l, "M");
            if r.usage.is_some() {
                captured = r.usage;
            }
        }
        assert_eq!(captured, Some((12, 3, 15)));
        assert_eq!(conv2.final_usage(), (12, 3, 15));
    }

    #[test]
    fn test_anthropic_passthrough_error() {
        let mut conv = PassthroughStreamConverter::new("anthropic");
        let r = conv.process(
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"overloaded"}}"#,
            "M",
        );
        assert_eq!(r.error.as_deref(), Some("overloaded"));
        assert!(r.done);
        assert!(r.lines.is_empty());
    }

    #[test]
    fn test_gemini_passthrough() {
        let lines = [
            r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi "}]},"index":0}]}"#,
            r#"data: {"candidates":[{"content":{"parts":[{"text":"there"}]},"index":0}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}"#,
            r#"data: {"candidates":[{"content":{"parts":[{"text":"!"}]},"finishReason":"STOP","index":0}]}"#,
        ];
        let mut conv = PassthroughStreamConverter::new("gemini_native");
        let out = collect(&mut conv, &lines, "Gem");
        let joined = out.join("");
        assert!(joined.contains(r#""text":"Hi ""#));
        assert!(joined.contains(r#""text":"there""#));
        // usage captured
        assert_eq!(conv.final_usage(), (5, 2, 7));
    }

    #[test]
    fn test_gemini_passthrough_error() {
        let mut conv = PassthroughStreamConverter::new("gemini_native");
        let r = conv.process(
            r#"data: {"error":{"code":429,"message":"rate limited"}}"#,
            "M",
        );
        assert_eq!(r.error.as_deref(), Some("rate limited"));
        assert!(r.done);
    }

    #[test]
    fn test_responses_passthrough() {
        let lines = [
            r#"data: {"type":"response.created","response":{"id":"resp_1","model":"gpt-5"}}"#,
            r#"data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hello"}"#,
            r#"data: {"type":"response.completed","response":{"id":"resp_1","model":"gpt-5","status":"completed","usage":{"input_tokens":12,"output_tokens":3}}}"#,
        ];
        let mut conv = PassthroughStreamConverter::new("openai_responses");
        let out = collect(&mut conv, &lines, "My Pool");
        let joined = out.join("");
        assert!(joined.contains(r#""type":"response.created""#), "events forwarded");
        assert!(joined.contains(r#""delta":"Hello""#), "delta forwarded");
        assert!(joined.contains(r#""model":"My Pool""#), "model replace missing");
        // usage captured
        let mut conv2 = PassthroughStreamConverter::new("openai_responses");
        let mut captured = None;
        for l in &lines {
            let r = conv2.process(l, "M");
            if r.usage.is_some() {
                captured = r.usage;
            }
        }
        assert_eq!(captured, Some((12, 3, 15)));
        assert_eq!(conv2.final_usage(), (12, 3, 15));
    }

    #[test]
    fn test_responses_passthrough_failed() {
        let mut conv = PassthroughStreamConverter::new("openai_responses");
        let r = conv.process(
            r#"data: {"type":"response.failed","response":{"id":"resp_1","error":{"message":"boom"}}}"#,
            "M",
        );
        assert_eq!(r.error.as_deref(), Some("boom"));
        assert!(r.done);
    }
}
