<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { app, lock, signIn, signOut, syncNow } from '$lib/state.svelte';
  import { applyTheme, ui, THEMES, type Theme } from '$lib/ui.svelte';
  import { isRestoreEnabled, setRestoreEnabled } from '$lib/scrollback';
  import Icon from './Icon.svelte';
  import { newsState } from '$lib/news.svelte';

  let {
    onclose,
    onopeninfo,
    onopenwhatsnew,
  }: {
    onclose?: () => void;
    onopeninfo?: (page: 'about' | 'terms' | 'privacy' | 'contact' | 'github') => void;
    onopenwhatsnew?: () => void;
  } = $props();

  let version = $state('');
  let restoreScrollback = $state(isRestoreEnabled());

  function toggleScrollback(enabled: boolean) {
    restoreScrollback = enabled;
    setRestoreEnabled(enabled);
  }

  const account = $derived(app.status?.account ?? null);

  onMount(() => {
    // Opening the page clears any stuck text selection from behind it
    // (WebKit keeps ::selection painted until something replaces it).
    window.getSelection()?.removeAllRanges();
    void api.appVersion().then(value => (version = value));
  });

</script>

<div class="settings-page">
  <header class="page-head">
    <div class="titles">
      <h1>Settings</h1>
      <p class="muted sub">Google sync, appearance and sessions — everything lives on this device.</p>
    </div>
    <span class="spacer"></span>
    {#if onclose}
      <button class="icon-btn" title="Close" aria-label="Close settings" onclick={onclose}>
        <Icon name="close" size={16} />
      </button>
    {/if}
  </header>

  <div class="scroll">
    <div class="column">
      <section class="card">
        <h3>Google and sync</h3>

        {#if account}
          <div class="account">
            <span class="avatar">{account.email.slice(0, 1).toUpperCase()}</span>
            <div class="who">
              <div class="email">{account.email}</div>
              <div class="muted tiny">Refresh token kept in the OS keychain</div>
            </div>
            <span class="spacer"></span>
            <button class="btn sm" onclick={() => void signOut()}>Sign out</button>
          </div>
        {:else}
          <button class="btn" onclick={() => void signIn()}>Sign in with Google</button>
        {/if}

        <p class="muted tiny hint">
          Zero configuration — one tap signs in with Google and starts sync. Your refresh token
          lives in the OS keychain on this device only.
        </p>

        <div class="row actions">
          <button class="btn sm" disabled={app.busy || !account} onclick={() => void syncNow()}>
            <Icon name="sync" size={14} />
            Sync now
          </button>
          <span class="muted tiny">revision {app.revision}</span>
        </div>
      </section>

      <section class="card">
        <h3>Appearance</h3>
        <div class="theme-row">
          {#each THEMES as option (option.id)}
            <button
              class="theme-card"
              class:on={ui.theme === option.id}
              type="button"
              onclick={() => applyTheme(option.id as Theme)}
            >
              <span class="preview {option.id}">
                <span class="preview-accent {option.id}"></span>
                <span class="preview-bar"></span>
                <span class="preview-body">
                  <span class="preview-pane"></span>
                  <span class="preview-pane wide"></span>
                </span>
              </span>
              <span class="theme-label">
                {option.label}
                <span class="theme-hint">{option.hint}</span>
                {#if ui.theme === option.id}<Icon name="check" size={13} />{/if}
              </span>
            </button>
          {/each}
        </div>
        <p class="muted tiny hint">
          Ember Black is the default. Same layout in all three — pick your contrast.
        </p>
      </section>

      <section class="card">
        <h3>Sessions</h3>
        <label class="switch-row">
          <input
            type="checkbox"
            class="switch"
            checked={restoreScrollback}
            onchange={event => toggleScrollback(event.currentTarget.checked)}
          />
          <span class="switch-text">
            Restore scrollback after restart
            <small>Last lines of each terminal, kept on this device only.</small>
          </span>
        </label>
        <p class="muted tiny hint">
          Quitting the app stops every session — the text you read is saved so it is still
          there when the app reopens, with a fresh connection below it. Stored as plain text in
          local storage, never synced. Turn off to store nothing; existing snapshots are erased.
        </p>
      </section>

      <section class="card">
        <h3>Local vault</h3>
        <dl>
          <dt>Location</dt>
          <dd class="mono">{app.status?.vault_path ?? '—'}</dd>
          <dt>Device</dt>
          <dd class="mono">{app.status?.device_id ?? '—'}</dd>
          <dt>Format</dt>
          <dd>JSON with sealed secrets — XChaCha20-Poly1305, Argon2id</dd>
        </dl>
        <button class="btn sm danger" onclick={() => void lock()}>
          <Icon name="lock" size={14} />
          Lock vault
        </button>
      </section>

      <section class="card">
        <h3>About</h3>
        <p class="muted tiny about">
          RuTerm {version} — Tauri and Svelte with a Rust core. The vault stays in a JSON
          document on this device and on the RuTerm sync bridge, where it is only ever
          ciphertext.
        </p>
      </section>

      {#if onopeninfo}
        <nav class="legal" aria-label="RuTerm resources">
          <button type="button" onclick={() => onopeninfo?.('about')}>About</button>
          <span aria-hidden="true">·</span>
          <button type="button" onclick={() => onopeninfo?.('terms')}>Terms</button>
          <span aria-hidden="true">·</span>
          <button type="button" onclick={() => onopeninfo?.('privacy')}>Privacy</button>
          <span aria-hidden="true">·</span>
          <button type="button" onclick={() => onopeninfo?.('contact')}>Contact</button>
          <span aria-hidden="true">·</span>
          <button type="button" onclick={() => onopeninfo?.('github')}>GitHub</button>
          {#if onopenwhatsnew}
            <span aria-hidden="true">·</span>
            <button type="button" class="whatsnew-link" onclick={onopenwhatsnew}>
              What's New
              {#if newsState.unread > 0}<span class="nav-badge">{newsState.unread > 9 ? '9+' : newsState.unread}</span>{/if}
            </button>
          {/if}
        </nav>
      {/if}
    </div>
  </div>
</div>

<style>
  .settings-page {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    overflow: hidden;
  }

  .page-head {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 16px 26px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .titles h1 {
    margin: 0;
    font-size: 20.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .sub {
    margin: 3px 0 0;
    font-size: 14px;
  }

  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 22px 26px 44px;
  }
  .column {
    width: 100%;
    max-width: 48rem;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .card {
    border: 1px solid var(--line);
    border-radius: 14px;
    background: transparent;
    padding: 17px 20px 15px;
  }
  .card h3 {
    margin: 0 0 14px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--line);
    font-size: 13.5px;
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--dim);
    user-select: none;
  }

  .switch-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 11px 13px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--surface);
    cursor: pointer;
    user-select: none;
    -webkit-user-select: none;
  }
  .switch {
    appearance: none;
    -webkit-appearance: none;
    flex: 0 0 auto;
    width: 40px;
    height: 22px;
    margin: 0;
    padding: 0;
    border: 0;
    border-radius: 999px;
    background: var(--hover-strong);
    box-shadow: inset 0 0 0 1px var(--line);
    position: relative;
    outline: none;
    cursor: pointer;
    transition:
      background 0.2s ease,
      box-shadow 0.2s ease;
  }
  .switch::after {
    content: '';
    position: absolute;
    top: 3px;
    left: 3px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #ffffff;
    box-shadow:
      0 1px 3px rgba(0, 0, 0, 0.32),
      0 0 0 0.5px rgba(0, 0, 0, 0.06);
    transition:
      transform 0.2s cubic-bezier(0.34, 1.56, 0.64, 1),
      background 0.2s ease,
      width 0.15s ease;
  }
  .switch:hover {
    background: color-mix(in srgb, var(--hover-strong) 70%, var(--line-strong));
  }
  .switch:active::after {
    width: 19px;
  }
  .switch:checked {
    background: var(--accent);
    box-shadow: inset 0 0 0 1px transparent;
  }
  .switch:checked:hover {
    background: var(--accent-strong);
  }
  .switch:checked::after {
    /* The global checkbox rule sets width/height/left/top/border here —
       re-declare the full knob geometry so it stays a circle. */
    left: 3px;
    top: 3px;
    width: 16px;
    height: 16px;
    border: 0;
    transform: translateX(18px);
    background: var(--accent-on);
  }
  .switch:checked:active::after {
    transform: translateX(15px);
    width: 19px;
  }
  .switch:focus-visible {
    box-shadow:
      0 0 0 3px var(--accent-dim),
      inset 0 0 0 1px var(--line);
  }
  .switch:checked:focus-visible {
    box-shadow: 0 0 0 3px var(--accent-dim);
  }
  @media (prefers-reduced-motion: reduce) {
    .switch,
    .switch::after {
      transition: none;
    }
  }
  .switch-text {
    font-size: 14.5px;
    line-height: 1.35;
  }
  .switch-text small {
    display: block;
    margin-top: 3px;
    font-size: 12.5px;
    color: var(--muted);
  }

  .account {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 20px;
  }
  .avatar {
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: var(--avatar-bg);
    border: 1px solid var(--line-strong);
    font-size: 15.5px;
    font-weight: 600;
  }
  .email {
    font-size: 15px;
    font-weight: 550;
  }
  .hint {
    margin: 8px 0 0;
    line-height: 1.55;
  }
  .tiny {
    font-size: 13.5px;
  }

  dl {
    display: grid;
    grid-template-columns: 104px minmax(0, 1fr);
    gap: 12px 16px;
    margin: 0 0 18px;
    font-size: 14.5px;
  }
  dt {
    color: var(--dim);
  }
  dd {
    margin: 0;
    word-break: break-word;
  }

  .actions {
    margin-top: 18px;
  }

  .legal {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 12px;
    padding-top: 4px;
    color: var(--dim);
    font-size: 13.5px;
    user-select: none;
  }
  .legal button {
    border: 0;
    background: none;
    padding: 2px 2px;
    color: var(--dim);
    font: inherit;
    cursor: pointer;
  }
  .legal button:hover {
    color: var(--text);
    text-decoration: underline;
    text-decoration-color: var(--accent-line);
    text-underline-offset: 4px;
  }
  .whatsnew-link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .nav-badge {
    display: inline-grid;
    place-items: center;
    min-width: 19px;
    height: 19px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--bad);
    color: #fff;
    font-size: 11px;
    font-weight: 700;
    line-height: 1;
  }

  .about {
    margin: 0;
    line-height: 1.65;
  }

  .theme-row {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
    margin-bottom: 10px;
  }
  @media (max-width: 900px) {
    .theme-row {
      grid-template-columns: 1fr 1fr;
    }
  }
  .theme-card {
    padding: 8px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    text-align: left;
    transition:
      border-color 0.15s ease,
      background 0.15s ease;
  }
  .theme-card:hover {
    border-color: var(--line-strong);
    background: var(--surface-hover);
  }
  .theme-card.on {
    border-color: var(--accent-line);
    background: var(--accent-dim);
    color: var(--text);
  }
  .preview {
    display: block;
    height: 74px;
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid var(--line);
    margin-bottom: 8px;
    position: relative;
  }
  .preview.ember-black,
  .preview.pine-black {
    background: #171513;
  }
  .preview.pine-black {
    background: #141917;
  }
  .preview.ember-light {
    background: #f4f2ee;
  }
  .preview-accent {
    display: block;
    height: 4px;
    width: 34%;
    border-radius: 0 0 6px 0;
    margin-bottom: 6px;
  }
  .preview-accent.ember-black,
  .preview-accent.ember-light {
    background: #f74c00;
  }
  .preview-accent.pine-black {
    background: #a9d4bd;
  }
  .preview-bar {
    display: block;
    height: 12px;
  }
  .preview.ember-black .preview-bar,
  .preview.pine-black .preview-bar {
    background: rgba(255, 255, 255, 0.14);
  }
  .preview.ember-light .preview-bar {
    background: rgba(0, 0, 0, 0.1);
  }
  .preview-body {
    display: flex;
    gap: 5px;
    padding: 7px 8px;
  }
  .preview-pane {
    display: block;
    width: 26px;
    height: 42px;
    border-radius: 4px;
  }
  .preview-pane.wide {
    flex: 1;
    width: auto;
  }
  .preview.ember-black .preview-pane,
  .preview.pine-black .preview-pane {
    background: rgba(255, 255, 255, 0.07);
  }
  .preview.ember-light .preview-pane {
    background: rgba(0, 0, 0, 0.06);
  }
  .theme-label {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 2px 2px;
    font-size: 14.5px;
    font-weight: 550;
  }
  .theme-hint {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--dim);
    margin-left: auto;
    margin-right: 2px;
  }
</style>
