# Type `ssh you@host:port`, hit Enter — and meet the AI dock

Two small things that make RuTerm feel faster every day:

- **Quick connect**: the home screen has a one-line box — type
  `ssh you@host:port` (or just `you@host`) and it connects immediately. SSH
  agent first, in-terminal password prompt if that fails. Nothing is stored
  unless you save the host.
- **AI agent dock**: if you have local AI agents installed (opencode and
  friends), the home screen detects them and shows a single icon per agent in
  the bottom-right dock — one click launches it in a local terminal tab.
  Detection is one `zsh -lic` probe, cached; agents that aren't installed
  never show up.
- **Home tab is pinned** and always first; the AI dock floats above the
  bottom-right corner and steps aside when you're not on Home.
