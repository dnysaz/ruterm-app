//! BYOK AI chat backed by OpenRouter.
//!
//! The API key lives in the encrypted vault: Rust opens it at call time and
//! never sends it to the renderer. Tools give the model read access to saved
//! hosts, notes, snippets and per-VPS work logs; credentials and config writes
//! need an explicit approval in the UI.

use crate::{commands, state::AppState};
use chrono::Utc;
use rterm_core::model::{now, AuthMethod, Host, Note, Snippet};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;
use uuid::Uuid;

const OPENROUTER_CHAT: &str = "https://openrouter.ai/api/v1/chat/completions";
const OPENROUTER_MODELS: &str = "https://openrouter.ai/api/v1/models";
const MAX_TURNS: usize = 12;
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

/// The drawer's watchdog must not kill a turn that is still doing real work.
/// A tool can legitimately wait up to 60 seconds, and recovery can make two
/// more model calls, so emit a silent heartbeat for the whole turn.
struct ActivityBeat {
    alive: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ActivityBeat {
    fn start(app: AppHandle) -> Self {
        let _ = app.emit("chat://activity", json!({}));
        let alive = Arc::new(AtomicBool::new(true));
        let flag = alive.clone();
        let handle = std::thread::spawn(move || {
            while flag.load(Ordering::Relaxed) {
                std::thread::park_timeout(Duration::from_secs(8));
                if !flag.load(Ordering::Relaxed) {
                    break;
                }
                let _ = app.emit("chat://activity", json!({}));
            }
        });
        Self {
            alive,
            handle: Some(handle),
        }
    }
}

impl Drop for ActivityBeat {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// Tool output can be very large (log tails, directory trees). Keep the last
/// model turn inside a predictable context budget without losing the beginning
/// of a short command result.
fn cap_tool_text(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit / 2).collect();
    let tail: String = text.chars().rev().take(limit / 2).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}\n... truncated ...\n{tail}")
}

/// A reply that only announces an action is not an answer. Detect the shape of
/// the sentence (not its topic) so RuTerm can continue the work automatically.
fn looks_unfinished(text: &str) -> bool {
    let value = text.trim().to_lowercase();
    if value.is_empty() {
        return true;
    }
    const INTENT: &[&str] = &[
        "saya akan",
        "akan saya",
        "sekarang saya",
        "saya cek",
        "saya coba",
        "saya buka",
        "saya jalankan",
        "mari kita",
        "baik, saya",
        "i'll",
        "i will",
        "let me",
        "now i",
        "i'm going to",
        "sure, i",
    ];
    if INTENT.iter().any(|phrase| value.contains(phrase)) {
        return true;
    }
    value.ends_with("...")
        || value.ends_with('…')
        || value.ends_with(':')
        || value.ends_with('—')
}

fn last_user_text(messages: &[ChatMessage]) -> &str {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .unwrap_or("")
}

fn fallback_after_tools(last_result: &str) -> String {
    if last_result.trim().is_empty() {
        "Tool selesai, tetapi model tidak menghasilkan ringkasan. Coba ulangi dengan pertanyaan lebih spesifik.".into()
    } else {
        format!(
            "Tool selesai — ini hasilnya:\n{}",
            cap_tool_text(last_result, 1_500)
        )
    }
}

/// Lowercased command words with a leading `sudo` (and its flags) removed.
fn command_words(command: &str) -> Vec<String> {
    let lower = command.trim().to_ascii_lowercase();
    let tokens: Vec<String> = lower.split_whitespace().map(str::to_string).collect();
    let mut index = 0;
    if tokens.first().map(String::as_str) == Some("sudo") {
        index = 1;
        while tokens.get(index).is_some_and(|token| token.starts_with('-')) {
            index += 1;
        }
    }
    tokens[index..].to_vec()
}

/// Commands that eventually finish but commonly take minutes. `vps_execute`
/// waits for its result, so these are auto-started as streaming jobs instead
/// of blocking the chat for the full timeout.
fn long_running_command(command: &str) -> bool {
    let words = command_words(command);
    let first = words.first().map(String::as_str).unwrap_or("");
    let second = words.get(1).map(String::as_str).unwrap_or("");

    // `nginx -T` dumps every config file and can stall behind a pipe; `nginx -t`
    // alone is fast and must stay synchronous.
    if first == "nginx" && command.contains("-T") {
        return true;
    }
    const PACKAGE: &[&str] = &["apt", "apt-get", "yum", "dnf", "pacman", "zypper", "apk"];
    if PACKAGE.contains(&first) {
        return matches!(
            second,
            "update" | "upgrade" | "install" | "dist-upgrade" | "full-upgrade" | "autoremove"
        );
    }
    let long = match first {
        "brew" => matches!(second, "update" | "upgrade" | "install" | "reinstall"),
        "npm" | "pnpm" | "yarn" => matches!(second, "install" | "i" | "ci" | "add" | "run"),
        "cargo" => matches!(second, "build" | "test" | "check" | "clippy" | "run" | "install"),
        "docker" => matches!(second, "build" | "compose" | "pull" | "push"),
        "pip" | "pip3" => matches!(second, "install" | "uninstall"),
        "composer" => matches!(second, "install" | "update" | "require"),
        "systemctl" | "service" => matches!(second, "restart" | "reload" | "start" | "stop"),
        "make" | "cmake" | "gradle" => true,
        "go" => matches!(second, "build" | "test" | "install"),
        _ => false,
    };
    long && !interactive_command(command)
}

/// Programs meant to stay on screen. They must be launched with a real Enter
/// and without the exit sentinel used by `execute`/`start_execute`.
fn interactive_command(command: &str) -> bool {
    let normalized = crate::mcp::terminal_keys(command);
    let words = command_words(&normalized);
    let first = words.first().map(String::as_str).unwrap_or("");
    const TUIS: &[&str] = &[
        "htop", "top", "btop", "atop", "vim", "nvim", "vi", "nano", "micro", "less", "more",
        "man", "mc", "ranger", "tmux", "screen", "watch", "ping", "ssh",
    ];
    if TUIS.contains(&first) {
        return true;
    }
    (first == "tail" || first == "journalctl") && words.iter().any(|word| word == "-f" || word == "--follow")
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
    pub allow_vps: bool,
}

/// Friendly target shown in the chat header — no internal session id.
#[derive(Debug, Clone, Serialize)]
pub struct ChatTarget {
    pub name: String,
    pub address: String,
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
    client_with_timeout(60)
}

