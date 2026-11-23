<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import {
    api,
    errorText,
    type ChatApproval,
    type ChatConfig,
    type ChatTarget,
  } from '#lib/api';
  import { app } from '#lib/state.svelte';
  import { applyTheme } from '#lib/ui.svelte';
  import { renderSafeMarkdown } from '#lib/markdown';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import Icon from './Icon.svelte';

  let {
    variant = 'drawer',
    startOpen = false,
    onfullscreen,
    onminimize,
    onclose,
    onopenchange,
  }: {
    variant?: 'drawer' | 'tab';
    /** Keep the drawer expanded when returning from the full tab. */
    startOpen?: boolean;
    onfullscreen?: () => void;
    onminimize?: () => void;
    onclose?: () => void;
    onopenchange?: (open: boolean) => void;
  } = $props();

  /** Stable per-process conversation id; never persisted by Rust. */
  const SESSION = 'ruterm-drawer';
  /** Chat history lives in this browser profile, scoped to the signed-in account. */
  const HISTORY_KEY = 'rterm:ai-chat:history:v1';

  interface Bubble {
    role: 'user' | 'assistant' | 'tool';
    text: string;
    name?: string;
  }

  function historyKey() {
    const email = app.status?.account?.email ?? 'local';
    return `${HISTORY_KEY}:${email}`;
  }

  function isBubble(value: unknown): value is Bubble {
    if (!value || typeof value !== 'object') return false;
    const item = value as Partial<Bubble>;
    return (
      (item.role === 'user' || item.role === 'assistant' || item.role === 'tool') &&
      typeof item.text === 'string'
    );
  }

  function loadHistory(key = historyKey()) {
    try {
      const raw = localStorage.getItem(key);
      if (!raw) return;
      const parsed: unknown = JSON.parse(raw);
      if (!Array.isArray(parsed)) return;
      messages = parsed.filter(isBubble).map(bubble =>
        bubble.role === 'tool'
          ? { role: 'tool', name: bubble.name ?? 'tool', text: '' }
          : { role: bubble.role, text: bubble.text },
      );
      // Old histories can contain empty placeholders from tool-only turns.
      pruneEmptyBubbles();
    } catch {
      /* corrupt or unavailable storage — start with an empty chat */
    }
  }

  /** Pasted credentials must not be written to plaintext localStorage. */
  function redactUserText(text: string): string {
    return text.replace(
      /((?:pass(?:word)?|pwd|secret|token|api[_-]?key)\s*[:=]\s*)(?:"[^"]*"|'[^']*'|\S+)/gi,
      '$1[REDACTED]',
    );
  }

  function persistHistory(snapshot: Bubble[], key = historyKey()) {
    try {
      if (snapshot.length === 0) localStorage.removeItem(key);
      else localStorage.setItem(key, JSON.stringify(snapshot));
    } catch {
      /* private mode / quota — history is best-effort only */
    }
  }

  let cfg = $state<ChatConfig>({
    configured: false,
    model: '',
    allow_credentials: false,
    allow_vps: false,
  });
  let target = $state<ChatTarget | null>(null);
  let open = $state(false);

  function setOpen(value: boolean) {
    open = value;
    onopenchange?.(value);
  }

  let messages = $state<Bubble[]>([]);
  let input = $state('');
  let busy = $state(false);
  let error = $state('');
  let approval = $state<ChatApproval | null>(null);
  let clearConfirm = $state(false);
  let scroller = $state<HTMLDivElement>();
  let inputEl = $state<HTMLTextAreaElement>();
  let hydrated = $state(false);
  let loadedKey = $state('');
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let watchdog: ReturnType<typeof setInterval> | null = null;
  let lastActivity = 0;

  function stopWatchdog() {
    if (watchdog) clearInterval(watchdog);
    watchdog = null;
  }

  function touch() {
    lastActivity = Date.now();
  }

  /** A hung stream or an unanswered approval must never lock the composer. */
  function startWatchdog() {
    stopWatchdog();
    touch();
    watchdog = setInterval(() => {
      if (!busy) return stopWatchdog();
      if (Date.now() - lastActivity < 75_000) return;
      const last = messages[messages.length - 1];
      if (last?.role === 'assistant' && !last.text) last.text = '(response timed out)';
      error = 'No response for 75 seconds. Generation stopped — try again.';
      approval = null;
      void api.chatCancel(SESSION).catch(() => {});
      busy = false;
      stopWatchdog();
    }, 2_000);
  }

  async function focusInput() {
    await tick();
    inputEl?.focus();
  }

  $effect(() => {
    if (open && hydrated) void focusInput();
  });

  async function refreshConfig() {
    try {
      cfg = await api.chatConfig();
      if (!cfg.configured) setOpen(false);
    } catch {
      /* vault locked — the $effect below will hide everything */
    }
  }

  async function refreshTarget() {
    try {
      target = await api.chatTarget();
    } catch {
      target = null;
    }
  }

  function appendDelta(text: string) {
    // Ignore late/duplicate stream events (e.g. from a leaked listener during
    // dev HMR) — they must never be appended to a finished reply.
    if (!busy || !text) return;
    touch();
    const last = messages[messages.length - 1];
    if (last?.role === 'assistant') last.text += text;
    else messages.push({ role: 'assistant', text });
  }

  /** Drop empty assistant placeholders left behind by tool-only turns. */
  function pruneEmptyBubbles() {
    messages = messages.filter(
      (bubble, index) =>
        !(
          bubble.role === 'assistant' &&
          !bubble.text.trim() &&
          (index === messages.length - 1 || messages[index + 1]?.role === 'tool')
        ),
    );
  }

  async function send(preset?: string) {
    const text = (preset ?? input).trim();
    if (!text || busy || !cfg.configured) return;
    input = '';
    error = '';
    messages.push({ role: 'user', text });
    messages.push({ role: 'assistant', text: '' });
    busy = true;
    startWatchdog();
    void focusInput();
    try {
      await api.chatSend(SESSION, text);
    } catch (cause) {
      error = errorText(cause);
      busy = false;
      stopWatchdog();
      void focusInput();
    }
  }

  function stop() {
    void api.chatCancel(SESSION).catch(cause => (error = errorText(cause)));
    approval = null;
    busy = false;
    stopWatchdog();
    void focusInput();
  }

  function answer(choice: 'deny' | 'allow' | 'always') {
    const id = approval?.id;
    approval = null;
    if (id) void api.chatApprove(id, choice).catch(cause => (error = errorText(cause)));
  }

  async function clearChat() {
    if (busy) stop();
    try {
      await api.chatClear(SESSION);
      messages = [];
      try {
        localStorage.removeItem(historyKey());
      } catch {
        /* ignore */
      }
      error = '';
    } catch (cause) {
      error = errorText(cause);
    }
  }

  function close() {
    setOpen(false);
    approval = null;
  }

  onMount(() => {
    setOpen(startOpen);
    const initialKey = historyKey();
    loadHistory(initialKey);
    loadedKey = initialKey;
    hydrated = true;
    void refreshConfig();
    void refreshTarget();
    const unlisteners: UnlistenFn[] = [];
    let disposed = false;
    const on = <T>(name: string, handler: (payload: T) => void) => {
      void listen<T>(name, event => {
        if (disposed) return;
        const payload = event.payload as T & { session?: string };
        if (payload && payload.session && payload.session !== SESSION) return;
        handler(payload);
      })
        .then(fn => {
          // `listen` resolves asynchronously: if the component already
          // unmounted (HMR/tab switch), remove the listener immediately.
          if (disposed) fn();
          else unlisteners.push(fn);
        })
        .catch(() => {});
    };

    // Rust emits this while a tool or recovery model call is still running.
    // It deliberately renders nothing — it only keeps the watchdog honest.
    on('chat://activity', () => touch());
    on<{ text: string }>('chat://delta', payload => appendDelta(payload.text));
    on<{ text: string; cancelled: boolean }>('chat://done', payload => {
      // The backend sends the complete final text. Use it as the source of
      // truth so a lost/duplicated delta during dev HMR can never leave the
      // visible reply truncated or repeated.
      if (payload.text) {
        const last = messages[messages.length - 1];
        if (last?.role === 'assistant') last.text = payload.text;
        else messages.push({ role: 'assistant', text: payload.text });
      } else if (busy) {
        const last = messages[messages.length - 1];
        if (last?.role === 'assistant' && !last.text) {
          last.text = payload.cancelled ? '(cancelled)' : '(no response)';
        }
      }
      busy = false;
      stopWatchdog();
      void focusInput();
      pruneEmptyBubbles();
    });
    on<{ message: string }>('chat://error', payload => {
      error = payload.message;
      busy = false;
      stopWatchdog();
      void focusInput();
    });
    on<{ name: string; preview: string }>('chat://tool', payload => {
      // The streaming placeholder is empty until real text arrives. A tool
      // call means this turn produced no text yet, so drop it instead of
      // leaving a tiny blank bubble behind the tool chip.
      const last = messages[messages.length - 1];
      if (last?.role === 'assistant' && !last.text) messages.pop();
      messages.push({ role: 'tool', name: payload.name, text: payload.preview });
      touch();
    });
    on<ChatApproval>('chat://approval', payload => (approval = payload));
    on<ChatTarget>('rterm:mcp-target', payload => (target = payload));
    // The `set_theme` tool asks the renderer to persist/apply the choice.
    on<string>('rterm:set-theme', theme => {
      if (theme === 'dark' || theme === 'light') applyTheme(theme);
    });

    const onUpdated = () => void refreshConfig();
    window.addEventListener('rterm:chat-updated', onUpdated);
    return () => {
      disposed = true;
      if (saveTimer) clearTimeout(saveTimer);
      stopWatchdog();
      unlisteners.forEach(fn => fn());
      window.removeEventListener('rterm:chat-updated', onUpdated);
    };
  });

  $effect(() => {
    if (app.screen !== 'vault' && open) {
      // Keep the conversation in memory and localStorage — only hide the panel.
      setOpen(false);
      approval = null;
      busy = false;
      error = '';
      stopWatchdog();
    }
  });

  $effect(() => {
    const key = historyKey();
    if (!hydrated || key === loadedKey) return;
    // A different account must never inherit the previous account's chat.
    loadedKey = key;
    loadHistory(key);
  });

  $effect(() => {
    if (!hydrated) return;
    // Tool previews can contain approved credential snippets, so only the
    // tool name is persisted — the live chips still show the full preview.
    const snapshot = messages
      .filter(bubble => !(bubble.role === 'assistant' && !bubble.text.trim()))
      .map(bubble =>
        bubble.role === 'tool'
          ? { role: 'tool' as const, name: bubble.name, text: '' }
          : bubble.role === 'user'
            ? { role: 'user' as const, text: redactUserText(bubble.text) }
            : { role: bubble.role, text: bubble.text },
      );
    const key = historyKey();
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => persistHistory(snapshot, key), 200);
  });

  $effect(() => {
    // Re-run on any message change, then scroll to the newest bubble.
    messages.length;
    messages[messages.length - 1]?.text;
    const node = scroller;
    if (node) void tick().then(() => (node.scrollTop = node.scrollHeight));
  });

  $effect(() => {
    if ((variant === 'tab' || open) && inputEl) void tick().then(() => inputEl?.focus());
  });
