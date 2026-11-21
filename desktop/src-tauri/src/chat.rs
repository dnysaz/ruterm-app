//! BYOK AI chat backed by OpenRouter.
//!
//! The API key lives in the encrypted vault: Rust opens it at call time and
//! never sends it to the renderer. Tools give the model read access to saved
//! hosts, notes, snippets and per-VPS work logs; credentials and config writes
//! need an explicit approval in the UI.

use crate::{commands, state::AppState};
use chrono::Utc;
use rterm_core::model::{now, AuthMethod, Host, Snippet};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;
use uuid::Uuid;

const OPENROUTER_CHAT: &str = "https://openrouter.ai/api/v1/chat/completions";
const OPENROUTER_MODELS: &str = "https://openrouter.ai/api/v1/models";
const MAX_TURNS: usize = 6;
const BEGIN_MARKER: &str = "# BEGIN RUTERM AI CHAT";
const END_MARKER: &str = "# END RUTERM AI CHAT";
/// Agent names must never be mistaken for SSH hosts by `create_host`.
const AGENT_NAMES: &[&str] = &[
    "opencode", "freebuff", "kilo", "kilo code", "claude", "kiro", "kiro-cli", "cmd",
    "command ai",
];

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalChoice {
    Deny,
    Allow,
    Always,
}

