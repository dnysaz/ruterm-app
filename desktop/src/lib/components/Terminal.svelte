<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import type { SessionStatus } from '$lib/api';
  import { Terminal as XTerm } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { SerializeAddon } from '@xterm/addon-serialize';
  import { LigaturesAddon } from '@xterm/addon-ligatures';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { api } from '$lib/api';
  import { ui, type Theme } from '$lib/ui.svelte';
  import { isRestoreEnabled, readSnapshot, saveSnapshot } from '$lib/scrollback';
  import Icon from './Icon.svelte';
  import '@xterm/xterm/css/xterm.css';

  let {
    sessionId,
    hostId,
    title,
    address,
    kind = 'ssh',
    onfingerprint,
    onstatus,
  }: {
    sessionId: string;
    hostId: string;
    /** `local` runs the user's own shell in a PTY instead of SSH. */
    kind?: 'ssh' | 'local';
    title: string;
    /** Shown under the title in the connecting overlay. */
    address?: string;
    onfingerprint?: (fingerprint: string) => void;
    onstatus?: (status: SessionStatus) => void;
  } = $props();

  const palettes: Record<Theme, Record<string, string>> = {
    'ember-black': {
      background: '#171513',
      foreground: '#edeae4',
      cursor: '#f74c00',
      cursorAccent: '#171513',
      selectionBackground: 'rgba(247, 76, 0, 0.3)',
      black: '#17171a',
      red: '#e8898b',
      green: '#8fd3a8',
      yellow: '#e4c082',
      blue: '#9db8cf',
      magenta: '#c3a8cf',
      cyan: '#8fd0cf',
      white: '#d9d7d2',
      brightBlack: '#5d5b57',
      brightRed: '#f0a0a1',
      brightGreen: '#a8e2bc',
      brightYellow: '#f0d29b',
      brightBlue: '#b3c9dc',
      brightMagenta: '#d4bcdc',
      brightCyan: '#a8e0df',
      brightWhite: '#ffffff',
    },
    'pine-black': {
      background: '#141917',
      foreground: '#e6ece8',
      cursor: '#a9d4bd',
      cursorAccent: '#141917',
      selectionBackground: 'rgba(169, 212, 189, 0.28)',
      black: '#1a201d',
      red: '#ee8b8b',
      green: '#8fd3a8',
      yellow: '#e4c082',
      blue: '#9db8cf',
      magenta: '#c3a8cf',
      cyan: '#8fd0cf',
      white: '#d5dbd2',
      brightBlack: '#5b6660',
      brightRed: '#f0a0a1',
      brightGreen: '#a8e2bc',
      brightYellow: '#f0d29b',
      brightBlue: '#b3c9dc',
      brightMagenta: '#d4bcdc',
      brightCyan: '#a8e0df',
      brightWhite: '#ffffff',
    },
    'ember-light': {
      background: '#fdfcf9',
      foreground: '#25221f',
      cursor: '#bf4303',
      cursorAccent: '#fdfcf9',
      selectionBackground: 'rgba(191, 67, 3, 0.22)',
      black: '#2f2f33',
      red: '#b3392f',
      green: '#2f7d5b',
      yellow: '#9a6a12',
      blue: '#2f5f96',
      magenta: '#7a4d8c',
      cyan: '#1f6f74',
      white: '#c9c7c2',
      brightBlack: '#6a6862',
      brightRed: '#c25248',
      brightGreen: '#3c9370',
      brightYellow: '#ab7a20',
      brightBlue: '#42709f',
      brightMagenta: '#8d5f9d',
      brightCyan: '#2d8186',
      brightWhite: '#ffffff',
    },
  };

  let container: HTMLDivElement;
  let term: XTerm | null = null;
  let unlisteners: UnlistenFn[] = [];
  let observer: ResizeObserver | null = null;
  let flushSave: (() => void) | null = null;
  let status = $state<SessionStatus>('connecting');

  $effect(() => {
    onstatus?.(status);
  });
  let disposed = false;

  // Follow theme changes without rebuilding the terminal.
  $effect(() => {
    const palette = palettes[ui.theme];
    if (term) term.options.theme = palette;
  });

  function toBase64(bytes: Uint8Array): string {
    let out = '';
    for (let i = 0; i < bytes.length; i++) out += String.fromCharCode(bytes[i]);
    return btoa(out);
  }

  function fromBase64(value: string): Uint8Array | null {
    try {
      const raw = atob(value);
      const bytes = new Uint8Array(raw.length);
      for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
      return bytes;
    } catch {
      // A malformed frame must not kill the whole terminal.
      console.warn('[rterm] dropping frame with invalid base64 payload');
      return null;
    }
  }

  onMount(() => {
    const terminal = new XTerm({
      fontFamily: "'JetBrainsMonoNF', 'SF Mono', Menlo, monospace",
      fontSize: 16,
      scrollback: 10000,
      // Fullscreen TUIs (opencode & co.) draw half-block art that must tile
      // seamlessly — fractional line-height/letter-spacing breaks it.
      lineHeight: 1,
      fontWeight: 500,
      fontWeightBold: 700,
      letterSpacing: 0,
      cursorBlink: true,
      cursorStyle: 'bar',
      allowProposedApi: true,
      macOptionIsMeta: true,
      theme: palettes[ui.theme],
    });
    const fit = new FitAddon();
    const serializeAddon = new SerializeAddon();
    terminal.loadAddon(fit);
    terminal.loadAddon(serializeAddon);
    terminal.open(container);
    // Ligatures activate only after the terminal is attached to the DOM —
    // and an addon failure must never take the session down with it.
    try {
      terminal.loadAddon(new LigaturesAddon());
    } catch (error) {
      console.warn('[rterm] ligatures unavailable:', error);
    }
    terminal.focus();
    fit.fit();
    term = terminal;

    // Scrollback snapshot: the backend PTY dies with the app, but the text
    // survives — saved shortly after new output, and on window close.
    let saveTimer: ReturnType<typeof setTimeout> | undefined;
    const doFlush = () => {
      clearTimeout(saveTimer);
      if (disposed) return;
      try {
        const snapshot = serializeAddon.serialize({ scrollback: 1000 });
        if (snapshot) saveSnapshot(sessionId, snapshot);
      } catch {
        /* terminal already disposed */
      }
    };
    flushSave = doFlush;
    const scheduleSave = () => {
      clearTimeout(saveTimer);
      saveTimer = setTimeout(doFlush, 1500);
    };
    window.addEventListener('beforeunload', doFlush);

    const write = (data: string) => {
      const payload = toBase64(new TextEncoder().encode(data));
      void (kind === 'local' ? api.localWrite(sessionId, payload) : api.sshWrite(sessionId, payload)).catch(
        () => {},
      );
    };
    const resize = (cols: number, rows: number) => {
      void (kind === 'local' ? api.localResize(sessionId, cols, rows) : api.sshResize(sessionId, cols, rows)).catch(
        () => {},
      );
    };

    terminal.onData(write);
    terminal.onResize(({ cols, rows }) => resize(cols, rows));

    observer = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        /* pane hidden during tab switches */
      }
    });
    observer.observe(container);

    const applyFrame = (payload: Record<string, any>) => {
      switch (payload.event) {
        case 'connected':
          status = 'connected';
          if (payload.fingerprint) {
            terminal.writeln(`\x1b[2m[host key ${payload.fingerprint}]\x1b[0m`);
            onfingerprint?.(String(payload.fingerprint));
          }
          break;
        case 'banner':
          terminal.writeln(`\x1b[2m${payload.message}\x1b[0m`);
          break;
        case 'error':
          status = 'error';
          terminal.writeln(`\r\n\x1b[31m${payload.message}\x1b[0m`);
          break;
        case 'exit_status':
          status = 'closed';
          terminal.writeln(`\r\n\x1b[2m[exit ${payload.code}]\x1b[0m`);
          break;
        case 'closed':
          status = 'closed';
          break;
      }
    };

    // Frames that arrive while the replay boundary is still being computed.
    let booted = false;
    const queuedBytes: Uint8Array[] = [];
    const queuedFrames: Record<string, any>[] = [];

    const isBytes = (payload: Record<string, any>) =>
      payload.event === 'data' || payload.event === 'stderr';

    // Subscribe before attaching so nothing goes missing.
    void listen(`ssh://${sessionId}`, event => {
      const payload = event.payload as Record<string, any>;
      if (!booted) {
        if (isBytes(payload)) {
          const bytes = fromBase64(String(payload.data_b64 ?? ''));
          if (bytes) queuedBytes.push(bytes);
        } else {
          queuedFrames.push(payload);
        }
        return;
      }
      if (isBytes(payload)) {
        const bytes = fromBase64(String(payload.data_b64 ?? ''));
        if (bytes) {
          terminal.write(bytes);
          scheduleSave();
        }
        return;
      }
      applyFrame(payload);
    }).then(unlisten => {
      if (disposed) {
        unlisten();
        return;
      }
      unlisteners.push(unlisten);

      // After a webview reload the Rust session is still alive: replay the
      // buffered output and go live instead of opening a second shell.
      void (async () => {
        let info = null;
        try {
          info = await api.sessionAttach(sessionId);
        } catch {
          info = null;
        }

        if (info && info.found) {
          const replay = fromBase64(String(info.data_b64 ?? ''));
          if (replay?.length) terminal.write(replay);
          let remaining = Math.max(0, Number(info.bytes) || 0);
          for (const raw of queuedBytes.splice(0)) {
            if (remaining >= raw.length) {
              remaining -= raw.length;
            } else {
              terminal.write(remaining > 0 ? raw.subarray(remaining) : raw);
              remaining = 0;
            }
          }
          for (const frame of queuedFrames.splice(0)) applyFrame(frame);
          booted = true;
          status = info.status === 'closed' ? 'closed' : 'connected';
          scheduleSave();
          return;
        }

        // The backend session is gone (the app was fully quit): restore the
        // previous scrollback, then let a fresh session connect below it.
        if (isRestoreEnabled()) {
          const restored = readSnapshot(sessionId);
          if (restored) {
            terminal.write(restored);
            terminal.writeln(
              `\x1b[2m── previous session (process not running) · scrollback restored ──\x1b[0m`,
            );
          }
        }

        for (const frame of queuedFrames.splice(0)) applyFrame(frame);
        for (const raw of queuedBytes.splice(0)) terminal.write(raw);
        booted = true;

        const openRequest =
          kind === 'local'
            ? api.localOpen(sessionId, terminal.cols, terminal.rows)
            : api.sshOpen(sessionId, hostId, terminal.cols, terminal.rows);
        // The overlay above the terminal shows the connection animation while
        // the session is opening; it goes away on the first terminal state.
        openRequest
          .then(() => {
            // Local PTYs have no handshake frame ('connected' is emitted only
            // for SSH); once the spawn resolves the overlay must go away.
            if (kind === 'local' && status === 'connecting') status = 'connected';
          })
          .catch(error => {
            status = 'error';
            terminal.writeln(`\r\n\x1b[31m${String(error)}\x1b[0m`);
          });
      })();
    });
  });

  onDestroy(() => {
    disposed = true;
    if (flushSave) {
      window.removeEventListener('beforeunload', flushSave);
      flushSave = null;
    }
    unlisteners.forEach(fn => fn());
    observer?.disconnect();
    term?.dispose();
    void (kind === 'local' ? api.localClose(sessionId) : api.sshClose(sessionId)).catch(() => {});
  });
