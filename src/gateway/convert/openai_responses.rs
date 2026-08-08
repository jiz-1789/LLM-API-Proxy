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
    // {role, content}
    if let Some(role) = item.get("role").and_then(|r| r.as_str()) {
        return Some(json!({
            "role": chat_role(role),
            "content": responses_content_to_chat(item.get("content").cloned().unwrap_or(Value::String(String::new()))),
        }));
    }
    // {type: "message", role, content}
    if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
        match item_type {
            "message" => {
                let mrole = item.get("role").and_then(|r| r.as_str()).unwrap_or("user");
                return Some(json!({
                    "role": chat_role(mrole),
                    "content": responses_content_to_chat(item.get("content").cloned().unwrap_or(Value::String(String::new()))),
                }));
            }
            "function_call" => {
                // function_call input -> assistant tool message
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

/// Map a Responses API role to one accepted by Chat Completions upstreams.
///
/// Most OpenAI-compatible upstreams only accept `system`/`user`/`assistant`/
/// `tool`; Responses' `developer` role is rejected with a schema error, so it
/// is folded into `system`.
fn chat_role(role: &str) -> &str {
    match role {
        "developer" => "system",
        other => other,
    }
}

/// Convert a Responses `content` value (a string or an array of content parts)
/// into an exact string a Chat Completions upstream will accept.
///
/// Responses content parts use Response-only types (`input_text`, `output_text`,
/// etc.). Chat upstreams expect either a plain string or `{"type":"text"}` parts.
/// Text parts are joined into a single string; image/audio parts are dropped
/// since Chat upstreams require explicit typed conversion (handled per-provider).
fn responses_content_to_chat(content: Value) -> Value {
    let arr = match content.as_array() {
        Some(a) => a,
        None => return content,
    };
    let mut texts: Vec<String> = Vec::new();
    for part in arr {
        let ptype = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ptype {
            "input_text" | "output_text" | "text" | "refusal" => {
                if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
                    texts.push(t.to_string());
                }
            }
            _ => { /* image/video/audio/tool_reference blocks: not representable
                       as plain text; skip so the request stays valid */ }
        }
    }
    Value::String(texts.join("\n"))
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

/// Convert an OpenAI Chat Completions **request** body into an OpenAI Responses
/// API request body (used when an upstream uses `api_format = "openai_responses"`).
///
/// - `messages[]` -> `input[]` (system -> `instructions`, assistant tool calls
///   -> `function_call` items, tool results -> `function_call_output`)
/// - `max_tokens` -> `max_output_tokens`
/// - `reasoning_effort` -> `reasoning.effort`
/// - Chat `tools` (function) -> Responses `tools` (function)
pub fn chat_to_responses_request(body: &Value) -> Result<Value, String> {
    let obj = body.as_object().ok_or("request body must be a JSON object")?;

    let mut input: Vec<Value> = Vec::new();
    let mut instructions: Vec<String> = Vec::new();

    if let Some(messages) = obj.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");
            if role == "system" || role == "developer" {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str())
                    && !content.is_empty()
                {
                    instructions.push(content.to_string());
                }
                continue;
            }
            if role == "tool" {
                let call_id = msg
                    .get("tool_call_id")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                let output = msg
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
                continue;
            }

            let content = msg.get("content").cloned().unwrap_or(Value::String(String::new()));
            let text = match &content {
                Value::String(s) => s.clone(),
                Value::Array(blocks) => {
                    let mut t = String::new();
                    for b in blocks {
                        if let Some(txt) = b.get("text").and_then(|x| x.as_str()) {
                            t.push_str(txt);
                        }
                    }
                    t
                }
                _ => String::new(),
            };

            if role == "assistant" {
                // Assistant message may carry both text and tool calls.
                if !text.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    }));
                }
                if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tool_calls {
                        let call_id = tc.get("id").and_then(|x| x.as_str()).unwrap_or("");
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("");
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("{}");
                        input.push(json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": name,
                            "arguments": args,
                        }));
                    }
                }
            } else {
                input.push(json!({
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                }));
            }
        }
    }

    let mut out = Map::new();
    if let Some(model) = obj.get("model") {
        out.insert("model".to_string(), model.clone());
    }
    if !instructions.is_empty() {
        out.insert("instructions".to_string(), Value::String(instructions.join("\n")));
    }
    if !input.is_empty() {
        out.insert("input".to_string(), Value::Array(input));
    }

    // max_tokens -> max_output_tokens
    if let Some(v) = obj.get("max_tokens") {
        out.insert("max_output_tokens".to_string(), v.clone());
    }

    // scalar passthrough
    for key in ["temperature", "top_p", "stream", "store", "stop"] {
        if let Some(v) = obj.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }

    // reasoning_effort -> reasoning.effort
    if let Some(level) = obj.get("reasoning_effort").and_then(|v| v.as_str()) {
        out.insert("reasoning".to_string(), json!({"effort": level}));
    } else if obj.get("reasoning").and_then(|v| v.as_bool()) == Some(true) {
        out.insert("reasoning".to_string(), json!({"effort": "high"}));
    }

    // tools: chat function -> responses function
    if let Some(tools) = obj.get("tools").and_then(|t| t.as_array()) {
        let mut resp_tools: Vec<Value> = Vec::new();
        for tool in tools {
            if tool.get("type").and_then(|t| t.as_str()) == Some("function")
                && let Some(func) = tool.get("function")
            {
                resp_tools.push(json!({
                    "type": "function",
                    "name": func.get("name").cloned().unwrap_or(Value::Null),
                    "description": func.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": func
                        .get("parameters")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                }));
            }
        }
        if !resp_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(resp_tools));
        }
    }

    Ok(Value::Object(out))
}