/// One approval request waiting for the drawer. Dropping its sender denies it.
pub struct PendingApproval {
    pub session: String,
    pub dedupe: String,
    pub tx: oneshot::Sender<ApprovalChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    fn plain(role: &str, content: String) -> Self {
        Self {
            role: role.into(),
            content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatConfig {
    pub configured: bool,
    pub model: String,
    pub allow_credentials: bool,
}

/// A model offered by OpenRouter, for the Settings dropdown.
#[derive(Debug, Clone, Serialize)]
pub struct ChatModel {
    pub id: String,
    pub name: String,
}

#[derive(Default)]
struct Call {
    id: String,
    name: String,
    arguments: String,
}

fn client() -> Result<reqwest::Client, String> {
    // OpenRouter asks apps to identify themselves; some providers require it
    // for free-tier requests.
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = "https://github.com/dnysaz/ruterm-app".parse() {
        headers.insert("HTTP-Referer", value);
    }
    if let Ok(value) = "RuTerm".parse() {
        headers.insert("X-Title", value);
    }
    reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .default_headers(headers)
        .build()
        .map_err(|error| format!("Could not build HTTP client: {error}"))
}

/// Read the key from the unlocked vault. `open_vault_inner` already decrypted
/// this field at unlock time, so decrypting it again would fail — only the
/// on-disk file holds ciphertext.
async fn openrouter_key(state: &AppState) -> Result<String, String> {
    let vault = state.unlocked_vault().await?;
    let key = vault.settings.openrouter_api_key.trim().to_string();
    if key.is_empty() {
        return Err("OpenRouter API key is not set in Settings".into());
    }
    Ok(key)
}

/// Transient connect/TLS hiccups are common on macOS networks. Retry a couple
/// of times before surfacing an error in the chat, without retrying HTTP
/// status errors (those already carry a useful OpenRouter message).
async fn send_with_retry<F>(build: F) -> Result<reqwest::Response, String>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut last = String::new();
    for attempt in 0..3u32 {
        match build().send().await {
            Ok(response) => return Ok(response),
            Err(error) => {
                last = format!("Network error: {error}");
                if attempt + 1 < 3 {
                    tokio::time::sleep(Duration::from_millis(400 * u64::from(attempt + 1))).await;
                }
            }
        }
    }
    Err(last)
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn system_prompt() -> String {
    "You are RuTerm's built-in assistant. You can read the user's saved SSH hosts, notes, \
     snippets, groups, per-VPS work logs, app preferences, Google/sync status and the AI agent \
     CLIs installed on this Mac. You can also switch the interface theme between dark and light, \
     launch a detected AI agent in a local tab, and save a new SSH host and open it in a terminal \
     tab. \
     Always call a tool for facts — never invent host names, addresses, note contents, settings \
     or sync state. Credential fields are omitted by default and only appear after the user \
     explicitly approves a read; API keys and tokens are never exposed. Other config writes \
     require approval. When the user asks to change the theme, call `set_theme` immediately — do \
     not merely say that you can. When the user asks to open/start an AI agent from `list_agents` \
     (OpenCode, Freebuff, Kilo, Claude, Kiro, Command AI), call `launch_agent` with its id — never \
     call `create_host` for an agent name. `create_host` is ONLY for a real SSH host with an IP or \
     hostname. When the user pastes an address/IP, port, user or password and asks to save/open a \
     host, call `create_host` with those exact values; it asks for confirmation and opens the SSH \
     tab automatically. Never invent a password. Always answer in the same \
     language as the user's latest message \
     (Indonesian in, Indonesian out). Be concise and refer to hosts by their saved labels rather \
     than internal ids."
        .into()
}

fn tools() -> Value {
    json!([
      {
        "type": "function",
        "function": {
          "name": "list_hosts",
          "description": "List every saved SSH host with label, address, port, username, tags and notes. Credentials are omitted unless include_credentials is true (requires approval).",
          "parameters": { "type": "object", "properties": {
            "include_credentials": { "type": "boolean", "description": "Ask to include passwords, private keys and env values." }
          } }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "get_host",
          "description": "Get full details for one saved host by label, address, username or id.",
          "parameters": { "type": "object", "properties": {
            "name": { "type": "string" },
            "include_credentials": { "type": "boolean" }
          }, "required": ["name"] }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "list_notes",
          "description": "List saved notes with their id and opening words.",
          "parameters": { "type": "object", "properties": {} }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "read_note",
          "description": "Read a note by id, or find notes containing a query string.",
          "parameters": { "type": "object", "properties": {
            "id": { "type": "string" }, "query": { "type": "string" }
          } }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "list_snippets",
          "description": "List saved command snippets.",
          "parameters": { "type": "object", "properties": {} }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "list_groups",
          "description": "List host groups.",
          "parameters": { "type": "object", "properties": {} }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "read_session",
          "description": "Read the persistent work log for a VPS by host label or address. Omit name to list available memory files.",
          "parameters": { "type": "object", "properties": { "name": { "type": "string" } } }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "create_snippet",
          "description": "Create a reusable snippet in the encrypted vault. Requires approval; the snippet syncs with the vault.",
          "parameters": { "type": "object", "properties": {
            "label": { "type": "string" }, "content": { "type": "string" },
            "tags": { "type": "array", "items": { "type": "string" } }
          }, "required": ["label", "content"] }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "write_chat_config",
          "description": "Write a small config. targets: ai-configs (~/.ruterm/ai-configs), desktop (~/Desktop), ssh_config (append/replace a managed block in ~/.ssh/config with backup). Requires approval.",
          "parameters": { "type": "object", "properties": {
            "target": { "type": "string", "enum": ["ai-configs", "desktop", "ssh_config"] },
            "filename": { "type": "string", "description": "Simple filename for ai-configs/desktop targets." },
            "content": { "type": "string" }
          }, "required": ["target", "content"] }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "launch_agent",
          "description": "Launch one of the AI agents returned by list_agents in a new local terminal tab, with the RuTerm MCP bridge attached. Use this whenever the user asks to open/start an agent such as OpenCode, Freebuff, Kilo, Claude, Kiro or Command AI. Accepts the agent id, label or command.",
          "parameters": {
            "type": "object",
            "properties": {
              "agent": { "type": "string", "description": "Agent id, label or command from list_agents" }
            },
            "required": ["agent"]
          }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "create_host",
          "description": "Save a real SSH host (IP or hostname) in the encrypted vault and open it in a terminal tab. NEVER use this for AI agent names — use launch_agent instead. port defaults to 22, user defaults to root. Requires approval: the password is masked in the dialog and never appears in a tool result. Duplicate address/port/user is rejected.",
          "parameters": {
            "type": "object",
            "properties": {
              "address": { "type": "string", "description": "IP address or hostname" },
              "port": { "type": "integer", "description": "SSH port, default 22" },
              "username": { "type": "string", "description": "SSH user, default root" },
              "password": { "type": "string", "description": "SSH password, if the user supplied one" },
              "label": { "type": "string", "description": "Friendly name, defaults to user@address" },
              "notes": { "type": "string" }
            },
            "required": ["address"]
          }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "get_app_settings",
          "description": "Read non-secret RuTerm preferences: active theme, terminal type, font size, scrollback, auto-lock timeout, copy-on-select, sync interval, chat model and whether credential access is enabled. API keys, passwords and tokens are never returned.",
          "parameters": { "type": "object", "properties": {} }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "get_sync_status",
          "description": "Check Google sign-in and vault sync status: signed-in account, local revision, last synced revision and timestamp, and whether local changes are waiting to sync.",
          "parameters": { "type": "object", "properties": {} }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "list_agents",
          "description": "List the AI agent CLIs detected on this Mac (OpenCode, Freebuff, Kilo Code, Claude, Kiro, Command AI) with their resolved paths. These agents run locally and use SSH tabs through the embedded MCP bridge.",
          "parameters": { "type": "object", "properties": {} }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "set_theme",
          "description": "Switch the RuTerm interface theme on this device to dark or light. No approval needed; it only changes this app's appearance.",
          "parameters": {
            "type": "object",
            "properties": { "theme": { "type": "string", "enum": ["dark", "light"] } },
            "required": ["theme"]
          }
        }
      }
    ])
}

fn short_error(raw: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
        {
            return message.chars().take(300).collect();
        }
    }
    raw.trim().chars().take(300).collect()
}

/// Stream one completion. Content deltas are pushed to the drawer as they
/// arrive; tool-call fragments are aggregated across chunks before returning.
async fn stream_completion(
    app: &AppHandle,
    session: &str,
    http: &reqwest::Client,
    key: &str,
    model: &str,
    messages: &[ChatMessage],
    allow_tools: bool,
) -> Result<(String, Vec<Call>), String> {
    let mut body = json!({
        "model": model,
        "stream": true,
        "messages": messages,
    });
    if allow_tools {
        body["tools"] = tools();
        body["tool_choice"] = json!("auto");
    }
    let mut response = send_with_retry(|| {
        http.post(OPENROUTER_CHAT)
            .bearer_auth(key)
            .json(&body)
    })
    .await?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let message = short_error(&text);
        // Several free models do not support tool calling. Retry as a plain
        // chat instead of failing the whole turn.
        if allow_tools
            && (status == 400 || status == 422)
            && message.to_lowercase().contains("tool")
        {
            return Box::pin(stream_completion(app, session, http, key, model, messages, false))
                .await;
        }
        return Err(format!("OpenRouter {status} — {message}"));
    }

    let mut buffer = String::new();
    let mut content = String::new();
    let mut calls: Vec<Call> = Vec::new();
    let mut saw_data = false;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Stream interrupted: {error}"))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(position) = buffer.find('\n') {
            let line: String = buffer[..position].trim().to_string();
            buffer = buffer[position + 1..].to_string();
            if !line.starts_with("data:") {
                continue;
            }
            saw_data = true;
            let payload = line.trim_start_matches("data:").trim();
            if payload == "[DONE]" {
                continue;
            }
            let value: Value = match serde_json::from_str(payload) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if let Some(error) = value.get("error") {
                return Err(format!(
                    "OpenRouter: {}",
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                ));
            }
            let Some(choice) = value.get("choices").and_then(|list| list.get(0)) else {
                continue;
            };
            let Some(delta) = choice.get("delta") else { continue };

            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    content.push_str(text);
                    let _ = app.emit(
                        "chat://delta",
                        json!({ "session": session, "text": text }),
                    );
                }
            }

            if let Some(fragments) = delta.get("tool_calls").and_then(Value::as_array) {
                for fragment in fragments {
                    let index = fragment
                        .get("index")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    while calls.len() <= index {
                        calls.push(Call::default());
                    }
                    if let Some(id) = fragment.get("id").and_then(Value::as_str) {
                        if !id.is_empty() {
                            calls[index].id = id.into();
                        }
                    }
                    let function = fragment.get("function");
                    if let Some(name) = function
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                    {
                        calls[index].name.push_str(name);
                    }
                    if let Some(arguments) = function
                        .and_then(|function| function.get("arguments"))
                        .and_then(Value::as_str)
                    {
                        calls[index].arguments.push_str(arguments);
                    }
                }
            }
        }
    }

    if !saw_data {
        return Err(format!("Unexpected OpenRouter response: {}", short_error(&buffer)));
    }
    for call in &mut calls {
        if call.id.is_empty() {
            call.id = format!("call_{}", Uuid::new_v4().simple());
        }
    }
    Ok((content, calls.into_iter().filter(|call| !call.name.is_empty()).collect()))
}

