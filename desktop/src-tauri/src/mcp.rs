//! Embedded MCP server for locally-running AI agents.
//!
//! RuTerm exposes a tiny Streamable-HTTP-compatible JSON-RPC endpoint on
//! `127.0.0.1:<ephemeral port>/mcp`. Agents launched in a RuTerm local PTY get
//! the endpoint + one-shot token either through `OPENCODE_CONFIG_CONTENT` or a
//! private config file under `~/.ruterm/agents/<id>/` (see `agent_launch`), so
//! no user config file is ever modified.
//!
//! The tools deliberately operate on **SSH sessions only**. A local PTY is
//! never a valid target, so an agent cannot type into its own terminal.

use crate::state::AppState;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use uuid::Uuid;

const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);

/// User's answer to a risky-command prompt.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmOutcome {
    Deny,
    Allow,
    Always,
}

/// Payload sent to the webview when an agent asks to run a risky command.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmRequest {
    pub id: String,
    pub session: String,
    pub command: String,
    pub reason: String,
}

/// Shared handle to the running bridge. Stored in Tauri state so the frontend
/// can fetch the OpenCode config and so SSH creation can update target order.
#[derive(Clone)]
pub struct Bridge {
    pub port: u16,
    pub token: String,
    recent: Arc<Mutex<Vec<String>>>,
    /// Stable address (`user@host:port`) per live SSH tab; also keys the memory file.
    labels: Arc<Mutex<HashMap<String, String>>>,
    /// Friendly saved host name shown to agents (the internal session id stays hidden).
    names: Arc<Mutex<HashMap<String, String>>>,
    target: Arc<Mutex<Option<String>>>,
    pending: Arc<Mutex<HashMap<String, Sender<ConfirmOutcome>>>>,
    /// Exact commands the user approved with "Always allow" (this launch only).
    allowlist: Arc<Mutex<HashSet<String>>>,
}

impl Bridge {
    /// Bind loopback only. The port is ephemeral so two RuTerm instances never
    /// fight over a fixed port; the token is regenerated on every launch.
    pub fn start(app: tauri::AppHandle) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("mcp bind: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("mcp addr: {e}"))?
            .port();
        let bridge = Self {
            port,
            token: Uuid::new_v4().simple().to_string(),
            recent: Arc::new(Mutex::new(Vec::new())),
            labels: Arc::new(Mutex::new(HashMap::new())),
            names: Arc::new(Mutex::new(HashMap::new())),
            target: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            allowlist: Arc::new(Mutex::new(HashSet::new())),
        };

        let runner = bridge.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let app = app.clone();
                        let bridge = runner.clone();
                        std::thread::spawn(move || serve_connection(stream, app, bridge));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(bridge)
    }

    /// Remember the order in which SSH tabs were opened, their address (used
    /// for the stable memory file) and their saved human name (shown to agents).
    pub fn note_open(&self, session_id: &str, address: &str, name: &str) {
        if let Ok(mut recent) = self.recent.lock() {
            recent.retain(|id| id != session_id);
            recent.push(session_id.to_string());
        }
        if let Ok(mut labels) = self.labels.lock() {
            labels.insert(session_id.to_string(), address.to_string());
        }
        if let Ok(mut names) = self.names.lock() {
            let friendly = if name.trim().is_empty() {
                address.to_string()
            } else {
                name.trim().to_string()
            };
            names.insert(session_id.to_string(), friendly);
        }
    }

