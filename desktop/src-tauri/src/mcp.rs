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
use rterm_core::model::{now, Note};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use uuid::Uuid;

const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);

/// `timeout_ms: 0` means "no limit"; positive values are capped at one hour.
fn normalize_timeout(timeout_ms: u64) -> Option<Duration> {
    if timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(timeout_ms.clamp(1_000, 3_600_000)))
    }
}

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

/// One long-running remote command being streamed to an agent.
struct StreamJob {
    session: String,
    command: String,
    marker: String,
    since: u64,
    consumed: u64,
    /// Last `next_offset` handed out, used when a caller omits `offset`.
    last_offset: u64,
    buffer: Vec<u8>,
    done: bool,
    exit_status: String,
    deadline: Option<Instant>,
    created: Instant,
    /// Last time new PTY bytes arrived. A finished command whose marker was
    /// missed must not keep the shell marked busy forever.
    last_activity: Instant,
}

/// What `read_stream` returns to the agent at one poll.
struct JobView {
    job: String,
    session: String,
    command: String,
    vps: String,
    address: String,
    chunk: Vec<u8>,
    offset: u64,
    next_offset: u64,
    done: bool,
    exit_status: String,
    elapsed: Duration,
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
    /// Live/streaming command jobs, so output can be read while it is produced.
    jobs: Arc<Mutex<HashMap<String, StreamJob>>>,
    /// Exact commands the user approved with "Always allow" (this launch only).
    allowlist: Arc<Mutex<HashSet<String>>>,
    /// Serializes keystrokes so the chat, an agent and the user can never
    /// interleave half-commands into the same SSH PTY.
    write_lock: Arc<Mutex<()>>,
    /// SSH tabs currently showing a TUI. Any new command closes it first.
    tui: Arc<Mutex<HashSet<String>>>,
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
            jobs: Arc::new(Mutex::new(HashMap::new())),
            allowlist: Arc::new(Mutex::new(HashSet::new())),
            write_lock: Arc::new(Mutex::new(())),
            tui: Arc::new(Mutex::new(HashSet::new())),
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
        // The first available SSH tab is a sensible default until the webview
        // pins the one the user is actually looking at.
        if let Ok(mut target) = self.target.lock() {
            if target.is_none() {
                *target = Some(session_id.to_string());
            }
        }
    }

    /// Pin the SSH tab RuTerm AI Chat should drive. Only live SSH sessions are
    /// accepted here; a local agent tab can never become a target.
    pub fn set_target(&self, session_id: &str) {
        if let Ok(mut target) = self.target.lock() {
            *target = Some(session_id.to_string());
        }
    }

    /// Friendly name/address of the pinned target, safe to show in the UI.
    /// Internal session ids are never exposed.
    pub fn target_view(&self) -> Option<(String, String)> {
        let id = self.target.lock().ok()?.clone()?;
        Some((self.display_name(&id), self.label(&id)))
    }

    /// Internal id, only for Rust-side liveness checks — never sent to the UI.
    pub fn target_id(&self) -> Option<String> {
        self.target.lock().ok()?.clone()
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

    /// Saved host name, e.g. "production-api". Falls back to the address for
    /// adhoc quick-connect tabs that have no saved name.
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
    pub(crate) fn remember(&self, session_id: &str, entry: &str, note: bool) -> PathBuf {
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

/// Keep the bridge target in step with the SSH tab the user is looking at.
/// Called from the webview on tab focus, so RuTerm AI Chat can omit `session`
/// and naturally act on "the terminal that currently has the host open".
#[tauri::command]
pub fn mcp_set_target(
    app: tauri::AppHandle,
    bridge: tauri::State<'_, Bridge>,
    session: String,
) -> Result<(), String> {
    if !ssh_ids(&app).contains(&session) {
        return Err("SSH session is not active".into());
    }
    bridge.set_target(&session);
    let (name, address) = bridge.target_view().unwrap_or((session, String::new()));
    app.emit("rterm:mcp-target", json!({ "name": name, "address": address }))
        .map_err(|error| error.to_string())
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
                    "timeout_ms": { "type": "integer", "description": "Wait limit in ms: 0 = no limit, default 180000, max 3600000. Prefer start_execute + read_stream for long commands." }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        },
        {
            "name": "start_execute",
            "description": "Type a command into the VPS and return a job id immediately. Output is available as it is produced via read_stream — use this for apt-get, builds, and other long commands instead of blocking in execute.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run on the remote VPS." },
                    "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS tab." },
                    "timeout_ms": { "type": "integer", "description": "Job deadline in ms: 0 = no limit (default), max 3600000." }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        },
        {
            "name": "read_stream",
            "description": "Read newly produced output from a start_execute job. Long-polls until data arrives or wait_ms elapses, so agents can follow progress in real time.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job": { "type": "string", "description": "Job id returned by start_execute." },
                    "offset": { "type": "integer", "description": "Byte offset returned by the previous read_stream call (default 0)." },
                    "wait_ms": { "type": "integer", "description": "Long-poll wait: 0 = return immediately, default 1000, max 60000." },
                    "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS." }
                },
                "required": ["job"],
                "additionalProperties": false
            }
        },
        {
            "name": "cancel_stream",
            "description": "Cancel a running start_execute job by sending Ctrl-C to that VPS and marking the job finished.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job": { "type": "string", "description": "Job id returned by start_execute." }
                },
                "required": ["job"],
                "additionalProperties": false
            }
        },
        {
            "name": "execute_batch",
            "description": "Run several short VPS commands one after another in the same tab and return all results at once. Each command still goes through the risky-command check; use start_execute for long-running commands.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "commands": { "type": "array", "items": { "type": "string" }, "description": "Commands to run in order." },
                    "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS." },
                    "timeout_ms": { "type": "integer", "description": "Wait limit per command: 0 = no limit, default 180000." }
                },
                "required": ["commands"],
                "additionalProperties": false
            }
        },
        {
            "name": "open_tui",
            "description": "Launch an interactive terminal program (htop, vim, nano, less, tail -f) in the active VPS tab. The command is typed with a real Enter keystroke; no sentinel is appended.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "TUI command to launch, e.g. `htop` or `sudo htop`." },
                    "session": { "type": "string", "description": "Internal session id, VPS name, or address shown by list_targets. Omit to use the active VPS." }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        },
        {
            "name": "close_tui",
            "description": "Exit an open interactive program (htop, vim, nano, less, tail -f) and return to the shell prompt. Send this before running any command if a TUI is open.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keys": { "type": "string", "description": "Exit keys. Defaults to Ctrl-C (\\u0003); use `q` for htop/less or `:q\\n` for vim." },
                    "session": { "type": "string", "description": "Omit to use the active VPS." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "send_keys",
            "description": "Send raw keystrokes to the remote terminal (for interactive programs such as vim or htop).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keys": { "type": "string", "description": "Raw keystrokes. Use \\n for Enter; never type the word 'Enter'. Ctrl-C is \\u0003." },
                    "include_screen": { "type": "boolean", "description": "Also return the latest screen after sending — handy for vim/btop/mc." },
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
                    "max_bytes": { "type": "integer" },
                    "lines": { "type": "integer", "description": "Return only the last N clean lines (default 40)." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inspect_terminal",
            "description": "Snapshot the active SSH terminal BEFORE typing: host, clean recent screen, detected prompt, whether the shell is idle, and any running jobs. Use this instead of guessing what is on screen.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Omit to inspect the focused SSH tab." },
                    "lines": { "type": "integer", "description": "Recent screen lines to return (default 30, max 120)." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "verify_screen",
            "description": "Read the actual VPS screen AFTER an action and report objective evidence: whether the expected command is visible, whether new output appeared, and whether the shell is back at a clean prompt. Use this to confirm done/not done instead of trusting a tool status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command that should be visible on screen. Omit for a general screen check." },
                    "lines": { "type": "integer", "description": "Screen tail lines to include (default 20)." },
                    "session": { "type": "string", "description": "Omit to use the focused SSH tab." }
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
        },
        {
            "name": "list_hosts",
            "description": "List ALL SAVED SSH hosts in the encrypted RuTerm vault with label, address, port, user, group, tags and notes. Use this for 'which VPS hosts do I have?' — list_targets only shows currently OPEN tabs. Passwords, keys and env values are never returned.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "get_host",
            "description": "Get full details for ONE saved host by label, address, username or id. Credentials are never returned.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "Saved host label, address or user." } },
                "required": ["name"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_notes",
            "description": "List notes in the encrypted vault with their id and opening words. Call this before read_note or update_note to find the right note.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "read_note",
            "description": "Read a note by id, or find up to 5 notes containing a query string. Use list_notes first if you do not know the id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Note id from list_notes." },
                    "query": { "type": "string", "description": "Text to search for in note bodies." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "create_note",
            "description": "Create a new note in the encrypted RuTerm vault and open it in a new tab — exactly like RuTerm's native AI chat. Use this immediately when the user asks to record, recap or save a result into a note. No approval dialog is needed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short title; becomes the first line of the note." },
                    "body": { "type": "string", "description": "Full note content written from facts just verified." },
                    "favorite": { "type": "boolean" }
                },
                "required": ["body"],
                "additionalProperties": false
            }
        },
        {
            "name": "update_note",
            "description": "Edit an existing note by id (from list_notes), replacing or appending to its body, then refresh its open tab. Use this when the user asks to edit, update or add to a note.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Note id from list_notes or read_note." },
                    "body": { "type": "string", "description": "New text to write or append." },
                    "append": { "type": "boolean", "description": "true = add to the end instead of replacing (default false)." },
                    "favorite": { "type": "boolean", "description": "Optionally change the favorite flag." }
                },
                "required": ["id", "body"],
                "additionalProperties": false
            }
        }
    ])
}