fn host_view(host: &Host, with_credentials: bool) -> Value {
    let mut view = json!({
        "label": host.label,
        "address": host.address,
        "port": host.port,
        "username": host.username,
        "group": host.group_id,
        "tags": host.tags,
        "favorite": host.favorite,
        "notes": host.notes,
        "os": host.os,
        "host_key_fingerprint": host.host_key_fingerprint,
    });
    if with_credentials {
        view["auth"] = serde_json::to_value(&host.auth).unwrap_or_else(|_| json!({}));
        view["env"] = json!(host.env);
    }
    view
}

/// Build a host entirely from model-supplied arguments. The password is kept
/// only inside the returned `Host`; the second value is a masked description
/// safe to show in an approval dialog or a tool message.
fn build_host(args: &Value) -> Result<(Host, String), String> {
    let address = args
        .get("address")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if address.is_empty() {
        return Err("address is required".into());
    }
    let lowered = address.to_lowercase();
    if AGENT_NAMES.contains(&lowered.as_str()) {
        return Err(format!(
            "'{address}' is an AI agent, not an SSH host. Use launch_agent to open it."
        ));
    }
    let port = args.get("port").and_then(Value::as_u64).unwrap_or(22);
    if port == 0 || port > u16::MAX as u64 {
        return Err("port must be between 1 and 65535".into());
    }
    let username = {
        let value = args
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if value.is_empty() {
            "root".to_string()
        } else {
            value.to_string()
        }
    };
    let label = {
        let value = args
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if value.is_empty() {
            format!("{username}@{address}")
        } else {
            value.to_string()
        }
    };
    let notes = args
        .get("notes")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let password = args
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let auth = if password.is_empty() {
        AuthMethod::Agent
    } else {
        AuthMethod::Password { password }
    };
    let auth_label = match &auth {
        AuthMethod::Agent => "SSH agent".to_string(),
        _ => "Password: •••••••• (masked)".to_string(),
    };
    let stamp = now();
    let host = Host {
        id: Uuid::new_v4(),
        label,
        address,
        port: port as u16,
        username,
        auth,
        group_id: None,
        tags: vec![],
        color: None,
        icon: None,
        favorite: false,
        notes,
        startup_snippet_id: None,
        jump_host_id: None,
        keepalive_secs: None,
        connect_timeout_secs: None,
        env: vec![],
        terminal: Default::default(),
        agent_forwarding: false,
        compression: false,
        os: None,
        host_key_fingerprint: None,
        created_at: stamp,
        updated_at: stamp,
        last_used_at: None,
        deleted: false,
    };
    Ok((host, auth_label))
}

/// Only the two themes the UI actually supports. Also used by the `set_theme`
/// tool so a model can never push an arbitrary string into the renderer.
fn normalize_theme(value: &str) -> Option<&'static str> {
    match value.trim().to_lowercase().as_str() {
        "dark" | "ember-black" | "pine-black" => Some("dark"),
        "light" | "ember-light" => Some("light"),
        _ => None,
    }
}

/// Safe snapshot of RuTerm preferences. Deliberately enumerated field by field:
/// adding a secret to `VaultSettings` must never leak through this tool by
/// accident (e.g. via `serde_json::to_value(&vault.settings)`).
fn settings_view(settings: &rterm_core::model::VaultSettings, theme: &str) -> Value {
    json!({
        "theme": theme,
        "default_terminal": settings.default_term,
        "font_size": settings.font_size,
        "font_family": settings.font_family,
        "scrollback_lines": settings.scrollback,
        "sync_interval_secs": settings.sync_interval_secs,
        "auto_lock_secs": settings.lock_timeout_secs,
        "copy_on_select": settings.copy_on_select,
        "chat_model": settings.openrouter_model,
        "chat_configured": !settings.openrouter_api_key.is_empty(),
        "chat_credentials_allowed": settings.chat_allow_credentials,
    })
}

fn sync_status_view(
    signed_in: bool,
    email: Option<&str>,
    revision: i64,
    synced_revision: i64,
    last_synced_at: Option<&str>,
) -> Value {
    let state = if !signed_in {
        "signed_out"
    } else if revision > synced_revision {
        "local_changes_waiting"
    } else {
        "synced"
    };
    json!({
        "status": state,
        "signed_in": signed_in,
        "account": email,
        "local_revision": revision,
        "synced_revision": synced_revision,
        "last_synced_at": last_synced_at,
    })
}

fn find_host<'a>(vault: &'a rterm_core::model::Vault, query: &str) -> Option<&'a Host> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    vault.hosts.iter().find(|host| {
        !host.deleted
            && [&host.label, &host.address, &host.username, &host.id.to_string()]
                .iter()
                .any(|value| value.to_lowercase().contains(&needle))
    })
}

