//! Wire fork: the Anthropic Messages API as a model provider
//! (`wire_api = "anthropic"`; rittmananalytics/wire#408, roadmap phase 3).
//!
//! Codex builds a Responses API request and parses a Responses event stream.
//! For an Anthropic provider the request body is translated here into a
//! Messages API body (POST /messages), and Anthropic's event stream is turned
//! back into Responses events, so the rest of Codex is unchanged: the same SSE
//! parser, the same tools, the same history. This replaces the Wire CLI's
//! LiteLLM bridge and its shim; the rules below are the ones the shim needed.
//!
//! Request rules:
//! - developer and system messages before the first user turn become `system`;
//!   later ones become user text (Anthropic has one system prompt).
//! - function calls become `tool_use`, their outputs `tool_result`; a namespaced
//!   tool (`multi_agent_v1.spawn_agent`) is flattened to `multi_agent_v1__spawn_agent`
//!   and split back on the way in. A custom (freeform) tool takes one string,
//!   `input`. Built-in OpenAI tools (web_search and the like) are dropped.
//! - reasoning items are dropped (they carry OpenAI-encrypted content).
//! - roles alternate, the conversation starts and ends with a user turn (Anthropic
//!   refuses an assistant prefill), every tool_use has its tool_result next.
//! - prompt caching: breakpoints on the tools, the system prompt and the last two
//!   user turns.
//! - a JSON schema for the final message (`--output-schema`) is stated in the
//!   system prompt.

use bytes::Bytes;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const MESSAGES_PATH: &str = "/messages";
const NAMESPACE_SEPARATOR: &str = "__";
const DEFAULT_MAX_TOKENS: u64 = 32_000;

/// What the stream adapter needs to know about the request.
#[derive(Clone, Debug, Default)]
pub struct StreamContext {
    /// Tool names Anthropic sees for freeform (custom) tools.
    custom_tools: HashSet<String>,
    /// Flattened name -> (namespace, name).
    namespaced: HashMap<String, (String, String)>,
}

pub struct Translation {
    pub body: Value,
    pub context: StreamContext,
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn image_block(url: &str) -> Option<Value> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        let media_type = meta.split(';').next().unwrap_or("image/png");
        return Some(json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}}));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(json!({"type": "image", "source": {"type": "url", "url": url}}));
    }
    None
}