    /// Address (`user@host:port`), kept stable so the session memory filename
    /// never changes when a host is renamed.
    fn label(&self, session_id: &str) -> String {
        self.labels
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).cloned())
            .unwrap_or_else(|| session_id.to_string())
    }

    /// Saved host name, e.g. "VPS Murah". Falls back to the address for adhoc
    /// quick-connect tabs that have no saved name.
    fn display_name(&self, session_id: &str) -> String {
        self.names
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).cloned())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.label(session_id))
    }

    /// Per-VPS markdown memory, stable across reconnects and app restarts.
    fn session_file(&self, session_id: &str) -> PathBuf {
        let label = self.label(session_id);
        let safe: String = label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
            .collect();
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".ruterm")
            .join("sessions")
            .join(format!("{safe}.md"))
    }

    /// Append either a work-log entry or an agent note. Work-log entries are
    /// inserted before the Notes section so the file stays readable.
    fn remember(&self, session_id: &str, entry: &str, note: bool) -> PathBuf {
        let path = self.session_file(session_id);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let label = self.label(session_id);
        let stamp = Utc::now().format("%Y-%m-%d %H:%M UTC");
        let line = format!("- [{stamp}] {entry}\n");
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let content = if existing.trim().is_empty() {
            let header = format!(
                "# RuTerm session memory — {label}\n\nSession id: `{session_id}`\n\n## Work log\n\n## Notes\n"
            );
            if note {
                format!("{header}{line}")
            } else {
                format!("{header}{line}\n")
            }
        } else if note {
            format!("{existing}{line}")
        } else if let Some(pos) = existing.find("## Notes") {
            format!("{}{}\n{}", &existing[..pos], line, &existing[pos..])
        } else {
            format!("{existing}{line}")
        };
        let _ = fs::write(&path, content);
        path
    }

    /// True when the user picked "Always allow" for this exact command.
    fn is_allowed(&self, command: &str) -> bool {
        self.allowlist
            .lock()
            .map(|set| set.contains(command.trim()))
            .unwrap_or(false)
    }

    fn allow_always(&self, command: &str) {
        if let Ok(mut set) = self.allowlist.lock() {
            set.insert(command.trim().to_string());
        }
    }

    /// Ask the webview to approve a risky command. The HTTP worker blocks on
    /// the receiver until the user clicks, denies, or the request times out.
    fn confirm(
        &self,
        app: &tauri::AppHandle,
        session: &str,
        command: &str,
        reason: &str,
    ) -> Result<ConfirmOutcome, String> {
        let id = Uuid::new_v4().simple().to_string();
        let (tx, rx) = channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id.clone(), tx);
        }
        let request = ConfirmRequest {
            id: id.clone(),
            session: session.to_string(),
            command: command.to_string(),
            reason: reason.to_string(),
        };
        if let Err(error) = app.emit("ruterm://mcp-confirm", request) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(format!("cannot show confirmation: {error}"));
        }
        let answer = rx.recv_timeout(CONFIRM_TIMEOUT).unwrap_or(ConfirmOutcome::Deny);
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
        Ok(answer)
    }
}

/// Called by the three-button confirmation dialog in the webview.
#[tauri::command]
pub fn mcp_confirm(
    bridge: tauri::State<'_, Bridge>,
    id: String,
    choice: String,
) -> Result<(), String> {
    let outcome = match choice.as_str() {
        "deny" => ConfirmOutcome::Deny,
        "allow" => ConfirmOutcome::Allow,
        "always" => ConfirmOutcome::Always,
        other => return Err(format!("unknown choice: {other}")),
    };
    let tx = bridge
        .pending
        .lock()
        .map_err(|_| "pending lock".to_string())?
        .remove(&id)
        .ok_or("confirmation request expired")?;
    tx.send(outcome).map_err(|_| "request already closed".into())
}

/// Config snippet injected into the agent's local shell. OpenCode merges this
/// virtual document with the user's real config, so providers/models and other
/// MCP servers keep working. `--standalone` is required because OpenCode v2's
/// background service does not inherit the PTY's environment.
#[tauri::command]
pub fn agent_config(bridge: tauri::State<'_, Bridge>) -> String {
    agent_config_state(&bridge)
}

/// Everything the frontend needs to start one supported CLI agent with the
/// RuTerm MCP bridge attached.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLaunch {
    pub agent_id: String,
    /// Full command typed into the local PTY.
    pub command: String,
    /// Working directory for the PTY (private config workspace when needed).
    pub cwd: String,
    /// Shell environment for the PTY, if the agent is configured via env.
    pub env: Option<HashMap<String, String>>,
}

/// Private per-agent workspace. Nothing here is written into a user's real
/// agent config directory.
fn agent_workspace(agent: &str) -> Result<PathBuf, String> {
    let safe: String = agent
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        return Err("invalid agent id".into());
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".ruterm").join("agents").join(safe))
}

/// Write a 0600 config file inside a 0700 private directory.
fn write_private_config(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    file.write_all(body.as_bytes()).map_err(|e| e.to_string())
}