fn session_file(host: &Host) -> PathBuf {
    let label = format!("{}@{}:{}", host.username, host.address, host.port);
    let safe: String = label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.')
            {
                character
            } else {
                '-'
            }
        })
        .collect();
    home_dir()
        .join(".ruterm")
        .join("sessions")
        .join(format!("{safe}.md"))
}

fn safe_filename(input: Option<&str>, fallback: &str) -> Result<String, String> {
    let name = input.unwrap_or("").trim();
    if name.is_empty() {
        return Ok(fallback.into());
    }
    if name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name == "."
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err("Filename must be a simple name without paths".into());
    }
    Ok(name.into())
}

async fn ask_approval(
    app: &AppHandle,
    state: &AppState,
    session: &str,
    dedupe: &str,
    kind: &str,
    title: &str,
    detail: &str,
    preview: &str,
) -> Result<bool, String> {
    if lock(&state.chat_always).contains(dedupe) {
        return Ok(true);
    }
    let id = Uuid::new_v4().simple().to_string();
    let (tx, rx) = oneshot::channel();
    lock(&state.chat_pending).insert(
        id.clone(),
        PendingApproval {
            session: session.into(),
            dedupe: dedupe.into(),
            tx,
        },
    );
    let _ = app.emit(
        "chat://approval",
        json!({
            "id": id,
            "kind": kind,
            "title": title,
            "detail": detail,
            "preview": preview.chars().take(1200).collect::<String>(),
        }),
    );
    let choice = rx.await.unwrap_or(ApprovalChoice::Deny);
    lock(&state.chat_pending).remove(&id);
    match choice {
        ApprovalChoice::Deny => Ok(false),
        ApprovalChoice::Allow => Ok(true),
        ApprovalChoice::Always => {
            lock(&state.chat_always).insert(dedupe.into());
            Ok(true)
        }
    }
}

async fn credentials_allowed(
    app: &AppHandle,
    state: &AppState,
    session: &str,
    dedupe: &str,
    detail: &str,
) -> Result<bool, String> {
    let vault = state.unlocked_vault().await?;
    if !vault.settings.chat_allow_credentials {
        return Err("Credential access is disabled in Settings".into());
    }
    ask_approval(
        app,
        state,
        session,
        dedupe,
        "credential",
        "Allow AI to read credentials?",
        detail,
        "Passwords, private keys and env values will be included in this tool result only.",
    )
    .await
}

