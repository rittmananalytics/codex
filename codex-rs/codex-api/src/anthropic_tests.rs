use super::*;
use pretty_assertions::assert_eq;

fn request() -> Value {
    json!({
        "model": "claude-sonnet-5-5",
        "stream": true,
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "tools": [
            {"type": "function", "name": "exec_command", "description": "run", "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}}, "strict": false},
            {"type": "namespace", "name": "multi_agent_v1", "description": "agents", "tools": [
                {"type": "function", "name": "spawn_agent", "description": "spawn", "parameters": {"type": "object", "properties": {}}}
            ]},
            {"type": "custom", "name": "apply_patch", "description": "patch", "format": {"type": "grammar", "syntax": "lark", "definition": "start: x"}},
            {"type": "web_search", "external_web_access": false}
        ],
        "input": [
            {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "You are Codex."}]},
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "List the files."}]},
            {"type": "reasoning", "summary": [], "encrypted_content": "xyz"},
            {"type": "function_call", "call_id": "c1", "name": "exec_command", "arguments": "{\"cmd\":\"ls\"}"},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Running ls."}]},
            {"type": "function_call_output", "call_id": "c1", "output": "a.sql\nb.sql"},
            {"type": "custom_tool_call", "call_id": "c2", "name": "apply_patch", "input": "*** Begin Patch"},
            {"type": "custom_tool_call_output", "call_id": "c2", "output": "Done"},
            {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "Context changed."}]},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Finished."}]}
        ],
        "text": {"format": {"type": "json_schema", "name": "report", "schema": {"type": "object"}}}
    })
}

#[test]
fn system_tools_and_roles() {
    let t = to_messages(&request(), None);
    let b = &t.body;
    assert_eq!(b["model"], "claude-sonnet-5-5");
    assert_eq!(b["max_tokens"], 32000);
    let system = b["system"][0]["text"].as_str().unwrap();
    assert!(system.starts_with("You are Codex."));
    assert!(system.contains("JSON Schema"));
    assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
    let names: Vec<&str> = b["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["exec_command", "multi_agent_v1__spawn_agent", "apply_patch"]);
    assert_eq!(b["tools"][2]["input_schema"]["required"][0], "input");
    assert_eq!(b["tools"][2]["cache_control"]["type"], "ephemeral");
    assert_eq!(b["tool_choice"], json!({"type": "auto", "disable_parallel_tool_use": false}));
    let roles: Vec<&str> = b["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    // user, assistant (tool_use + text), user (tool_result), assistant (custom tool_use), user (result + developer note),
    // assistant (Finished.), user (Continue.: no prefill)
    assert_eq!(roles, vec!["user", "assistant", "user", "assistant", "user", "assistant", "user"]);
    let m = b["messages"].as_array().unwrap();
    assert_eq!(m[1]["content"][0], json!({"type": "tool_use", "id": "c1", "name": "exec_command", "input": {"cmd": "ls"}}));
    assert_eq!(m[1]["content"][1]["text"], "Running ls.");
    assert_eq!(m[2]["content"][0]["type"], "tool_result");
    assert_eq!(m[2]["content"][0]["content"], "a.sql\nb.sql");
    assert_eq!(m[3]["content"][0]["input"], json!({"input": "*** Begin Patch"}));
    assert_eq!(m[4]["content"][0]["tool_use_id"], "c2");
    assert_eq!(m[4]["content"][1]["text"], "Context changed.");
    assert_eq!(m[6]["content"][0]["text"], "Continue.");
    assert_eq!(m[6]["content"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(m[4]["content"][1]["cache_control"]["type"], "ephemeral");
}

#[test]
fn orphans_are_repaired() {
    let req = json!({"model": "m", "input": [
        {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "go"}]},
        {"type": "function_call", "call_id": "x", "name": "exec_command", "arguments": "{}"},
        {"type": "function_call_output", "call_id": "nobody", "output": "stray"}
    ]});
    let m = to_messages(&req, Some(1000)).body["messages"].as_array().unwrap().clone();
    assert_eq!(m.len(), 3);
    assert_eq!(m[2]["content"][0]["type"], "tool_result");
    assert_eq!(m[2]["content"][0]["content"], "(no result recorded)");
    assert_eq!(m[2]["content"][0]["tool_use_id"], "x");
    assert!(m[2]["content"][1]["text"].as_str().unwrap().contains("stray"));
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)).collect()
}

fn parse(out: &str) -> Vec<Value> {
    out.split("\n\n")
        .filter_map(|f| f.lines().find_map(|l| l.strip_prefix("data: ")))
        .map(|d| serde_json::from_str(d).unwrap())
        .collect()
}

#[test]
fn stream_text_and_tools() {
    let t = to_messages(&request(), None);
    let mut a = StreamAdapter::new(t.context);
    let body = sse(&[
        json!({"type": "message_start", "message": {"id": "msg_1", "usage": {"input_tokens": 100, "cache_read_input_tokens": 900, "cache_creation_input_tokens": 50, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hel"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "lo"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "t1", "name": "multi_agent_v1__spawn_agent", "input": {}}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"a\":"}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "1}"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "t2", "name": "apply_patch", "input": {}}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"input\":\"*** Begin Patch\"}"}}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 40}}),
        json!({"type": "message_stop"}),
    ]);
    // fed in awkward pieces, as a network delivers it
    let mut out = String::new();
    for piece in body.as_bytes().chunks(37) {
        out.push_str(&a.feed(piece));
    }
    let ev = parse(&out);
    let kinds: Vec<&str> = ev.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["response.created", "response.output_item.added", "response.output_text.delta", "response.output_text.delta",
                           "response.output_item.done", "response.output_item.done", "response.output_item.done", "response.completed"]);
    assert_eq!(ev[4]["item"]["content"][0]["text"], "Hello");
    assert_eq!(ev[5]["item"], json!({"type": "function_call", "call_id": "t1", "name": "spawn_agent", "namespace": "multi_agent_v1", "arguments": "{\"a\":1}"}));
    assert_eq!(ev[6]["item"], json!({"type": "custom_tool_call", "call_id": "t2", "name": "apply_patch", "input": "*** Begin Patch"}));
    let u = &ev[7]["response"]["usage"];
    assert_eq!(u["input_tokens"], 1050);
    assert_eq!(u["input_tokens_details"]["cached_tokens"], 900);
    assert_eq!(u["input_tokens_details"]["cache_write_tokens"], 50);
    assert_eq!(u["output_tokens"], 40);
    assert_eq!(ev[7]["response"]["end_turn"], false);
    assert!(a.finished());
}

#[test]
fn stream_max_tokens_and_error() {
    let mut a = StreamAdapter::new(StreamContext::default());
    let out = a.feed(sse(&[
        json!({"type": "message_start", "message": {"id": "m", "usage": {"input_tokens": 1}}}),
        json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 9}}),
        json!({"type": "message_stop"}),
    ]).as_bytes());
    let ev = parse(&out);
    assert_eq!(ev[1]["type"], "response.incomplete");
    assert_eq!(ev[1]["response"]["incomplete_details"]["reason"], "max_output_tokens");
    let mut b = StreamAdapter::new(StreamContext::default());
    let ev = parse(&b.feed(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n"));
    assert_eq!(ev[0]["type"], "response.failed");
    assert_eq!(ev[0]["response"]["error"]["message"], "Overloaded");
}
