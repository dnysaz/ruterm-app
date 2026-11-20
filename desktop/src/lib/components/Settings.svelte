<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorText } from '#lib/api';
  import { app, fail, lock, notify, setRememberKey, signIn, signOut, syncNow } from '#lib/state.svelte';
  import { applyTheme, ui, type Theme } from '#lib/ui.svelte';
  import { isRestoreEnabled, setRestoreEnabled } from '#lib/scrollback';
  import Icon from './Icon.svelte';
  import BackgroundSettings from './BackgroundSettings.svelte';
  import { newsState } from '#lib/news.svelte';

  let {
    onclose,
    onopeninfo,
    onopenwhatsnew,
    onopenjson,
  }: {
    onclose?: () => void;
    onopeninfo?: (page: 'about' | 'terms' | 'privacy' | 'contact' | 'github') => void;
    onopenwhatsnew?: () => void;
    onopenjson?: (kind: 'vault' | 'settings') => void;
  } = $props();

  let version = $state('');
  let restoreScrollback = $state(isRestoreEnabled());

  function toggleScrollback(enabled: boolean) {
    restoreScrollback = enabled;
    setRestoreEnabled(enabled);
  }

  const account = $derived(app.status?.account ?? null);

  const relativeLastSync = (iso: string | null | undefined): string => {
    if (!iso) return 'never';
    const then = new Date(iso);
    if (Number.isNaN(then.getTime())) return 'never';
    const secs = Math.max(0, Math.floor((Date.now() - then.getTime()) / 1000));
    if (secs < 45) return 'just now';
    const mins = Math.floor(secs / 60);
    if (mins < 60) return `${mins} minute${mins === 1 ? '' : 's'} ago`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours} hour${hours === 1 ? '' : 's'} ago`;
    const days = Math.floor(hours / 24);
    if (days < 31) return `${days} day${days === 1 ? '' : 's'} ago`;
    return then.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
  };
  const lastSyncAt = $derived(app.status?.last_synced_at ?? null);
  const synced = $derived(
    !!account && !!lastSyncAt && app.revision === (app.status?.synced_revision ?? -1),
  );
  const lastSyncText = $derived(relativeLastSync(lastSyncAt));

  async function downloadJson(kind: 'vault' | 'settings') {
    try {
      const dest = await api.jsonDownload(kind);
      notify(`Saved a copy to ${dest}`);
    } catch (error) {
      fail(error);
    }
  }

  onMount(() => {
    // Opening the page clears any stuck text selection from behind it
    // (WebKit keeps ::selection painted until something replaces it).
    window.getSelection()?.removeAllRanges();
    void api.appVersion().then(value => (version = value));
  });

</script>

<div class="page">
  <header class="page-head">
    <div class="titles">
      <h1>Settings</h1>
      <p class="sub">Google sync, appearance and sessions — everything lives on this device.</p>
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

      <section class="section">
        <h2 class="section-title">Google and sync</h2>
        <div class="group">
          {#if account}
            <div class="row">
              {#if account.picture}
                <img
                  class="avatar avatar-img"
                  src={account.picture}
                  alt=""
                  referrerpolicy="no-referrer"
                />
              {:else}
                <span class="avatar">{(account.name ?? account.email).slice(0, 1).toUpperCase()}</span>
              {/if}
              <div class="cell">
                <div class="strong">{account.name ?? account.email}</div>
                <small>{account.email}</small>
              </div>
              <button class="btn sm" onclick={() => void signOut()}>Sign out</button>
            </div>
            {#if account.sub}
              <div class="row kv">
                <span class="kv-label">Google ID</span>
                <span class="value mono">{account.sub}</span>
              </div>
            {/if}
          {:else}
            <div class="row">
              <div class="cell">
                <div class="strong">Not signed in</div>
                <small>Sign in once to start syncing this vault between your devices.</small>
              </div>
              <button class="btn sm primary" onclick={() => void signIn()}>Sign in with Google</button>
            </div>
          {/if}

          <div class="row">
            {#if account}
              <span class="sync-badge" class:bad={!synced} aria-hidden="true">
                {#if synced}
                  <svg
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2.6"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  ><path d="M5.5 12.9l4.2 4.1L18.5 8.4" /></svg>
                {:else}
                  <svg
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2.6"
                    stroke-linecap="round"
                  ><path d="M7 7l10 10M17 7L7 17" /></svg>
                {/if}
              </span>
              <div class="cell">
                <div class="strong">{synced ? 'Synced' : 'Not synced'}</div>
                <small>Last synced: {lastSyncText}</small>
              </div>
            {:else}
              <span class="sync-badge off" aria-hidden="true">
                <svg
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2.2"
                  stroke-linecap="round"
                ><path d="M6.5 12h11" /></svg>
              </span>
              <div class="cell">
                <div class="strong">Sync is off</div>
                <small>Sign in with Google to sync this vault between your devices.</small>
              </div>
            {/if}
            <button class="btn sm" disabled={app.busy || !account} onclick={() => void syncNow()}>
              <Icon name="sync" size={14} />
              Sync now
            </button>
          </div>
        </div>
        <p class="hint">
          Zero configuration — one tap signs in with Google and starts sync. Your refresh token
          lives in the OS keychain on this device only.
        </p>
      </section>

      <section class="section">
        <h2 class="section-title">Appearance</h2>
        <div class="group">
          <label class="row switch-row">
            <div class="cell">
              <div class="strong">Dark theme</div>
              <small>Both themes are macOS-native. Flip to pick your contrast.</small>
            </div>
            <input
              type="checkbox"
              class="switch"
              checked={ui.theme === 'dark'}
              onchange={event => applyTheme(event.currentTarget.checked ? 'dark' : 'light')}
              aria-label="Dark theme"
            />
          </label>
        </div>
      </section>

      <BackgroundSettings />

      <section class="section">
        <h2 class="section-title">Sessions</h2>
        <div class="group">
          <label class="row switch-row">
            <div class="cell">
              <div class="strong">Restore scrollback after restart</div>
              <small>Last lines of each terminal, kept on this device only.</small>
            </div>
            <input
              type="checkbox"
              class="switch"
              checked={restoreScrollback}
              onchange={event => toggleScrollback(event.currentTarget.checked)}
            />
          </label>
        </div>
        <p class="hint">
          Off by default: terminal text can contain secrets. If enabled, the last lines are kept
          as plain text in local storage (never synced) and erased on lock or sign-out.
        </p>
      </section>

      <section class="section">
        <h2 class="section-title">Local vault</h2>
        <div class="group">
          <div class="row kv">
            <span class="kv-label">Location</span>
            <span class="value mono">{app.status?.vault_path ?? '—'}</span>
          </div>
          <div class="row kv">
            <span class="kv-label">Device</span>
            <span class="value mono">{app.status?.device_id ?? '—'}</span>
          </div>
          <div class="row kv">
            <span class="kv-label">Encryption</span>
            <span class="value">XChaCha20-Poly1305 · Argon2id</span>
          </div>
          <div class="row">
            <div class="cell">
              <div class="strong">Lock vault</div>
              <small>Require your password again before opening hosts.</small>
            </div>
            <button class="btn sm danger" onclick={() => void lock()}>
              <Icon name="lock" size={14} />
              Lock vault
            </button>
          </div>
          <div class="row">
            <div class="cell">
              <div class="strong">Remember key on this device</div>
              <small>Off by default. Stores the vault key in your OS keychain for one-click unlock.</small>
            </div>
            <input
              type="checkbox"
              class="switch"
              checked={app.canAutoUnlock}
              onchange={event =>
                void setRememberKey(event.currentTarget.checked).catch(error =>
                  notify(errorText(error)),
                )}
            />
          </div>
        </div>
        <p class="hint">
          Anyone who can unlock this device account could then open your vault without the
          passphrase. Turn this off to remove the saved key at any time.
        </p>
      </section>

      <section class="section">
        <h2 class="section-title">Data files</h2>
        <div class="group">
          <div class="row">
            <div class="cell">
              <div class="strong mono file-name">vault.json</div>
              <small>Hosts, groups, keys and preferences. Secrets stay sealed.</small>
            </div>
            <div class="row-actions">
              {#if onopenjson}
                <button class="btn sm ghost" type="button" onclick={() => onopenjson?.('vault')}>
                  <Icon name="edit" size={13} /> Edit
                </button>
              {/if}
              <button class="btn sm" type="button" onclick={() => void downloadJson('vault')}>
                <Icon name="download" size={13} /> Download
              </button>
            </div>
          </div>

          <div class="row">
            <div class="cell">
              <div class="strong mono file-name">settings.json</div>
              <small>Google client id, bridge URL and device flags for this install.</small>
            </div>
            <div class="row-actions">
              {#if onopenjson}
                <button class="btn sm ghost" type="button" onclick={() => onopenjson?.('settings')}>
                  <Icon name="edit" size={13} /> Edit
                </button>
              {/if}
              <button class="btn sm" type="button" onclick={() => void downloadJson('settings')}>
                <Icon name="download" size={13} /> Download
              </button>
            </div>
          </div>
        </div>
        <p class="hint">
          Saves are validated before they reach disk — the app refuses a vault it could not reopen
          and keeps a <span class="mono">.bak</span> copy of the previous file. Download saves a
          timestamped copy to your Downloads folder.
        </p>
      </section>

      <section class="section">
        <h2 class="section-title">About</h2>
        <div class="group">
          <div class="row">
            <div class="cell">
              <div class="strong">RuTerm {version}</div>
              <small>
                Tauri and Svelte with a Rust core. The vault stays in a JSON document on this
                device and on the RuTerm sync bridge, where it is only ever ciphertext.
              </small>
            </div>
          </div>
        </div>

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
      </section>
    </div>
  </div>
</div>

<style>
  .page {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    overflow: hidden;
  }

  .page-head {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 22px 32px 16px;
    background: var(--bg);
  }
  .titles h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .sub {
    margin: 3px 0 0;
    font-size: 13px;
    color: var(--text-secondary);
  }

  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 10px 32px 80px;
  }
  .column {
    width: 100%;
    max-width: 700px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 36px;
  }

  .section-title {
    margin: 0 0 10px 4px;
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.015em;
    color: var(--text);
    user-select: none;
  }

  .group {
    border-radius: 12px;
    background: var(--bg-elevated);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 16px;
    min-height: 50px;
    padding: 12px 16px;
    margin: 0;
  }
  .row + .row {
    border-top: 1px solid var(--line);
  }

  .cell {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .strong {
    font-size: 13.5px;
    font-weight: 500;
    color: var(--text);
    letter-spacing: -0.01em;
  }
  .cell small {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-tertiary);
  }

  .avatar-img {
    display: block;
    object-fit: cover;
    border-radius: 50%;
  }
  .sync-badge {
    flex: 0 0 auto;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--ok) 15%, transparent);
    color: var(--ok);
  }
  .sync-badge.bad {
    background: color-mix(in srgb, var(--bad) 13%, transparent);
    color: var(--bad);
  }
  .sync-badge.off {
    background: color-mix(in srgb, var(--text) 7%, transparent);
    color: var(--text-tertiary);
  }
  .sync-badge svg {
    width: 15px;
    height: 15px;
  }
  .kv {
    align-items: baseline;
  }
  .kv-label {
    flex: 0 0 120px;
    font-size: 13px;
    color: var(--text-secondary);
  }
  .value {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    color: var(--text);
    word-break: break-all;
  }

  .row-actions {
    display: flex;
    flex: 0 0 auto;
    gap: 6px;
  }

  .file-name {
    font-size: 13px;
    font-weight: 550;
  }

  .hint {
    margin: 9px 4px 0;
    font-size: 12.5px;
    line-height: 1.6;
    color: var(--text-tertiary);
  }

  /* ---------------------------------------------------------- switch ------- */
  .switch-row {
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
    background: color-mix(in srgb, var(--text) 16%, transparent);
    position: relative;
    outline: none;
    cursor: pointer;
    transition: background 0.2s ease;
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
    transition:
      transform 0.2s cubic-bezier(0.34, 1.56, 0.64, 1),
      width 0.15s ease;
  }
  .switch:active::after {
    width: 19px;
  }
  .switch:checked {
    background: var(--accent);
  }
  .switch:checked:hover {
    background: var(--accent-hover);
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
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  @media (prefers-reduced-motion: reduce) {
    .switch,
    .switch::after {
      transition: none;
    }
  }

  /* ---------------------------------------------------------- links -------- */
  .legal {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 12px;
    margin-top: 18px;
    color: var(--text-tertiary);
    font-size: 12px;
    user-select: none;
  }
  .legal button {
    border: 0;
    background: none;
    padding: 2px;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    text-decoration: underline;
    text-decoration-thickness: 1px;
    text-underline-offset: 2px;
  }
  .legal button:hover {
    color: var(--accent-hover);
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

  @media (max-width: 720px) {
    .page-head {
      padding: 18px 20px 14px;
    }
    .scroll {
      padding: 6px 20px 56px;
    }
    .column {
      gap: 28px;
    }
    .row {
      flex-wrap: wrap;
    }
    .kv-label {
      flex-basis: 110px;
    }

  }
</style>