</script>

{#snippet chatPanel()}
  <section class="chat-drawer" class:tab={variant === 'tab'} aria-label="RuTerm AI Chat">
    <header class="chat-head">
      <div class="chat-title">
        <span class="chat-logo"><Icon name="spark" size={16} /></span>
        <div class="chat-heading">
          <strong>RuTerm AI Chat</strong>
          <small title={cfg.allow_vps && target ? `${target.name} — ${target.address}` : undefined}>
            {cfg.allow_vps
              ? target
                ? `${target.name} · ${cfg.model || 'OpenRouter'}`
                : `No SSH tab focused · ${cfg.model || 'OpenRouter'}`
              : cfg.model || 'OpenRouter'}
          </small>
        </div>
      </div>
      <div class="head-actions">
        <button
          class="icon-btn"
          title={busy
            ? 'Wait for the reply to finish'
            : variant === 'tab'
              ? 'Minimize to side panel'
              : 'Open as full tab'}
          aria-label={variant === 'tab' ? 'Minimize to side panel' : 'Open as full tab'}
          disabled={busy}
          onclick={() => (variant === 'tab' ? onminimize?.() : onfullscreen?.())}
        >
          <Icon name={variant === 'tab' ? 'minimize' : 'maximize'} size={15} />
        </button>
        <button class="icon-btn" title="Clear conversation" aria-label="Clear conversation" onclick={() => (clearConfirm = true)}>
          <Icon name="trash" size={15} />
        </button>
        <button
          class="icon-btn"
          title={variant === 'tab' ? 'Close tab' : 'Close chat'}
          aria-label={variant === 'tab' ? 'Close tab' : 'Close chat'}
          onclick={() => (variant === 'tab' ? onclose?.() : close())}
        >
          <Icon name="close" size={17} />
        </button>
      </div>
    </header>

    <div class="chat-body" bind:this={scroller}>
      {#if messages.length === 0}
        <div class="empty">
          <span class="empty-icon"><Icon name="spark" size={26} /></span>
          <h2>What can I help with?</h2>
          <p>Ask about your saved hosts, notes, snippets and VPS work logs.</p>
          <div class="suggestions">
            <button type="button" disabled={busy || !cfg.configured} onclick={() => void send('List my saved hosts')}>List my saved hosts</button>
            <button type="button" disabled={busy || !cfg.configured} onclick={() => void send('Summarize my notes')}>Summarize my notes</button>
            <button type="button" disabled={busy || !cfg.configured} onclick={() => void send('Show snippets for deploying')}>Show deploy snippets</button>
            {#if cfg.allow_vps}
              <button type="button" disabled={busy || !cfg.configured || !target} onclick={() => void send('Check disk space on this VPS')}>Check disk space on this VPS</button>
            {/if}
          </div>
          <p class="scope">
            {cfg.allow_vps
              ? target
                ? 'Can read vault data and drive the focused SSH tab. Risky commands ask for approval.'
                : 'SSH control is on, but no tab is focused. Open a saved host first — ask me to open it for you.'
              : 'Reads vault data and writes config only. Enable SSH control in Settings to run VPS commands.'}
          </p>
        </div>
      {:else}
        <div class="thread">
          {#each messages as bubble, index (index)}
            {#if bubble.role === 'tool'}
              <div class="tool-chip"><Icon name="check" size={12} /> {bubble.name ?? 'tool'} <span>{bubble.text}</span></div>
            {:else}
              <div class="bubble-row" class:mine={bubble.role === 'user'}>
                <div class="bubble" class:placeholder={bubble.role === 'assistant' && bubble.text === '' && busy}>
                  {#if bubble.text === '' && bubble.role === 'assistant' && busy}
                    <span class="dots"><i></i><i></i><i></i></span>
                  {:else if bubble.role === 'assistant'}
                    {@html renderSafeMarkdown(bubble.text)}
                  {:else}
                    {bubble.text}
                  {/if}
                </div>
              </div>
            {/if}
          {/each}
          {#if error}
            <div class="chat-error">{error}</div>
          {/if}
        </div>
      {/if}
    </div>

    <div class="composer">
      <div class="composer-inner">
        <div class="composer-input">
          <textarea
            bind:this={inputEl}
            bind:value={input}
            rows={variant === 'tab' ? 3 : 2}
            placeholder="Ask about your vault…"
            aria-label="Chat message"
            onkeydown={event => {
              if (event.key === 'Enter' && !event.shiftKey) {
                event.preventDefault();
                void send();
              }
            }}
          ></textarea>
          {#if busy}
            <button
              class="send-inside stop"
              type="button"
              title="Stop generating"
              aria-label="Stop generating"
              onclick={stop}
            >
              <Icon name="stop" size={15} />
            </button>
          {:else}
            <button
              class="send-inside"
              type="button"
              title="Send message"
              aria-label="Send message"
              disabled={!input.trim()}
              onclick={() => void send()}
            >
              <Icon name="send" size={18} stroke={1.9} />
            </button>
          {/if}
        </div>
        <div class="composer-actions">
          <small>Enter to send · Shift+Enter for a new line</small>
        </div>
      </div>
    </div>
  </section>
{/snippet}

{#if variant === 'tab'}
  {@render chatPanel()}
{:else}
  {#if cfg.configured && app.screen === 'vault'}
    <button
      class="chat-fab"
      class:hidden={open}
      type="button"
      title="RuTerm AI Chat"
      aria-label="Open RuTerm AI Chat"
      onclick={() => setOpen(true)}
    >
      <Icon name="spark" size={22} stroke={1.8} />
    </button>
  {/if}

  {#if open}
    {@render chatPanel()}
  {/if}
{/if}

{#if approval}
  <ConfirmDialog
    title={approval.title}
    cancelLabel="Deny"
    confirmLabel={approval.kind === 'credential' ? 'Allow once' : 'Allow'}
    alwaysLabel="Always allow"
    tone="primary"
    oncancel={() => answer('deny')}
    onconfirm={() => answer('allow')}
    onalways={() => answer('always')}
  >
    <p>{approval.detail}</p>
    <pre class="approval-preview">{approval.preview}</pre>
    <p class="approval-hint">“Always allow” remembers this exact operation for the current RuTerm session.</p>
  </ConfirmDialog>
{/if}

{#if clearConfirm}
  <ConfirmDialog
    title="Clear chat history?"
    cancelLabel="Cancel"
    confirmLabel="Clear history"
    tone="danger"
    oncancel={() => (clearConfirm = false)}
    onconfirm={() => {
      clearConfirm = false;
      void clearChat();
    }}
  >
    <p>
      This removes every message in RuTerm AI Chat from this device. Your vault data and saved hosts
      are not affected. This cannot be undone.
    </p>
  </ConfirmDialog>
{/if}

<style>
  .chat-fab {
    position: fixed;
    right: 26px;
    /* Above the Home tab's footer pill links. */
    bottom: 82px;
    z-index: 800;
    width: 52px;
    height: 52px;
    display: grid;
    place-items: center;
    border: 1px solid color-mix(in srgb, var(--accent) 45%, transparent);
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-on, #fff);
    cursor: pointer;
    transition: transform 0.18s ease, filter 0.18s ease, opacity 0.18s ease;
  }
  .chat-fab:hover {
    transform: translateY(-2px);
    filter: brightness(1.08);
  }
  .chat-fab.hidden {
    opacity: 0;
    pointer-events: none;
  }

  /* In-page right column: part of the shell layout, never an overlay. */
  .chat-drawer {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--line);
    background: var(--bg);
    animation: panel-in 0.24s cubic-bezier(0.22, 0.9, 0.3, 1);
  }
  /* Full tab mode fills the pane and needs no slide animation. */
  .chat-drawer.tab {
    border-left: 0;
    animation: none;
  }

  @keyframes panel-in {
    from { transform: translateX(18px); opacity: 0; }
  }
  @media (prefers-reduced-motion: reduce) {
    .chat-drawer { animation: none; }
  }

  .chat-head {
    /* Same 68px rhythm as the tab bar so both divider lines line up. */
    flex: 0 0 68px;
    height: 68px;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 18px;
    border-bottom: 1px solid var(--line);
  }
  .chat-title {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: 1;
    min-width: 0;
  }
  .chat-logo {
    width: 30px;
    height: 30px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    color: var(--accent);
  }
  .chat-heading {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .chat-heading strong {
    font-size: 14px;
    font-weight: 600;
  }
  .chat-heading small {
    font-size: 11px;
    color: var(--text-tertiary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .head-actions { display: flex; gap: 4px; }
  .icon-btn {
    width: 30px;
    height: 30px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }
  .icon-btn:hover { background: var(--hover); color: var(--text); }
  .icon-btn:disabled { opacity: 0.4; cursor: default; background: transparent; }

  .chat-body {
    flex: 1;
    overflow-y: auto;
    padding: 18px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .thread {
    display: flex;
    flex-direction: column;
    gap: 14px;
    width: 100%;
    max-width: 860px;
    margin: 0 auto;
  }
  .chat-drawer.tab .chat-body {
    padding: 28px 32px;
  }
  .chat-drawer.tab .bubble {
    max-width: 82%;
    font-size: 15px;
  }
  .chat-drawer.tab .head-actions {
    gap: 6px;
  }

  .empty {
    margin: auto;
    text-align: center;
    max-width: 280px;
  }
  .empty-icon {
    display: inline-grid;
    place-items: center;
    width: 52px;
    height: 52px;
    border-radius: 16px;
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
  }
  .empty h2 { margin: 14px 0 4px; font-size: 16px; font-weight: 600; }
  .empty p { margin: 0; font-size: 13.5px; line-height: 1.6; color: var(--text-secondary); }
  .empty .scope { margin-top: 16px; color: var(--text-tertiary); font-size: 11px; }
  .suggestions { display: flex; flex-direction: column; gap: 6px; margin-top: 16px; }
  .suggestions button {
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: var(--bg-elevated);
    color: var(--text-secondary);
    font-size: 12.5px;
    cursor: pointer;
  }
  .suggestions button:hover:not(:disabled) { border-color: var(--accent); color: var(--text); }
  .suggestions button:disabled { opacity: 0.45; cursor: not-allowed; }

  .bubble-row { display: flex; justify-content: flex-start; }
  .bubble-row.mine { justify-content: flex-end; }
  .bubble {
    max-width: 88%;
    padding: 9px 12px;
    border-radius: 14px 14px 14px 4px;
    background: var(--bg-elevated);
    border: 1px solid var(--line);
    font-size: 15.5px;
    line-height: 1.65;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .bubble :global(p) { margin: 0; }
  .bubble :global(p + p) { margin-top: 6px; }
  .bubble :global(strong) { font-weight: 650; color: var(--text); }
  .bubble :global(a) {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  .bubble :global(a:hover) { filter: brightness(1.15); }
  .bubble :global(code) {
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--bg-elevated-2, rgba(127, 127, 127, 0.16));
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 0.92em;
  }
  .mine .bubble {
    border-color: transparent;
    background: var(--accent);
    color: var(--accent-on, #fff);
    border-radius: 14px 14px 4px 14px;
  }
  .bubble.placeholder { min-width: 46px; }
  .dots { display: inline-flex; gap: 4px; }
  .dots i {
    width: 5px; height: 5px; border-radius: 50%;
    background: currentColor; opacity: 0.5;
    animation: blink 1s infinite ease-in-out;
  }
  .dots i:nth-child(2) { animation-delay: 0.15s; }
  .dots i:nth-child(3) { animation-delay: 0.3s; }
  @keyframes blink { 50% { opacity: 1; } }

  .tool-chip {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    padding: 5px 9px;
    border: 1px solid var(--line);
    border-radius: 999px;
    color: var(--ok);
    font-size: 11.5px;
  }
  .tool-chip span {
    color: var(--text-tertiary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 260px;
  }
  .chat-error {
    padding: 9px 11px;
    border: 1px solid color-mix(in srgb, var(--bad) 40%, transparent);
    border-radius: 9px;
    background: color-mix(in srgb, var(--bad) 10%, transparent);
    color: var(--bad);
    font-size: 12px;
    line-height: 1.5;
  }

  .composer {
    flex: 0 0 auto;
    padding: 12px 14px 14px;
    border-top: 1px solid var(--line);
    background: var(--bg);
  }
  .composer-inner {
    width: 100%;
    max-width: 860px;
    margin: 0 auto;
  }
  .chat-drawer.tab .composer {
    padding: 16px 32px 20px;
  }
  .chat-drawer.tab .composer textarea {
    min-height: 84px;
    font-size: 15px;
    border-radius: 16px;
    padding: 14px 58px 14px 16px;
  }
  .composer-input {
    position: relative;
  }
  .composer textarea {
    min-height: 56px;
    font-size: 14px;
    border-radius: 16px;
    padding: 11px 54px 11px 13px;
    resize: none;
    line-height: 1.55;
  }
  .send-inside {
    position: absolute;
    right: 9px;
    top: 50%;
    transform: translateY(-50%);
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-on, #fff);
    cursor: pointer;
    transition: transform 0.15s ease, filter 0.15s ease, opacity 0.15s ease;
  }
  .send-inside:hover:not(:disabled) {
    transform: translateY(calc(-50% - 1px));
    filter: brightness(1.08);
  }
  .send-inside:disabled {
    opacity: 0.42;
    cursor: default;
  }
  .send-inside.stop {
    background: var(--bg-elevated);
    border: 1px solid var(--line);
    color: var(--text-secondary);
  }
  .chat-drawer.tab .send-inside {
    right: 12px;
    width: 40px;
    height: 40px;
  }
  .composer-actions {
    display: flex;
    align-items: center;
    justify-content: center;
    margin-top: 7px;
  }
  .composer-actions small { color: var(--text-tertiary); font-size: 10.5px; }

  .approval-preview {
    max-height: 150px;
    overflow: auto;
    margin: 12px 0 0;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: rgba(0, 0, 0, 0.22);
    color: var(--text);
    font-size: 11.5px;
    text-align: left;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .approval-hint { font-size: 11px !important; color: var(--text-tertiary) !important; }
</style>