/// Sent with MCP `initialize` so agents know the vault tools exist even before
/// they call `tools/list`. Keeps host/note questions on the native tools.
const VAULT_TOOLS_HINT: &str = "RuTerm vault tools: ruterm_list_hosts lists every SAVED SSH host \
 (ruterm_list_targets only shows currently OPEN tabs); ruterm_get_host details one host. \
 ruterm_list_notes + ruterm_read_note find notes; ruterm_create_note writes a note and opens it in \
 a new tab, ruterm_update_note edits one by id and refreshes its tab. Use these whenever the user \
 asks which hosts they have, or asks to save, recap or edit a note — no local file writing is needed.";

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
             Use ruterm_execute for short server/VPS work (SSL, setup, config); commands appear visibly \
             in that terminal and run on the VPS, not on this Mac. For long commands (apt-get, builds, \
             kernel changes) use ruterm_start_execute + ruterm_read_stream: output arrives in chunks as \
             it is produced, and timeout_ms: 0 means no limit. Use ruterm_execute_batch for several \
             short commands in a row. Use ruterm_send_keys (with include_screen: true) + \
             ruterm_read_screen for interactive TUIs such as vim, btop or mc; the tab reconnects \
             automatically with a Connecting overlay if the VPS reboots, so just retry your tool call \
             after a moment. Destructive commands require the \
             user to choose Deny / Allow / Always allow in a RuTerm dialog; wait for the tool result. \
             Call ruterm_read_session when you start or switch VPS, and ruterm_note_session whenever \
             you learn an important fact or decision so future sessions remember it. \
             Memory for the active VPS: {}. {}",
            bridge.display_name(&id),
            bridge.label(&id),
            bridge.session_file(&id).display(),
            VAULT_TOOLS_HINT
        ),
        Err(_) => [
            "You are connected to RuTerm, an SSH client on the user's Mac.",
            "No SSH session is open yet. Ask the user to open a VPS terminal tab, then call ruterm_list_targets.",
            "Targets can change while you run; call ruterm_list_targets before every server task — no reload is needed.",
            "Use ruterm_execute (never local shell tools) for server/VPS work.",
            "Destructive commands require user approval (Deny / Allow / Always allow).",
            "Once a VPS tab is open, call ruterm_read_session to load previous work context.",
            VAULT_TOOLS_HINT,
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

/// Models often write `htopEnter` or `htop<Enter>` instead of a real newline.
/// Translate those conventions into actual terminal keystrokes so nothing is
/// ever typed as the literal word "Enter".
pub fn terminal_keys(keys: &str) -> String {
    let mut text = keys
        .replace("\\r\n", "\n")
        .replace("\\n", "\n")
        .replace("\\r", "\n");
    for marker in [
        "<enter>", "[enter]", "<return>", "[return]", "<cr>", "<Enter>", "[Enter]", "<Return>",
        "[Return]", "<CR>",
    ] {
        text = text.replace(marker, "\n");
    }
    let trimmed = text.trim_end().to_string();
    let lower = trimmed.to_ascii_lowercase();
    for suffix in ["enter", "return"] {
        if lower.ends_with(suffix) {
            let prefix = &trimmed[..trimmed.len() - suffix.len()];
            // `htopEnter` (model-style) and `htop Enter` are both requests to
            // press Enter; a lowercase `center` is just a command name.
            let capitalized = format!("{}{}", suffix[..1].to_uppercase(), &suffix[1..]);
            if prefix.is_empty()
                || prefix.ends_with(char::is_whitespace)
                || trimmed.ends_with(&capitalized)
            {
                return format!("{}\n", prefix.trim_end());
            }
        }
    }
    text
}

/// A command that tries to SSH again from inside an already-open SSH tab.
/// Models produce this when they want "a shell on the server" without realising
/// the tab is already that shell. It must never be typed into the VPS.
pub fn nested_ssh_command(command: &str) -> bool {
    let mut words = command.trim().split_whitespace();
    let first = words.next().unwrap_or("").trim_matches(|c: char| !c.is_alphanumeric());
    if first.eq_ignore_ascii_case("ssh") {
        return true;
    }
    if first.eq_ignore_ascii_case("sudo") {
        return words
            .find(|word| !word.starts_with('-'))
            .is_some_and(|word| word.eq_ignore_ascii_case("ssh"));
    }
    false
}

/// Detect a `send_keys` payload that is really a request to launch a TUI
/// (`htop`, `htopEnter`, `sudo htop`, `tail -f …`). Navigation keys such as `q`
/// are deliberately not matched.
pub fn looks_like_tui_launch(keys: &str) -> bool {
    let normalized = terminal_keys(keys);
    let mut words = normalized.split_whitespace();
    let clean = |word: &str| {
        word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '/')
            .to_ascii_lowercase()
    };
    let mut first = clean(words.next().unwrap_or(""));
    if first == "sudo" {
        first = words
            .find(|word| !word.starts_with('-'))
            .map(clean)
            .unwrap_or_default();
    }
    const TUIS: &[&str] = &[
        "htop", "top", "btop", "atop", "vim", "nvim", "vi", "nano", "micro", "less", "more", "man",
        "mc", "ranger", "tmux", "screen", "watch", "ping", "ssh",
    ];
    if TUIS.contains(&first.as_str()) {
        return true;
    }
    (first == "tail" || first == "journalctl")
        && normalized
            .split_whitespace()
            .any(|word| word == "-f" || word == "--follow")
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

/// Run an async vault operation off the MCP listener's plain std thread.
/// `spawn` + a channel (instead of `block_on`) also keeps this safe when a
/// caller is already on Tauri's async runtime, e.g. chat's `invoke_tool`.
fn vault_call<F, Fut, T>(app: &tauri::AppHandle, task: F) -> Result<T, String>
where
    F: FnOnce(tauri::AppHandle) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = channel();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tx.send(task(handle).await);
    });
    rx.recv()
        .map_err(|_| "vault operation was interrupted".to_string())?
}