/// Message content parts (input_text, output_text, input_image) as Anthropic blocks.
fn content_blocks(content: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    match content {
        Value::String(s) if !s.is_empty() => out.push(json!({"type": "text", "text": s})),
        Value::Array(parts) => {
            for p in parts {
                match p.get("type").and_then(Value::as_str) {
                    Some("input_text") | Some("output_text") | Some("text") => {
                        if let Some(t) = p.get("text").and_then(Value::as_str)
                            && !t.is_empty()
                        {
                            out.push(json!({"type": "text", "text": t}));
                        }
                    }
                    Some("input_image") => {
                        if let Some(b) = p.get("image_url").and_then(Value::as_str).and_then(image_block) {
                            out.push(b);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

/// A function or custom tool call's output as a tool_result's content.
fn tool_result_content(output: &Value) -> Value {
    match output {
        Value::String(s) => Value::String(if s.is_empty() { "(no output)".into() } else { s.clone() }),
        Value::Array(_) => {
            let blocks = content_blocks(output);
            if blocks.is_empty() { Value::String("(no output)".into()) } else { Value::Array(blocks) }
        }
        Value::Object(o) => match o.get("content") {
            Some(c) => tool_result_content(c),
            None => Value::String(output.to_string()),
        },
        _ => Value::String("(no output)".into()),
    }
}

fn push_block(messages: &mut Vec<Value>, role: &str, block: Value) {
    if let Some(last) = messages.last_mut()
        && last.get("role").and_then(Value::as_str) == Some(role)
        && let Some(arr) = last.get_mut("content").and_then(Value::as_array_mut)
    {
        arr.push(block);
        return;
    }
    messages.push(json!({"role": role, "content": [block]}));
}

fn flat_name(namespace: Option<&str>, name: &str) -> String {
    match namespace {
        Some(ns) if !ns.is_empty() => format!("{ns}{NAMESPACE_SEPARATOR}{name}"),
        _ => name.to_string(),
    }
}

fn tools_of(tools: &[Value], ctx: &mut StreamContext) -> Vec<Value> {
    let mut out = Vec::new();
    for t in tools {
        match t.get("type").and_then(Value::as_str) {
            Some("function") => {
                let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
                out.push(json!({
                    "name": name,
                    "description": t.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema": t.get("parameters").cloned().unwrap_or(json!({"type": "object", "properties": {}})),
                }));
            }
            Some("namespace") => {
                let ns = t.get("name").and_then(Value::as_str).unwrap_or_default();
                for sub in t.get("tools").and_then(Value::as_array).cloned().unwrap_or_default() {
                    if sub.get("type").and_then(Value::as_str) != Some("function") {
                        continue;
                    }
                    let name = sub.get("name").and_then(Value::as_str).unwrap_or_default();
                    let flat = flat_name(Some(ns), name);
                    ctx.namespaced.insert(flat.clone(), (ns.to_string(), name.to_string()));
                    out.push(json!({
                        "name": flat,
                        "description": sub.get("description").cloned().unwrap_or(Value::String(String::new())),
                        "input_schema": sub.get("parameters").cloned().unwrap_or(json!({"type": "object", "properties": {}})),
                    }));
                }
            }
            Some("custom") => {
                let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
                ctx.custom_tools.insert(name.to_string());
                let mut description = t.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
                if let Some(def) = t.get("format").and_then(|f| f.get("definition")).and_then(Value::as_str) {
                    description.push_str("\n\nThe input follows this grammar:\n");
                    description.push_str(def);
                }
                out.push(json!({
                    "name": name,
                    "description": description,
                    "input_schema": {"type": "object", "properties": {"input": {"type": "string", "description": "The tool's raw input."}}, "required": ["input"]},
                }));
            }
            _ => {} // OpenAI built-ins (web_search, image_generation, ...) have no Anthropic form here
        }
    }
    out
}

/// Every tool_use is answered in the next user turn; a tool_result with no
/// tool_use before it becomes text. Tool results lead their user message.
fn repair(messages: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for mut m in messages {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("user").to_string();
        let blocks = m.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
        if role == "assistant" {
            if !pending.is_empty() {
                let results: Vec<Value> = pending
                    .drain(..)
                    .map(|id| json!({"type": "tool_result", "tool_use_id": id, "content": "(no result recorded)"}))
                    .collect();
                out.push(json!({"role": "user", "content": results}));
            }
            for b in &blocks {
                if b.get("type").and_then(Value::as_str) == Some("tool_use")
                    && let Some(id) = b.get("id").and_then(Value::as_str)
                {
                    pending.push(id.to_string());
                }
            }
            out.push(m);
            continue;
        }
        let mut results = Vec::new();
        let mut others = Vec::new();
        for b in blocks {
            if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or_default().to_string();
                if let Some(pos) = pending.iter().position(|p| *p == id) {
                    pending.remove(pos);
                    results.push(b);
                } else {
                    let text = text_of(b.get("content").unwrap_or(&Value::Null));
                    others.push(json!({"type": "text", "text": format!("(tool output) {text}")}));
                }
            } else {
                others.push(b);
            }
        }
        for id in pending.drain(..) {
            results.push(json!({"type": "tool_result", "tool_use_id": id, "content": "(no result recorded)"}));
        }
        results.extend(others);
        if results.is_empty() {
            continue;
        }
        m["content"] = Value::Array(results);
        out.push(m);
    }
    if !pending.is_empty() {
        let results: Vec<Value> = pending
            .drain(..)
            .map(|id| json!({"type": "tool_result", "tool_use_id": id, "content": "(no result recorded)"}))
            .collect();
        out.push(json!({"role": "user", "content": results}));
    }
    // merge any two neighbours with the same role
    let mut merged: Vec<Value> = Vec::new();
    for m in out {
        if let Some(last) = merged.last_mut()
            && last.get("role") == m.get("role")
        {
            let extra = m.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
            if let Some(arr) = last.get_mut("content").and_then(Value::as_array_mut) {
                arr.extend(extra);
            }
            continue;
        }
        merged.push(m);
    }
    if merged.first().and_then(|m| m.get("role")).and_then(Value::as_str) != Some("user") {
        merged.insert(0, json!({"role": "user", "content": [{"type": "text", "text": "Continue."}]}));
    }
    if merged.last().and_then(|m| m.get("role")).and_then(Value::as_str) != Some("user") {
        merged.push(json!({"role": "user", "content": [{"type": "text", "text": "Continue."}]}));
    }
    merged
}

fn mark_cache(block: &mut Value) {
    if let Some(o) = block.as_object_mut() {
        o.insert("cache_control".into(), json!({"type": "ephemeral"}));
    }
}

/// The Messages API body for a Responses API request.
pub fn to_messages(request: &Value, max_tokens: Option<u64>) -> Translation {
    let mut ctx = StreamContext::default();
    let mut system: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    if let Some(instructions) = request.get("instructions").and_then(Value::as_str)
        && !instructions.is_empty()
    {
        system.push(instructions.to_string());
    }
    let tools = tools_of(
        request.get("tools").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]),
        &mut ctx,
    );
    for item in request.get("input").and_then(Value::as_array).cloned().unwrap_or_default() {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("message");
        match kind {
            "message" => {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let content = item.get("content").cloned().unwrap_or(Value::Null);
                match role {
                    "developer" | "system" => {
                        let text = text_of(&content);
                        if text.is_empty() {
                            continue;
                        }
                        if messages.is_empty() {
                            system.push(text);
                        } else {
                            push_block(&mut messages, "user", json!({"type": "text", "text": text}));
                        }
                    }
                    "assistant" => {
                        for b in content_blocks(&content) {
                            push_block(&mut messages, "assistant", b);
                        }
                    }
                    _ => {
                        for b in content_blocks(&content) {
                            push_block(&mut messages, "user", b);
                        }
                    }
                }
            }
            "function_call" => {
                let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
                let ns = item.get("namespace").and_then(Value::as_str);
                let args = item.get("arguments").and_then(Value::as_str).unwrap_or("{}");
                let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({"raw": args}));
                let input = if input.is_object() { input } else { json!({"value": input}) };
                let id = item.get("call_id").and_then(Value::as_str).unwrap_or_default();
                push_block(&mut messages, "assistant", json!({"type": "tool_use", "id": id, "name": flat_name(ns, name), "input": input}));
            }
            "custom_tool_call" => {
                let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
                let id = item.get("call_id").and_then(Value::as_str).unwrap_or_default();
                let input = item.get("input").and_then(Value::as_str).unwrap_or_default();
                push_block(&mut messages, "assistant", json!({"type": "tool_use", "id": id, "name": name, "input": {"input": input}}));
            }
            "function_call_output" | "custom_tool_call_output" => {
                let id = item.get("call_id").and_then(Value::as_str).unwrap_or_default();
                let content = tool_result_content(item.get("output").unwrap_or(&Value::Null));
                push_block(&mut messages, "user", json!({"type": "tool_result", "tool_use_id": id, "content": content}));
            }
            _ => {} // reasoning, web search and other OpenAI-only items
        }
    }
    let mut messages = repair(messages);

    if let Some(schema) = request.get("text").and_then(|t| t.get("format")).and_then(|f| f.get("schema")) {
        system.push(format!(
            "When the work is done, your final message is only a JSON object that matches this JSON Schema, with no other text:\n{schema}"
        ));
    }

    // prompt caching: the last two user turns
    let user_idx: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.get("role").and_then(Value::as_str) == Some("user"))
        .map(|(i, _)| i)
        .collect();
    for &i in user_idx.iter().rev().take(2) {
        if let Some(last) = messages[i].get_mut("content").and_then(Value::as_array_mut).and_then(|a| a.last_mut()) {
            mark_cache(last);
        }
    }

    let mut body = Map::new();
    body.insert("model".into(), request.get("model").cloned().unwrap_or(Value::Null));
    body.insert("max_tokens".into(), json!(max_tokens.unwrap_or(DEFAULT_MAX_TOKENS)));
    body.insert("stream".into(), Value::Bool(true));
    if !system.is_empty() {
        let mut block = json!({"type": "text", "text": system.join("\n\n")});
        mark_cache(&mut block);
        body.insert("system".into(), json!([block]));
    }
    body.insert("messages".into(), Value::Array(messages));
    if !tools.is_empty() {
        let mut tools = tools;
        if let Some(last) = tools.last_mut() {
            mark_cache(last);
        }
        body.insert("tools".into(), Value::Array(tools));
        let parallel = request.get("parallel_tool_calls").and_then(Value::as_bool).unwrap_or(true);
        let choice = match request.get("tool_choice").and_then(Value::as_str).unwrap_or("auto") {
            "none" => json!({"type": "none"}),
            "required" => json!({"type": "any", "disable_parallel_tool_use": !parallel}),
            _ => json!({"type": "auto", "disable_parallel_tool_use": !parallel}),
        };
        body.insert("tool_choice".into(), choice);
    }
    Translation { body: Value::Object(body), context: ctx }
}

// ---------------------------------------------------------------- the stream

#[derive(Debug)]
enum Block {
    Text(String),
    ToolUse { id: String, name: String, json: String },
    Other,
}

/// Anthropic's event stream in, Responses API event text out.
pub struct StreamAdapter {
    ctx: StreamContext,
    buffer: String,
    blocks: HashMap<u64, Block>,
    response_id: String,
    input_tokens: i64,
    cache_read: i64,
    cache_write: i64,
    output_tokens: i64,
    stop_reason: Option<String>,
    finished: bool,
}

fn event(kind: &str, data: Value) -> String {
    format!("event: {kind}\ndata: {data}\n\n")
}

impl StreamAdapter {
    pub fn new(ctx: StreamContext) -> Self {
        Self {
            ctx,
            buffer: String::new(),
            blocks: HashMap::new(),
            response_id: "msg".into(),
            input_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            output_tokens: 0,
            stop_reason: None,
            finished: false,
        }
    }

    /// One chunk of the HTTP body; returns the Responses events it completes.
    pub fn feed(&mut self, chunk: &[u8]) -> String {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut out = String::new();
        loop {
            let normalised = self.buffer.replace("\r\n", "\n");
            let Some(end) = normalised.find("\n\n") else {
                self.buffer = normalised;
                break;
            };
            let frame = normalised[..end].to_string();
            self.buffer = normalised[end + 2..].to_string();
            let data: String = frame
                .lines()
                .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                out.push_str(&self.on_event(&v));
            }
        }
        out
    }

    fn usage(&self) -> Value {
        let input = self.input_tokens + self.cache_read + self.cache_write;
        json!({
            "input_tokens": input,
            "input_tokens_details": {"cached_tokens": self.cache_read, "cache_write_tokens": self.cache_write},
            "output_tokens": self.output_tokens,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": input + self.output_tokens,
        })
    }

    fn on_event(&mut self, v: &Value) -> String {
        let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "message_start" => {
                let m = v.get("message").cloned().unwrap_or(Value::Null);
                if let Some(id) = m.get("id").and_then(Value::as_str) {
                    self.response_id = id.to_string();
                }
                let u = m.get("usage").cloned().unwrap_or(Value::Null);
                self.input_tokens = u.get("input_tokens").and_then(Value::as_i64).unwrap_or(0);
                self.cache_read = u.get("cache_read_input_tokens").and_then(Value::as_i64).unwrap_or(0);
                self.cache_write = u.get("cache_creation_input_tokens").and_then(Value::as_i64).unwrap_or(0);
                self.output_tokens = u.get("output_tokens").and_then(Value::as_i64).unwrap_or(0);
                event("response.created", json!({"type": "response.created", "response": {"id": self.response_id}}))
            }
            "content_block_start" => {
                let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                let b = v.get("content_block").cloned().unwrap_or(Value::Null);
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        self.blocks.insert(idx, Block::Text(b.get("text").and_then(Value::as_str).unwrap_or_default().to_string()));
                        event(
                            "response.output_item.added",
                            json!({"type": "response.output_item.added",
                                   "item": {"type": "message", "role": "assistant", "id": format!("{}_{idx}", self.response_id), "content": []}}),
                        )
                    }
                    Some("tool_use") => {
                        self.blocks.insert(
                            idx,
                            Block::ToolUse {
                                id: b.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                                name: b.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                                json: String::new(),
                            },
                        );
                        String::new()
                    }
                    _ => {
                        self.blocks.insert(idx, Block::Other);
                        String::new()
                    }
                }
            }
            "content_block_delta" => {
                let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                let d = v.get("delta").cloned().unwrap_or(Value::Null);
                match (self.blocks.get_mut(&idx), d.get("type").and_then(Value::as_str)) {
                    (Some(Block::Text(acc)), Some("text_delta")) => {
                        let t = d.get("text").and_then(Value::as_str).unwrap_or_default();
                        acc.push_str(t);
                        event("response.output_text.delta", json!({"type": "response.output_text.delta", "delta": t}))
                    }
                    (Some(Block::ToolUse { json, .. }), Some("input_json_delta")) => {
                        json.push_str(d.get("partial_json").and_then(Value::as_str).unwrap_or_default());
                        String::new()
                    }
                    _ => String::new(),
                }
            }
            "content_block_stop" => {
                let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                match self.blocks.remove(&idx) {
                    Some(Block::Text(text)) => event(
                        "response.output_item.done",
                        json!({"type": "response.output_item.done",
                               "item": {"type": "message", "role": "assistant", "id": format!("{}_{idx}", self.response_id),
                                        "content": [{"type": "output_text", "text": text}]}}),
                    ),
                    Some(Block::ToolUse { id, name, json }) => {
                        let args = if json.trim().is_empty() { "{}".to_string() } else { json };
                        let item = if self.ctx.custom_tools.contains(&name) {
                            let input = serde_json::from_str::<Value>(&args)
                                .ok()
                                .and_then(|v| v.get("input").and_then(Value::as_str).map(str::to_owned))
                                .unwrap_or(args);
                            json!({"type": "custom_tool_call", "call_id": id, "name": name, "input": input})
                        } else if let Some((ns, short)) = self.ctx.namespaced.get(&name) {
                            json!({"type": "function_call", "call_id": id, "name": short, "namespace": ns, "arguments": args})
                        } else {
                            json!({"type": "function_call", "call_id": id, "name": name, "arguments": args})
                        };
                        event("response.output_item.done", json!({"type": "response.output_item.done", "item": item}))
                    }
                    _ => String::new(),
                }
            }
            "message_delta" => {
                if let Some(r) = v.get("delta").and_then(|d| d.get("stop_reason")).and_then(Value::as_str) {
                    self.stop_reason = Some(r.to_string());
                }
                if let Some(o) = v.get("usage").and_then(|u| u.get("output_tokens")).and_then(Value::as_i64) {
                    self.output_tokens = o;
                }
                String::new()
            }
            "message_stop" => {
                self.finished = true;
                if self.stop_reason.as_deref() == Some("max_tokens") {
                    return event(
                        "response.incomplete",
                        json!({"type": "response.incomplete",
                               "response": {"id": self.response_id, "usage": self.usage(),
                                            "incomplete_details": {"reason": "max_output_tokens"}}}),
                    );
                }
                let end_turn = self.stop_reason.as_deref() != Some("tool_use");
                event(
                    "response.completed",
                    json!({"type": "response.completed",
                           "response": {"id": self.response_id, "usage": self.usage(), "end_turn": end_turn}}),
                )
            }
            "error" => {
                self.finished = true;
                let e = v.get("error").cloned().unwrap_or(Value::Null);
                event(
                    "response.failed",
                    json!({"type": "response.failed",
                           "response": {"id": self.response_id,
                                        "error": {"code": e.get("type").cloned().unwrap_or(Value::Null),
                                                  "message": e.get("message").cloned().unwrap_or(Value::Null)}}}),
                )
            }
            _ => String::new(), // ping
        }
    }

    pub fn finished(&self) -> bool {
        self.finished
    }
}

/// The byte stream adapter used by the Responses client.
pub fn adapt_stream(
    bytes: codex_client::ByteStream,
    ctx: StreamContext,
) -> codex_client::ByteStream {
    use futures::StreamExt;
    let mut adapter = StreamAdapter::new(ctx);
    Box::pin(bytes.map(move |chunk| chunk.map(|b| Bytes::from(adapter.feed(&b)))))
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
