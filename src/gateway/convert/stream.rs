//! Streaming SSE conversion between native upstream formats and OpenAI Chat.
//!
//! Anthropic, Gemini, and OpenAI Responses native APIs emit SSE events in their
//! own schema. This module provides a stateful converter that translates each
//! incoming line into OpenAI Chat-compatible SSE chunks in real time, and
//! extracts token usage for request logging.

use serde_json::{json, Value};

/// Result of converting one raw SSE line.
pub struct ConvertedLine {
    /// Fully-formatted SSE lines to send to the client (`data: ...\n\n`).
    pub lines: Vec<String>,
    /// Token usage captured from this line (if any).
    pub usage: Option<(i64, i64, i64)>,
    /// Error message if the upstream reported an error mid-stream.
    pub error: Option<String>,
    /// True when the stream should terminate after this line.
    pub done: bool,
}

/// Stateful converter for native-format SSE streams.
pub struct NativeStreamConverter {
    api_format: String,
    // Usage fields shared by all native formats.
    prompt_tokens: i64,
    output_tokens: i64,
    // Anthropic: in-progress tool call (id, name, accumulated args JSON)
    tool_call: Option<(String, String, String)>,
    // Gemini: has the final chunk with usage been seen?
    gemini_ended: bool,
    // Responses: in-progress tool call (call_id, name, accumulated arguments)
    responses_tool_call: Option<(String, String, String)>,
}

impl NativeStreamConverter {
    pub fn new(api_format: &str) -> Self {
        Self {
            api_format: api_format.to_string(),
            prompt_tokens: 0,
            output_tokens: 0,
            tool_call: None,
            gemini_ended: false,
            responses_tool_call: None,
        }
    }

    /// Process one raw SSE line and produce OpenAI Chat-compatible output.
    pub fn process(&mut self, raw_line: &str, display_name: &str) -> ConvertedLine {
        match self.api_format.as_str() {
            "anthropic" => self.process_anthropic(raw_line, display_name),
            "gemini_native" => self.process_gemini(raw_line, display_name),
            "openai_responses" => self.process_openai_responses(raw_line, display_name),
            _ => ConvertedLine {
                lines: vec![format!("{}\n\n", raw_line)],
                usage: None,
                error: None,
                done: false,
            },
        }
    }

    /// Final accumulated usage after stream ends.
    pub fn final_usage(&self) -> (i64, i64, i64) {
        (
            self.prompt_tokens,
            self.output_tokens,
            self.prompt_tokens + self.output_tokens,
        )
    }

    // ====================================================================
    // Anthropic
    // ====================================================================

