//! A stdio MCP server. The shared Barista app owns power requests, not this subprocess.
use crate::connection::AgentConnection;
use crate::models::*;
use crate::usage;
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};

const VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

struct RpcError {
    code: i64,
    message: String,
}

fn rpc(code: i64, message: impl Into<String>) -> RpcError {
    RpcError {
        code,
        message: message.into(),
    }
}

pub struct McpServer {
    pub connection: AgentConnection,
}

impl McpServer {
    pub fn run(&self) {
        let stdin = std::io::stdin();
        let mut initialized = false;
        let mut reader = stdin.lock();
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
                line.pop();
            }
            let mut response_id = Value::Null;
            match self.process(&line, &mut initialized, &mut response_id) {
                Ok(Some(result)) => emit(json!({"jsonrpc": "2.0", "id": response_id, "result": result})),
                Ok(None) => {}
                Err(error) => emit(json!({"jsonrpc": "2.0", "id": response_id, "error": {"code": error.code, "message": error.message}})),
            }
        }
    }

    fn process(&self, line: &[u8], initialized: &mut bool, response_id: &mut Value) -> std::result::Result<Option<Value>, RpcError> {
        if line.len() > 1_048_576 {
            return Err(rpc(-32700, "Expected a JSON message under 1 MiB."));
        }
        let decoded: Value = serde_json::from_slice(line).map_err(|_| rpc(-32700, "Invalid JSON."))?;
        let Value::Object(request) = decoded else {
            return Err(rpc(-32600, "Expected a JSON-RPC request object."));
        };
        if let Some(id) = request.get("id") {
            if !(id.is_string() || id.is_number()) {
                return Err(rpc(-32600, "Request ID must be a string or number."));
            }
            *response_id = id.clone();
        }
        let method = match (
            request.get("jsonrpc").and_then(Value::as_str),
            request.get("method").and_then(Value::as_str),
        ) {
            (Some("2.0"), Some(method)) => method,
            _ => return Err(rpc(-32600, "Expected a JSON-RPC 2.0 request.")),
        };
        // Notifications never produce replies (including initialized/cancelled).
        if !request.contains_key("id") {
            return Ok(None);
        }
        let params = match request.get("params") {
            None => Map::new(),
            Some(Value::Object(params)) => params.clone(),
            Some(_) => return Err(rpc(-32602, "params must be an object.")),
        };
        let result = match method {
            "initialize" => {
                let Some(requested) = params.get("protocolVersion").and_then(Value::as_str) else {
                    return Err(rpc(-32602, "protocolVersion is required."));
                };
                *initialized = true;
                json!({
                    "protocolVersion": if VERSIONS.contains(&requested) { requested } else { VERSIONS[0] },
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "barista", "title": "Barista", "version": VERSION},
                    "instructions": "Keep this computer awake only during active work. Acquire a named session, renew it with heartbeats (default TTL: 300 seconds), report progress, and release your own ID when done or waiting for the user. Sessions stop on expiry, process exit, battery protection, or the user's Stop action. Never reacquire to override a user's Stop. Display stays off-friendly by default. No lid-closed sleep override."
                })
            }
            "ping" => json!({}),
            "tools/list" => {
                if !*initialized {
                    return Err(rpc(-32000, "Initialize first."));
                }
                if params.contains_key("cursor") {
                    return Err(rpc(-32602, "This server has no additional tool pages."));
                }
                json!({"tools": tools()})
            }
            "tools/call" => {
                if !*initialized {
                    return Err(rpc(-32000, "Initialize first."));
                }
                self.call(&params)?
            }
            _ => return Err(rpc(-32601, format!("Unsupported method: {method}"))),
        };
        Ok(Some(result))
    }

    fn call(&self, params: &Map<String, Value>) -> std::result::Result<Value, RpcError> {
        let tools = tools();
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let Some(tool) = tools.iter().find(|t| t["name"] == name) else {
            return Err(rpc(-32602, "Unknown tool."));
        };
        let arguments = match params.get("arguments") {
            None => Map::new(),
            Some(Value::Object(arguments)) => arguments.clone(),
            Some(_) => return Err(rpc(-32602, "arguments must be an object.")),
        };
        let properties = tool["inputSchema"]["properties"].as_object().cloned().unwrap_or_default();
        let required: Vec<&str> = tool["inputSchema"]["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !arguments.keys().all(|key| properties.contains_key(key)) || !required.iter().all(|key| arguments.contains_key(*key)) {
            return Err(rpc(-32602, format!("Unexpected or missing arguments for {name}.")));
        }
        let outcome = (|| -> Result<Value> {
            if name == "barista_usage" {
                // Read locally from the agents' logs; the app isn't involved.
                let period = match arguments.get("period") {
                    None => usage::Period::Week,
                    Some(Value::String(text)) => {
                        usage::Period::parse(text).ok_or_else(|| CoffeeError::msg("period must be today, week, month, or all."))?
                    }
                    Some(_) => return Err(CoffeeError::msg("period must be a string.")),
                };
                let report = usage::report(&[period]);
                return Ok(json!({"content": [{"type": "text", "text": to_json(&report)}], "structuredContent": report, "isError": false}));
            }
            let request = match name {
                "barista_status" => ControlRequest::new(ControlAction::Status),
                "barista_history" => ControlRequest::new(ControlAction::History),
                "barista_acquire" => {
                    let process_id = if arguments.contains_key("pid") {
                        let value = number(&arguments, "pid", 0.0)?;
                        if value <= 0.0 || value.round() != value || value > f64::from(i32::MAX) {
                            return Err(CoffeeError::msg("pid must be a positive process ID."));
                        }
                        Some(value as i32)
                    } else {
                        None
                    };
                    ControlRequest {
                        title: Some(string(&arguments, "title")?),
                        seconds: Some(number(&arguments, "ttl_seconds", 300.0)?),
                        display: Some(boolean(&arguments, "display_awake", false)?),
                        process_id,
                        max_seconds: Some(number(&arguments, "max_awake_seconds", 28800.0)?),
                        ..ControlRequest::new(ControlAction::Acquire)
                    }
                }
                "barista_heartbeat" => ControlRequest {
                    seconds: Some(number(&arguments, "ttl_seconds", 300.0)?),
                    progress: if arguments.contains_key("progress") {
                        Some(number(&arguments, "progress", 0.0)?)
                    } else {
                        None
                    },
                    message: if arguments.contains_key("message") {
                        Some(string(&arguments, "message")?)
                    } else {
                        None
                    },
                    ..ControlRequest::with_id(ControlAction::Heartbeat, string(&arguments, "id")?)
                },
                "barista_extend" => ControlRequest {
                    seconds: Some(number(&arguments, "seconds", 900.0)?),
                    ..ControlRequest::with_id(ControlAction::Extend, string(&arguments, "id")?)
                },
                "barista_release" => ControlRequest::with_id(ControlAction::Release, string(&arguments, "id")?),
                _ => return Err(CoffeeError::msg("Unknown tool.")),
            };
            let reply = self.connection.send(&request)?;
            let text = to_json(&reply);
            let structured: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            Ok(json!({"content": [{"type": "text", "text": text}], "structuredContent": structured, "isError": !reply.ok}))
        })();
        Ok(outcome.unwrap_or_else(|error| json!({"content": [{"type": "text", "text": error.to_string()}], "isError": true})))
    }
}

