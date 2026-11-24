# Cmd+R is a no-op — and the app picks up where you left off

**Reload (⌘R) no longer tears down your sessions.** Your layout — tab order,
pane splits, active tab per pane, window focus — is restored, and every live
SSH/PTY session is re-attached in place: nothing reconnects, nothing is lost.

- **App restart** restores the same layout; SSH tabs reconnect automatically,
  local shells respawn in the same directory, and each tab carries up to 1000
  lines of scrollback with you (local-only, never synced, toggleable in
  Settings).
- **Quick-connect tabs** (`ssh you@host:port` from the home screen) now persist
  too — reconnected via your SSH agent, with an in-terminal password prompt as
  fallback. Credentials are never written to disk.
- **Note tabs** are restored alongside sessions, pointing at the same notes.
- Tabs that can no longer be honored (deleted host/note) are dropped silently
  instead of ghosting.

The one hard truth: programs running *inside* a session can't outlive the app
itself — that's what tmux on the server is for.
