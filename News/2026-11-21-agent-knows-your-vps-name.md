# Agents now know which VPS you mean — without a reload

Open a VPS tab while an agent is already running in a local shell, and it sees the new tab immediately. No restart, no re-initialisation.

The MCP bridge now reports SSH targets using the **saved host name** (for example “production-api”) together with its address, instead of exposing an internal session id. Session ids stay private inside RuTerm: an agent can simply omit `session` to use the active VPS, or pass the friendly name if you have several hosts open.

`ruterm_list_targets` is live and safe to call before every server task. Tool results such as `ruterm_execute` and `ruterm_read_screen` also start with the VPS name, so an agent always knows which server it just touched. Session memory files are still keyed by `user@host:port`, so renaming a host never loses its work log.

This works for every supported agent — OpenCode, Freebuff, Kilo Code, Claude, Kiro and Command AI.
