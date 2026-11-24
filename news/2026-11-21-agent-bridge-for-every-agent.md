# Your SSH tab is now a tool every AI agent can use

RuTerm now embeds the MCP bridge itself, so a local agent can type commands into your VPS tab — no agent install, no config editing, no extra server.

An OpenCode agent running in a RuTerm local tab used to be the only one that could reach the embedded bridge. Now the same zero-config setup is generated for **Freebuff, Kilo Code, Claude, Kiro and Command AI** too, alongside OpenCode. Click an agent in the sidebar and RuTerm writes a private, per-launch config under `~/.ruterm/agents/<id>/`; your agent's own config files are never touched. The bridge itself is a loopback HTTP endpoint with a random token, and only real SSH tabs are valid targets — an agent can never type into its own terminal.

Server work stays on the server, but the agent runs on your Mac. That means your VPS does not need Node, OpenCode, or any other agent runtime installed — it just receives the commands you already watch in the terminal.

Every important command and decision is also remembered per VPS in `~/.ruterm/sessions/<user>-<host>-<port>.md`. Agents call `ruterm_read_session` when they start and `ruterm_note_session` when they learn something worth keeping, so a fresh session picks up where the last one left off. Risky commands still stop at a **Deny / Allow / Always allow** dialog; "Always allow" applies only to that exact command and only until RuTerm restarts.