pub(crate) fn call_tool(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    name: &str,
    args: &Value,
) -> RpcResult {
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
                     Omit `session` to use the active VPS. Only name another tab if the user \
                     explicitly asks for it. Next step: run the requested command with `execute` \
                     (or `open_tui` for interactive programs) — do not just list targets again. \
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
            // A TUI must never get an exit sentinel appended — route it to
            // open_tui even when the caller asked for a normal execute.
            if looks_like_tui_launch(command) {
                let mut tui = json!({ "command": command });
                if let Some(session) = args.get("session") {
                    tui["session"] = session.clone();
                }
                return call_tool(app, bridge, "open_tui", &tui);
            }
            let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(60_000);
            execute(app, bridge, command, args.get("session").and_then(Value::as_str), timeout_ms)?
        }
        "start_execute" => {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .ok_or((-32602, "command is required".into()))?;
            if looks_like_tui_launch(command) {
                let mut tui = json!({ "command": command });
                if let Some(session) = args.get("session") {
                    tui["session"] = session.clone();
                }
                return call_tool(app, bridge, "open_tui", &tui);
            }
            let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(0);
            let job = start_job(
                app,
                bridge,
                command,
                args.get("session").and_then(Value::as_str),
                timeout_ms,
                true,
            )?;
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .unwrap_or_default();
            format!(
                "VPS: {} ({})\njob: {job}\nstarted. Call ruterm_read_stream with this job id to follow output.",
                bridge.display_name(&session),
                bridge.label(&session)
            )
        }
        "read_stream" => {
            let job = args
                .get("job")
                .and_then(Value::as_str)
                .ok_or((-32602, "job is required".into()))?;
            let job = resolve_job_id(bridge, job)?;
            let wait_ms = args.get("wait_ms").and_then(Value::as_u64).unwrap_or(1_000);
            // Models almost never remember to pass `offset`. Continue from the
            // last chunk instead of replaying the whole job on every poll.
            let offset = if let Some(explicit) = args.get("offset").and_then(Value::as_u64) {
                explicit
            } else {
                bridge
                    .jobs
                    .lock()
                    .ok()
                    .and_then(|jobs| jobs.get(&job).map(|entry| entry.last_offset))
                    .unwrap_or(0)
            };
            let view = poll_job(app, bridge, &job, offset, wait_ms)?;
            if let Ok(mut jobs) = bridge.jobs.lock() {
                if let Some(entry) = jobs.get_mut(&job) {
                    entry.last_offset = view.next_offset;
                }
            }
            let chunk = strip_ansi(&view.chunk);
            let status = if view.done {
                format!("exit {}", if view.exit_status.is_empty() { "unknown" } else { &view.exit_status })
            } else {
                "running".to_string()
            };
            format!(
                "VPS: {} ({})\njob: {}\noffset: {}\nnext_offset: {}\ndone: {}\nstatus: {}\nelapsed_s: {}\n--- chunk ---\n{}",
                view.vps,
                view.address,
                view.job,
                view.offset,
                view.next_offset,
                view.done,
                status,
                view.elapsed.as_secs(),
                if chunk.trim().is_empty() { "(no new output)".to_string() } else { chunk }
            )
        }
        "cancel_stream" => {
            let job = args
                .get("job")
                .and_then(Value::as_str)
                .ok_or((-32602, "job is required".into()))?;
            cancel_job(app, bridge, &resolve_job_id(bridge, job)?)?
        }
        "execute_batch" => {
            let commands = args
                .get("commands")
                .and_then(Value::as_array)
                .ok_or((-32602, "commands must be an array".into()))?;
            if commands.is_empty() {
                return Err((-32602, "commands must not be empty".into()));
            }
            let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(60_000);
            let requested = args.get("session").and_then(Value::as_str);
            let mut parts = Vec::new();
            for (index, value) in commands.iter().enumerate() {
                let command = value
                    .as_str()
                    .ok_or((-32602, format!("commands[{index}] must be a string")))?;
                let result = execute(app, bridge, command, requested, timeout_ms)?;
                parts.push(format!("--- {}/{} ---\n{}", index + 1, commands.len(), result));
            }
            parts.join("\n")
        }
        "open_tui" => {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .ok_or((-32602, "command is required".into()))?;
            // Models often append a textual `Enter`; normalize it to a real
            // newline so the literal word never reaches the shell.
            let command = terminal_keys(command);
            let command = command.trim();
            if command.is_empty() {
                return Err((-32602, "command cannot be empty".into()));
            }
            if nested_ssh_command(command) {
                return Err((
                    -32600,
                    "Already inside the SSH tab — do not run a nested `ssh` command.".into(),
                ));
            }
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            gate(app, bridge, &session, command)?;
            // A real newline is required. No exit sentinel: TUIs stay open.
            let (_, ready) = write_command(app, bridge, &format!("{command}\n"), Some(&session))?;
            if let Ok(mut open) = bridge.tui.lock() {
                open.insert(session.clone());
            }
            bridge.remember(
                &session,
                &format!("opened interactive `{}`", one_line(command, 400)),
                false,
            );
            format!(
                "Opened {} in {} ({}). Use send_keys for navigation and Ctrl-C (\\u0003) to exit.{}",
                one_line(command, 200),
                bridge.display_name(&session),
                bridge.label(&session),
                prompt_warning(ready)
            )
        }
        "close_tui" => {
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            let keys = args
                .get("keys")
                .and_then(Value::as_str)
                .unwrap_or("\u{3}");
            let keys = terminal_keys(keys);
            write_target(app, bridge, Some(&session), &keys).map_err(|e| (-32000, e))?;
            if let Ok(mut open) = bridge.tui.lock() {
                open.remove(&session);
            }
            std::thread::sleep(Duration::from_millis(200));
            format!(
                "Closed the interactive program in {} ({}). Shell prompt should be ready now.",
                bridge.display_name(&session),
                bridge.label(&session)
            )
        }
        "send_keys" => {
            let keys = args
                .get("keys")
                .and_then(Value::as_str)
                .ok_or((-32602, "keys is required".into()))?;
            let keys = terminal_keys(keys);
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            gate(app, bridge, &session, &keys)?;
            write_target(app, bridge, Some(&session), &keys).map_err(|e| (-32000, e))?;
            if keys.contains('\u{3}')
                || keys.trim().eq_ignore_ascii_case("q")
                || keys.trim_start().starts_with(":q")
            {
                if let Ok(mut open) = bridge.tui.lock() {
                    open.remove(&session);
                }
            }
            bridge.remember(
                &session,
                &format!("sent keys `{}`", one_line(&keys, 400)),
                false,
            );
            let mut message = format!(
                "Keys sent to {} ({}).",
                bridge.display_name(&session),
                bridge.label(&session)
            );
            if args
                .get("include_screen")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                // Give a TUI a moment to repaint before sampling the screen.
                std::thread::sleep(Duration::from_millis(300));
                let screen = output_tail(app, &session, 20_000);
                message.push_str("\n--- screen ---\n");
                message.push_str(&clean_screen(&screen));
            }
            message
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
            let cleaned = clean_screen(&bytes);
            let body = if cleaned.trim().is_empty() {
                "(no output captured yet)".to_string()
            } else {
                let lines: Vec<&str> = cleaned.lines().collect();
                let take = args
                    .get("lines")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .clamp(1, 200) as usize;
                if take == 0 || lines.len() <= take {
                    cleaned
                } else {
                    lines[lines.len() - take..].join("\n")
                }
            };
            format!(
                "VPS: {} ({})\n{}",
                bridge.display_name(&session),
                bridge.label(&session),
                body
            )
        }
        "verify_screen" => {
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            let lines = args
                .get("lines")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(5, 120) as usize;
            screen_verification(
                app,
                bridge,
                &session,
                args.get("command").and_then(Value::as_str),
                None,
                lines,
            )
        }
        "inspect_terminal" => {
            let session = resolve_target(app, bridge, args.get("session").and_then(Value::as_str))
                .map_err(|e| (-32000, e))?;
            let take = args
                .get("lines")
                .and_then(Value::as_u64)
                .unwrap_or(30)
                .clamp(1, 120) as usize;
            // A just-opened tab often has no captured output yet. Give the
            // shell a short chance to print its prompt before we report it as
            // busy — otherwise the model stops instead of running the command.
            // This checks the screen only, so a stale job cannot block the AI.
            let idle = wait_for_shell_prompt(app, &session);
            let screen = clean_screen(&output_tail(app, &session, 200_000));
            let recent: Vec<&str> = screen.lines().collect();
            let shown = if recent.len() > take {
                recent[recent.len() - take..].join("\n")
            } else {
                screen.clone()
            };
            let prompt = screen.lines().last().unwrap_or("").trim().to_string();
            let running: Vec<String> = bridge
                .jobs
                .lock()
                .map(|jobs| {
                    jobs.values()
                        .filter(|job| job.session == session && !job.done)
                        .map(|job| format!("{} (elapsed {:?})", one_line(&job.command, 160), job.created.elapsed()))
                        .collect()
                })
                .unwrap_or_default();
            let tui_open = bridge
                .tui
                .lock()
                .map(|open| open.contains(&session))
                .unwrap_or(false);
            format!(
                "VPS: {} ({})\nShell: {}\nTUI open: {}\nIdle at prompt: {}\nRunning jobs: {}\nMemory file: {}\n--- recent screen ---\n{}",
                bridge.display_name(&session),
                bridge.label(&session),
                if prompt.is_empty() { "(unknown)" } else { &prompt },
                if tui_open { "yes — close it before running a command" } else { "no" },
                if idle { "yes" } else { "no — do not inject a new command yet" },
                if running.is_empty() { "none".to_string() } else { running.join("; ") },
                bridge.session_file(&session).display(),
                if shown.trim().is_empty() { "(no output captured yet)" } else { &shown }
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
            let _ = app.emit(
                "rterm:mcp-target",
                json!({
                    "name": bridge.display_name(&resolved),
                    "address": bridge.label(&resolved),
                }),
            );
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
        // Vault-backed tools give local agents the same reach as the built-in
        // chat. Credentials are deliberately never included: an agent only
        // needs labels/addresses to work on a VPS, and no secret is echoed
        // into a terminal or model transcript.
        "list_hosts" => vault_call(app, |app| async move {
            let state = app.state::<AppState>();
            let vault = state.unlocked_vault().await?;
            let hosts: Vec<Value> = vault
                .hosts
                .iter()
                .filter(|host| !host.deleted)
                .map(|host| crate::chat::host_view(host, false))
                .collect();
            Ok(format!(
                "{} saved host(s). Credentials are never returned.\n{}",
                hosts.len(),
                serde_json::to_string_pretty(&hosts).unwrap_or_default()
            ))
        })
        .map_err(|error| (-32000, error))?,
        "get_host" => {
            let query = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or((-32602, "name is required".into()))?
                .to_string();
            vault_call(app, move |app| async move {
                let state = app.state::<AppState>();
                let vault = state.unlocked_vault().await?;
                let host = crate::chat::find_host(&vault, &query).ok_or_else(|| {
                    format!(
                        "No saved host matches '{query}'. Available: {}",
                        vault
                            .hosts
                            .iter()
                            .filter(|host| !host.deleted)
                            .map(|host| host.label.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
                Ok(format!(
                    "Host details (credentials omitted):\n{}",
                    serde_json::to_string_pretty(&crate::chat::host_view(host, false))
                        .unwrap_or_default()
                ))
            })
            .map_err(|error| (-32000, error))?
        }
        "list_notes" => vault_call(app, |app| async move {
            let state = app.state::<AppState>();
            let vault = state.unlocked_vault().await?;
            let notes: Vec<Value> = vault
                .notes
                .iter()
                .filter(|note| !note.deleted)
                .map(|note| {
                    json!({
                        "id": note.id,
                        "favorite": note.favorite,
                        "updated_at": note.updated_at,
                        "preview": note.body.chars().take(140).collect::<String>(),
                    })
                })
                .collect();
            Ok(format!(
                "{} note(s).\n{}",
                notes.len(),
                serde_json::to_string_pretty(&notes).unwrap_or_default()
            ))
        })
        .map_err(|error| (-32000, error))?,
        "read_note" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() && query.is_empty() {
                return Err((-32602, "id or query is required".into()));
            }
            vault_call(app, move |app| async move {
                let state = app.state::<AppState>();
                let vault = state.unlocked_vault().await?;
                let wanted = query.to_lowercase();
                let matches: Vec<&Note> = vault
                    .notes
                    .iter()
                    .filter(|note| !note.deleted)
                    .filter(|note| {
                        (!id.is_empty() && note.id.to_string() == id)
                            || (!wanted.is_empty()
                                && note.body.to_lowercase().contains(&wanted))
                    })
                    .take(5)
                    .collect();
                if matches.is_empty() {
                    return Ok("No matching note.".to_string());
                }
                Ok(matches
                    .iter()
                    .map(|note| format!("--- note {} ---\n{}", note.id, note.body))
                    .collect::<Vec<_>>()
                    .join("\n\n"))
            })
            .map_err(|error| (-32000, error))?
        }
        "create_note" => {
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .chars()
                .take(80)
                .collect::<String>();
            let content = args
                .get("body")
                .and_then(Value::as_str)
                .ok_or((-32602, "body is required".into()))?
                .trim()
                .chars()
                .take(50_000)
                .collect::<String>();
            if content.is_empty() {
                return Err((-32602, "body must not be empty".into()));
            }
            let favorite = args.get("favorite").and_then(Value::as_bool).unwrap_or(false);
            vault_call(app, move |app| async move {
                let state = app.state::<AppState>();
                let body = if title.is_empty() {
                    content
                } else {
                    format!("# {title}\n\n{content}")
                };
                let label = if title.is_empty() {
                    crate::chat::note_preview(&body)
                } else {
                    title.clone()
                };
                let id = uuid::Uuid::new_v4();
                let note_id = id.to_string();
                let stamp = now();
                let mut vault = state.unlocked_vault().await?;
                vault.notes.push(Note {
                    id,
                    body,
                    favorite,
                    created_at: stamp,
                    updated_at: stamp,
                    deleted: false,
                });
                let payload = crate::commands::persist_vault(&state, vault).await?;
                let _ = app.emit("rterm:note-created", json!({ "id": note_id, "payload": payload }));
                Ok(format!(
                    "Note '{label}' saved in your encrypted vault and opened in a new tab."
                ))
            })
            .map_err(|error| (-32000, error))?
        }
        "update_note" => {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or((-32602, "id is required".into()))?
                .trim()
                .to_string();
            let body = args
                .get("body")
                .and_then(Value::as_str)
                .ok_or((-32602, "body is required".into()))?
                .chars()
                .take(50_000)
                .collect::<String>();
            if body.is_empty() {
                return Err((-32602, "body must not be empty".into()));
            }
            let append = args.get("append").and_then(Value::as_bool).unwrap_or(false);
            let favorite = args.get("favorite").and_then(Value::as_bool);
            vault_call(app, move |app| async move {
                let state = app.state::<AppState>();
                let mut vault = state.unlocked_vault().await?;
                let note = vault
                    .notes
                    .iter_mut()
                    .find(|note| !note.deleted && note.id.to_string() == id)
                    .ok_or_else(|| {
                        format!("No note with id '{id}'. Use list_notes to find it.")
                    })?;
                if append {
                    if !note.body.is_empty() && !note.body.ends_with('\n') {
                        note.body.push('\n');
                    }
                    note.body.push_str(&body);
                } else {
                    note.body = body;
                }
                if let Some(favorite) = favorite {
                    note.favorite = favorite;
                }
                note.updated_at = now();
                let note_id = note.id.to_string();
                let label = crate::chat::note_preview(&note.body);
                let payload = crate::commands::persist_vault(&state, vault).await?;
                let _ = app.emit("rterm:note-updated", json!({ "id": note_id, "payload": payload }));
                Ok(format!(
                    "Note '{label}' updated in your encrypted vault. Its tab has been refreshed."
                ))
            })
            .map_err(|error| (-32000, error))?
        }
        _ => return Err((-32602, format!("unknown tool: {name}"))),
    };
    Ok(json!({
        "content": [{ "type": "text", "text": result }],
        "isError": false
    }))
}

/// Call one MCP tool from another Rust module (RuTerm AI Chat). Returns the
/// same plain text an external agent would see, or a clean error message.
pub(crate) fn invoke_tool(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let response = call_tool(app, bridge, name, args).map_err(|(_, message)| message)?;
    let text = response
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if response.get("isError").and_then(Value::as_bool).unwrap_or(false) {
        return Err(text);
    }
    Ok(text)
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
    let _guard = bridge
        .write_lock
        .lock()
        .map_err(|_| "terminal write lock is poisoned".to_string())?;
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

/// Write a whole command only after the shell looks idle. Navigation keys in a
/// TUI keep using `write_target` directly — they must not wait for a prompt.
fn write_command(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    text: &str,
    requested: Option<&str>,
) -> Result<(String, bool), (i64, String)> {
    let session = resolve_target(app, bridge, requested).map_err(|e| (-32000, e))?;
    let tui_open = bridge
        .tui
        .lock()
        .map(|open| open.contains(&session))
        .unwrap_or(false);
    if tui_open {
        // A command typed into htop/vim never reaches the shell. Exit the TUI
        // first — this is enforced by the runtime, not left to the model.
        write_target(app, bridge, Some(&session), "\u{3}").map_err(|e| (-32000, e))?;
        if let Ok(mut open) = bridge.tui.lock() {
            open.remove(&session);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let mut ready = wait_for_prompt(app, bridge, &session);
    if !ready {
        let screen = clean_screen(&output_tail(app, &session, 8_000));
        // A visible prompt with trailing text is the user's partial input.
        // Clear that line (Ctrl-U) before typing so commands never get glued
        // to it and silently fail.
        if screen.lines().last().is_some_and(|line| {
            let line = line.trim();
            (line.contains("$ ") || line.contains("# ") || line.contains("❯") || line.contains("➜"))
                && line.len() < 200
        }) {
            write_target(app, bridge, Some(&session), "\u{15}")
                .map_err(|e| (-32000, e))?;
            std::thread::sleep(Duration::from_millis(150));
            ready = wait_for_prompt(app, bridge, &session);
        }
    }
    write_target(app, bridge, Some(&session), text).map_err(|e| (-32000, e))?;
    Ok((session, ready))
}

fn prompt_warning(ready: bool) -> &'static str {
    if ready {
        ""
    } else {
        "\nWarning: the shell prompt was not detected before typing; verify the command reached the prompt."
    }
}

/// Start a remote command and return a job id immediately. The command is
/// typed into the user's SSH tab, but the caller does not block waiting for it.
fn start_job(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    command: &str,
    requested: Option<&str>,
    timeout_ms: u64,
    use_marker: bool,
) -> Result<String, (i64, String)> {
    if nested_ssh_command(command) {
        return Err((
            -32600,
            "Already inside the SSH tab — do not run a nested `ssh` command. Run the command directly in this VPS."
                .into(),
        ));
    }
    let session = resolve_target(app, bridge, requested).map_err(|e| (-32000, e))?;
    gate(app, bridge, &session, command)?;
    let since = app
        .state::<AppState>()
        .output
        .lock()
        .ok()
        .and_then(|map| map.get(&session).map(|entry| entry.total))
        .unwrap_or(0);

    // The PTY echoes what we type. A unique marker plus "last occurrence" wins
    // over the echoed command text when we later parse the real exit status.
    // Short chat commands finish by watching the prompt return, so no status
    // marker is written to the user's terminal. Long streaming jobs still use
    // a marker to capture the exact exit code.
    let marker = if use_marker {
        format!("__RUTERM_DONE_{}__", &Uuid::new_v4().simple().to_string()[..10])
    } else {
        String::new()
    };
    // A trailing `&&`/`;`/backslash would produce a shell syntax error once
    // we append the exit-status line. Strip it before building the script.
    let base = command
        .trim_end()
        .trim_end_matches(|c: char| matches!(c, '&' | ';' | '|' | '\\'))
        .trim_end();
    let script = if use_marker {
        format!("{base}\nprintf '\\n{}:%s\\n' \"$?\"\n", marker)
    } else {
        format!("{base}\n")
    };
    let (session, ready) = write_command(app, bridge, &script, Some(&session))?;

    let id = Uuid::new_v4().simple().to_string()[..10].to_string();
    let now = Instant::now();
    let deadline = normalize_timeout(timeout_ms).map(|d| now + d);
    if let Ok(mut jobs) = bridge.jobs.lock() {
        // Finished jobs stay readable for an hour; no unbounded growth.
        jobs.retain(|_, job| job.created.elapsed() < Duration::from_secs(3_600));
        jobs.insert(
            id.clone(),
            StreamJob {
                session,
                command: command.to_string(),
                marker,
                since,
                consumed: 0,
                last_offset: 0,
                buffer: if ready {
                    Vec::new()
                } else {
                    b"[RuTerm] shell prompt was not detected before typing; verify the command reached the prompt.\n".to_vec()
                },
                done: false,
                exit_status: String::new(),
                deadline,
                created: now,
                last_activity: now,
            },
        );
    }
    Ok(id)
}

fn job_view(job: &StreamJob, job_id: &str, bridge: &Bridge, offset: u64) -> JobView {
    // The PTY echoes the sentinel line we append for the exit status, often
    // *after* the command's own output. Hide it from agents; the real marker
    // was already cut off when the job completed.
    let cleaned = strip_sentinel_echo(&job.buffer, &job.marker);
    let start = offset.min(cleaned.len() as u64) as usize;
    JobView {
        job: job_id.to_string(),
        session: job.session.clone(),
        command: job.command.clone(),
        vps: bridge.display_name(&job.session),
        address: bridge.label(&job.session),
        chunk: cleaned[start..].to_vec(),
        offset,
        next_offset: cleaned.len() as u64,
        done: job.done,
        exit_status: job.exit_status.clone(),
        elapsed: job.created.elapsed(),
    }
}

/// Models often pass the command text instead of the short job id returned by
/// `start_execute`. Accept both, plus the newest job for the active tab, and
/// list what is actually available when nothing matches.
fn resolve_job_id(bridge: &Bridge, query: &str) -> Result<String, (i64, String)> {
    let jobs = bridge
        .jobs
        .lock()
        .map_err(|_| (-32000, "stream job lock failed".into()))?;
    let q = query.trim();
    if jobs.contains_key(q) {
        return Ok(q.to_string());
    }
    if let Some((id, _)) = jobs.iter().find(|(_, job)| job.command.trim() == q) {
        return Ok(id.clone());
    }
    let mut all: Vec<(&String, &StreamJob)> = jobs.iter().collect();
    all.sort_by(|a, b| b.1.created.cmp(&a.1.created));
    let needle = q.to_lowercase();
    if let Some((id, _)) = all.iter().find(|(_, job)| {
        let command = job.command.to_lowercase();
        command.contains(&needle) || needle.contains(command.as_str())
    }) {
        return Ok((*id).clone());
    }
    // The query looks like a shell command, so the newest job on the focused
    // tab is almost certainly what the model meant.
    if q.contains(' ') || q.contains('/') || q.contains('-') {
        let active = bridge.target_id();
        if let Some((id, _)) = all
            .iter()
            .find(|(_, job)| active.as_deref() == Some(job.session.as_str()))
        {
            return Ok((*id).clone());
        }
    }
    let available = all
        .iter()
        .map(|(id, job)| format!("{} ({})", id, one_line(&job.command, 80)))
        .collect::<Vec<_>>()
        .join(", ");
    Err((
        -32000,
        format!(
            "stream job `{query}` is not available. Available jobs: {}",
            if available.is_empty() { "none" } else { &available }
        ),
    ))
}

/// Poll one streaming job. With `wait_ms > 0` this long-polls, returning as
/// soon as new bytes arrive (or the job finishes), so agents can follow apt or
/// build progress chunk by chunk instead of waiting for the final result.
fn poll_job(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    job_id: &str,
    offset: u64,
    wait_ms: u64,
) -> Result<JobView, (i64, String)> {
    let wait_until = Instant::now() + Duration::from_millis(wait_ms.min(60_000));
    loop {
        let (view, just_completed) = {
            let mut jobs = bridge
                .jobs
                .lock()
                .map_err(|_| (-32000, "stream job lock failed".into()))?;
            let job = jobs
                .get_mut(job_id)
                .ok_or_else(|| (-32000, format!("stream job `{job_id}` is not available")))?;

            if job.done {
                (job_view(job, job_id, bridge, offset), false)
            } else {
                let session = job.session.clone();
                let marker = job.marker.clone();
                let bytes = output_since(app, &session, job.since);
                let consumed = job.consumed as usize;
                let fresh = if bytes.len() > consumed {
                    &bytes[consumed..]
                } else {
                    &bytes[bytes.len()..]
                };
                if !fresh.is_empty() {
                    // Use the LAST marker: the first one is in the echoed command.
                    // Short jobs have no marker; rfind_bytes ignores empty needles.
                    if let Some(pos) = rfind_bytes(fresh, marker.as_bytes()) {
                        job.buffer.extend_from_slice(&fresh[..pos]);
                        let tail = &fresh[pos..];
                        job.exit_status = tail
                            .get(marker.len()..)
                            .and_then(|rest| {
                                let text = String::from_utf8_lossy(rest);
                                let line = text.lines().next().unwrap_or("");
                                line.strip_prefix(':').map(str::trim).map(str::to_string)
                            })
                            .unwrap_or_default();
                        job.done = true;
                    } else {
                        job.buffer.extend_from_slice(fresh);
                    }
                    job.consumed = bytes.len() as u64;
                    job.last_activity = Instant::now();
                }
                // Short commands use no marker: a fresh prompt after the
                // echoed command is the proof that the command finished.
                if !job.done
                    && marker.is_empty()
                    && job.consumed > 0
                    && prompt_ready(&clean_screen(fresh))
                {
                    job.done = true;
                    job.exit_status = "prompt returned".into();
                }
                if !job.done {
                    if let Some(deadline) = job.deadline {
                        if Instant::now() >= deadline {
                            job.done = true;
                            job.exit_status = "timeout".into();
                        }
                    }
                }
                // If the shell is visibly back at its prompt and no new bytes
                // have arrived, the command is over even when the exit marker
                // was lost. Otherwise one missed marker leaves the tab marked
                // busy forever and the model refuses to run anything else.
                if !job.done
                    && job.consumed > 0
                    && job.last_activity.elapsed() > Duration::from_secs(3)
                    && prompt_ready(&clean_screen(&output_tail(app, &session, 8_000)))
                {
                    job.done = true;
                    job.exit_status = "prompt returned".into();
                    job.buffer
                        .extend_from_slice(b"\n[prompt returned; exit status not captured]\n");
                }
                let completed = job.done;
                (job_view(job, job_id, bridge, offset), completed)
            }
        };

        if just_completed {
            // One extra poll at offset 0 gives the full body for the work log.
            let full = poll_job(app, bridge, job_id, 0, 0)?;
            let body = strip_ansi(&full.chunk);
            let status = if full.exit_status.is_empty() {
                "unknown".to_string()
            } else {
                full.exit_status.clone()
            };
            bridge.remember(
                &full.session,
                &format!(
                    "`{}` → exit {} — {}",
                    one_line(&full.command, 400),
                    status,
                    one_line(&body, 1200)
                ),
                false,
            );
            return Ok(view);
        }
        if view.next_offset > offset || view.done || Instant::now() >= wait_until {
            return Ok(view);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Stop a streaming job by sending Ctrl-C to the same VPS tab.
fn cancel_job(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    job_id: &str,
) -> Result<String, (i64, String)> {
    let (session, command) = {
        let jobs = bridge
            .jobs
            .lock()
            .map_err(|_| (-32000, "stream job lock failed".into()))?;
        let job = jobs
            .get(job_id)
            .ok_or_else(|| (-32000, format!("stream job `{job_id}` is not available")))?;
        (job.session.clone(), job.command.clone())
    };
    write_target(app, bridge, Some(&session), "\u{3}").map_err(|e| (-32000, e))?;
    let view = {
        let mut jobs = bridge
            .jobs
            .lock()
            .map_err(|_| (-32000, "stream job lock failed".into()))?;
        let job = jobs
            .get_mut(job_id)
            .ok_or_else(|| (-32000, format!("stream job `{job_id}` is not available")))?;
        if !job.done {
            job.done = true;
            job.exit_status = "cancelled".into();
            job.buffer.extend_from_slice(b"\n[cancelled by user]\n");
        }
        job_view(job, job_id, bridge, 0)
    };
    bridge.remember(
        &session,
        &format!("`{}` → cancelled by user", one_line(&command, 400)),
        false,
    );
    Ok(format!(
        "VPS: {} ({})\njob: {job_id}\ncancelled. Final output is available via ruterm_read_stream.",
        view.vps, view.address
    ))
}

/// Read the real terminal screen and report objective evidence that an action
/// happened. This is what lets the AI say "sudah/belum" from the VPS itself.
fn screen_verification(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    session: &str,
    command: Option<&str>,
    since: Option<u64>,
    lines: usize,
) -> String {
    let cleaned = clean_screen(&output_tail(app, session, 30_000));
    let all: Vec<&str> = cleaned.lines().collect();
    let shown = if all.len() > lines {
        all[all.len() - lines..].join("\n")
    } else {
        cleaned.clone()
    };
    let prompt = prompt_ready(&cleaned);
    let fresh = since.map(|value| output_since(app, session, value)).unwrap_or_default();
    let new_text = strip_ansi(&fresh);
    let typed = command
        .filter(|value| !value.trim().is_empty())
        .map(|value| cleaned.contains(value.trim()))
        .unwrap_or(false);
    let output_seen = since.is_some() && new_text.trim().lines().count() > 1;
    let tui_open = bridge
        .tui
        .lock()
        .map(|open| open.contains(session))
        .unwrap_or(false);
    let verdict = if let Some(expected) = command {
        if typed && output_seen && prompt && !tui_open {
            "VERIFIED: the command is visible on the VPS screen, new output was captured, \
             and the shell is back at its prompt."
                .to_string()
        } else {
            let mut reasons = Vec::new();
            if !typed {
                reasons.push(format!("expected command `{}` is not on screen", one_line(expected, 120)));
            }
            if !output_seen {
                reasons.push("no new output beyond the echoed command".to_string());
            }
            if !prompt {
                reasons.push("shell is not at a clean prompt".to_string());
            }
            if tui_open {
                reasons.push("a TUI is still open".to_string());
            }
            format!("NOT VERIFIED: {}.", reasons.join("; "))
        }
    } else if prompt && !tui_open {
        "Screen read: shell is at a clean prompt. No expected command was supplied, so execution \
         cannot be confirmed."
            .to_string()
    } else {
        "Screen read: shell is busy or a TUI is open. Do not inject a command yet.".to_string()
    };
    format!(
        "--- screen check ---\nverdict: {verdict}\ncommand visible: {typed}\n\
         new output bytes: {}\nprompt ready: {prompt}\nTUI open: {tui_open}\n\
         --- screen tail ---\n{}",
        fresh.len(),
        if shown.trim().is_empty() { "(no output captured)".to_string() } else { shown }
    )
}

/// Blocking wrapper for short commands; long commands should use
/// `start_execute` + `read_stream` instead.
fn execute(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    command: &str,
    requested: Option<&str>,
    timeout_ms: u64,
) -> Result<String, (i64, String)> {
    let job_id = start_job(app, bridge, command, requested, timeout_ms, false)?;
    let (job_session, job_since) = bridge
        .jobs
        .lock()
        .ok()
        .and_then(|jobs| {
            jobs.get(&job_id)
                .map(|job| (job.session.clone(), job.since))
        })
        .unwrap_or_default();
    let mut offset = 0;
    loop {
        let view = poll_job(app, bridge, &job_id, offset, 1_000)?;
        offset = view.next_offset;
        if view.done {
            let full = poll_job(app, bridge, &job_id, 0, 0)?;
            let text = strip_ansi(&full.chunk);
            let body = if text.trim().is_empty() {
                "(command produced no output)".to_string()
            } else {
                text
            };
            let header = format!("VPS: {} ({})", full.vps, full.address);
            let check = screen_verification(
                app,
                bridge,
                &job_session,
                Some(command),
                Some(job_since),
                12,
            );
            if full.exit_status == "timeout" {
                return Ok(format!(
                    "{header}\n[timed out after {timeout_ms} ms; command may still be running]\n{body}\n{check}"
                ));
            }
            let status = if full.exit_status.is_empty() {
                "unknown".to_string()
            } else {
                full.exit_status.clone()
            };
            return Ok(format!("{header}\n{body}\n[exit status: {status}]\n{check}"));
        }
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

/// Screen-only idle check. Used for reporting: a stale job entry must not make
/// a shell that is clearly back at its prompt look busy to the model.
fn wait_for_shell_prompt(app: &tauri::AppHandle, session: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if prompt_ready(&clean_screen(&output_tail(app, session, 8_000))) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Wait briefly until the shell is idle at a prompt before injecting a new
/// command. Returns false on an unusual prompt — callers then still proceed
/// but warn the model, so a non-standard shell never blocks the chat.
fn wait_for_prompt(app: &tauri::AppHandle, bridge: &Bridge, session: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let busy_job = bridge
            .jobs
            .lock()
            .map(|jobs| {
                jobs.values()
                    .any(|job| job.session == session && !job.done)
            })
            .unwrap_or(false);
        if !busy_job && prompt_ready(&clean_screen(&output_tail(app, session, 8_000))) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Like `find_bytes`, but returns the last match. A PTY echoes the command we
/// type, so the first marker hit is the echoed text, not the command's result.
fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).rposition(|part| part == needle)
}

/// Remove the echoed `printf '\\n<marker>:%s\\n' "$?"` line from captured
/// output. The terminal echoes everything we type, and because the shell
/// processes our two-line script one line at a time, that echo frequently
/// appears after the command's real output. Keeping it would leak internal
/// plumbing into the agent's context.
fn strip_sentinel_echo(raw: &[u8], marker: &str) -> Vec<u8> {
    let needle = format!("printf '\\n{}:%s\\n' \"$?\"", marker);
    let Some(pos) = raw.windows(needle.len()).position(|part| part == needle.as_bytes()) else {
        return raw.to_vec();
    };
    // A shell prompt usually precedes the echo on the same line; drop the
    // whole line, not just the sentinel text.
    let start = raw[..pos]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut end = pos + needle.len();
    while end < raw.len() && matches!(raw[end], b'\r' | b'\n') {
        end += 1;
    }
    let mut out = raw[..start].to_vec();
    out.extend_from_slice(&raw[end..]);
    out
}

/// Minimal ANSI/osc stripper: enough to give an agent readable plain text
/// without pulling a regex or terminal-emulator dependency into the app.
/// Keeps carriage returns so `clean_screen` can resolve redraws; plain callers
/// use `strip_ansi` below.
fn strip_ansi_raw(input: &[u8]) -> String {
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
    String::from_utf8_lossy(&out).to_string()
}

fn strip_ansi(input: &[u8]) -> String {
    strip_ansi_raw(input).replace('\r', "").trim_end().to_string()
}

/// Readable snapshot of what is on screen right now. Resolves carriage-return
/// redraws (progress bars, status lines) instead of concatenating every frame,
/// and drops leftover control characters so the model sees clean text.
fn clean_screen(raw: &[u8]) -> String {
    let text = strip_ansi_raw(raw).replace("\r\n", "\n");
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let frame = line.rsplit('\r').next().unwrap_or("");
            frame
                .chars()
                .filter(|c| !c.is_control() || *c == '\t')
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Heuristic: is the shell sitting at a prompt and ready for a command?
fn prompt_ready(screen: &str) -> bool {
    screen.lines().last().is_some_and(|line| {
        let line = line.trim();
        if line.is_empty() || line.len() > 160 {
            return false;
        }
        let lower = line.to_ascii_lowercase();
        // The prompt must be the LAST thing on the line. A loose `contains`
        // check mistakes `root@host:~# half-typed-text` for an idle shell and
        // appends our command to the user's partial input.
        let looks_prompt = line.ends_with('$')
            || line.ends_with('#')
            || line.ends_with('❯')
            || line.ends_with('➜')
            || line.ends_with('>')
            || (line.contains('➜') && line.ends_with('~'));
        looks_prompt && !lower.contains("password:")
    })
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
            jobs: Arc::new(Mutex::new(HashMap::new())),
            allowlist: Arc::new(Mutex::new(HashSet::new())),
            write_lock: Arc::new(Mutex::new(())),
            tui: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    #[test]
    fn stream_jobs_can_be_found_by_command_text() {
        let bridge = test_bridge();
        let now = Instant::now();
        bridge.jobs.lock().unwrap().insert(
            "abc123".into(),
            StreamJob {
                session: "s1".into(),
                command: "grep -rhnE \"server_name|listen \" /etc/nginx".into(),
                marker: "m".into(),
                since: 0,
                consumed: 0,
                last_offset: 0,
                buffer: Vec::new(),
                done: false,
                exit_status: String::new(),
                deadline: None,
                created: now,
                last_activity: now,
            },
        );
        assert_eq!(resolve_job_id(&bridge, "abc123").unwrap(), "abc123");
        assert_eq!(
            resolve_job_id(&bridge, "grep -rhnE \"server_name|listen \" /etc/nginx").unwrap(),
            "abc123"
        );
        assert_eq!(resolve_job_id(&bridge, "server_name").unwrap(), "abc123");
        let error = resolve_job_id(&bridge, "totally other").unwrap_err().1;
        assert!(error.contains("Available jobs"), "{error}");
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
    fn strips_echoed_exit_sentinel_from_output() {
        let marker = "__RUTERM_DONE_abc__";
        let raw = format!(
            "echo hi\r\nhi\r\nroot@host:~# printf '\\n{}:%s\\n' \"$?\"\r\n",
            marker
        );
        let cleaned = strip_sentinel_echo(raw.as_bytes(), marker);
        assert_eq!(String::from_utf8_lossy(&cleaned), "echo hi\r\nhi\r\n");
        // Output without a sentinel must pass through untouched.
        let plain = b"echo hi\r\nhi\r\n";
        assert_eq!(strip_sentinel_echo(plain, marker), plain.to_vec());
    }

    #[test]
    fn timeout_zero_means_no_limit() {
        assert_eq!(normalize_timeout(0), None);
        assert_eq!(normalize_timeout(500), Some(Duration::from_millis(1_000)));
        assert_eq!(normalize_timeout(5_000_000), Some(Duration::from_secs(3_600)));
    }

    #[test]
    fn display_name_prefers_saved_host_name() {
        let bridge = test_bridge();
        bridge.note_open("s1", "root@10.0.0.1:22", "production-api");
        assert_eq!(bridge.display_name("s1"), "production-api");
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
    fn terminal_keys_turns_model_phrases_into_real_enter() {
        assert_eq!(terminal_keys("htopEnter"), "htop\n");
        assert_eq!(terminal_keys("htop Enter"), "htop\n");
        assert_eq!(terminal_keys("htop<Enter>"), "htop\n");
        assert_eq!(terminal_keys("htop\\n"), "htop\n");
        assert_eq!(terminal_keys("q"), "q");
    }

    #[test]
    fn clean_screen_resolves_redraws_and_detects_prompts() {
        let raw = b"\x1b[32mroot@vps:~#\x1b[0m uptime\r\n 10:01:02 up 1 day\r 10:01:03 up 2 days\n";
        let clean = clean_screen(raw);
        assert!(clean.contains("root@vps:~#"));
        assert!(clean.contains("10:01:03 up 2 days"), "last CR frame must win");
        assert!(!clean.contains("10:01:02"), "stale frame must be dropped");
        assert!(prompt_ready("root@vps:~#"));
        assert!(prompt_ready("user@host:~$"));
        assert!(!prompt_ready("Password: "));
        assert!(!prompt_ready("apt update"));
        assert!(!prompt_ready("root@vps:~# kalsdalskdma"));
    }

    #[test]
    fn tui_launches_are_distinguished_from_navigation() {
        for keys in ["htop", "htopEnter", "htop<Enter>", "sudo htop", "tail -f /var/log/syslog"] {
            assert!(looks_like_tui_launch(keys), "missed {keys}");
        }
        for keys in ["q", "j", "k", "ctrl-c", ":wq", "", "echo htop"] {
            assert!(!looks_like_tui_launch(keys), "false positive: {keys}");
        }
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

    #[test]
    fn agents_get_saved_host_and_note_tools() {
        let tools = tool_definitions();
        let names: Vec<&str> = tools
            .as_array()
            .expect("tool list")
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or(""))
            .collect();
        for expected in [
            "list_targets",
            "list_hosts",
            "get_host",
            "list_notes",
            "read_note",
            "create_note",
            "update_note",
        ] {
            assert!(names.contains(&expected), "missing tool: {expected}");
        }
        // Saved hosts must not be confused with open tabs, or agents repeat
        // the "no VPS is open" mistake when asked for the host list.
        let description = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "list_hosts")
            .unwrap()["description"]
            .as_str()
            .unwrap();
        assert!(description.contains("SAVED"));
        assert!(description.contains("list_targets"));
        assert!(VAULT_TOOLS_HINT.contains("ruterm_list_hosts"));
        assert!(VAULT_TOOLS_HINT.contains("ruterm_create_note"));
        assert!(VAULT_TOOLS_HINT.contains("ruterm_update_note"));
    }
}