/// Convert an OpenAI Responses API **response** body into an internal OpenAI
/// Chat Completions response body (used when the upstream `api_format` is
/// `openai_responses`).
///
/// - `output[]` message items -> `choices[0].message.content`
/// - `output[]` `function_call` items -> `choices[0].message.tool_calls`
/// - `output[]` `reasoning` items -> `reasoning_content`
/// - `usage.input_tokens/output_tokens` -> `prompt_tokens/completion_tokens`
/// - `status` -> `finish_reason` (`incomplete` -> `length`)
pub fn responses_response_to_chat(response: &Value, model_display: &str) -> Value {
    let obj = response.as_object().cloned().unwrap_or_default();

    let mut text = String::new();
    let mut reasoning_text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    if let Some(output) = obj.get("output").and_then(|o| o.as_array()) {
        for item in output {
            let itype = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match itype {
                "message" => {
                    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                        for block in content {
                            let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            if btype == "output_text"
                                && let Some(t) = block.get("text").and_then(|t| t.as_str())
                            {
                                text.push_str(t);
                            }
                        }
                    }
                }
                "function_call" => {
                    tool_calls.push(json!({
                        "id": item.get("call_id").cloned().unwrap_or(Value::Null),
                        "type": "function",
                        "function": {
                            "name": item.get("name").cloned().unwrap_or(Value::Null),
                            "arguments": item.get("arguments").cloned().unwrap_or_else(|| Value::String("{}".to_string())),
                        }
                    }));
                }
                "reasoning" => {
                    if let Some(summary) = item.get("summary").and_then(|s| s.as_array()) {
                        for s in summary {
                            if let Some(t) = s.get("text").and_then(|t| t.as_str()) {
                                reasoning_text.push_str(t);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let finish_reason = match obj
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("completed")
    {
        "incomplete" => "length",
        "failed" => "stop",
        _ => "stop",
    };

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
        .unwrap_or_else(|| json!(format!("resp_{}", uuid::Uuid::new_v4().simple())));

    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(text));
    if !reasoning_text.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_text),
        );
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }

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

/// Stateful converter turning an OpenAI Chat SSE stream into Responses API
/// SSE events, chunk by chunk.
///
/// The event lifecycle mirrors the OpenAI Responses wire protocol (as used by
/// Codex and the official SDKs):
/// `response.created` → `response.in_progress` → `output_item.added` →
/// `content_part.added` → `output_text.delta` → done events →
/// `response.completed`.
///
/// The conversion is stateful because:
/// - token usage arrives in a separate trailing chunk (when the upstream is
///   configured with `stream_options.include_usage`), so the final
///   `response.completed` event must be deferred until usage is known;
/// - streaming tool calls arrive as `delta.tool_calls` fragments across
///   multiple chunks and must be accumulated before `output_item.done`;
/// - the client requires scaffolding events (item/part) referenced by deltas.
pub struct ResponsesStreamConverter {
    display_name: String,
    /// Stable response id shared by created/in_progress/completed events.
    response_id: String,
    created_at: i64,
    last_usage: Option<(i64, i64, i64)>,
    /// Set once `finish_reason` has been seen on a chunk.
    finished: bool,
    /// The deferred completion event (and its trailing `[DONE]`) was sent.
    completion_sent: bool,
    /// `response.created` + `response.in_progress` were emitted.
    started: bool,
    /// The assistant message item lifecycle.
    msg: ItemState,
    /// The reasoning item lifecycle.
    reasoning: ItemState,
    /// Streaming tool calls, indexed by their Chat `delta.tool_calls` index.
    tools: Vec<ToolCallState>,
    /// Next free index into the client-side `output[]` snapshot.
    output_index: u32,
}

/// A streaming output item (message or reasoning) that has been announced via
/// `output_item.added` + `content_part.added` and accumulates delta text.
#[derive(Clone, Default)]
struct ItemState {
    added: bool,
    done: bool,
    item_id: String,
    index: u32,
    text: String,
}

/// In-progress streaming tool call (Chat `delta.tool_calls` fragment).
#[derive(Clone, Default)]
struct ToolCallState {
    /// Responses item id; stable across fragments.
    item_id: String,
    call_id: String,
    name: String,
    args: String,
    added: bool,
    index: u32,
}

impl ResponsesStreamConverter {
    pub fn new(display_name: &str) -> Self {
        let response_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
        Self {
            display_name: display_name.to_string(),
            response_id,
            created_at: chrono::Utc::now().timestamp(),
            last_usage: None,
            finished: false,
            completion_sent: false,
            started: false,
            msg: ItemState::default(),
            reasoning: ItemState::default(),
            tools: Vec::new(),
            output_index: 0,
        }
    }

    /// Process one Chat SSE chunk payload (JSON without the `data: ` prefix).
    ///
    /// Returns `(output_lines, usage, error)` where `output_lines` is empty
    /// when the chunk should be swallowed (role announcements, already-emitted
    /// usage-only chunks). The final completion event may be deferred.
    pub fn process(
        &mut self,
        json_str: &str,
    ) -> crate::gateway::convert::SseChunkResult {
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
            // No choice: this is the trailing usage-only chunk. If the final
            // completion event is still pending, emit it now with real usage.
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
        self.ensure_started(&mut lines);

        // Emit reasoning delta if present (Responses: `reasoning_summary_text.delta`)
        if !reasoning.is_empty() {
            self.push_reasoning_delta(reasoning, &mut lines);
        }

        // Emit text delta
        if !text.is_empty() {
            self.push_text_delta(text, &mut lines);
        }

        // Streaming tool calls: accumulate fragments and mirror them as
        // Responses function_call events.
        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            self.process_tool_calls(tcs, &mut lines);
        }

        // Completion event on finish (deferred until usage is known)
        if !finish_reason.is_empty() && finish_reason != "null" {
            self.finished = true;
            if self.last_usage.is_some() {
                lines.extend(self.emit_completion());
            }
        }

        (lines, usage, None)
    }

    /// Emit `response.created` + `response.in_progress` if not yet emitted.
    /// These are always the first events; the SDK rejects deltas before them.
    fn ensure_started(&mut self, lines: &mut Vec<String>) {
        if self.started {
            return;
        }
        self.started = true;

        for event_type in ["response.created", "response.in_progress"] {
            let event = json!({
                "type": event_type,
                "response": {
                    "id": self.response_id,
                    "object": "response",
                    "created_at": self.created_at,
                    "status": "in_progress",
                    "model": self.display_name,
                    "output": [],
                    "usage": self.responses_usage(None),
                }
            });
            lines.push(format!("event: {event_type}\ndata: {}\n\n", event));
        }
    }

    /// Annonce the assistant message item (item + initial content part) so a
    /// following text delta has stable `item_id`/`output_index` to attach to.
    fn push_text_delta(&mut self, delta: &str, lines: &mut Vec<String>) {
        let item = &mut self.msg;
        if !item.added {
            item.added = true;
            item.index = self.output_index;
            self.output_index += 1;
            item.item_id = format!("{}_msg", self.response_id);
            let added = json!({
                "type": "response.output_item.added",
                "output_index": item.index,
                "item": {
                    "id": item.item_id,
                    "type": "message",
                    "status": "in_progress",
                    "role": "assistant",
                    "content": [],
                },
            });
            lines.push(format!("event: response.output_item.added\ndata: {}\n\n", added));
            let part = json!({
                "type": "response.content_part.added",
                "item_id": item.item_id,
                "output_index": item.index,
                "content_index": 0,
                "part": {"type": "output_text", "text": "", "annotations": []},
            });
            lines.push(format!("event: response.content_part.added\ndata: {}\n\n", part));
        }

        item.text.push_str(delta);
        let event = json!({
            "type": "response.output_text.delta",
            "item_id": item.item_id,
            "output_index": item.index,
            "content_index": 0,
            "delta": delta,
        });
        lines.push(format!("event: response.output_text.delta\ndata: {}\n\n", event));
    }

    /// Announce the reasoning item (only when reasoning content is produced).
    fn push_reasoning_delta(&mut self, delta: &str, lines: &mut Vec<String>) {
        let item = &mut self.reasoning;
        if !item.added {
            item.added = true;
            item.index = self.output_index;
            self.output_index += 1;
            item.item_id = format!("rs_{}", self.response_id);
            let added = json!({
                "type": "response.output_item.added",
                "output_index": item.index,
                "item": {
                    "id": item.item_id,
                    "type": "reasoning",
                    "status": "in_progress",
                    "summary": [],
                },
            });
            lines.push(format!("event: response.output_item.added\ndata: {}\n\n", added));
            let part = json!({
                "type": "response.reasoning_summary_part.added",
                "item_id": item.item_id,
                "output_index": item.index,
                "summary_index": 0,
                "part": {"type": "summary_text", "text": ""},
            });
            lines.push(format!("event: response.reasoning_summary_part.added\ndata: {}\n\n", part));
        }

        item.text.push_str(delta);
        let event = json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": item.item_id,
            "output_index": item.index,
            "summary_index": 0,
            "delta": delta,
        });
        lines.push(format!("event: response.reasoning_summary_text.delta\ndata: {}\n\n", event));
    }

    /// Mirror Chat `delta.tool_calls` fragments as Responses function_call items.
    fn process_tool_calls(&mut self, tcs: &[Value], lines: &mut Vec<String>) {
        for tc in tcs {
            let index = tc.get("index").and_then(|i| i.as_i64()).unwrap_or(0) as usize;
            let tc_delta = tc.get("function").unwrap_or(&Value::Null);
            let name = tc_delta.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = tc_delta.get("arguments").and_then(|a| a.as_str()).unwrap_or("");

            if self.tools.len() <= index {
                self.tools.resize(index + 1, ToolCallState::default());
            }

            let call_id = tc
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();

            let needs_add = {
                let call = &mut self.tools[index];
                if !call_id.is_empty() && call.call_id.is_empty() {
                    call.call_id = call_id;
                }
                if !name.is_empty() && call.name.is_empty() {
                    call.name = name.to_string();
                }
                if !args.is_empty() {
                    call.args.push_str(args);
                }
                !call.added && (!call.call_id.is_empty() || !call.name.is_empty())
            };

            if needs_add {
                let call = &mut self.tools[index];
                call.item_id = format!("fc_{}", call.call_id);
                call.index = self.output_index;
                self.output_index += 1;
                call.added = true;
                let item = json!({
                    "id": call.item_id,
                    "type": "function_call",
                    "status": "in_progress",
                    "call_id": call.call_id,
                    "name": call.name,
                    "arguments": "",
                });
                let event = json!({
                    "type": "response.output_item.added",
                    "output_index": call.index,
                    "item": item,
                });
                lines.push(format!("event: response.output_item.added\ndata: {}\n\n", event));
            }

            // Argument fragments -> function_call_arguments.delta
            if !args.is_empty() {
                let (item_id, call_idx) = {
                    let call = &self.tools[index];
                    (call.item_id.clone(), call.index)
                };
                let event = json!({
                    "type": "response.function_call_arguments.delta",
                    "item_id": item_id,
                    "output_index": call_idx,
                    "delta": args,
                });
                lines.push(format!("event: response.function_call_arguments.delta\ndata: {}\n\n", event));
            }
        }
    }

    /// Usage object for the created/completed snapshots.
    fn responses_usage(&self, usage: Option<(i64, i64, i64)>) -> Value {
        let (p, c, t) = usage.unwrap_or((0, 0, 0));
        json!({
            "input_tokens": p,
            "output_tokens": c,
            "total_tokens": t,
        })
    }

    /// Flush the deferred completion event and terminal `[DONE]` line.
    ///
    /// Call this when the upstream stream ends (EOF or `[DONE]`), so clients
    /// receive the final `response.completed` even when the upstream did not
    /// send a separate usage chunk. Returns the lines to forward (possibly
    /// empty if completion was already emitted).
    pub fn finish(&mut self) -> Vec<String> {
        if self.completion_sent {
            return Vec::new();
        }
        let mut lines = Vec::new();
        // Only emit a completion for streams that actually produced content
        // or finished normally; a hard error is surfaced via `process`.
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

        // Build the final `output[]` snapshot ordered by output_index, and the
        // close-out events for each started item.
        let mut items: Vec<(u32, Value)> = Vec::new();

        for call in &self.tools {
            if call.added {
                let completed = json!({
                    "id": call.item_id,
                    "type": "function_call",
                    "status": "completed",
                    "call_id": call.call_id,
                    "name": call.name,
                    "arguments": call.args,
                });
                // The SDK expects the arguments stream to be closed before the
                // item is marked done.
                lines.push(format!(
                    "event: response.function_call_arguments.done\ndata: {}\n\n",
                    json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": call.item_id,
                        "output_index": call.index,
                        "arguments": call.args,
                    })
                ));
                let event = json!({
                    "type": "response.output_item.done",
                    "output_index": call.index,
                    "item": completed,
                });
                lines.push(format!("event: response.output_item.done\ndata: {}\n\n", event));
                items.push((call.index, completed));
            }
        }

        if self.msg.added && !self.msg.done {
            self.msg.done = true;
            let text = self.msg.text.clone();
            let item = json!({
                "id": self.msg.item_id,
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text, "annotations": []}],
            });
            lines.push(format!(
                "event: response.output_text.done\ndata: {}\n\n",
                json!({
                    "type": "response.output_text.done",
                    "item_id": self.msg.item_id,
                    "output_index": self.msg.index,
                    "content_index": 0,
                    "text": text,
                })
            ));
            lines.push(format!(
                "event: response.content_part.done\ndata: {}\n\n",
                json!({
                    "type": "response.content_part.done",
                    "item_id": self.msg.item_id,
                    "output_index": self.msg.index,
                    "content_index": 0,
                    "part": {"type": "output_text", "text": text, "annotations": []},
                })
            ));
            lines.push(format!(
                "event: response.output_item.done\ndata: {}\n\n",
                json!({
                    "type": "response.output_item.done",
                    "output_index": self.msg.index,
                    "item": item,
                })
            ));
            items.push((self.msg.index, item));
        }

        if self.reasoning.added && !self.reasoning.done {
            self.reasoning.done = true;
            let text = self.reasoning.text.clone();
            let item = json!({
                "id": self.reasoning.item_id,
                "type": "reasoning",
                "status": "completed",
                "summary": [{"type": "summary_text", "text": text}],
            });
            lines.push(format!(
                "event: response.reasoning_summary_text.done\ndata: {}\n\n",
                json!({
                    "type": "response.reasoning_summary_text.done",
                    "item_id": self.reasoning.item_id,
                    "output_index": self.reasoning.index,
                    "summary_index": 0,
                    "text": text,
                })
            ));
            lines.push(format!(
                "event: response.reasoning_summary_part.done\ndata: {}\n\n",
                json!({
                    "type": "response.reasoning_summary_part.done",
                    "item_id": self.reasoning.item_id,
                    "output_index": self.reasoning.index,
                    "summary_index": 0,
                    "part": {"type": "summary_text", "text": text},
                })
            ));
            lines.push(format!(
                "event: response.output_item.done\ndata: {}\n\n",
                json!({
                    "type": "response.output_item.done",
                    "output_index": self.reasoning.index,
                    "item": item,
                })
            ));
            items.push((self.reasoning.index, item));
        }

        items.sort_by_key(|(i, _)| *i);
        let output: Vec<Value> = items.into_iter().map(|(_, v)| v).collect();

        let event = json!({
            "type": "response.completed",
            "response": {
                "id": self.response_id,
                "object": "response",
                "created_at": self.created_at,
                "status": "completed",
                "model": self.display_name,
                "output": output,
                "usage": self.responses_usage(self.last_usage),
            }
        });
        lines.push(format!("event: response.completed\ndata: {}\n\n", event));
        lines.push("data: [DONE]\n\n".to_string());
        lines
    }
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
    fn test_responses_to_chat_developer_role_and_input_text_parts() {
        // Mirrors a Codex desktop request: developer message carries content as
        // Responses-native `input_text` parts, which Chat upstreams reject.
        let body = json!({
            "model": "gpt-4o",
            "input": [
                {"type": "message", "role": "developer", "content": [
                    {"type": "input_text", "text": "<app-context>system prompt</app-context>"}
                ]},
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "你好"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,xxx"}}
                ]}
            ]
        });
        let out = responses_to_chat(&body).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "<app-context>system prompt</app-context>");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "你好");
    }

    #[test]
    fn test_responses_content_to_chat_string_passthrough() {
        assert_eq!(responses_content_to_chat(Value::String("hi".into())), Value::String("hi".into()));
    }

    #[test]
    fn test_responses_content_to_chat_drops_media_parts() {
        let content = json!([
            {"type": "input_text", "text": "one"},
            {"type": "image_url", "image_url": {"url": "x"}},
            {"type": "output_text", "text": "two"}
        ]);
        assert_eq!(responses_content_to_chat(content), Value::String("one\ntwo".into()));
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

    #[test]
    fn test_chat_to_responses_request_basic() {
        let body = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hi there"}
            ],
            "max_tokens": 200,
            "temperature": 0.5,
            "stream": true,
            "reasoning_effort": "high"
        });
        let out = chat_to_responses_request(&body).unwrap();
        assert_eq!(out["model"], "gpt-5");
        assert_eq!(out["instructions"], "You are helpful");
        assert_eq!(out["max_output_tokens"], 200);
        assert_eq!(out["temperature"], 0.5);
        assert_eq!(out["stream"], true);
        assert_eq!(out["reasoning"]["effort"], "high");
        let input = out["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][0]["text"], "Hi there");
    }

    #[test]
    fn test_chat_to_responses_request_conversation_with_tools() {
        let body = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "user", "content": "What's the weather?"},
                {"role": "assistant", "content": "", "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"SF\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "72F"}
            ],
            "tools": [{
                "type": "function",
                "function": {"name": "get_weather", "description": "weather", "parameters": {"type": "object"}}
            }]
        });
        let out = chat_to_responses_request(&body).unwrap();
        let input = out["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[1]["name"], "get_weather");
        assert_eq!(input[1]["arguments"], "{\"city\":\"SF\"}");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["output"], "72F");
        let tools = out["tools"].as_array().unwrap();
        assert_eq!(tools[0]["name"], "get_weather");
        assert_eq!(tools[0]["type"], "function");
    }

    #[test]
    fn test_responses_response_to_chat_basic() {
        let resp = json!({
            "id": "resp_1",
            "object": "response",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Hello world"}]
            }],
            "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}
        });
        let out = responses_response_to_chat(&resp, "My Pool");
        assert_eq!(out["object"], "chat.completion");
        assert_eq!(out["model"], "My Pool");
        assert_eq!(out["choices"][0]["message"]["content"], "Hello world");
        assert_eq!(out["choices"][0]["finish_reason"], "stop");
        assert_eq!(out["usage"]["prompt_tokens"], 10);
        assert_eq!(out["usage"]["completion_tokens"], 5);
        assert_eq!(out["usage"]["total_tokens"], 15);
    }

    #[test]
    fn test_responses_response_to_chat_incomplete_and_tools() {
        let resp = json!({
            "id": "resp_2",
            "object": "response",
            "status": "incomplete",
            "output": [
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "think"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": ""}]},
                {"type": "function_call", "call_id": "call_9", "name": "get_weather", "arguments": "{\"city\":\"NYC\"}"}
            ],
            "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
        });
        let out = responses_response_to_chat(&resp, "M");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
        let msg = &out["choices"][0]["message"];
        assert_eq!(msg["reasoning_content"], "think");
        assert_eq!(msg["content"], "");
        let tc = msg["tool_calls"].as_array().unwrap();
        assert_eq!(tc[0]["function"]["name"], "get_weather");
        assert_eq!(tc[0]["function"]["arguments"], "{\"city\":\"NYC\"}");
    }

    #[test]
    fn test_responses_stream_converter_deferred_completion() {
        let mut conv = ResponsesStreamConverter::new("Pool");
        let chunk = r#"{"id":"x","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"#;
        let (lines, usage, error) = conv.process(chunk);
        assert!(error.is_none());
        assert!(usage.is_none());
        let joined = lines.join("");
        // SDK lifecycle: `response.created` + `response.in_progress` must
        // precede every delta; the message item is scaffolded before its delta.
        assert!(joined.starts_with("event: response.created"), "got: {joined}");
        assert!(joined.contains("event: response.in_progress"), "got: {joined}");
        assert!(joined.contains("response.output_item.added"), "scaffold missing: {joined}");
        assert!(joined.contains("response.content_part.added"), "scaffold missing: {joined}");
        assert!(joined.contains("response.output_text.delta"), "got: {joined}");
        assert!(joined.contains(r#""delta":"Hi""#), "text missing");

        // Finish chunk WITHOUT usage: `response.completed` must be deferred.
        let done = r#"{"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
        let (lines2, usage2, error2) = conv.process(done);
        assert!(error2.is_none());
        assert!(usage2.is_none());
        assert!(
            lines2.join("").is_empty(),
            "completion deferred until usage, got: {}",
            lines2.join("")
        );

        // Trailing usage-only chunk flushes the completion with real usage.
        let usage_chunk = r#"{"id":"x","choices":[],"usage":{"prompt_tokens":7,"completion_tokens":4}}"#;
        let (lines3, usage3, error3) = conv.process(usage_chunk);
        assert!(error3.is_none());
        assert_eq!(usage3, Some((7, 4, 11)));
        let joined3 = lines3.join("");
        assert!(joined3.contains("response.output_text.done"), "got: {joined3}");
        assert!(joined3.contains("response.content_part.done"), "got: {joined3}");
        assert!(joined3.contains("response.output_item.done"), "got: {joined3}");
        assert!(joined3.contains("response.completed"), "got: {joined3}");
        assert!(joined3.contains(r#""input_tokens":7"#), "usage missing: {joined3}");
        assert!(joined3.contains(r#""output_tokens":4"#), "usage missing: {joined3}");
        assert!(joined3.contains("data: [DONE]"), "got: {joined3}");
        // The completed snapshot must carry the accumulated text.
        assert!(
            joined3.contains(r#""text":"Hi""#),
            "completed snapshot text missing: {joined3}"
        );

        // finish() after completion already sent: no duplicate [DONE].
        let tail = conv.finish();
        assert!(tail.is_empty(), "no double [DONE] expected: {}", tail.join(""));
    }

    #[test]
    fn test_responses_stream_converter_tool_calls() {
        let mut conv = ResponsesStreamConverter::new("Pool");
        let start = r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#;
        let (lines, _, error) = conv.process(start);
        assert!(error.is_none());
        let joined = lines.join("");
        assert!(joined.starts_with("event: response.created"), "got: {joined}");
        assert!(joined.contains("response.output_item.added"), "got: {joined}");
        assert!(joined.contains(r#""name":"get_weather""#), "name missing: {joined}");

        let frag = r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\":\"SF\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let (lines2, _, error2) = conv.process(frag);
        assert!(error2.is_none());
        let joined2 = lines2.join("");
        assert!(
            joined2.contains("response.function_call_arguments.delta"),
            "got: {joined2}"
        );
        assert!(!joined2.contains("response.completed"), "deferred: {joined2}");

        let usage_chunk = r#"{"id":"x","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#;
        let (lines3, _, error3) = conv.process(usage_chunk);
        assert!(error3.is_none());
        let joined3 = lines3.join("");
        assert!(joined3.contains("response.function_call_arguments.done"), "got: {joined3}");
        assert!(joined3.contains("response.output_item.done"), "got: {joined3}");
        assert!(joined3.contains(r#""name":"get_weather""#), "name missing: {joined3}");
        assert!(
            joined3.contains(r#""arguments":"{\"city\":\"SF\"}""#),
            "accumulated args missing: {joined3}"
        );
        assert!(
            joined3.contains(r#""id":"fc_call_1""#),
            "stable item_id from call_id missing: {joined3}"
        );
        assert!(joined3.contains("response.completed"), "got: {joined3}");
    }

    #[test]
    fn test_responses_stream_converter_finish_flushes_without_usage() {
        // Upstream that never sends a usage chunk: finish() must still flush
        // the completion so the client is not left hanging.
        let mut conv = ResponsesStreamConverter::new("Pool");
        let done = r#"{"id":"x","choices":[{"index":0,"delta":{"content":"Bye"},"finish_reason":"stop"}]}"#;
        let (lines, _, _) = conv.process(done);
        assert!(lines.join("").contains("response.output_text.delta"));
        let tail = conv.finish();
        let joined = tail.join("");
        assert!(joined.contains("response.completed"), "got: {joined}");
        assert!(joined.contains("data: [DONE]"), "got: {joined}");
    }
}
