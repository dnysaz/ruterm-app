# AI agents now share your vault — and chat got smarter

Local agents (OpenCode, Kilo, Freebuff, Claude, Kiro and Command AI) now get the same vault tools as RuTerm's built-in AI chat through the embedded MCP bridge. Ask an agent which VPS hosts you have and it uses `ruterm_list_hosts` for saved hosts — `ruterm_list_targets` still means only the tabs that are open right now. Agents can also list, read, create and edit notes (`ruterm_list_notes`, `ruterm_read_note`, `ruterm_create_note`, `ruterm_update_note`); a note they write opens in a new tab, and an edited note refreshes the tab that is already open. Credentials are never returned to an agent.

The built-in chat is more reliable too. "List my saved hosts" works before any SSH tab is focused, a silent heartbeat stops long tool calls from tripping the 75-second watchdog, and `nginx -T` now runs as a background stream instead of blocking the conversation. Replies render bold text and blue underlined links properly, at a slightly larger size.

Everything still follows the same safety rules: risky commands ask for Deny / Allow / Always allow, VPS control stays off until you enable it, and a result is only reported as verified when the command and its output are actually visible on the SSH tab.