async fn run_tool(
    app: &AppHandle,
    state: &AppState,
    session: &str,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    match name {
        "list_hosts" => {
            let vault = state.unlocked_vault().await?;
            let requested = args
                .get("include_credentials")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let with_credentials = if requested {
                credentials_allowed(app, state, session, "credential:hosts", "All saved hosts")
                    .await?
            } else {
                false
            };
            let hosts: Vec<Value> = vault
                .hosts
                .iter()
                .filter(|host| !host.deleted)
                .map(|host| host_view(host, with_credentials))
                .collect();
            Ok(format!(
                "{} saved host(s). Credentials {}.\n{}",
                hosts.len(),
                if with_credentials { "included" } else { "omitted" },
                serde_json::to_string_pretty(&hosts).unwrap_or_default()
            ))
        }
        "get_host" => {
            let query = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("name is required")?;
            let vault = state.unlocked_vault().await?;
            let host = find_host(&vault, query).ok_or_else(|| {
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
            let requested = args
                .get("include_credentials")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let with_credentials = if requested {
                credentials_allowed(
                    app,
                    state,
                    session,
                    &format!("credential:host:{}", host.id),
                    &host.label,
                )
                .await?
            } else {
                false
            };
            Ok(format!(
                "Host details (credentials {}):\n{}",
                if with_credentials { "included" } else { "omitted" },
                serde_json::to_string_pretty(&host_view(host, with_credentials)).unwrap_or_default()
            ))
        }
        "list_notes" => {
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
        }
        "read_note" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or("");
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            if id.is_empty() && query.is_empty() {
                return Err("id or query is required".into());
            }
            let vault = state.unlocked_vault().await?;
            let matches: Vec<&rterm_core::model::Note> = vault
                .notes
                .iter()
                .filter(|note| !note.deleted)
                .filter(|note| {
                    (!id.is_empty() && note.id.to_string() == id)
                        || (!query.is_empty()
                            && note.body.to_lowercase().contains(&query.to_lowercase()))
                })
                .take(5)
                .collect();
            if matches.is_empty() {
                return Ok("No matching note.".into());
            }
            Ok(matches
                .iter()
                .map(|note| format!("--- note {} ---\n{}", note.id, note.body))
                .collect::<Vec<_>>()
                .join("\n\n"))
        }
        "list_snippets" => {
            let vault = state.unlocked_vault().await?;
            let snippets: Vec<Value> = vault
                .snippets
                .iter()
                .filter(|snippet| !snippet.deleted)
                .map(|snippet| {
                    json!({
                        "id": snippet.id,
                        "label": snippet.label,
                        "tags": snippet.tags,
                        "content": snippet.content,
                    })
                })
                .collect();
            Ok(format!(
                "{} snippet(s).\n{}",
                snippets.len(),
                serde_json::to_string_pretty(&snippets).unwrap_or_default()
            ))
        }
        "list_groups" => {
            let vault = state.unlocked_vault().await?;
            let groups: Vec<Value> = vault
                .groups
                .iter()
                .filter(|group| !group.deleted)
                .map(|group| json!({ "id": group.id, "label": group.label }))
                .collect();
            Ok(format!(
                "{} group(s).\n{}",
                groups.len(),
                serde_json::to_string_pretty(&groups).unwrap_or_default()
            ))
        }
        "read_session" => {
            let name = args.get("name").and_then(Value::as_str).unwrap_or("");
            let vault = state.unlocked_vault().await?;
            if name.is_empty() {
                let dir = home_dir().join(".ruterm").join("sessions");
                let files = fs::read_dir(&dir)
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| entry.file_name().to_string_lossy().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                return Ok(format!("Available VPS memory files: {files}"));
            }
            let host = find_host(&vault, name).ok_or_else(|| format!("No saved host matches '{name}'"))?;
            let path = session_file(host);
            let body = fs::read_to_string(&path).unwrap_or_default();
            Ok(format!(
                "VPS: {} ({})\nMemory file: {}\n\n{}",
                host.label,
                format!("{}@{}:{}", host.username, host.address, host.port),
                path.display(),
                if body.trim().is_empty() { "(empty)" } else { &body }
            ))
        }
        "create_snippet" => {
            let label = args
                .get("label")
                .and_then(Value::as_str)
                .ok_or("label is required")?
                .trim()
                .to_string();
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or("content is required")?
                .to_string();
            if label.is_empty() || content.trim().is_empty() {
                return Err("label and content must not be empty".into());
            }
            let tags: Vec<String> = args
                .get("tags")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let approved = ask_approval(
                app,
                state,
                session,
                &format!("write:snippet:{}", label.to_lowercase()),
                "write",
                "Create this snippet?",
                &format!("Snippet '{label}' will be saved in your encrypted vault and synced."),
                &content,
            )
            .await?;
            if !approved {
                return Ok("Snippet creation denied by user.".into());
            }
            let mut vault = state.unlocked_vault().await?;
            let snippet = Snippet {
                id: Uuid::new_v4(),
                label,
                content,
                tags,
                updated_at: now(),
                deleted: false,
            };
            let answer = format!("Snippet '{}' created (id {}).", snippet.label, snippet.id);
            vault.snippets.push(snippet);
            commands::persist_vault(state, vault).await?;
            Ok(answer)
        }
        "write_chat_config" => {
            let target = args
                .get("target")
                .and_then(Value::as_str)
                .ok_or("target is required")?;
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or("content is required")?;
            if content.trim().is_empty() {
                return Err("content must not be empty".into());
            }
            let (path, description) = match target {
                "ai-configs" => (
                    home_dir()
                        .join(".ruterm")
                        .join("ai-configs")
                        .join(safe_filename(args.get("filename").and_then(Value::as_str), "config.txt")?),
                    "~/.ruterm/ai-configs/".to_string(),
                ),
                "desktop" => (
                    home_dir()
                        .join("Desktop")
                        .join(safe_filename(args.get("filename").and_then(Value::as_str), "ruterm-config.txt")?),
                    "~/Desktop/".to_string(),
                ),
                "ssh_config" => (home_dir().join(".ssh").join("config"), "~/.ssh/config".to_string()),
                other => return Err(format!("Unknown target '{other}'")),
            };
            let dedupe = format!("write:config:{target}:{}", path.file_name().unwrap_or_default().to_string_lossy());
            let approved = ask_approval(
                app,
                state,
                session,
                &dedupe,
                "write",
                "Write this config?",
                &format!("Target: {description} ({})", path.display()),
                content,
            )
            .await?;
            if !approved {
                return Ok("Config write denied by user.".into());
            }
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                if metadata.file_type().is_symlink() {
                    return Err("Refusing to write through a symlink".into());
                }
            }
            if target == "ssh_config" {
                let existing = fs::read_to_string(&path).unwrap_or_default();
                let backup = home_dir().join(".ssh").join(format!(
                    "config.ruterm-backup-{}",
                    Utc::now().format("%Y%m%d-%H%M%S")
                ));
                fs::write(&backup, &existing)
                    .map_err(|error| format!("Could not back up SSH config: {error}"))?;
                let block = format!("{BEGIN_MARKER}\n{}\n{END_MARKER}\n", content.trim_end());
                let updated = if let Some(start) = existing.find(BEGIN_MARKER) {
                    let end = existing[start..]
                        .find(END_MARKER)
                        .map(|offset| start + offset + END_MARKER.len())
                        .ok_or("Malformed existing RuTerm block in ~/.ssh/config")?;
                    format!("{}{}{}", &existing[..start], block, &existing[end..])
                } else {
                    let mut text = existing;
                    if !text.ends_with('\n') && !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&block);
                    text
                };
                fs::write(&path, &updated)
                    .map_err(|error| format!("Could not write SSH config: {error}"))?;
                Ok(format!(
                    "SSH config updated (backup: {}).",
                    backup.display()
                ))
            } else {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("Could not create directory: {error}"))?;
                }
                fs::write(&path, content)
                    .map_err(|error| format!("Could not write config: {error}"))?;
                Ok(format!("Config written to {}.", path.display()))
            }
        }
        "launch_agent" => {
            let query = args
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if query.is_empty() {
                return Err("agent is required".into());
            }
            let agents = commands::ai_agents_detect().await?;
            let needle = query.to_lowercase();
            let found = agents
                .iter()
                .find(|agent| {
                    [&agent.id, &agent.label, &agent.command].iter().any(|value| {
                        let value = value.to_lowercase();
                        value == needle || value.contains(&needle)
                    })
                })
                .ok_or_else(|| {
                    let available: Vec<&str> = agents.iter().map(|a| a.label.as_str()).collect();
                    format!(
                        "Agent '{query}' is not available on this device. Available: {}",
                        if available.is_empty() {
                            "none".to_string()
                        } else {
                            available.join(", ")
                        }
                    )
                })?;
            let bridge = app.state::<crate::mcp::Bridge>();
            let launch = crate::mcp::agent_launch(bridge, found.id.clone())?;
            let _ = app.emit(
                "rterm:agent-launch",
                json!({ "launch": launch, "label": found.label }),
            );
            Ok(format!("Launching {} in a new local tab.", found.label))
        }
        "create_host" => {
            let (host, auth_label) = build_host(args)?;
            let duplicate = |vault: &rterm_core::model::Vault, host: &Host| {
                vault.hosts.iter().any(|existing| {
                    !existing.deleted
                        && existing.address.eq_ignore_ascii_case(&host.address)
                        && existing.port == host.port
                        && existing.username == host.username
                })
            };
            if duplicate(&state.unlocked_vault().await?, &host) {
                return Err(format!(
                    "A host for {}@{}:{} already exists",
                    host.username, host.address, host.port
                ));
            }
            let has_password = matches!(host.auth, AuthMethod::Password { .. });
            let preview = format!(
                "Label: {}\nAddress: {}:{}\nUser: {}\nAuth: {}\nNotes: {}",
                host.label, host.address, host.port, host.username, auth_label,
                if host.notes.is_empty() { "—" } else { &host.notes }
            );
            let approved = ask_approval(
                app,
                state,
                session,
                &format!(
                    "write:host:{}:{}:{}",
                    host.address.to_lowercase(), host.port, host.username
                ),
                if has_password { "credential" } else { "write" },
                "Save this SSH host?",
                if has_password {
                    "The password will be stored encrypted in your vault and opened in a new SSH tab."
                } else {
                    "The host will be saved in your encrypted vault and opened in a new SSH tab."
                },
                &preview,
            )
            .await?;
            if !approved {
                return Ok("Host creation denied by user.".into());
            }
            let mut vault = state.unlocked_vault().await?;
            // Recheck after the approval round-trip: another tab could have
            // saved the same host while the dialog was open.
            if duplicate(&vault, &host) {
                return Err(format!(
                    "A host for {}@{}:{} already exists",
                    host.username, host.address, host.port
                ));
            }
            let id = host.id.to_string();
            let label = host.label.clone();
            let address = host.address.clone();
            vault.hosts.push(host);
            let payload = commands::persist_vault(state, vault).await?;
            let _ = app.emit("rterm:host-created", json!({ "id": id, "payload": payload }));
            Ok(format!(
                "Host '{label}' ({address}) saved and opened in a new SSH tab."
            ))
        }
        "get_app_settings" => {
            // Take each lock separately and drop it before the next one.
            let theme = state.settings_rw.read().await.ui_theme.clone();
            let vault = state.unlocked_vault().await?;
            Ok(settings_view(&vault.settings, &theme).to_string())
        }
        "get_sync_status" => {
            let settings = state.settings_rw.read().await.clone();
            let revision = *state.revision.read().await;
            let session = state.session.read().await;
            Ok(sync_status_view(
                session.is_some(),
                session.as_ref().map(|value| value.account.email.as_str()),
                revision,
                settings.synced_revision,
                settings.last_synced_at.as_deref(),
            )
            .to_string())
        }
        "list_agents" => {
            let agents = commands::ai_agents_detect().await?;
            Ok(json!({ "agents": agents }).to_string())
        }
        "set_theme" => {
            let requested = args.get("theme").and_then(Value::as_str).unwrap_or("");
            let theme = normalize_theme(requested).ok_or("theme must be 'dark' or 'light'")?;
            // The webview owns localStorage; mirror the change there through an
            // event. `applyTheme` in the renderer calls `uiSetTheme` back, which
            // updates the native title bar and the persisted settings copy.
            let _ = app.emit("rterm:set-theme", theme);
            Ok(format!("Theme switched to {theme}."))
        }
        other => Err(format!("Unknown tool '{other}'")),
    }
}