fn client_with_timeout(seconds: u64) -> Result<reqwest::Client, String> {
    // OpenRouter asks apps to identify themselves; some providers require it
    // for free-tier requests.
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = "https://github.com/dnysaz/ruterm-app".parse() {
        headers.insert("HTTP-Referer", value);
    }
    if let Ok(value) = "RuTerm".parse() {
        headers.insert("X-Title", value);
    }
    // Tool results are returned outside this stream. A completion itself
    // should be quick; a short ceiling lets us fall back instead of hanging.
    reqwest::Client::builder()
        .timeout(Duration::from_secs(seconds))
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

/// The no-target variant is sent on every request, so an old tool result in
/// localStorage can never be presented as a live reading again.
fn vps_target_ready(app: &AppHandle, state: &AppState) -> bool {
    let Some(id) = app.state::<crate::mcp::Bridge>().target_id() else {
        return false;
    };
    lock(&state.ssh).contains_key(&id)
}

fn system_prompt_for(allow_vps: bool, has_target: bool) -> String {
    let base = "You are RuTerm's built-in assistant. You can read the user's saved SSH hosts, notes, \
     snippets, groups, per-VPS work logs, app preferences, Google/sync status and the AI agent \
     CLIs installed on this Mac. You can also switch the interface theme between dark and light, \
     launch a detected AI agent in a local tab, and save a new SSH host and open it in a terminal \
     tab. \
     Always call a tool for facts — never invent host names, addresses, note contents, settings, \
     sync state, or command output. For current, recent or uncertain facts (news, versions, prices, \
     docs), call `web_search` first and cite the URLs it returns — never guess from memory. \
     Credential fields are omitted by default and only appear after \
     the user explicitly approves a read; API keys and tokens are never exposed. Other config \
     writes require approval. When the user asks to change the theme, call `set_theme` immediately \
     — do not merely say that you can. When the user asks to open/start an AI agent from \
     `list_agents` (OpenCode, Freebuff, Kilo, Claude, Kiro, Command AI), call `launch_agent` with \
     its id — never call `create_host` for an agent name. `create_host` is ONLY for a real SSH host \
     with an IP or hostname. When the user pastes an address/IP, port, user or password and asks to \
     save/open a host, call `create_host` with those exact values; it asks for confirmation and \
     opens the SSH tab automatically. When the user asks to record, recap, or save a result into \
     a RuTerm note, call `create_note` immediately with the full summary in `body` and a short \
     `title` — never merely say you will save it. The `body` is plain markdown, not a JSON-escaped string: write punctuation once, never pile up backslashes, and never paste the same summary twice into one body. The note opens in a new tab automatically. \
     Notes are fully editable: find one with `list_notes`/`read_note`, then call `update_note` to \
     replace its body or pass `append: true` to add text at the end, or `delete_note` to remove it. \
     Never claim you cannot edit or delete a note — call the tool. `delete_note` asks for confirmation. \
     To open an existing saved note in a new tab, call `open_note` with its id from `list_notes` or \
     `read_note` — never say only newly created notes can be opened. To close a note tab without \
     deleting the note, call `close_note` with its id — never say you cannot close note tabs. \
     When the user asks to clear/erase/reset the chat history, call `clear_chat_history`: a \
     confirmation dialog appears and nothing is removed until the user confirms. Never claim the \
     history is cleared before they confirm. \
     Never invent a password. Always answer in the same language \
     as the user's latest message (Indonesian in, Indonesian out). Be concise and refer to hosts \
     by their saved labels rather than internal ids.";
    if allow_vps {
        let guard = if has_target {
            "An SSH tab is focused right now. It is the ONLY valid target. Any RAM, disk, \
             process, log or command-output claim must come from a `vps_*` tool result in THIS turn."
        } else {
            "IMPORTANT: NO SSH TAB IS FOCUSED RIGHT NOW. Never report current RAM, disk, \
             processes, logs or command output from memory or from earlier chat history — those \
             readings are stale. If the user asks WHICH saved hosts exist, call `list_hosts` — \
             `vps_list_targets` only shows currently OPEN terminal tabs, not saved hosts. If the \
             user asks to connect/work on a saved host, call `vps_open_host` first; otherwise say \
             no VPS tab is open and ask them to open/focus one. Never present old tool output as \
             a current result."
        };
        format!(
            "{base} VPS control is enabled and NATIVE: you drive the focused SSH tab through \
             RuTerm's embedded MCP bridge. The only valid target is the tab the user is looking \
             at; you cannot switch tabs or pass a `session`. WORKFLOW — follow it every time: \
             (1) If the user asks to open a SAVED host in a new tab, call `vps_open_host` with its \
             saved label (e.g. 'VPS Murah') or address. `create_host` is ONLY for a brand-new host \
             the user pastes — never claim an existing saved host cannot be opened. The new tab \
             becomes the active target automatically. \
             (2) Decide the next step from the question, not from a fixed script. For a direct \
             fact (RAM, disk, domains, services, logs, versions, processes), run ONE read-only \
             command with `vps_execute`; do not call `vps_inspect` first unless you need terminal \
             state. Use `vps_inspect` and `vps_list_targets` when the task depends on what the \
             shell is doing right now, then check the host, clean \
             recent screen, detected prompt and running jobs. If 'Idle at prompt' is no, follow \
             the running job with `vps_read_stream` or stop it with `vps_send_keys` Ctrl-C \
             (\\u0003) — never inject a command into a busy shell. \
             (3) Call `vps_read_memory` for durable context on this host — it also contains an \
             automatic log of every previous AI chat action and reply, so use it to keep continuity. \
             Use `vps_note_memory` when you learn a fact worth remembering (deploy path, service \
             name, quirks). \
             (4) Use `vps_execute` for short commands; installs/builds auto-start as background \
             jobs — reply that it is running and use `vps_read_stream` (`wait_ms` up to 60000) \
             when asked for the result. Use `vps_open_tui` to LAUNCH htop/vim/nano/less/tail -f, \
             then `vps_send_keys` for navigation and `vps_read_screen` to look. Never type the \
             word 'Enter'. \
             (5) If a TUI is open (inspect reports it), close it first with `vps_close_tui` before \
             any command — the bridge also does this automatically, but never type a command into \
             htop/vim. Confirm done vs not done from the ACTUAL SCREEN: call `vps_verify_screen` \
             after an action (every `vps_execute` result also includes a screen check). Only say \
             'sudah/selesai' when the screen check is VERIFIED — the command must be visible on \
             the VPS terminal and new output must be captured. NEVER type a nested `ssh` command inside \
             an already-open VPS tab — the tab is already that SSH session; run commands directly. \
             A timed-out or NOT VERIFIED tool means NO DATA — never report RAM/disk/domain numbers \
             from memory or history. Report failures \
             honestly; never expose internal session ids. \
             (6) If the user asks to close the terminal tab, call `vps_close_terminal` — it closes \
             only the focused SSH tab, not the saved host. Never say terminal tabs cannot be closed. \
             Risky commands open Deny / Allow / Always allow; \
             'Always allow' is exact-command and process-only. ALWAYS finish with a short text \
             answer in the user's language — never end a turn with an empty message. {guard}"
        )
    } else {
        format!(
            "{base} VPS control is disabled in Settings. Never try to run server commands and \
             never call any `vps_*` tool; tell the user to enable `Allow AI Chat to control SSH \
             tabs` in Settings if they want that."
        )
    }
}

fn tools_for(allow_vps: bool, has_target: bool) -> Value {
    let mut list = match json!([
      {
        "type": "function",
        "function": {
          "name": "list_hosts",
          "description": "List every saved SSH host with label, address, port, username, tags and notes. Use this for 'list my saved hosts'; `vps_list_targets` only shows open terminal tabs. Credentials are omitted unless include_credentials is true (requires approval).",
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
          "name": "create_note",
          "description": "Create a new note in the encrypted RuTerm vault and open it in a new tab. Use this immediately whenever the user asks to record, recap, or save a result/check into a note. No approval dialog is needed for a note.",
          "parameters": { "type": "object", "properties": {
            "title": { "type": "string", "description": "Short title; it becomes the first line of the note." },
            "body": { "type": "string", "description": "Full note content, written from the facts just obtained from tools." },
            "favorite": { "type": "boolean", "description": "Mark as favorite (default false)." }
          }, "required": ["body"] }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "open_note",
          "description": "Open an EXISTING note from the encrypted vault in a new RuTerm tab. Find its id with list_notes or read_note first. Use this whenever the user asks to open/show/view a saved note — never say you cannot open notes.",
          "parameters": { "type": "object", "properties": {
            "id": { "type": "string", "description": "Note id from list_notes/read_note, or a unique title/body query." }
          }, "required": ["id"] }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "close_note",
          "description": "Close a note TAB in RuTerm without deleting the note. Find its id with list_notes/read_note first. Use this whenever the user asks to close/tutup a note tab — never say you cannot close tabs.",
          "parameters": { "type": "object", "properties": {
            "id": { "type": "string", "description": "Note id from list_notes/read_note, or a unique title/body query." }
          }, "required": ["id"] }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "update_note",
          "description": "Edit an EXISTING note in the encrypted vault. Find its id with list_notes or read_note. Use append:true to add text at the end; otherwise body replaces the whole note. Use this whenever the user asks to edit, update, add to, or fix a note — never say you cannot edit notes.",
          "parameters": { "type": "object", "properties": {
            "id": { "type": "string", "description": "Note id from list_notes/read_note, or a unique title/body query." },
            "body": { "type": "string", "description": "New full body, or the text to append." },
            "title": { "type": "string", "description": "Optional title for a replacement body." },
            "append": { "type": "boolean", "description": "true = add to the end instead of replacing." },
            "favorite": { "type": "boolean" }
          }, "required": ["body"] }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "delete_note",
          "description": "Delete an existing note from the encrypted vault. Find its id with list_notes or read_note. Requires confirmation. Use this when the user asks to delete/remove a note.",
          "parameters": { "type": "object", "properties": {
            "id": { "type": "string", "description": "Note id, or a unique title/body query." }
          }, "required": ["id"] }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "clear_chat_history",
          "description": "Ask RuTerm to clear this AI chat's conversation history. This opens the 'Clear chat history?' confirmation dialog; nothing is deleted until the user confirms. Use this when the user asks to clear/erase/reset chat history.",
          "parameters": { "type": "object", "properties": {} }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "web_search",
          "description": "Search the public web for current, recent or uncertain facts (news, docs, versions, prices). Returns titles, URLs and snippets; cite the URLs in your answer. No API key or user credential is used.",
          "parameters": { "type": "object", "properties": {
            "query": { "type": "string", "description": "Search query in the user's language or English." },
            "limit": { "type": "integer", "description": "How many results to return (1-8, default 5)." }
          }, "required": ["query"] }
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
    ]) {
        Value::Array(items) => items,
        _ => Vec::new(),
    };
    if allow_vps {
        let extra = json!([
          {
            "type": "function",
            "function": {
              "name": "vps_list_targets",
              "description": "List SSH terminals currently open in RuTerm with friendly VPS names. Call this before a server task; the focused tab is marked active. Never call local shell tools for server work.",
              "parameters": { "type": "object", "properties": {} }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_open_host",
              "description": "Open an ALREADY SAVED SSH host (e.g. 'VPS Murah') in a new terminal tab and make it the active target. Use this when the user asks to open/connect to a host from their vault — never say it cannot be done and never use create_host for it.",
              "parameters": { "type": "object", "properties": {
                "name": { "type": "string", "description": "Saved host label, address or IP from list_hosts." }
              }, "required": ["name"] }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_close_terminal",
              "description": "Close the currently focused SSH terminal tab in RuTerm. It only closes the live tab; the saved host and its vault data stay. Use this when the user asks to close/tutup the current VPS terminal — never say you cannot close terminal tabs.",
              "parameters": { "type": "object", "properties": {} }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_inspect",
              "description": "Snapshot the focused VPS terminal before typing: host, clean recent screen, detected prompt, idle/busy state and running jobs. Call this before any server task.",
              "parameters": { "type": "object", "properties": {
                "lines": { "type": "integer", "description": "Recent screen lines (default 30)." }
              } }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_verify_screen",
              "description": "Read the ACTUAL VPS screen after an action and report whether the expected command is visible, whether new output appeared, and whether the shell is at a clean prompt. Use this to decide done vs not done instead of trusting tool status.",
              "parameters": { "type": "object", "properties": {
                "command": { "type": "string", "description": "The command that should be visible on screen." },
                "lines": { "type": "integer", "description": "Screen tail lines (default 20)." }
              } }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_execute",
              "description": "Type a short command into the focused VPS terminal and return its output. Long installs/upgrades/builds are auto-started as background jobs so the chat does not block. Risky commands open a Deny / Allow / Always allow dialog.",
              "parameters": { "type": "object", "properties": {
                "command": { "type": "string" },
                "timeout_ms": { "type": "integer", "description": "0 = no limit, default 60000. Prefer vps_start_execute for anything slower." }
              }, "required": ["command"] }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_open_tui",
              "description": "Launch an interactive terminal program (htop, vim, nano, less, tail -f) in the focused VPS tab. Sends a real Enter with no exit sentinel, so the program stays open. Never type the word 'Enter' into the terminal.",
              "parameters": { "type": "object", "properties": {
                "command": { "type": "string", "description": "TUI command, e.g. `htop`, `sudo htop`, `vim /etc/nginx/nginx.conf`." }
              }, "required": ["command"] }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_close_tui",
              "description": "Exit an open htop/vim/nano/less session and return to the shell prompt. Call this before running a command if vps_inspect says a TUI is open.",
              "parameters": { "type": "object", "properties": {
                "keys": { "type": "string", "description": "Exit keys. Default Ctrl-C; use `q` for htop/less or `:q\\n` for vim." }
              } }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_start_execute",
              "description": "Start a long VPS command in the focused tab and return a job id immediately. Follow it with vps_read_stream instead of blocking.",
              "parameters": { "type": "object", "properties": {
                "command": { "type": "string" },
                "timeout_ms": { "type": "integer", "description": "0 = no limit (default)." }
              }, "required": ["command"] }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_read_stream",
              "description": "Read newly produced output from a VPS job, with the next offset and done/exit status. Pass the job id returned by vps_start_execute; the command text is also accepted. For a long job pass wait_ms up to 60000 instead of polling repeatedly.",
              "parameters": { "type": "object", "properties": {
                "job": { "type": "string", "description": "Job id from vps_start_execute, or the command text itself." },
                "offset": { "type": "integer" },
                "wait_ms": { "type": "integer", "description": "Long-poll wait (max 60000), default 1000." }
              }, "required": ["job"] }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_cancel_stream",
              "description": "Send Ctrl-C to cancel a running VPS job.",
              "parameters": { "type": "object", "properties": { "job": { "type": "string" } }, "required": ["job"] }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_send_keys",
              "description": "Send navigation keys to an already-open TUI. Use vps_open_tui to LAUNCH htop/vim/etc.; never send the literal word 'Enter' — use a real \\n.",
              "parameters": { "type": "object", "properties": {
                "keys": { "type": "string" },
                "include_screen": { "type": "boolean" }
              }, "required": ["keys"] }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_read_screen",
              "description": "Read recent clean screen lines from the focused VPS terminal (ANSI and redraws removed).",
              "parameters": { "type": "object", "properties": {
                "max_bytes": { "type": "integer" },
                "lines": { "type": "integer" }
              } }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_read_memory",
              "description": "Read the persistent work log for the focused VPS so you remember earlier decisions on this host.",
              "parameters": { "type": "object", "properties": {} }
            }
          },
          {
            "type": "function",
            "function": {
              "name": "vps_note_memory",
              "description": "Append a durable fact or decision about the focused VPS to its session memory file.",
              "parameters": { "type": "object", "properties": {
                "text": { "type": "string" }
              }, "required": ["text"] }
            }
          }
        ]);
        if let Value::Array(extra) = extra {
            list.extend(extra);
        }
    }
    // With no focused SSH tab, hide only the tools that actually type into a
    // VPS. Saved-host reads, notes, settings and theme tools stay available —
    // the user can still ask "list my saved hosts" before opening any tab.
    if allow_vps && !has_target {
        list.retain(|entry| {
            let name = entry["function"]["name"].as_str().unwrap_or("");
            !name.starts_with("vps_") || matches!(name, "vps_list_targets" | "vps_open_host")
        });
    }
    Value::Array(list)
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

#[derive(Clone)]
struct SearchHit {
    title: String,
    url: String,
    snippet: String,
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn strip_html_tags(input: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for character in input.chars() {
        if character == '<' {
            inside = true;
        } else if character == '>' {
            inside = false;
        } else if !inside {
            out.push(character);
        }
    }
    decode_html_entities(out.trim())
}

fn between(input: &str, start: &str, end: &str) -> Option<String> {
    let from = input.find(start)? + start.len();
    let rest = &input[from..];
    let to = rest.find(end)?;
    Some(rest[..to].to_string())
}

fn percent_decode(input: &str) -> String {
    fn hex(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' { b' ' } else { bytes[index] });
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn clean_result_url(href: &str) -> String {
    let href = href.trim();
    if let Some(pos) = href.find("uddg=") {
        let encoded = href[pos + 5..].split('&').next().unwrap_or("");
        return percent_decode(encoded);
    }
    if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_string()
    }
}

fn parse_ddg_results(html: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    for chunk in html.split("class=\"result__a\"").skip(1) {
        let open_end = match chunk.find('>') {
            Some(index) => index,
            None => continue,
        };
        let close = chunk.find("</a>").unwrap_or(chunk.len());
        let title = strip_html_tags(&chunk[open_end + 1..close]);
        let href = between(&chunk[..open_end], "href=\"", "\"").unwrap_or_default();
        let url = clean_result_url(&href);
        if title.is_empty() || url.is_empty() {
            continue;
        }
        let rest = &chunk[close..];
        let snippet = if let Some(snip) = rest.split("class=\"result__snippet\"").nth(1) {
            let body = snip.find('>').map(|index| &snip[index + 1..]).unwrap_or(snip);
            let body = body.find("</a>").map(|index| &body[..index]).unwrap_or(body);
            strip_html_tags(body).chars().take(400).collect()
        } else {
            String::new()
        };
        hits.push(SearchHit { title, url, snippet });
        if hits.len() >= limit {
            break;
        }
    }
    hits
}

/// Best-effort public web search. No API key, no credential data, no renderer
/// involvement — only the title/URL/snippet list goes back to the model.
async fn web_search(query: &str, limit: usize) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("query is required".into());
    }
    let http = client_with_timeout(20)?;
    let agent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) RuTerm-AI-Chat";
    let url = reqwest::Url::parse_with_params("https://html.duckduckgo.com/html/", &[("q", query)])
        .map_err(|error| format!("Could not build search URL: {error}"))?;
    let mut hits = match http.get(url).header("User-Agent", agent).send().await {
        Ok(response) if response.status().is_success() => {
            parse_ddg_results(&response.text().await.unwrap_or_default(), limit)
        }
        _ => Vec::new(),
    };

    if hits.is_empty() {
        // DuckDuckGo's Instant Answer JSON is a smaller but sturdier fallback.
        if let Ok(url) = reqwest::Url::parse_with_params(
            "https://api.duckduckgo.com/",
            &[
                ("q", query),
                ("format", "json"),
                ("no_html", "1"),
                ("skip_disambig", "1"),
            ],
        ) {
            if let Ok(response) = http.get(url).header("User-Agent", agent).send().await {
                if response.status().is_success() {
                    if let Ok(value) = response.json::<Value>().await {
                        let snippet = value["AbstractText"].as_str().unwrap_or("").to_string();
                        let link = value["AbstractURL"].as_str().unwrap_or("").to_string();
                        if !snippet.is_empty() && !link.is_empty() {
                            hits.push(SearchHit {
                                title: value["Heading"]
                                    .as_str()
                                    .unwrap_or("DuckDuckGo")
                                    .to_string(),
                                url: link,
                                snippet: snippet.chars().take(400).collect(),
                            });
                        }
                        if let Some(topics) = value["RelatedTopics"].as_array() {
                            for topic in topics {
                                let text = topic["Text"].as_str().unwrap_or("");
                                let link = topic["FirstURL"].as_str().unwrap_or("");
                                if !text.is_empty() && !link.is_empty() {
                                    hits.push(SearchHit {
                                        title: strip_html_tags(text).chars().take(120).collect(),
                                        url: link.to_string(),
                                        snippet: String::new(),
                                    });
                                }
                                if hits.len() >= limit {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if hits.is_empty() {
        return Ok(format!("No web results found for '{query}'. Try a different query."));
    }
    let mut output = format!("Web results for '{query}':\n");
    for (index, hit) in hits.iter().enumerate() {
        output.push_str(&format!(
            "{}. {}\n   {}\n   {}\n",
            index + 1,
            hit.title,
            hit.url,
            hit.snippet
        ));
    }
    Ok(output.chars().take(8000).collect())
}

const RECOVERY_SYSTEM: &str = "You are RuTerm's recovery writer. The main assistant stopped \
after a tool ran, sometimes after only saying what it would do. Answer the user's question using \
ONLY the tool result shown below. Reply in the user's language, concise, with no markdown fences. \
If the user asked to save/record the result in a RuTerm note and the result actually contains the \
answer, reply with exactly `CREATE_NOTE: <short title>` on the first line and the full note body on \
the following lines. If the result is empty, failed, or not enough to answer — or if the assistant \
only promised an action — reply with exactly one line and nothing else: \
NEED_COMMAND: <the single safest read-only shell command that would answer the question>. \
Never invent values.";

const COMMAND_SYSTEM: &str = "You choose RuTerm's next step. If the user asked to save the result \
in a RuTerm note, reply with `CREATE_NOTE: <short title>` on the first line and the note body after \
it. Otherwise reply with ONLY one safe, read-only shell command that answers the user's question. \
No prose, no markdown fences.";

const ANSWER_SYSTEM: &str = "You are RuTerm's recovery writer. Answer the user's question using \
ONLY the command output and the screen check below. Reply in the user's language, concise, with no \
markdown fences. If the user asked to save the result in a RuTerm note, reply with exactly \
`CREATE_NOTE: <short title>` on the first line and the full note body after it instead of a normal \
answer. If the screen check says NOT VERIFIED, do not claim the task succeeded — say what could not \
be confirmed. Never output NEED_COMMAND and never invent values.";

/// A tiny tool-free completion. Short context and no tool schema avoid the
/// exact failure mode that makes free models stall after a tool result.
async fn short_completion(
    http: &reqwest::Client,
    key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u64,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "stream": false,
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    });
    let response = send_with_retry(|| {
        http.post(OPENROUTER_CHAT)
            .bearer_auth(key)
            .json(&body)
    })
    .await?;
    if !response.status().is_success() {
        let status = response.status();
        let raw = response.text().await.unwrap_or_default();
        return Err(format!("OpenRouter {status} — {}", short_error(&raw)));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|error| format!("Invalid OpenRouter response: {error}"))?;
    let text = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        Err("OpenRouter returned an empty reply".into())
    } else {
        Ok(text)
    }
}

/// Parse the model's own next-step suggestion. No topic keywords are involved.
fn proposed_command(text: &str) -> Option<String> {
    let at = text.to_lowercase().find("need_command:")?;
    let rest = &text[at + "need_command:".len()..];
    let line = rest.lines().next()?.trim();
    let line = line.trim_matches('`').trim();
    let line = line.strip_prefix("$ ").unwrap_or(line).trim();
    if line.is_empty() || line.len() > 400 {
        return None;
    }
    Some(line.to_string())
}

/// The recovery writer can also ask RuTerm to persist an artifact. This keeps
/// note creation working when a free model stalls after a tool result and the
/// normal tool-calling turn never reaches `create_note`.
fn proposed_note(text: &str) -> Option<(String, String)> {
    let at = text.to_lowercase().find("create_note:")?;
    let rest = &text[at + "create_note:".len()..];
    let mut lines = rest.lines();
    let title = lines.next()?.trim().trim_matches('`').trim();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    if title.is_empty() || body.is_empty() {
        return None;
    }
    Some((title.chars().take(80).collect(), body))
}

async fn persist_recovery_note(
    app: &AppHandle,
    state: &State<'_, AppState>,
    session: &str,
    allow_vps: bool,
    title: &str,
    body: &str,
) -> String {
    let args = json!({ "title": title, "body": body });
    let saved = run_tool(app, state, session, "create_note", &args, allow_vps)
        .await
        .unwrap_or_else(|error| format!("Tool error: {error}"));
    format!("{body}\n\n{saved}")
}

/// Generic recovery for ANY question: let the model decide, from the tool
/// result, whether to answer now, run one more command, or save a note. Keeps
/// the assistant useful when a free model falls silent in the post-tool
/// completion.
async fn recover_after_tools(
    app: &AppHandle,
    state: &State<'_, AppState>,
    session: &str,
    key: &str,
    model: &str,
    allow_vps: bool,
    question: &str,
    partial: &str,
    last_tool_name: &str,
    last_result: &str,
    may_run: bool,
) -> String {
    let http = match client_with_timeout(25) {
        Ok(http) => http,
        Err(_) => {
            return format!(
                "Model tidak mengirim ringkasan. Berikut hasil tool terakhir:\n{}",
                cap_tool_text(last_result, 2_500)
            )
        }
    };
    let shown = cap_tool_text(last_result, 4_000);
    // If the main turn already created the note, a stalled follow-up must not
    // create a duplicate one.
    let note_allowed = !matches!(
        last_tool_name,
        "create_note" | "open_note" | "close_note" | "update_note" | "delete_note"
    ) && !last_tool_name.starts_with("clear_");
    let prompt = format!(
        "User question:\n{question}\n\nTool `{last_tool_name}` returned:\n{shown}\n\n\
         Assistant's unfinished message: {}\n\nSSH control available: {}",
        if partial.trim().is_empty() { "(none)" } else { partial.trim() },
        if may_run { "yes" } else { "no" }
    );
    let first = short_completion(&http, key, model, RECOVERY_SYSTEM, &prompt, 500)
        .await
        .unwrap_or_default();
    if note_allowed {
        if let Some((title, body)) = proposed_note(&first) {
            return persist_recovery_note(app, state, session, allow_vps, &title, &body).await;
        }
    }
    let mut command = proposed_command(&first);
    // If even the recovery writer went silent, ask one even smaller question:
    // just the command. Still no topic keywords — the model chooses it.
    if command.is_none() && first.trim().is_empty() && may_run {
        if let Ok(text) = short_completion(&http, key, model, COMMAND_SYSTEM, question, 120).await {
            if note_allowed {
                if let Some((title, body)) = proposed_note(&text) {
                    return persist_recovery_note(app, state, session, allow_vps, &title, &body).await;
                }
            }
            command = proposed_command(&text).or_else(|| {
                let line = text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('`')
                    .trim();
                if line.is_empty() || line.to_lowercase().starts_with("need_command:") {
                    None
                } else {
                    Some(line.to_string())
                }
            });
        }
    }
    if may_run {
        if let Some(command) = command {
            let args = json!({ "command": command, "timeout_ms": 15_000 });
            let ran = run_tool(app, state, session, "vps_execute", &args, allow_vps)
                .await
                .unwrap_or_else(|error| format!("Tool error: {error}"));
            let follow_up = format!(
                "User question:\n{question}\n\nCommand run on the VPS:\n{command}\n\nOutput:\n{}",
                cap_tool_text(&ran, 4_000)
            );
            let answer = match short_completion(&http, key, model, ANSWER_SYSTEM, &follow_up, 500)
                .await
            {
                Ok(text) => text,
                Err(_) => format!(
                    "Sudah saya jalankan di VPS, tetapi model tidak mengirim ringkasan:\n{}",
                    cap_tool_text(&ran, 2_500)
                ),
            };
            if note_allowed {
                if let Some((title, body)) = proposed_note(&answer) {
                    return persist_recovery_note(app, state, session, allow_vps, &title, &body).await;
                }
            }
            return answer;
        }
    }
    if !first.trim().is_empty() {
        first
    } else {
        format!(
            "Model tidak mengirim ringkasan. Berikut hasil tool terakhir:\n{}",
            cap_tool_text(&shown, 2_500)
        )
    }
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
    allow_vps: bool,
    has_target: bool,
) -> Result<(String, Vec<Call>, bool), String> {
    // Third value = model stalled; the caller must not nudge it again.
    let mut body = json!({
        "model": model,
        "stream": true,
        "messages": messages,
        // A bounded answer keeps free-tier models from stalling mid-generation.
        "max_tokens": 1200,
    });
    if allow_tools {
        body["tools"] = tools_for(allow_vps, has_target);
        body["tool_choice"] = json!("auto");
    }
    let mut response = match send_with_retry(|| {
        http.post(OPENROUTER_CHAT)
            .bearer_auth(key)
            .json(&body)
    })
    .await
    {
        Ok(response) => response,
        Err(error) => {
            if messages.last().is_some_and(|message| message.role == "tool") {
                return Ok((String::new(), Vec::new(), true));
            }
            return Err(error);
        }
    };

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
            return Box::pin(stream_completion(
                app, session, http, key, model, messages, false, allow_vps, has_target,
            ))
            .await;
        }
        if messages.last().is_some_and(|message| message.role == "tool") {
            return Ok((String::new(), Vec::new(), true));
        }
        return Err(format!("OpenRouter {status} — {message}"));
    }

    let mut buffer = String::new();
    let mut content = String::new();
    let mut calls: Vec<Call> = Vec::new();
    let mut saw_data = false;
    let mut last_delta = Instant::now();

    loop {
        // Providers may keep the SSE socket alive with comments while the
        // model itself is silent. A token-idle watchdog is the only reliable
        // way to stop waiting and hand control back to RuTerm.
        if last_delta.elapsed() > Duration::from_secs(30) {
            if messages.last().is_some_and(|message| message.role == "tool") {
                return Ok((content, Vec::new(), true));
            }
            return Err("Model stopped responding before finishing.".into());
        }
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                // A stalled completion right after a tool result must not eat
                // the tool work. Hand an empty turn back so the caller can use
                // its deterministic fallback.
                if messages.last().is_some_and(|message| message.role == "tool") {
                    return Ok((content, Vec::new(), true));
                }
                return Err(format!("Stream interrupted: {error}"));
            }
        };
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
                    last_delta = Instant::now();
                    content.push_str(text);
                    let _ = app.emit(
                        "chat://delta",
                        json!({ "session": session, "text": text }),
                    );
                }
            }

            if let Some(fragments) = delta.get("tool_calls").and_then(Value::as_array) {
                for fragment in fragments {
                    last_delta = Instant::now();
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
    Ok((
        content,
        calls.into_iter().filter(|call| !call.name.is_empty()).collect(),
        false,
    ))
}

pub(crate) fn host_view(host: &Host, with_credentials: bool) -> Value {
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
        "chat_vps_allowed": settings.chat_allow_vps,
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

pub(crate) fn find_host<'a>(vault: &'a rterm_core::model::Vault, query: &str) -> Option<&'a Host> {
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

fn compact_entry(text: &str, max: usize) -> String {
    let flat = text.replace(['\n', '\r', '\t'], " ");
    let mut out: String = flat.chars().take(max).collect();
    if flat.chars().count() > max {
        out.push('…');
    }
    out
}

/// Notes have no title field; the UI derives one from the first line. Keep the
/// tool reply consistent with what the user will see in the new tab.
/// Tool arguments are produced by a chat model. It sometimes JSON-escapes the
/// markdown repeatedly, so an address arrives as `16\\\\.176...` instead of
/// `16.176...`. Collapse backslash runs before anything touches the vault.
pub(crate) fn normalize_note_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            while chars.peek() == Some(&'\\') {
                chars.next();
            }
            out.push('\\');
        } else {
            out.push(ch);
        }
    }
    out.trim().to_string()
}

pub(crate) fn note_preview(body: &str) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Untitled note")
        .trim_start_matches('#')
        .trim();
    compact_entry(if first.is_empty() { "Untitled note" } else { first }, 56)
}

/// Resolve a note for the write tools. An exact id always wins; a short query
/// is accepted for convenience, but only when it matches one live note.
fn find_note<'a>(
    vault: &'a rterm_core::model::Vault,
    id: &str,
    query: &str,
) -> Result<&'a Note, String> {
    let id = id.trim();
    if !id.is_empty() {
        return vault
            .notes
            .iter()
            .find(|note| !note.deleted && note.id.to_string() == id)
            .ok_or_else(|| format!("No note with id '{id}'. Use list_notes to find the id."));
    }
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Err("id or query is required".into());
    }
    let mut matches = vault.notes.iter().filter(|note| {
        !note.deleted
            && (note.body.to_lowercase().contains(&needle)
                || note_preview(&note.body).to_lowercase().contains(&needle))
    });
    let first = matches
        .next()
        .ok_or_else(|| format!("No note matches '{query}'."))?;
    if matches.next().is_some() {
        return Err(format!(
            "More than one note matches '{query}'. Use list_notes and pass the exact id."
        ));
    }
    Ok(first)
}