/// Build a zero-config launch for any agent RuTerm detects. Every supported
/// CLI reads its MCP setup from this private workspace, never from the user's
/// own config files.
#[tauri::command]
pub fn agent_launch(
    bridge: tauri::State<'_, Bridge>,
    agent: String,
) -> Result<AgentLaunch, String> {
    let url = format!("http://127.0.0.1:{}/mcp", bridge.port);
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let desktop = format!("{home}/Desktop");
    let workspace = agent_workspace(&agent)?;
    let workspace_text = workspace.to_string_lossy().to_string();

    let (command, cwd, env) = match agent.as_str() {
        "opencode" => (
            "opencode --standalone".to_string(),
            desktop,
            Some(HashMap::from([(
                "OPENCODE_CONFIG_CONTENT".to_string(),
                agent_config_state(&bridge),
            )])),
        ),
        "freebuff" => {
            let config = json!({
                "mcpServers": {
                    "ruterm": {
                        "type": "http",
                        "url": url,
                        "headers": { "X-RuTerm-Token": bridge.token }
                    }
                }
            });
            write_private_config(&workspace.join(".agents/mcp.json"), &config.to_string())?;
            ("freebuff --trust-agents".to_string(), workspace_text, None)
        }
        "kilo" => {
            let config = json!({
                "mcp": {
                    "ruterm": {
                        "type": "remote",
                        "url": url,
                        "enabled": true,
                        "headers": { "X-RuTerm-Token": bridge.token }
                    }
                }
            });
            write_private_config(&workspace.join("kilo.json"), &config.to_string())?;
            ("kilo".to_string(), workspace_text, None)
        }
        "claude" => {
            let config = json!({
                "mcpServers": {
                    "ruterm": {
                        "type": "http",
                        "url": url,
                        "headers": { "X-RuTerm-Token": bridge.token }
                    }
                }
            });
            let path = workspace.join("mcp.json");
            write_private_config(&path, &config.to_string())?;
            (
                format!(
                    "claude --strict-mcp-config --mcp-config '{}'",
                    path.display()
                ),
                workspace_text,
                None,
            )
        }
        "kiro" => {
            let config = json!({
                "mcpServers": {
                    "ruterm": {
                        "url": url,
                        "headers": { "X-RuTerm-Token": bridge.token }
                    }
                }
            });
            write_private_config(&workspace.join(".kiro/settings/mcp.json"), &config.to_string())?;
            ("kiro-cli chat".to_string(), workspace_text, None)
        }
        "commandai" => {
            let config = json!({
                "mcpServers": {
                    "ruterm": {
                        "transport": "http",
                        "url": url,
                        "headers": { "X-RuTerm-Token": bridge.token }
                    }
                }
            });
            write_private_config(&workspace.join(".mcp.json"), &config.to_string())?;
            ("cmd -t --skip-onboarding".to_string(), workspace_text, None)
        }
        other => return Err(format!("unsupported agent: {other}")),
    };

    Ok(AgentLaunch {
        agent_id: agent,
        command,
        cwd,
        env,
    })
}

/// Same payload as `agent_config`, reusable at call sites that already hold
/// a `Bridge` state instead of a Tauri command wrapper.
fn agent_config_state(bridge: &Bridge) -> String {
    json!({
        "mcp": {
            "servers": {
                "ruterm": {
                    "type": "remote",
                    "url": format!("http://127.0.0.1:{}/mcp", bridge.port),
                    "oauth": false,
                    "codemode": false,
                    "headers": { "X-RuTerm-Token": bridge.token }
                }
            }
        }
    })
    .to_string()
}

fn serve_connection(mut stream: TcpStream, app: tauri::AppHandle, bridge: Bridge) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));

    let Some((request, body)) = read_request(&mut stream) else {
        return;
    };
    let (method, path) = request
        .split_whitespace()
        .nth(0)
        .zip(request.split_whitespace().nth(1))
        .unwrap_or(("", ""));

    if method == "DELETE" {
        write_response(&mut stream, 204, None, None);
        return;
    }
    if method == "OPTIONS" {
        write_response(&mut stream, 204, None, None);
        return;
    }
    if path != "/mcp" && path != "/" {
        write_response(&mut stream, 404, None, None);
        return;
    }
    if method != "POST" {
        write_response(&mut stream, 405, None, None);
        return;
    }

    let headers = parse_headers(&request);
    if headers.get("x-ruterm-token").map(String::as_str) != Some(bridge.token.as_str()) {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": { "code": -32001, "message": "invalid RuTerm bridge token" }
        })
        .to_string();
        write_response(&mut stream, 401, Some(&payload), None);
        return;
    }

    let rpc: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32700, "message": "parse error" }
            })
            .to_string();
            write_response(&mut stream, 400, Some(&payload), None);
            return;
        }
    };

    let id = rpc.get("id").cloned().unwrap_or(Value::Null);
    let rpc_method = rpc.get("method").and_then(Value::as_str).unwrap_or("");

    // Notifications have no id and expect no response body.
    if rpc_method.starts_with("notifications/") {
        write_response(&mut stream, 202, None, None);
        return;
    }

    let result = dispatch(&app, &bridge, &rpc);
    let payload = match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }).to_string(),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        })
        .to_string(),
    };
    let session = format!("ruterm-{}", &bridge.token[..8]);
    write_response(&mut stream, 200, Some(&payload), Some(&session));
}

fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut raw = Vec::with_capacity(4096);
    let mut byte = [0u8; 1];
    let header_end;
    loop {
        if stream.read_exact(&mut byte).is_err() {
            return None;
        }
        raw.push(byte[0]);
        if raw.len() > 64 * 1024 {
            return None;
        }
        if raw.ends_with(b"\r\n\r\n") {
            header_end = raw.len();
            break;
        }
    }
    let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let length: usize = parse_headers(&head)
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if length > 1024 * 1024 {
        return None;
    }
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    Some((head, body))
}

fn parse_headers(head: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for line in head.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    headers
}

fn write_response(stream: &mut TcpStream, status: u16, body: Option<&str>, session: Option<&str>) {
    let payload = body.unwrap_or("");
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        payload.len()
    );
    if body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    if let Some(session) = session {
        head.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(payload.as_bytes());
    let _ = stream.flush();
}

type RpcResult = Result<Value, (i64, String)>;

fn dispatch(app: &tauri::AppHandle, bridge: &Bridge, rpc: &Value) -> RpcResult {
    let method = rpc.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "ruterm", "version": env!("CARGO_PKG_VERSION") },
            "instructions": target_instructions(app, bridge),
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let params = rpc.get("params").unwrap_or(&Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            call_tool(app, bridge, name, &args)
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_targets",
            "description": "List SSH terminals currently open in RuTerm with their saved names. Call this before every server task: tabs can change while you run, without reloading this local shell. The local agent terminal is never a target.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "execute",
            "description": "Type a command into the remote VPS terminal and return its output. The command is visibly typed in the user's SSH tab. Use this instead of local shell tools for server work.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run on the remote VPS." },
                    "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS tab." },
                    "timeout_ms": { "type": "integer", "description": "Wait limit in milliseconds (default 180000)." }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        },
        {
            "name": "send_keys",
            "description": "Send raw keystrokes to the remote terminal (for interactive programs such as vim or htop).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keys": { "type": "string" },
                    "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS." }
                },
                "required": ["keys"],
                "additionalProperties": false
            }
        },
        {
            "name": "read_screen",
            "description": "Read recent output from the remote terminal as plain text (ANSI escapes removed).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS." },
                    "max_bytes": { "type": "integer" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "switch_target",
            "description": "Pin a specific SSH tab as the target for later commands.",
            "inputSchema": {
                "type": "object",
                "properties": { "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS." } },
                "required": ["session"],
                "additionalProperties": false
            }
        },
        {
            "name": "read_session",
            "description": "Read RuTerm's persistent work log and notes for this VPS. Call this at the start of a new session so you remember previous work and decisions.",
            "inputSchema": {
                "type": "object",
                "properties": { "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS." } },
                "additionalProperties": false
            }
        },
        {
            "name": "note_session",
            "description": "Append an important fact, decision, or explanation to this VPS's session memory file so future agent sessions remember it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Note to remember." },
                    "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS." }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        }
    ])
}

fn target_instructions(app: &tauri::AppHandle, bridge: &Bridge) -> String {
    match resolve_target(app, bridge, None) {
        Ok(id) => format!(
            "You are connected to RuTerm, an SSH client on the user's Mac. \
             An AI agent runs in a local RuTerm tab; all server/VPS work must go through an SSH tab, \
             never through this Mac's shell. \
             VPS tabs can open or close WHILE YOU ARE RUNNING: call ruterm_list_targets before every \
             server task to see the current tabs. No reload or restart of this local shell is needed — \
             a tab opened a moment ago is already visible. \
             Active VPS: {} ({}). \
             Session ids are internal and hidden; omit `session` to use the active VPS, or pass the \
             VPS name/address returned by ruterm_list_targets to choose a specific one. \
             Use ruterm_execute for ALL server/VPS work (SSL, setup, config); commands appear visibly \
             in that terminal and run on the VPS, not on this Mac. Destructive commands require the \
             user to choose Deny / Allow / Always allow in a RuTerm dialog; wait for the tool result. \
             Call ruterm_read_session when you start or switch VPS, and ruterm_note_session whenever \
             you learn an important fact or decision so future sessions remember it. \
             Memory for the active VPS: {}.",
            bridge.display_name(&id),
            bridge.label(&id),
            bridge.session_file(&id).display()
        ),
        Err(_) => [
            "You are connected to RuTerm, an SSH client on the user's Mac.",
            "No SSH session is open yet. Ask the user to open a VPS terminal tab, then call ruterm_list_targets.",
            "Targets can change while you run; call ruterm_list_targets before every server task — no reload is needed.",
            "Use ruterm_execute (never local shell tools) for server/VPS work.",
            "Destructive commands require user approval (Deny / Allow / Always allow).",
            "Once a VPS tab is open, call ruterm_read_session to load previous work context.",
        ]
        .join(" "),
    }
}