#[tauri::command]
pub async fn chat_config(state: State<'_, AppState>) -> Result<ChatConfig, String> {
    let vault = state.vault.read().await;
    Ok(match vault.as_ref() {
        Some(vault) => ChatConfig {
            configured: !vault.settings.openrouter_api_key.is_empty(),
            model: vault.settings.openrouter_model.clone(),
            allow_credentials: vault.settings.chat_allow_credentials,
        },
        None => ChatConfig {
            configured: false,
            model: String::new(),
            allow_credentials: false,
        },
    })
}

/// Store the BYOK key without round-tripping it through the renderer. The
/// plaintext key never appears in IPC responses. An empty `key` with `clear`
/// false leaves the existing key untouched; `clear` true removes it.
#[tauri::command]
pub async fn chat_set_key(
    state: State<'_, AppState>,
    key: String,
    model: String,
    allow_credentials: bool,
    clear: bool,
) -> Result<ChatConfig, String> {
    let mut vault = state.unlocked_vault().await?;
    let key = key.trim();
    if clear {
        vault.settings.openrouter_api_key.clear();
    } else if !key.is_empty() {
        vault.settings.openrouter_api_key = key.to_string();
    }
    let model = model.trim();
    if !model.is_empty() {
        vault.settings.openrouter_model = model.to_string();
    }
    vault.settings.chat_allow_credentials = allow_credentials;
    vault.settings.updated_at = Some(Utc::now());
    let configured = !vault.settings.openrouter_api_key.is_empty();
    let model = vault.settings.openrouter_model.clone();
    commands::persist_vault(&state, vault).await?;
    Ok(ChatConfig {
        configured,
        model,
        allow_credentials,
    })
}