fn append_action(path: &PathBuf, entry: &str) {
    let stamp = Utc::now().format("%Y-%m-%d %H:%M UTC");
    let line = format!("- [{stamp}] {}\n", compact_entry(entry, 500));
    let existing = fs::read_to_string(path).unwrap_or_default();
    let content = if existing.trim().is_empty() {
        format!("# RuTerm session memory\n\n## Work log\n\n{line}\n## Notes\n")
    } else if let Some(pos) = existing.find("## Notes") {
        format!("{}{line}\n", &existing[..pos])
    } else {
        format!("{existing}{line}\n")
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, content);
}

/// Every chat action is recorded in the focused VPS's memory file, so the next
/// conversation starts with the same context the current one had.
fn remember_chat_action(app: &AppHandle, entry: &str) {
    let bridge = app.state::<crate::mcp::Bridge>();
    if let Some(id) = bridge.target_id() {
        bridge.remember(&id, &compact_entry(entry, 500), false);
    }
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
    // A modal left unanswered (or lost during a dev reload) must never block
    // the chat forever. Deny it after the same window used by the MCP bridge.
    let choice = tokio::time::timeout(Duration::from_secs(120), rx)
        .await
        .unwrap_or(Ok(ApprovalChoice::Deny))
        .unwrap_or(ApprovalChoice::Deny);
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
    allow_vps: bool,
) -> Result<String, String> {
    match name {
        tool if tool.starts_with("vps_") => {
            if !allow_vps {
                return Err(
                    "VPS control is disabled. Enable 'Allow AI Chat to control SSH tabs' in Settings."
                        .into(),
                );
            }
            let mcp_name = &tool["vps_".len()..];
            // Built-in chat never switches tabs: the only legal target is the
            // SSH tab the user is looking at. Strip a hallucinated `session`
            // and refuse the bridge's tab-switching tool entirely.
            if mcp_name == "switch_target" {
                return Err("AI Chat cannot switch SSH tabs. Ask the user to focus the target tab.".into());
            }
            let mut safe_args = args.clone();
            if let Value::Object(map) = &mut safe_args {
                map.remove("session");
            }
            let bridge = app.state::<crate::mcp::Bridge>();

            if mcp_name == "open_host" {
                let name = args.get("name").and_then(Value::as_str).unwrap_or("");
                let vault = state.unlocked_vault().await?;
                let host = find_host(&vault, name).ok_or_else(|| {
                    format!("No saved host matches '{name}'. Use list_hosts to see saved hosts.")
                })?;
                let id = host.id.to_string();
                let label = host.label.clone();
                let address = host.address.clone();
                // Same path as clicking a host in the sidebar: a real SSH tab
                // with its own session; the UI pins it as the chat target once
                // connected.
                let _ = app.emit("rterm:open-host", json!({ "id": id }));
                append_action(
                    &session_file(host),
                    &format!("AI chat opened saved host '{label}' ({address}) in a new tab"),
                );
                return Ok(format!(
                    "Opening saved host '{label}' ({address}) in a new SSH tab. It becomes the active target once connected."
                ));
            }

            // The model sometimes calls `execute` or `send_keys` for a TUI.
            // Launch it correctly with a real Enter and no exit sentinel so
            // `htop` actually opens instead of sitting typed at the prompt.
            let command = args.get("command").and_then(Value::as_str);
            let normalized = command.map(crate::mcp::terminal_keys);
            let tui = normalized
                .as_deref()
                .filter(|value| interactive_command(value))
                .map(|value| value.trim().to_string());
            if normalized.as_deref().is_some_and(crate::mcp::nested_ssh_command) {
                return Ok(
                    "Already inside the SSH tab — do not run a nested `ssh` command. Run the command directly in this VPS."
                        .into(),
                );
            }
            let mut launched_tui = false;
            let mut started_in_background = false;
            let invoke_name = if mcp_name == "execute" {
                if let Some(clean) = tui {
                    launched_tui = true;
                    if let Value::Object(map) = &mut safe_args {
                        map.insert("command".into(), json!(clean));
                    }
                    "open_tui"
                } else if normalized
                    .as_deref()
                    .map(long_running_command)
                    .unwrap_or(false)
                {
                    started_in_background = true;
                    if let Value::Object(map) = &mut safe_args {
                        map.insert("timeout_ms".into(), json!(0));
                    }
                    "start_execute"
                } else {
                    mcp_name
                }
            } else if mcp_name == "send_keys"
                && args
                    .get("keys")
                    .and_then(Value::as_str)
                    .map(crate::mcp::nested_ssh_command)
                    .unwrap_or(false)
            {
                return Ok(
                    "Already inside the SSH tab — do not run a nested `ssh` command. Run the command directly in this VPS."
                        .into(),
                );
            } else if mcp_name == "send_keys"
                && args
                    .get("keys")
                    .and_then(Value::as_str)
                    .map(crate::mcp::looks_like_tui_launch)
                    .unwrap_or(false)
            {
                launched_tui = true;
                if let Value::Object(map) = &mut safe_args {
                    let keys = args
                        .get("keys")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    map.insert("command".into(), json!(crate::mcp::terminal_keys(keys).trim()));
                    map.remove("keys");
                }
                "open_tui"
            } else if mcp_name == "inspect" {
                "inspect_terminal"
            } else if mcp_name == "read_memory" {
                "read_session"
            } else if mcp_name == "note_memory" {
                "note_session"
            } else {
                mcp_name
            };
            let action_args = safe_args
                .get("command")
                .or_else(|| safe_args.get("keys"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let action = format!(
                "AI chat tool {name}({})",
                compact_entry(action_args, 200)
            );
            let result = match crate::mcp::invoke_tool(app, &bridge, invoke_name, &safe_args) {
                Ok(result) => {
                    remember_chat_action(app, &format!("{action} -> ok"));
                    result
                }
                Err(error) => {
                    remember_chat_action(app, &format!("{action} -> error: {error}"));
                    return Err(error);
                }
            };
            // Internal session ids belong in the file, never in chat prose.
            let result = result
                .lines()
                .filter(|line| !line.trim_start().starts_with("Session id:"))
                .collect::<Vec<_>>()
                .join("\n");
            let result = if launched_tui {
                format!(
                    "{result}\nThe command was launched with a real Enter; do not type the word \
                     'Enter' into the terminal."
                )
            } else if started_in_background {
                format!(
                    "{result}\nStarted in the background because this command is long-running. \
                     Use vps_read_stream with this job id if you need the final output; meanwhile \
                     the command is visible in the focused SSH tab."
                )
            } else {
                result
            };
            Ok(cap_tool_text(&result, 16_000))
        }
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
        "create_note" => {
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .chars()
                .take(80)
                .collect::<String>();
            let content = normalize_note_body(
                &args
                    .get("body")
                    .and_then(Value::as_str)
                    .ok_or("body is required")?
                    .chars()
                    .take(50_000)
                    .collect::<String>(),
            );
            if content.is_empty() {
                return Err("body must not be empty".into());
            }
            let body = if title.is_empty() {
                content
            } else {
                format!("# {title}\n\n{content}")
            };
            let favorite = args
                .get("favorite")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let id = Uuid::new_v4();
            let note_id = id.to_string();
            let stamp = now();
            let label = if title.is_empty() {
                note_preview(&body)
            } else {
                title.clone()
            };
            let mut vault = state.unlocked_vault().await?;
            // A model sometimes repeats the same create_note tool call. Open the
            // existing note instead of writing a second copy into the vault.
            if let Some(existing) = vault.notes.iter().find(|n| !n.deleted && n.body == body) {
                let existing_id = existing.id.to_string();
                let _ = app.emit("rterm:note-open", json!({ "id": existing_id }));
                return Ok(format!(
                    "Note '{label}' already exists in your vault; opened the existing note."
                ));
            }
            vault.notes.push(Note {
                id,
                body,
                favorite,
                created_at: stamp,
                updated_at: stamp,
                deleted: false,
            });
            let payload = commands::persist_vault(state, vault).await?;
            let _ = app.emit("rterm:note-created", json!({ "id": note_id, "payload": payload }));
            Ok(format!(
                "Note '{label}' saved in your encrypted vault and opened in a new tab."
            ))
        }
        "close_note" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or("");
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let (note_id, label) = {
                let vault = state.unlocked_vault().await?;
                let found = find_note(&vault, id, query)?;
                (found.id.to_string(), note_preview(&found.body))
            };
            let _ = app.emit("rterm:close-note", json!({ "id": note_id }));
            Ok(format!(
                "Note '{label}' tab closed. The note is still saved in your vault."
            ))
        }
        "open_note" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or("");
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let (note_id, label) = {
                let vault = state.unlocked_vault().await?;
                let found = find_note(&vault, id, query)?;
                (found.id.to_string(), note_preview(&found.body))
            };
            let _ = app.emit("rterm:note-open", json!({ "id": note_id }));
            Ok(format!("Note '{label}' opened in a new tab."))
        }
        "update_note" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or("");
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let mut content = normalize_note_body(
                &args
                    .get("body")
                    .and_then(Value::as_str)
                    .ok_or("body is required")?
                    .chars()
                    .take(50_000)
                    .collect::<String>(),
            );
            if content.is_empty() {
                return Err("body must not be empty".into());
            }
            let append = args.get("append").and_then(Value::as_bool).unwrap_or(false);
            let title = args.get("title").and_then(Value::as_str).unwrap_or("").trim();
            if !append && !title.is_empty() {
                let clean = title.chars().take(80).collect::<String>();
                content = format!("# {clean}\n\n{content}");
            }
            let favorite = args.get("favorite").and_then(Value::as_bool);
            let mut vault = state.unlocked_vault().await?;
            let (note_id, label) = {
                let found = find_note(&vault, id, query)?;
                (found.id.to_string(), note_preview(&found.body))
            };
            let stamp = now();
            let note = vault
                .notes
                .iter_mut()
                .find(|note| note.id.to_string() == note_id)
                .ok_or("note disappeared")?;
            note.body = if append {
                if !note.body.is_empty() && !note.body.ends_with('\n') {
                    note.body.push('\n');
                }
                note.body.push_str(&content);
                note.body.clone()
            } else {
                content
            };
            note.updated_at = stamp;
            if let Some(favorite) = favorite {
                note.favorite = favorite;
            }
            let payload = commands::persist_vault(state, vault).await?;
            let _ = app.emit("rterm:note-updated", json!({ "id": note_id, "payload": payload }));
            Ok(format!(
                "Note '{label}' updated in your encrypted vault and refreshed in its tab."
            ))
        }
        "delete_note" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or("");
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let mut vault = state.unlocked_vault().await?;
            let (note_id, label) = {
                let found = find_note(&vault, id, query)?;
                (found.id.to_string(), note_preview(&found.body))
            };
            let approved = ask_approval(
                app,
                state,
                session,
                &format!("delete:note:{note_id}"),
                "write",
                "Delete this note?",
                &format!("Note '{label}' will be removed from your encrypted vault and sync."),
                &label,
            )
            .await?;
            if !approved {
                return Ok("Note deletion denied by user.".into());
            }
            let note = vault
                .notes
                .iter_mut()
                .find(|note| note.id.to_string() == note_id)
                .ok_or("note disappeared")?;
            note.deleted = true;
            note.updated_at = now();
            let payload = commands::persist_vault(state, vault).await?;
            let _ = app.emit("rterm:note-deleted", json!({ "id": note_id, "payload": payload }));
            Ok(format!("Note '{label}' deleted from your encrypted vault."))
        }
        "clear_chat_history" => {
            let _ = app.emit("chat://clear-request", json!({ "session": session }));
            Ok("Clear chat history confirmation opened. Nothing was deleted until you confirm.".into())
        }
        "web_search" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .clamp(1, 8) as usize;
            web_search(query, limit).await
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
            allow_vps: vault.settings.chat_allow_vps,
        },
        None => ChatConfig {
            configured: false,
            model: String::new(),
            allow_credentials: false,
            allow_vps: false,
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
    allow_vps: bool,
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
    vault.settings.chat_allow_vps = allow_vps;
    vault.settings.updated_at = Some(Utc::now());
    let configured = !vault.settings.openrouter_api_key.is_empty();
    let model = vault.settings.openrouter_model.clone();
    commands::persist_vault(&state, vault).await?;
    Ok(ChatConfig {
        configured,
        model,
        allow_credentials,
        allow_vps,
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

/// Friendly target for the chat header; no internal session id crosses IPC.
#[tauri::command]
pub async fn chat_target(app: tauri::AppHandle) -> Result<Option<ChatTarget>, String> {
    let bridge = app.state::<crate::mcp::Bridge>();
    Ok(bridge
        .target_view()
        .map(|(name, address)| ChatTarget { name, address }))
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
    let (model, allow_vps) = {
        let vault = state.unlocked_vault().await?;
        let model = vault.settings.openrouter_model.trim().to_string();
        if model.is_empty() {
            return Err("OpenRouter model is not set in Settings".into());
        }
        (model, vault.settings.chat_allow_vps)
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
    let mut last_tool_result = String::new();
    let mut last_tool_name = String::new();
    loop {
        let _activity = ActivityBeat::start(app.clone());
        if lock(&state.chat_cancel).contains(&session) {
            let _ = app.emit(
                "chat://done",
                json!({ "session": session, "text": "", "cancelled": true }),
            );
            return Ok(());
        }
        let has_target = vps_target_ready(&app, &state);
        let messages: Vec<ChatMessage> = {
            let sessions = lock(&state.chat_sessions);
            let mut all = vec![ChatMessage::plain(
                "system",
                system_prompt_for(allow_vps, has_target),
            )];
            if let Some(history) = sessions.get(&session) {
                all.extend(history.iter().cloned());
            }
            all
        };

        // On the final turn hide the tools so the model must wrap up in text.
        let allow_tools = turn < MAX_TURNS;
        let (content, calls, _stalled) =
            match stream_completion(
                &app,
                &session,
                &http,
                &key,
                &model,
                &messages,
                allow_tools,
                allow_vps,
                has_target,
            )
            .await
            {
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
            let empty = content.trim().is_empty();
            // A silent post-tool turn is a model failure, not an answer. Let
            // the recovery writer inspect the SAME tool result and decide —
            // for any topic — whether to answer or run one more command.
            if (empty && turn > 0) || (!empty && looks_unfinished(&content)) {
                let final_text = recover_after_tools(
                    &app,
                    &state,
                    &session,
                    &key,
                    &model,
                    allow_vps,
                    last_user_text(&messages),
                    &content,
                    &last_tool_name,
                    &last_tool_result,
                    has_target && allow_vps,
                )
                .await;
                let mut sessions = lock(&state.chat_sessions);
                sessions
                    .entry(session.clone())
                    .or_default()
                    .push(ChatMessage::plain("assistant", final_text.clone()));
                drop(sessions);
                remember_chat_action(&app, &format!("AI chat reply: {final_text}"));
                let _ = app.emit(
                    "chat://done",
                    json!({ "session": session, "text": final_text, "cancelled": false }),
                );
                return Ok(());
            }
            // Never end a turn silently after tools ran.
            let final_text = if empty {
                if turn > 0 {
                    fallback_after_tools(&last_tool_result)
                } else {
                    "Model sedang tidak merespons. Silakan coba lagi.".to_string()
                }
            } else {
                content.clone()
            };
            let mut sessions = lock(&state.chat_sessions);
            sessions
                .entry(session.clone())
                .or_default()
                .push(ChatMessage::plain("assistant", final_text.clone()));
            drop(sessions);
            remember_chat_action(&app, &format!("AI chat reply: {final_text}"));
            let _ = app.emit(
                "chat://done",
                json!({ "session": session, "text": final_text, "cancelled": false }),
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
            let result = run_tool(&app, &state, &session, &call.name, &args, allow_vps)
                .await
                .unwrap_or_else(|error| format!("Tool error: {error}"));
            last_tool_result = result.clone();
            last_tool_name = call.name.clone();
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
                content: result.clone(),
                tool_calls: None,
                tool_call_id: Some(call.id.clone()),
                name: Some(call.name.clone()),
            });
            let lower = result.to_lowercase();
            if lower.contains("timed out")
                || lower.contains("command produced no output")
                || lower.starts_with("tool error")
            {
                sessions.entry(session.clone()).or_default().push(
                    ChatMessage::plain(
                        "user",
                        format!(
                            "The last tool ({}) FAILED and returned no usable data. Do NOT report \
                             RAM, disk, process or log values from memory or earlier messages. \
                             Retry with a different tool or say the check failed.",
                            call.name
                        ),
                    ),
                );
            }
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
    fn web_results_are_parsed_from_duckduckgo_html() {
        let html = r##"<a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdocs&amp;rut=x">Example <b>Docs</b></a><a class="result__snippet" href="#">Latest &amp; greatest docs.</a>"##;
        let hits = parse_ddg_results(html, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Example Docs");
        assert_eq!(hits[0].url, "https://example.com/docs");
        assert_eq!(hits[0].snippet, "Latest & greatest docs.");
        assert!(system_prompt_for(false, true).contains("web_search"));
    }

    #[test]
    fn tools_cover_app_reads_and_theme_switch() {
        let list = tools_for(false, true);
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
            "read_note",
            "create_note",
            "open_note",
            "close_note",
            "update_note",
            "delete_note",
            "clear_chat_history",
            "web_search",
            "get_app_settings",
            "get_sync_status",
            "list_agents",
            "set_theme",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
        assert!(!names.iter().any(|name| name.starts_with("vps_")));
    }

    #[test]
    fn vps_tools_are_opt_in() {
        let list = tools_for(true, true);
        let names: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["function"]["name"].as_str().unwrap())
            .collect();
        for expected in [
            "vps_list_targets",
            "vps_open_host",
            "vps_close_terminal",
            "vps_inspect",
            "vps_verify_screen",
            "vps_execute",
            "vps_open_tui",
            "vps_close_tui",
            "vps_start_execute",
            "vps_read_stream",
            "vps_cancel_stream",
            "vps_send_keys",
            "vps_read_screen",
            "vps_read_memory",
            "vps_note_memory",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
        assert!(!names.contains(&"vps_switch_target"));
        assert!(system_prompt_for(false, true).contains("VPS control is disabled"));
        assert!(system_prompt_for(true, true).contains("VPS control is enabled"));

        let no_target = tools_for(true, false);
        let no_target_names: Vec<&str> = no_target
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["function"]["name"].as_str().unwrap())
            .collect();
        assert!(no_target_names.contains(&"list_hosts"));
        assert!(no_target_names.contains(&"create_note"));
        assert!(no_target_names.contains(&"open_note"));
        assert!(no_target_names.contains(&"update_note"));
        assert!(no_target_names.contains(&"delete_note"));
        assert!(no_target_names.contains(&"clear_chat_history"));
        assert!(no_target_names.contains(&"web_search"));
        let no_target_vps: Vec<&str> = no_target_names
            .iter()
            .copied()
            .filter(|name| name.starts_with("vps_"))
            .collect();
        assert_eq!(no_target_vps, vec!["vps_list_targets", "vps_open_host"]);
        assert!(!no_target_names.contains(&"vps_close_terminal"));
        assert!(system_prompt_for(true, false).contains("NO SSH TAB IS FOCUSED"));
        assert!(system_prompt_for(true, false).contains("call `list_hosts`"));
    }

    #[test]
    fn unfinished_answers_are_detected() {
        assert!(looks_unfinished("Sekarang saya cek konfigurasi nginx dan SSL."));
        assert!(looks_unfinished("I'll check the disk usage now."));
        assert!(looks_unfinished("Baik, saya coba jalankan apt update."));
        assert!(!looks_unfinished("RAM 522 MiB / 5.78 GiB (9%). Disk 8.56 GiB / 18.25 GiB (47%)."));
    }

    #[test]
    fn recovery_can_propose_a_note_artifact() {
        let parsed = proposed_note("CREATE_NOTE: Nginx check\nPanel listen 80;\nSSL aktif sampai 2027.").unwrap();
        assert_eq!(parsed.0, "Nginx check");
        assert_eq!(parsed.1, "Panel listen 80;\nSSL aktif sampai 2027.");
        assert!(proposed_note("CREATE_NOTE: only a title").is_none());
        assert!(proposed_note("No artifact here.").is_none());
    }

    #[test]
    fn note_preview_matches_the_ui_title() {
        assert_eq!(note_preview("# Rekap nginx\n\nDetail ..."), "Rekap nginx");
        assert_eq!(note_preview("\n\nfirst body line"), "first body line");
    }

    #[test]
    fn nested_ssh_is_blocked_before_it_reaches_the_vps() {
        assert!(crate::mcp::nested_ssh_command("ssh root@109.123.235.168 -p 20153 'free -h'"));
        assert!(crate::mcp::nested_ssh_command("sudo ssh root@example.com"));
        assert!(!crate::mcp::nested_ssh_command("sshd -T | grep PermitRootLogin"));
        assert!(!crate::mcp::nested_ssh_command("grep -R 'ssh' /etc/nginx"));
    }

    #[test]
    fn recovery_parses_any_next_command_without_topic_keywords() {
        let a = proposed_command("Need to look at the config.\nNEED_COMMAND: `grep -R server_name /etc/nginx`");
        assert_eq!(a.as_deref(), Some("grep -R server_name /etc/nginx"));
        let b = proposed_command("need_command: $ df -h");
        assert_eq!(b.as_deref(), Some("df -h"));
        assert_eq!(proposed_command("RAM is 9% used, disk 47%."), None);
    }

    #[test]
    fn long_commands_are_detected() {
        for command in [
            "apt update",
            "sudo apt-get install nginx",
            "npm ci",
            "cargo build --release",
            "docker compose up -d",
            "nginx -T | grep server_name",
        ] {
            assert!(long_running_command(command), "missed {command}");
        }
        for command in [
            "apt --version",
            "ls -la",
            "echo apt update",
            "nginx -t",
            "uptime",
            "htop",
            "tail -f /var/log/syslog",
        ] {
            assert!(!long_running_command(command), "false positive: {command}");
        }
    }

    #[test]
    fn interactive_commands_are_detected() {
        for command in [
            "htop",
            "htopEnter",
            "sudo htop",
            "vim /etc/nginx/nginx.conf",
            "nano",
            "less /var/log/syslog",
            "tail -f /var/log/syslog",
            "journalctl -f",
        ] {
            assert!(interactive_command(command), "missed {command}");
        }
        for command in ["apt update", "ls -la", "uptime", "echo htop"] {
            assert!(!interactive_command(command), "false positive: {command}");
        }
    }

    #[test]
    fn empty_tool_turns_still_produce_a_visible_reply() {
        let plain = fallback_after_tools("Disk usage: 42%");
        assert!(plain.contains("42%"));
        let empty = fallback_after_tools("   ");
        assert!(empty.contains("Coba ulangi"));
    }

    #[test]
    fn tool_output_is_capped_without_losing_the_tail() {
        let text = "A".repeat(100) + "TAIL";
        let capped = cap_tool_text(&text, 20);
        assert!(capped.contains("truncated"));
        assert!(capped.ends_with("TAIL"));
    }
}