/// Keep a command/output snippet to one readable markdown line.
fn one_line(text: &str, max: usize) -> String {
    let flat = text
        .replace('`', "'")
        .replace('\r', "")
        .replace('\n', " ⏎ ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

fn trim_word(word: &str) -> &str {
    word.trim_matches(|c: char| matches!(c, '\'' | '"' | '`' | '(' | ')' | '{' | '}' | '[' | ']'))
}

/// Heuristic guard for commands that can destroy data or take a server down.
/// It is intentionally conservative: unknown commands are allowed, known risky
/// ones always stop for a user decision.
fn risky_reason(command: &str) -> Option<String> {
    let lower = command.to_ascii_lowercase();
    let wrappers = ["sudo", "env", "nice", "nohup", "time", "command", "builtin", "exec"];

    for segment in lower.split(|c: char| matches!(c, '\n' | ';' | '&' | '|')) {
        let mut words = segment.split_whitespace();
        let mut word = trim_word(words.next().unwrap_or(""));
        while wrappers.contains(&word) {
            if word == "env" {
                // Skip `VAR=value` assignments until the real command appears.
                loop {
                    match words.next() {
                        Some(next) if next.contains('=') => continue,
                        Some(next) => {
                            word = trim_word(next);
                            break;
                        }
                        None => return None,
                    }
                }
            } else {
                word = trim_word(words.next().unwrap_or(""));
            }
        }
        if word.is_empty() {
            continue;
        }
        let rest: Vec<&str> = words.collect();
        let has = |needle: &str| rest.iter().any(|item| *item == needle);
        let reason = match word {
            "rm" | "rmdir" | "shred" | "wipefs" | "truncate" => {
                Some("menghapus atau menimpa data".to_string())
            }
            "mkfs" | "dd" | "fdisk" | "sfdisk" | "parted" => {
                Some("memodifikasi partisi atau sistem file".to_string())
            }
            "reboot" | "shutdown" | "poweroff" | "halt" => {
                Some("mematikan atau me-restart server".to_string())
            }
            "pkill" | "killall" => Some("menghentikan proses".to_string()),
            "kill" if has("-9") || has("-sigkill") => Some("menghentikan proses paksa".to_string()),
            "chown" | "chmod" if has("-r") || has("--recursive") => {
                Some("mengubah izin secara rekursif".to_string())
            }
            "systemctl" | "service"
                if has("stop") || has("disable") || has("mask") || has("restart") =>
            {
                Some("mengganggu layanan yang berjalan".to_string())
            }
            "apt" | "apt-get" | "yum" | "dnf" | "pacman"
                if has("remove") || has("purge") || has("autoremove") =>
            {
                Some("menghapus paket terinstal".to_string())
            }
            "git"
                if (has("push") && (has("--force") || has("-f")))
                    || (has("reset") && has("--hard"))
                    || has("clean") =>
            {
                Some("menimpa riwayat atau menghapus file git".to_string())
            }
            "docker" if has("rm") || has("prune") || (has("system") && has("prune")) => {
                Some("menghapus container atau data docker".to_string())
            }
            "sh" | "bash"
                if lower.contains("curl") || lower.contains("wget") =>
            {
                Some("menjalankan script yang diunduh dari internet".to_string())
            }
            _ => {
                if lower.contains("drop table") || lower.contains("drop database") {
                    Some("menghapus tabel/database".to_string())
                } else {
                    None
                }
            }
        };
        if reason.is_some() {
            return reason;
        }
    }
    None
}

/// Stop risky commands until the user answers the modal.
fn gate(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    session: &str,
    command: &str,
) -> Result<(), (i64, String)> {
    let Some(reason) = risky_reason(command) else {
        return Ok(());
    };
    if bridge.is_allowed(command) {
        return Ok(());
    }
    match bridge.confirm(app, session, command, &reason) {
        Ok(ConfirmOutcome::Deny) => {
            bridge.remember(
                session,
                &format!("user denied `{}` ({reason})", one_line(command, 400)),
                false,
            );
            Err((-32003, format!("Ditolak oleh pengguna: {reason}")))
        }
        Ok(ConfirmOutcome::Always) => {
            bridge.allow_always(command);
            bridge.remember(
                session,
                &format!("user chose Always allow for `{}`", one_line(command, 400)),
                false,
            );
            Ok(())
        }
        Ok(ConfirmOutcome::Allow) => {
            bridge.remember(
                session,
                &format!("user approved `{}` ({reason})", one_line(command, 400)),
                false,
            );
            Ok(())
        }
        Err(error) => Err((-32000, error)),
    }
}

fn call_tool(app: &tauri::AppHandle, bridge: &Bridge, name: &str, args: &Value) -> RpcResult {
    let result = match name {
        "list_targets" => {
            let ids = ssh_ids(app);
            if ids.is_empty() {
                "No SSH session is open. Ask the user to open a VPS terminal tab in RuTerm. \
                 Call this tool again whenever you need server work — no reload is required."
                    .to_string()
            } else {
                let active = resolve_target(app, bridge, None).unwrap_or_default();
                let lines = ids
                    .iter()
                    .map(|id| {
                        let marker = if id == &active { " (active)" } else { "" };
                        format!(
                            "- {} — {}{}",
                            bridge.display_name(id),
                            bridge.label(id),
                            marker
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "Open VPS tabs (saved names):\n{lines}\n\nSession ids are internal and hidden. \
                     Omit `session` to use the active VPS, or pass the name/address above to pick one. \
                     This list is live — call it again after the user opens another tab.\n\
                     Session memory: {}",
                    bridge.session_file(&active).display()
                )
            }
        }
        "execute" => {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .ok_or((-32602, "command is required".into()))?;
            let timeout_ms = args
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(180_000)
                .clamp(1_000, 3_600_000);
            execute(app, bridge, command, args.get("session").and_then(Value::as_str), timeout_ms)?
        }
        "send_keys" => {
            let keys = args
                .get("keys")
                .and_then(Value::as_str)
                .ok_or((-32602, "keys is required".into()))?;
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            gate(app, bridge, &session, keys)?;
            write_target(app, bridge, Some(&session), keys).map_err(|e| (-32000, e))?;
            bridge.remember(
                &session,
                &format!("sent keys `{}`", one_line(keys, 400)),
                false,
            );
            format!(
                "Keys sent to {} ({}).",
                bridge.display_name(&session),
                bridge.label(&session)
            )
        }
        "read_screen" => {
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            let max = args
                .get("max_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(20_000)
                .clamp(1_000, 512_000) as usize;
            let bytes = output_tail(app, &session, max);
            let body = if bytes.is_empty() {
                "(no output captured yet)".to_string()
            } else {
                strip_ansi(&bytes)
            };
            format!(
                "VPS: {} ({})\n{}",
                bridge.display_name(&session),
                bridge.label(&session),
                body
            )
        }
        "switch_target" => {
            let session = args
                .get("session")
                .and_then(Value::as_str)
                .ok_or((-32602, "session is required".into()))?;
            let resolved = resolve_target(app, bridge, Some(session)).map_err(|e| (-32000, e))?;
            if let Ok(mut target) = bridge.target.lock() {
                *target = Some(resolved.clone());
            }
            format!(
                "Target switched to {} ({}).",
                bridge.display_name(&resolved),
                bridge.label(&resolved)
            )
        }
        "read_session" => {
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            let path = bridge.session_file(&session);
            let body = fs::read_to_string(&path).unwrap_or_default();
            format!(
                "VPS: {} ({})\nSession memory file: {}\n\n{}",
                bridge.display_name(&session),
                bridge.label(&session),
                path.display(),
                if body.trim().is_empty() {
                    "(still empty — use ruterm_note_session to record context)".to_string()
                } else {
                    body
                }
            )
        }
        "note_session" => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or((-32602, "text is required".into()))?;
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            let path = bridge.remember(&session, &one_line(text, 2000), true);
            format!(
                "Noted in {} for {} ({}).",
                path.display(),
                bridge.display_name(&session),
                bridge.label(&session)
            )
        }
        _ => return Err((-32602, format!("unknown tool: {name}"))),
    };
    Ok(json!({
        "content": [{ "type": "text", "text": result }],
        "isError": false
    }))
}

fn ssh_ids(app: &tauri::AppHandle) -> Vec<String> {
    app.state::<AppState>()
        .ssh
        .lock()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

fn resolve_target(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    requested: Option<&str>,
) -> Result<String, String> {
    let ids = ssh_ids(app);
    if let Some(query) = requested {
        let query = query.trim();
        // Exact internal id still works, but agents normally use the friendly
        // name from `list_targets` instead.
        let found = ids
            .iter()
            .find(|id| id.as_str() == query)
            .or_else(|| {
                ids.iter().find(|id| {
                    bridge.display_name(id).eq_ignore_ascii_case(query)
                        || bridge.label(id).eq_ignore_ascii_case(query)
                })
            })
            .or_else(|| {
                let needle = query.to_lowercase();
                ids.iter().find(|id| {
                    bridge.display_name(id).to_lowercase().contains(&needle)
                        || bridge.label(id).to_lowercase().contains(&needle)
                })
            });
        return match found {
            Some(id) => {
                if let Ok(mut target) = bridge.target.lock() {
                    *target = Some(id.clone());
                }
                Ok(id.clone())
            }
            None => {
                let available = ids
                    .iter()
                    .map(|id| bridge.display_name(id))
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(format!(
                    "VPS `{query}` is not open. Open VPS tabs: {available}"
                ))
            }
        };
    }
    if let Ok(target) = bridge.target.lock() {
        if let Some(id) = target.as_ref() {
            if ids.contains(id) {
                return Ok(id.clone());
            }
        }
    }
    if let Ok(recent) = bridge.recent.lock() {
        if let Some(id) = recent.iter().rev().find(|id| ids.contains(id)) {
            return Ok(id.clone());
        }
    }
    ids.first()
        .cloned()
        .ok_or_else(|| "No SSH session is open yet. Ask the user to open a VPS terminal tab.".into())
}

fn write_target(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    requested: Option<&str>,
    text: &str,
) -> Result<(), String> {
    let session = resolve_target(app, bridge, requested)?;
    let handle = app
        .state::<AppState>()
        .ssh
        .lock()
        .map_err(|_| "state lock".to_string())?
        .get(&session)
        .cloned()
        .ok_or("Session is not active")?;
    handle.write(text.as_bytes().to_vec());
    Ok(())
}

fn execute(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    command: &str,
    requested: Option<&str>,
    timeout_ms: u64,
) -> Result<String, (i64, String)> {
    let session = resolve_target(app, bridge, requested).map_err(|e| (-32000, e))?;
    gate(app, bridge, &session, command)?;
    let since = app
        .state::<AppState>()
        .output
        .lock()
        .ok()
        .and_then(|map| map.get(&session).map(|entry| entry.total))
        .unwrap_or(0);

    let marker = format!("__RUTERM_DONE_{}__", &Uuid::new_v4().simple().to_string()[..10]);
    let script = format!(
        "{}\nprintf '\\n{}:%s\\n' \"$?\"\n",
        command.trim_end(),
        marker
    );
    write_target(app, bridge, Some(&session), &script).map_err(|e| (-32000, e))?;

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        // The terminal echoes the command we typed, including the marker text.
        // Use the LAST occurrence so we read the real `printf` result, not the echo.
        if let Some(pos) = rfind_bytes(&output_since(app, &session, since), marker.as_bytes()) {
            let raw = &output_since(app, &session, since)[..pos];
            let tail = &output_since(app, &session, since)[pos..];
            let code = tail
                .get(marker.len()..)
                .and_then(|rest| {
                    let text = String::from_utf8_lossy(rest);
                    let line = text.lines().next().unwrap_or("");
                    line.strip_prefix(':').map(str::trim).map(str::to_string)
                })
                .unwrap_or_default();
            let text = strip_ansi(raw);
            let body = if text.trim().is_empty() {
                "(command produced no output)".to_string()
            } else {
                text
            };
            bridge.remember(
                &session,
                &format!(
                    "`{}` → exit {} — {}",
                    one_line(command, 400),
                    code,
                    one_line(&body, 1200)
                ),
                false,
            );
            return Ok(format!(
                "VPS: {} ({})\n{body}\n[exit status: {code}]",
                bridge.display_name(&session),
                bridge.label(&session)
            ));
        }
        if Instant::now() >= deadline {
            let partial = strip_ansi(&output_since(app, &session, since));
            bridge.remember(
                &session,
                &format!("`{}` → timed out after {} ms", one_line(command, 400), timeout_ms),
                false,
            );
            return Ok(format!(
                "VPS: {} ({})\n[timed out after {} ms; command may still be running]\n{}",
                bridge.display_name(&session),
                bridge.label(&session),
                timeout_ms,
                if partial.trim().is_empty() {
                    "(no output captured)".to_string()
                } else {
                    partial
                }
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Bytes appended to a session since an absolute `total` offset. The replay
/// buffer is capped, so when it wrapped we return the newest known bytes.
fn output_since(app: &tauri::AppHandle, session: &str, since: u64) -> Vec<u8> {
    let state = app.state::<AppState>();
    let Ok(map) = state.output.lock() else {
        return Vec::new();
    };
    let Some(entry) = map.get(session) else {
        return Vec::new();
    };
    let base = entry.total.saturating_sub(entry.buf.len() as u64);
    let skip = since.saturating_sub(base).min(entry.buf.len() as u64) as usize;
    entry.buf[skip..].to_vec()
}

fn output_tail(app: &tauri::AppHandle, session: &str, max: usize) -> Vec<u8> {
    app.state::<AppState>()
        .output
        .lock()
        .ok()
        .and_then(|map| map.get(session).map(|entry| entry.buf.clone()))
        .map(|buf| {
            let start = buf.len().saturating_sub(max);
            buf[start..].to_vec()
        })
        .unwrap_or_default()
}

/// Like `find_bytes`, but returns the last match. A PTY echoes the command we
/// type, so the first marker hit is the echoed text, not the command's result.
fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).rposition(|part| part == needle)
}

/// Minimal ANSI/osc stripper: enough to give an agent readable plain text
/// without pulling a regex or terminal-emulator dependency into the app.
fn strip_ansi(input: &[u8]) -> String {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == 0x1b {
            i += 1;
            if i >= input.len() {
                break;
            }
            match input[i] {
                b'[' => {
                    i += 1;
                    while i < input.len() && !(0x40..=0x7e).contains(&input[i]) {
                        i += 1;
                    }
                    i += 1;
                }
                b']' => {
                    i += 1;
                    while i < input.len() {
                        if input[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if input[i] == 0x1b && i + 1 < input.len() && input[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                b'(' | b')' | b'#' | b'*' | b'+' => i += 2,
                _ => i += 1,
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out)
        .replace('\r', "")
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bridge() -> Bridge {
        Bridge {
            port: 19001,
            token: "secret".into(),
            recent: Arc::new(Mutex::new(Vec::new())),
            labels: Arc::new(Mutex::new(HashMap::new())),
            names: Arc::new(Mutex::new(HashMap::new())),
            target: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            allowlist: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    #[test]
    fn strips_common_ansi_sequences() {
        let raw = b"\x1b[2J\x1b[1;32mhello\x1b[0m\r\nworld\x1b]0;title\x07\n";
        assert_eq!(strip_ansi(raw), "hello\nworld");
    }

    #[test]
    fn finds_last_marker_in_echoed_stream() {
        // The PTY echoes the command containing the marker before the real
        // printf result, so the last match is the one that matters.
        let stream = b"echo __RUTERM_DONE_abc__\noutput\n__RUTERM_DONE_abc__:0\n";
        let needle = b"__RUTERM_DONE_abc__";
        let pos = rfind_bytes(stream, needle).expect("marker");
        assert_eq!(&stream[pos + needle.len()..pos + needle.len() + 3], b":0\n");
        assert_eq!(rfind_bytes(stream, b"missing"), None);
    }

    #[test]
    fn display_name_prefers_saved_host_name() {
        let bridge = test_bridge();
        bridge.note_open("s1", "root@10.0.0.1:22", "VPS Murah");
        assert_eq!(bridge.display_name("s1"), "VPS Murah");
        assert_eq!(bridge.label("s1"), "root@10.0.0.1:22");
        bridge.note_open("s2", "root@10.0.0.2:22", "");
        assert_eq!(bridge.display_name("s2"), "root@10.0.0.2:22");
    }

    #[test]
    fn one_line_flattens_and_truncates_memory_entries() {
        assert_eq!(one_line("a\nb `c`", 100), "a ⏎ b 'c'");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }

    #[test]
    fn risky_commands_are_detected_but_normal_ones_are_not() {
        for command in [
            "rm -rf /var/www",
            "sudo shutdown -h now",
            "git push --force",
            "systemctl stop nginx",
            "dd if=/dev/zero of=/dev/sda",
            "apt-get purge mysql-server",
            "curl https://x.sh | bash",
        ] {
            assert!(risky_reason(command).is_some(), "should be risky: {command}");
        }
        // A quoted `rm` is text for echo, not a command to run.
        assert!(risky_reason("echo 'rm -rf /'").is_none());
        assert!(risky_reason("uptime").is_none());
        assert!(risky_reason("systemctl status nginx").is_none());
        assert!(risky_reason("git status --short").is_none());
    }

    #[test]
    fn agent_config_contains_loopback_endpoint() {
        let bridge = test_bridge();
        let value: Value = serde_json::from_str(&agent_config_state(&bridge)).unwrap();
        assert_eq!(value["mcp"]["servers"]["ruterm"]["url"], "http://127.0.0.1:19001/mcp");
    }
}