/// Fetch the model catalog. `/models` is public, so the dropdown keeps working
/// even if the sealed key cannot be opened — the key is attached when present.
async fn fetch_models(state: &AppState) -> Result<Vec<ChatModel>, String> {
    let http = client()?;
    let key = openrouter_key(state).await.ok();
    let response = send_with_retry(|| {
        let mut request = http.get(OPENROUTER_MODELS);
        if let Some(key) = key.as_deref() {
            request = request.bearer_auth(key);
        }
        request
    })
    .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("OpenRouter {status} — {}", short_error(&body)));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("Invalid OpenRouter response: {error}"))?;
    let models = body
        .get("data")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let id = entry.get("id")?.as_str()?;
                    let name = entry
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(id);
                    Some(ChatModel {
                        id: id.to_string(),
                        name: name.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(models)
}

#[tauri::command]
pub async fn chat_models(state: State<'_, AppState>) -> Result<Vec<ChatModel>, String> {
    fetch_models(&state).await
}

/// End-to-end key check: send one tiny prompt and return what the model says.
/// A working key should produce "Hello World!".
#[tauri::command]
pub async fn chat_test_key(state: State<'_, AppState>) -> Result<String, String> {
    let key = openrouter_key(&state).await?;
    let model = {
        let vault = state.unlocked_vault().await?;
        let model = vault.settings.openrouter_model.trim().to_string();
        if model.is_empty() {
            return Err("OpenRouter model is not set in Settings".into());
        }
        model
    };
    let http = client()?;
    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            { "role": "system", "content": "Reply with exactly this text and nothing else: Hello World!" },
            { "role": "user", "content": "Hello" }
        ]
    });
    let response = send_with_retry(|| {
        http.post(OPENROUTER_CHAT)
            .bearer_auth(&key)
            .json(&body)
    })
    .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("OpenRouter {status} — {}", short_error(&body)));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("Invalid OpenRouter response: {error}"))?;
    let reply = body
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if reply.is_empty() {
        return Err("OpenRouter returned an empty reply".into());
    }
    Ok(format!("Connected — AI replied: {reply}"))
}

#[tauri::command]
pub async fn chat_send(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    text: String,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Message cannot be empty".into());
    }
    if text.chars().count() > 8000 {
        return Err("Message is too long (8000 characters max)".into());
    }
    let key = openrouter_key(&state).await?;
    let model = {
        let vault = state.unlocked_vault().await?;
        let model = vault.settings.openrouter_model.trim().to_string();
        if model.is_empty() {
            return Err("OpenRouter model is not set in Settings".into());
        }
        model
    };
    lock(&state.chat_cancel).remove(&session);
    {
        let mut sessions = lock(&state.chat_sessions);
        sessions
            .entry(session.clone())
            .or_default()
            .push(ChatMessage::plain("user", text.into()));
    }

    let http = client()?;
    let mut turn = 0usize;
    loop {
        if lock(&state.chat_cancel).contains(&session) {
            let _ = app.emit(
                "chat://done",
                json!({ "session": session, "text": "", "cancelled": true }),
            );
            return Ok(());
        }
        let messages: Vec<ChatMessage> = {
            let sessions = lock(&state.chat_sessions);
            let mut all = vec![ChatMessage::plain("system", system_prompt())];
            if let Some(history) = sessions.get(&session) {
                all.extend(history.iter().cloned());
            }
            all
        };

        // On the final turn hide the tools so the model must wrap up in text.
        let allow_tools = turn < MAX_TURNS;
        let (content, calls) =
            match stream_completion(&app, &session, &http, &key, &model, &messages, allow_tools).await {
                Ok(result) => result,
                Err(error) => {
                    let _ = app.emit(
                        "chat://error",
                        json!({ "session": session, "message": error }),
                    );
                    return Err(error);
                }
            };
        // Never let a tool-hungry model loop past the cap.
        let calls = if allow_tools { calls } else { Vec::new() };

        if calls.is_empty() {
            let mut sessions = lock(&state.chat_sessions);
            sessions
                .entry(session.clone())
                .or_default()
                .push(ChatMessage::plain("assistant", content.clone()));
            drop(sessions);
            let _ = app.emit(
                "chat://done",
                json!({ "session": session, "text": content, "cancelled": false }),
            );
            return Ok(());
        }

        let tool_calls: Vec<Value> = calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": call.arguments }
                })
            })
            .collect();
        {
            let mut sessions = lock(&state.chat_sessions);
            let history = sessions.entry(session.clone()).or_default();
            history.push(ChatMessage {
                role: "assistant".into(),
                content,
                tool_calls: Some(json!(tool_calls)),
                tool_call_id: None,
                name: None,
            });
        }

        for call in &calls {
            let args: Value = serde_json::from_str(&call.arguments)
                .unwrap_or_else(|_| json!({}));
            let result = run_tool(&app, &state, &session, &call.name, &args)
                .await
                .unwrap_or_else(|error| format!("Tool error: {error}"));
            let _ = app.emit(
                "chat://tool",
                json!({
                    "session": session,
                    "name": call.name,
                    "preview": result.chars().take(160).collect::<String>(),
                }),
            );
            let mut sessions = lock(&state.chat_sessions);
            sessions.entry(session.clone()).or_default().push(ChatMessage {
                role: "tool".into(),
                content: result,
                tool_calls: None,
                tool_call_id: Some(call.id.clone()),
                name: Some(call.name.clone()),
            });
        }

        turn += 1;
        if turn >= MAX_TURNS {
            let message = "Tool limit reached. Summarizing with the data gathered so far.";
            let mut sessions = lock(&state.chat_sessions);
            sessions
                .entry(session.clone())
                .or_default()
                .push(ChatMessage::plain("user", message.into()));
            drop(sessions);
        }
    }
}

fn parse_choice(value: &str) -> Result<ApprovalChoice, String> {
    match value.to_lowercase().as_str() {
        "deny" => Ok(ApprovalChoice::Deny),
        "allow" => Ok(ApprovalChoice::Allow),
        "always" => Ok(ApprovalChoice::Always),
        other => Err(format!("Unknown approval choice '{other}'")),
    }
}

#[tauri::command]
pub async fn chat_approve(
    state: State<'_, AppState>,
    id: String,
    choice: String,
) -> Result<(), String> {
    let choice = parse_choice(&choice)?;
    let approval = lock(&state.chat_pending)
        .remove(&id)
        .ok_or("Approval request expired")?;
    if choice == ApprovalChoice::Always {
        lock(&state.chat_always).insert(approval.dedupe);
    }
    let _ = approval.tx.send(choice);
    Ok(())
}

