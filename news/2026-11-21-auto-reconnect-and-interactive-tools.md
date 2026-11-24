# Dropped SSH sessions reconnect by themselves

When a VPS reboots or the network blips, RuTerm now reopens the same tab automatically. You do not need to open a new terminal or tell your agent to reconnect: the tab shows the **Connecting…** overlay again (with the retry number), retries with a short backoff, and returns to the live shell as soon as the host is back. A deliberate tab close still stays closed, and typing `exit` in the shell is treated as a normal logout.

Reconnects reuse the saved host from the vault, so a changed address or refreshed credentials are picked up. The persisted host-key pin is enforced on every attempt — a reconnect never silently trusts a different key.

## Smaller agent improvements

- `ruterm_execute_batch` runs several short commands in order and returns all results together.
- `ruterm_send_keys` accepts `include_screen: true`, which returns the latest screen in the same call — much smoother for `vim`, `btop`, `mc` and other TUIs.
- `ruterm_read_stream` reports `elapsed_s`, so an agent can show how long a job has been running.
- Long-running streaming jobs already continue in the background; a job id stays readable for an hour, so agents can check progress later without blocking the tab.
- Output is chunked by byte offset, keeping long command output usable page by page.