</script>

<div class="terminal">
  <div class="canvas" bind:this={container}></div>

  <!-- Termius-style connecting overlay: covers the blank terminal while the
       session is opening, then fades away on the first terminal state. -->
  {#if status === 'connecting'}
    <div class="connect" role="status" aria-live="polite" transition:fade={{ duration: 180 }}>
      <div class="connect-card">
        <div class="pulse">
          <span></span><span></span><span></span>
        </div>
        <div class="connect-text">
          <strong>{kind === 'local' ? 'Starting shell' : 'Connecting'}<span class="ellipsis"><i>.</i><i>.</i><i>.</i></span></strong>
          <span class="connect-addr">{address || title}</span>
        </div>
      </div>
      <div class="track"><span class="bar"></span></div>
    </div>
  {/if}
</div>

<style>
  .terminal {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    position: relative;
  }
  .canvas {
    flex: 1;
    min-height: 0;
    padding: 10px 6px 6px 14px;
  }

  /* ------------------------------------------------------- connect overlay */
  .connect {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: var(--panel);
    pointer-events: none;
    z-index: 5;
  }
  .connect-card {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 22px;
    border: 1px solid var(--line);
    border-radius: 14px;
    background: var(--glass);
    backdrop-filter: blur(24px) saturate(140%);
    -webkit-backdrop-filter: blur(24px) saturate(140%);
    box-shadow: var(--shadow-strong);
  }
  .pulse {
    display: flex;
    gap: 5px;
    align-items: center;
    padding: 6px 8px;
  }
  .pulse span {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
    animation: connectPulse 1.15s ease-in-out infinite;
  }
  .pulse span:nth-child(2) { animation-delay: 0.18s; }
  .pulse span:nth-child(3) { animation-delay: 0.36s; }
  @keyframes connectPulse {
    0%, 100% { opacity: 0.3; transform: scale(0.82); }
    50% { opacity: 1; transform: scale(1); }
  }
  .connect-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .connect-text strong {
    font-size: 16.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
    white-space: nowrap;
  }
  .connect-addr {
    font-family: var(--mono);
    font-size: 14px;
    color: var(--muted);
    white-space: nowrap;
  }
  .ellipsis i {
    font-style: normal;
    animation: connectDot 1.2s ease-in-out infinite;
  }
  .ellipsis i:nth-child(2) { animation-delay: 0.2s; }
  .ellipsis i:nth-child(3) { animation-delay: 0.4s; }
  @keyframes connectDot {
    0%, 60%, 100% { opacity: 0.25; }
    30% { opacity: 1; }
  }
  .track {
    position: absolute;
    left: 18px;
    right: 18px;
    bottom: 8px;
    height: 2px;
    border-radius: 2px;
    background: var(--line);
    overflow: hidden;
  }
  .track .bar {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 34%;
    border-radius: 2px;
    background: var(--accent);
    animation: connectSweep 1.45s cubic-bezier(0.4, 0, 0.2, 1) infinite;
  }
  @keyframes connectSweep {
    0% { left: -36%; }
    100% { left: 104%; }
  }

  @media (prefers-reduced-motion: reduce) {
    .pulse span,
    .ellipsis i,
    .track .bar { animation: none; }
  }
</style>