#[tauri::command]
pub async fn chat_cancel(state: State<'_, AppState>, session: String) -> Result<(), String> {
    lock(&state.chat_cancel).insert(session.clone());
    let mut pending = lock(&state.chat_pending);
    let stale: Vec<String> = pending
        .iter()
        .filter(|(_, entry)| entry.session == session)
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        pending.remove(&id);
    }
    Ok(())
}

#[tauri::command]
pub async fn chat_clear(state: State<'_, AppState>, session: String) -> Result<(), String> {
    lock(&state.chat_sessions).remove(&session);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_cannot_escape_their_directory() {
        assert_eq!(safe_filename(Some("note.txt"), "x"), Ok("note.txt".into()));
        assert_eq!(safe_filename(None, "config.txt"), Ok("config.txt".into()));
        assert!(safe_filename(Some("../ssh/config"), "x").is_err());
        assert!(safe_filename(Some("/etc/passwd"), "x").is_err());
        assert!(safe_filename(Some("a/b"), "x").is_err());
        assert!(safe_filename(Some(".."), "x").is_err());
    }

    #[test]
    fn approval_choices_are_lowercase() {
        assert_eq!(parse_choice("Deny"), Ok(ApprovalChoice::Deny));
        assert_eq!(parse_choice("ALLOW"), Ok(ApprovalChoice::Allow));
        assert_eq!(parse_choice("always"), Ok(ApprovalChoice::Always));
        assert!(parse_choice("maybe").is_err());
    }

    #[test]
    fn host_view_omits_credentials_by_default() {
        use rterm_core::model::AuthMethod;
        let host = Host {
            id: Uuid::new_v4(),
            label: "production-api".into(),
            address: "10.0.0.1".into(),
            port: 22,
            username: "root".into(),
            auth: AuthMethod::Password { password: "hunter2".into() },
            group_id: None,
            tags: vec![],
            color: None,
            icon: None,
            favorite: false,
            notes: "web server".into(),
            startup_snippet_id: None,
            jump_host_id: None,
            keepalive_secs: None,
            connect_timeout_secs: None,
            env: vec![("TOKEN".into(), "secret".into())],
            terminal: Default::default(),
            agent_forwarding: false,
            compression: false,
            os: None,
            host_key_fingerprint: None,
            created_at: now(),
            updated_at: now(),
            last_used_at: None,
            deleted: false,
        };
        let safe = serde_json::to_string(&host_view(&host, false)).unwrap();
        assert!(!safe.contains("hunter2"));
        assert!(!safe.contains("secret"));
        assert!(safe.contains("production-api"));

        let full = serde_json::to_string(&host_view(&host, true)).unwrap();
        assert!(full.contains("hunter2"));
        assert!(full.contains("secret"));
    }

    #[test]
    fn created_host_uses_safe_defaults() {
        let (host, auth) = build_host(&json!({ "address": "203.0.113.7" })).unwrap();
        assert_eq!(host.address, "203.0.113.7");
        assert_eq!(host.port, 22);
        assert_eq!(host.username, "root");
        assert_eq!(host.label, "root@203.0.113.7");
        assert!(matches!(host.auth, AuthMethod::Agent));
        assert_eq!(auth, "SSH agent");

        let (host, auth) =
            build_host(&json!({ "address": "203.0.113.7", "port": 2222, "username": "deploy", "password": "s3cret" }))
                .unwrap();
        assert_eq!(host.port, 2222);
        assert_eq!(host.username, "deploy");
        match &host.auth {
            AuthMethod::Password { password } => assert_eq!(password, "s3cret"),
            other => panic!("expected password auth, got {other:?}"),
        }
        assert!(!auth.contains("s3cret"));
        assert!(build_host(&json!({ "address": "x", "port": 0 })).is_err());
        assert!(build_host(&json!({ "address": "x", "port": 70000 })).is_err());
        assert!(build_host(&json!({ "address": "" })).is_err());
        let agent = build_host(&json!({ "address": "opencode" })).unwrap_err();
        assert!(agent.contains("launch_agent"));
    }

    #[test]
    fn themes_are_normalized() {
        assert_eq!(normalize_theme("dark"), Some("dark"));
        assert_eq!(normalize_theme(" Light "), Some("light"));
        assert_eq!(normalize_theme("ember-light"), Some("light"));
        assert_eq!(normalize_theme("neon"), None);
    }

    #[test]
    fn settings_view_never_exposes_secrets() {
        let mut settings = rterm_core::model::VaultSettings::default();
        settings.openrouter_api_key = "sk-secret-key".into();
        let view = serde_json::to_string(&settings_view(&settings, "dark")).unwrap();
        assert!(!view.contains("sk-secret-key"));
        assert!(!view.contains("openrouter_api_key"));
        assert!(view.contains("\"chat_configured\":true"));
        assert!(view.contains("\"theme\":\"dark\""));
    }

    #[test]
    fn sync_status_reflects_pending_changes() {
        let synced = sync_status_view(true, Some("me@example.com"), 12, 12, Some("today"));
        assert_eq!(synced["status"], "synced");
        let pending = sync_status_view(true, Some("me@example.com"), 13, 12, Some("today"));
        assert_eq!(pending["status"], "local_changes_waiting");
        let offline = sync_status_view(false, None, 13, 12, None);
        assert_eq!(offline["status"], "signed_out");
    }

    #[test]
    fn tools_cover_app_reads_and_theme_switch() {
        let list = tools();
        let names: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["function"]["name"].as_str().unwrap())
            .collect();
        for expected in [
            "list_hosts",
            "launch_agent",
            "create_host",
            "list_notes",
            "get_app_settings",
            "get_sync_status",
            "list_agents",
            "set_theme",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
    }
}