fn string(values: &Map<String, Value>, key: &str) -> Result<String> {
    values
        .get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| CoffeeError::msg(format!("{key} must be a string.")))
}

fn number(values: &Map<String, Value>, key: &str, fallback: f64) -> Result<f64> {
    match values.get(key) {
        None => Ok(fallback),
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| CoffeeError::msg(format!("{key} must be a finite number."))),
        Some(_) => Err(CoffeeError::msg(format!("{key} must be a finite number."))),
    }
}

fn boolean(values: &Map<String, Value>, key: &str, fallback: bool) -> Result<bool> {
    match values.get(key) {
        None => Ok(fallback),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(CoffeeError::msg(format!("{key} must be a boolean."))),
    }
}

fn emit(message: Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{}", serde_json::to_string(&message).unwrap_or_default());
    let _ = stdout.flush();
}

fn tool(name: &str, title: &str, description: &str, properties: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name, "title": title, "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required, "additionalProperties": false},
        "annotations": {"readOnlyHint": read_only, "destructiveHint": false, "idempotentHint": read_only || name == "barista_release", "openWorldHint": false}
    })
}

fn tools() -> Vec<Value> {
    let id = json!({"type": "string", "description": "The session ID returned by barista_acquire."});
    let ttl = json!({"type": "number", "exclusiveMinimum": 0, "maximum": 86400, "default": 300, "description": "Seconds until automatic expiry unless renewed. Use a short lease, then heartbeat during active work."});
    vec![
        tool("barista_status", "Inspect awake sessions", "Get awake/display state, active session owners, progress, expiry, and battery protection.", json!({}), &[], true),
        tool(
            "barista_acquire",
            "Keep the computer awake for a task",
            "Acquire your own expiring awake session. Other agents keep their independent sessions. Default: 5-minute lease, 8-hour hard limit, display may sleep.",
            json!({
                "title": {"type": "string", "minLength": 1, "maxLength": 100, "description": "A short human-readable task name shown in Barista's menu."},
                "ttl_seconds": ttl, "display_awake": {"type": "boolean", "default": false},
                "pid": {"type": "integer", "minimum": 1, "description": "Optional local process to watch; session ends when that process exits."},
                "max_awake_seconds": {"type": "number", "exclusiveMinimum": 0, "maximum": 86400, "default": 28800}
            }),
            &["title"],
            false,
        ),
        tool(
            "barista_heartbeat",
            "Renew task and report progress",
            "Renew a live session and optionally publish progress/message. An expired or user-stopped session cannot be revived.",
            json!({"id": id, "ttl_seconds": ttl, "progress": {"type": "number", "minimum": 0, "maximum": 1}, "message": {"type": "string", "maxLength": 200}}),
            &["id"],
            false,
        ),
        tool("barista_extend", "Extend a session timer", "Add seconds to an existing timer within its original hard limit.", json!({"id": id, "seconds": ttl}), &["id", "seconds"], false),
        tool(
            "barista_release",
            "Release your awake session",
            "Release your own session when work completes or waits for user input. Other sessions remain active. Repeated releases are safe.",
            json!({"id": id}),
            &["id"],
            false,
        ),
        tool("barista_history", "Read recent session completions", "Read the last 20 local session summaries and reasons they ended. No command arguments or output are recorded.", json!({}), &[], true),
        tool(
            "barista_usage",
            "Read coding-agent usage",
            "Sessions, requests, and tokens for Claude Code and Codex (from their local logs) and Cursor turns, per agent and model, with costs estimated at API list prices (not subscription charges), plus active time, sessions, tokens, models, and cost per project (Git repository). Read locally; nothing is sent anywhere.",
            json!({"period": {"type": "string", "enum": ["today", "week", "month", "all"], "default": "week", "description": "today, this week (from Monday), this month, or all time, in local time."}}),
            &[],
            true,
        ),
    ]
}