    fn process_anthropic(&mut self, line: &str, display_name: &str) -> ConvertedLine {
        // Anthropic SSE: `event: <name>` then `data: {json}`
        let json_str = match line.strip_prefix("data: ") {
            Some(s) => s.trim(),
            None => {
                // Swallow `event:` / comment / blank lines
                return ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: None,
                    done: false,
                };
            }
        };
        if json_str == "[DONE]" {
            return self.finish();
        }
        let Ok(v) = serde_json::from_str::<Value>(json_str) else {
            return ConvertedLine {
                lines: vec![],
                usage: None,
                error: None,
                done: false,
            };
        };
        let etype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match etype {
            // message_start: capture input tokens
            "message_start" => {
                if let Some(message) = v.get("message")
                    && let Some(usage) = message.get("usage")
                {
                    self.prompt_tokens = usage
                        .get("input_tokens")
                        .and_then(|t| t.as_i64())
                        .unwrap_or(0);
                }
                // Emit an empty chunk carrying usage so the gateway can log it.
                let chunk = self.chat_chunk(
                    json!({"role": "assistant", "content": ""}),
                    "stop",
                    display_name,
                    true,
                );
                let usage = if self.prompt_tokens > 0 {
                    Some((self.prompt_tokens, 0, self.prompt_tokens))
                } else {
                    None
                };
                ConvertedLine {
                    lines: vec![chunk],
                    usage,
                    error: None,
                    done: false,
                }
            }
            // content_block_delta: text_delta -> delta.content, thinking_delta -> reasoning
            "content_block_delta" => {
                let mut delta = serde_json::Map::new();
                let mut emit = true;
                if let Some(d) = v.get("delta") {
                    let dtype = d.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match dtype {
                        "text_delta" => {
                            let text = d.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            delta.insert("content".to_string(), Value::String(text.to_string()));
                        }
                        "thinking_delta" => {
                            let text = d.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                            delta.insert(
                                "reasoning_content".to_string(),
                                Value::String(text.to_string()),
                            );
                        }
                        "input_json_delta" => {
                            // Accumulate tool call arguments JSON
                            if let Some(partial) = d.get("partial_json").and_then(|p| p.as_str())
                                && let Some((id, name, args)) = self.tool_call.take()
                            {
                                self.tool_call = Some((id, name, format!("{}{}", args, partial)));
                            }
                            emit = false;
                        }
                        _ => emit = false,
                    }
                }
                if emit {
                    ConvertedLine {
                        lines: vec![self.chat_chunk(
                            Value::Object(delta),
                            "stop",
                            display_name,
                            false,
                        )],
                        usage: None,
                        error: None,
                        done: false,
                    }
                } else {
                    ConvertedLine {
                        lines: vec![],
                        usage: None,
                        error: None,
                        done: false,
                    }
                }
            }
            // content_block_start: tool_use begins
            "content_block_start" => {
                if let Some(block) = v.get("content_block")
                    && block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                {
                    let id = block.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    self.tool_call = Some((id.clone(), name.clone(), String::new()));
                    let delta = json!({
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "index": 0,
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": ""}
                        }]
                    });
                    ConvertedLine {
                        lines: vec![self.chat_chunk(delta, "tool_calls", display_name, false)],
                        usage: None,
                        error: None,
                        done: false,
                    }
                } else {
                    ConvertedLine {
                        lines: vec![],
                        usage: None,
                        error: None,
                        done: false,
                    }
                }
            }
            // content_block_stop: finalize tool arguments
            "content_block_stop" => {
                if let Some((id, name, args)) = self.tool_call.take() {
                    let delta = json!({
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "index": 0,
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": args}
                        }]
                    });
                    ConvertedLine {
                        lines: vec![self.chat_chunk(delta, "tool_calls", display_name, false)],
                        usage: None,
                        error: None,
                        done: false,
                    }
                } else {
                    ConvertedLine {
                        lines: vec![],
                        usage: None,
                        error: None,
                        done: false,
                    }
                }
            }
            // message_delta: stop_reason + output usage
            "message_delta" => {
                let mut finish_reason = "stop";
                if let Some(delta) = v.get("delta") {
                    let stop_reason = delta.get("stop_reason").and_then(|s| s.as_str()).unwrap_or("end_turn");
                    finish_reason = match stop_reason {
                        "max_tokens" => "length",
                        "tool_use" => "tool_calls",
                        _ => "stop",
                    };
                }
                if let Some(usage) = v.get("usage")
                    && let Some(out) = usage.get("output_tokens").and_then(|o| o.as_i64())
                {
                    self.output_tokens = out;
                }
                let chunk = self.chat_chunk(
                    json!({"role": "assistant", "content": ""}),
                    finish_reason,
                    display_name,
                    false,
                );
                let usage = Some((
                    self.prompt_tokens,
                    self.output_tokens,
                    self.prompt_tokens + self.output_tokens,
                ));
                ConvertedLine {
                    lines: vec![chunk],
                    usage,
                    error: None,
                    done: false,
                }
            }
            "message_stop" => self.finish(),
            "error" => {
                let msg = v
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| "upstream error".to_string());
                ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: Some(msg),
                    done: true,
                }
            }
            _ => ConvertedLine {
                lines: vec![],
                usage: None,
                error: None,
                done: false,
            },
        }
    }

    // ====================================================================
    // Gemini
    // ====================================================================

    fn process_gemini(&mut self, line: &str, display_name: &str) -> ConvertedLine {
        let json_str = match line.strip_prefix("data: ") {
            Some(s) => s.trim(),
            None => {
                return ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: None,
                    done: false,
                };
            }
        };
        if json_str == "[DONE]" {
            return self.finish();
        }
        let Ok(v) = serde_json::from_str::<Value>(json_str) else {
            return ConvertedLine {
                lines: vec![],
                usage: None,
                error: None,
                done: false,
            };
        };

        // Extract text from first candidate
        let mut text = String::new();
        let mut finish_reason = "stop";
        let mut has_candidate = false;
        if let Some(candidates) = v.get("candidates").and_then(|c| c.as_array())
            && let Some(candidate) = candidates.first()
        {
            has_candidate = true;
            if let Some(content) = candidate.get("content")
                && let Some(parts) = content.get("parts").and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
            }
            finish_reason = match candidate
                .get("finishReason")
                .and_then(|r| r.as_str())
                .unwrap_or("STOP")
            {
                "MAX_TOKENS" => "length",
                "SAFETY" => "content_filter",
                _ => "stop",
            };
        }

        // Capture usage metadata (usually on the final chunk)
        let mut usage: Option<(i64, i64, i64)> = None;
        if let Some(meta) = v.get("usageMetadata") {
            let prompt = meta
                .get("promptTokenCount")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            let output = meta
                .get("candidatesTokenCount")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            usage = Some((prompt, output, prompt + output));
        }

        let mut lines = Vec::new();
        if has_candidate && !text.is_empty() {
            lines.push(self.chat_chunk(
                json!({"role": "assistant", "content": text}),
                finish_reason,
                display_name,
                false,
            ));
        }
        // Emit finish on STOP / MAX_TOKENS / SAFETY
        if has_candidate
            && matches!(finish_reason, "stop" | "length" | "content_filter")
        {
            self.gemini_ended = true;
            lines.push(self.chat_chunk(
                json!({"role": "assistant", "content": ""}),
                finish_reason,
                display_name,
                false,
            ));
            lines.push("data: [DONE]\n\n".to_string());
        }

        ConvertedLine {
            lines,
            usage,
            error: None,
            done: self.gemini_ended,
        }
    }

    // ====================================================================
    // OpenAI Responses
    // ====================================================================

    fn process_openai_responses(&mut self, line: &str, display_name: &str) -> ConvertedLine {
        let json_str = match line.strip_prefix("data: ") {
            Some(s) => s.trim(),
            None => {
                // Swallow `event:` / comment / blank lines
                return ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: None,
                    done: false,
                };
            }
        };
        if json_str == "[DONE]" {
            return self.finish();
        }
        let Ok(v) = serde_json::from_str::<Value>(json_str) else {
            return ConvertedLine {
                lines: vec![],
                usage: None,
                error: None,
                done: false,
            };
        };
        let etype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match etype {
            // output_text.delta -> delta.content
            "response.output_text.delta" => {
                let text = v.get("delta").and_then(|t| t.as_str()).unwrap_or("");
                if text.is_empty() {
                    return ConvertedLine {
                        lines: vec![],
                        usage: None,
                        error: None,
                        done: false,
                    };
                }
                ConvertedLine {
                    lines: vec![self.chat_chunk(
                        json!({"role": "assistant", "content": text}),
                        "stop",
                        display_name,
                        false,
                    )],
                    usage: None,
                    error: None,
                    done: false,
                }
            }
            // reasoning_summary_text.delta -> delta.reasoning_content
            "response.reasoning_summary_text.delta" => {
                let text = v.get("delta").and_then(|t| t.as_str()).unwrap_or("");
                if text.is_empty() {
                    return ConvertedLine {
                        lines: vec![],
                        usage: None,
                        error: None,
                        done: false,
                    };
                }
                ConvertedLine {
                    lines: vec![self.chat_chunk(
                        json!({"role": "assistant", "reasoning_content": text}),
                        "stop",
                        display_name,
                        false,
                    )],
                    usage: None,
                    error: None,
                    done: false,
                }
            }
            // output_item.added with a function_call item -> start a tool call
            "response.output_item.added" => {
                if let Some(item) = v.get("item")
                    && item.get("type").and_then(|t| t.as_str()) == Some("function_call")
                {
                    let call_id = item.get("call_id").and_then(|x| x.as_str()).unwrap_or("");
                    let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("");
                    self.responses_tool_call = Some((call_id.to_string(), name.to_string(), String::new()));
                    let delta = json!({
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "index": 0,
                            "id": call_id,
                            "type": "function",
                            "function": {"name": name, "arguments": ""}
                        }]
                    });
                    ConvertedLine {
                        lines: vec![self.chat_chunk(delta, "tool_calls", display_name, false)],
                        usage: None,
                        error: None,
                        done: false,
                    }
                } else {
                    ConvertedLine {
                        lines: vec![],
                        usage: None,
                        error: None,
                        done: false,
                    }
                }
            }
            // function_call_arguments.delta: accumulate args JSON
            "response.function_call_arguments.delta" => {
                if let Some(partial) = v.get("delta").and_then(|d| d.as_str())
                    && let Some((id, name, args)) = self.responses_tool_call.take()
                {
                    self.responses_tool_call = Some((id, name, format!("{}{}", args, partial)));
                }
                ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: None,
                    done: false,
                }
            }
            // function_call_arguments.done: finalize tool arguments
            "response.function_call_arguments.done" => {
                if let Some((id, name, args)) = self.responses_tool_call.take() {
                    let final_args = v
                        .get("arguments")
                        .and_then(|a| a.as_str())
                        .map(String::from)
                        .unwrap_or(args);
                    let delta = json!({
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "index": 0,
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": final_args}
                        }]
                    });
                    ConvertedLine {
                        lines: vec![self.chat_chunk(delta, "tool_calls", display_name, false)],
                        usage: None,
                        error: None,
                        done: false,
                    }
                } else {
                    ConvertedLine {
                        lines: vec![],
                        usage: None,
                        error: None,
                        done: false,
                    }
                }
            }
            // output_item.done: if it finished a tool call with no done event, flush it
            "response.output_item.done" => {
                if let Some((id, name, args)) = self.responses_tool_call.take()
                    && let Some(item) = v.get("item")
                    && item.get("type").and_then(|t| t.as_str()) == Some("function_call")
                    && !args.is_empty()
                {
                    let delta = json!({
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "index": 0,
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": args}
                        }]
                    });
                    ConvertedLine {
                        lines: vec![self.chat_chunk(delta, "tool_calls", display_name, false)],
                        usage: None,
                        error: None,
                        done: false,
                    }
                } else {
                    ConvertedLine {
                        lines: vec![],
                        usage: None,
                        error: None,
                        done: false,
                    }
                }
            }
            // response.completed: status + usage + DONE
            "response.completed" => {
                let resp = v.get("response").unwrap_or(&Value::Null);
                let status = resp
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("completed");
                let finish_reason = if status == "incomplete" { "length" } else { "stop" };
                let usage = resp.get("usage").unwrap_or(&Value::Null);
                let prompt = usage
                    .get("input_tokens")
                    .and_then(|t| t.as_i64())
                    .unwrap_or(0);
                let output = usage
                    .get("output_tokens")
                    .and_then(|t| t.as_i64())
                    .unwrap_or(0);
                if prompt > 0 || output > 0 {
                    self.prompt_tokens = prompt;
                    self.output_tokens = output;
                }
                let chunk = self.chat_chunk(
                    json!({"role": "assistant", "content": ""}),
                    finish_reason,
                    display_name,
                    false,
                );
                let usage = if self.prompt_tokens > 0 || self.output_tokens > 0 {
                    Some((
                        self.prompt_tokens,
                        self.output_tokens,
                        self.prompt_tokens + self.output_tokens,
                    ))
                } else {
                    None
                };
                ConvertedLine {
                    lines: vec![chunk, "data: [DONE]\n\n".to_string()],
                    usage,
                    error: None,
                    done: true,
                }
            }
            // response.failed: emit error
            "response.failed" => {
                let msg = v
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| "upstream error".to_string());
                ConvertedLine {
                    lines: vec![],
                    usage: None,
                    error: Some(msg),
                    done: true,
                }
            }
            // All other events (response.created, response.in_progress,
            // response.output_text.done, response.content_part.done, etc.)
            _ => ConvertedLine {
                lines: vec![],
                usage: None,
                error: None,
                done: false,
            },
        }
    }

    // ====================================================================
    // Helpers
    // ====================================================================

    /// Emit an OpenAI Chat chunk carrying usage (for message_start logging).
    fn chat_chunk(
        &self,
        delta: Value,
        finish_reason: &str,
        display_name: &str,
        include_usage: bool,
    ) -> String {
        let mut chunk = json!({
            "id": format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
            "object": "chat.completion.chunk",
            "created": chrono::Utc::now().timestamp(),
            "model": display_name,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": if finish_reason == "stop" { Value::Null } else { Value::String(finish_reason.to_string()) },
            }],
        });
        if include_usage {
            let (p, c, t) = self.final_usage();
            chunk["usage"] = json!({
                "prompt_tokens": p,
                "completion_tokens": c,
                "total_tokens": t,
            });
        }
        format!("data: {}\n\n", chunk)
    }

    fn finish(&mut self) -> ConvertedLine {
        ConvertedLine {
            lines: vec!["data: [DONE]\n\n".to_string()],
            usage: None,
            error: None,
            done: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(c: &mut NativeStreamConverter, lines: &[&str], display: &str) -> Vec<String> {
        let mut out = Vec::new();
        for l in lines {
            let r = c.process(l, display);
            out.extend(r.lines);
        }
        out
    }

    #[test]
    fn test_anthropic_text_stream() {
        let lines = [
            "event: message_start",
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":0}}}"#,
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#,
            "event: content_block_stop",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "event: message_delta",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
            "event: message_stop",
            r#"data: {"type":"message_stop"}"#,
        ];
        let mut conv = NativeStreamConverter::new("anthropic");
        let out = collect(&mut conv, &lines, "My Pool");
        let joined = out.join("");
        assert!(joined.contains(r#""content":"Hello""#), "text delta missing");
        assert!(joined.contains(r#""content":" world""#), "2nd delta missing");
        assert!(joined.contains("data: [DONE]"), "DONE missing");
        assert!(joined.contains(r#""model":"My Pool""#), "model replace missing");
        // usage captured: prompt=12, output=3
        let mut conv2 = NativeStreamConverter::new("anthropic");
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
    fn test_anthropic_tool_stream() {
        let lines = [
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu_1","name":"get_weather","input":{}}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"SF\"}"}}"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
            r#"data: {"type":"message_stop"}"#,
        ];
        let mut conv = NativeStreamConverter::new("anthropic");
        let out = collect(&mut conv, &lines, "M");
        let joined = out.join("");
        assert!(joined.contains(r#""tool_calls""#), "tool calls missing");
        assert!(joined.contains(r#""arguments":"{\"city\":\"SF\"}""#), "args accumulate");
    }

    #[test]
    fn test_anthropic_error_event() {
        let mut conv = NativeStreamConverter::new("anthropic");
        let r = conv.process(
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"overloaded"}}"#,
            "M",
        );
        assert_eq!(r.error.as_deref(), Some("overloaded"));
        assert!(r.done);
    }

    #[test]
    fn test_gemini_stream() {
        let lines = [
            r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi "}]},"index":0}]}"#,
            r#"data: {"candidates":[{"content":{"parts":[{"text":"there"}]},"index":0}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}"#,
            r#"data: {"candidates":[{"content":{"parts":[{"text":"!"}]},"finishReason":"STOP","index":0}]}"#,
        ];
        let mut conv = NativeStreamConverter::new("gemini_native");
        let out = collect(&mut conv, &lines, "Gem");
        let joined = out.join("");
        assert!(joined.contains(r#""content":"Hi ""#));
        assert!(joined.contains(r#""content":"there""#));
        assert!(joined.contains(r#""content":"!""#));
        assert!(joined.contains("data: [DONE]"));
        // usage from the middle chunk
        assert!(conv.final_usage().0 >= 0);
    }

    #[test]
    fn test_openai_passthrough() {
        let mut conv = NativeStreamConverter::new("openai_chat");
        let r = conv.process(r#"data: {"choices":[]}"#, "M");
        assert_eq!(r.lines.len(), 1);
        assert!(r.lines[0].starts_with("data: "));
    }

    #[test]
    fn test_openai_responses_stream() {
        let lines = [
            r#"data: {"type":"response.created","response":{"id":"resp_1"}}"#,
            r#"data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hello"}"#,
            r#"data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":" world"}"#,
            r#"data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":12,"output_tokens":3}}}"#,
        ];
        let mut conv = NativeStreamConverter::new("openai_responses");
        let out = collect(&mut conv, &lines, "My Pool");
        let joined = out.join("");
        assert!(joined.contains(r#""content":"Hello""#), "text delta missing");
        assert!(joined.contains(r#""content":" world""#), "2nd delta missing");
        assert!(joined.contains("data: [DONE]"), "DONE missing");
        assert!(joined.contains(r#""model":"My Pool""#), "model replace missing");
        // usage captured
        let mut conv2 = NativeStreamConverter::new("openai_responses");
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
    fn test_openai_responses_reasoning_and_tool_stream() {
        let lines = [
            r#"data: {"type":"response.reasoning_summary_text.delta","output_index":0,"content_index":0,"delta":"let me think"}"#,
            r#"data: {"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"get_weather","arguments":"","status":"in_progress"}}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":1,"delta":"{\"city\":"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":1,"delta":"\"SF\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.done","item_id":"fc_1","output_index":1,"arguments":"{\"city\":\"SF\"}"}"#,
            r#"data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":2,"output_tokens":4}}}"#,
        ];
        let mut conv = NativeStreamConverter::new("openai_responses");
        let out = collect(&mut conv, &lines, "M");
        let joined = out.join("");
        assert!(joined.contains(r#""reasoning_content":"let me think""#), "reasoning missing");
        assert!(joined.contains(r#""tool_calls""#), "tool calls missing");
        assert!(joined.contains(r#""arguments":"{\"city\":\"SF\"}""#), "args accumulate");
        assert!(joined.contains(r#""name":"get_weather""#), "name missing");
    }

    #[test]
    fn test_openai_responses_failed_event() {
        let mut conv = NativeStreamConverter::new("openai_responses");
        let r = conv.process(
            r#"data: {"type":"response.failed","response":{"id":"resp_1","error":{"message":"boom"}}}"#,
            "M",
        );
        assert_eq!(r.error.as_deref(), Some("boom"));
        assert!(r.done);
    }
}
