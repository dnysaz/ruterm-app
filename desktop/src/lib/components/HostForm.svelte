<script lang="ts">
  import { api, type Host, type Identity } from '$lib/api';
  import { addIdentity, app, fail, identities, notify } from '$lib/state.svelte';
  import Icon from './Icon.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  let {
    host,
    onclose,
    onsaved,
    ondelete,
  }: {
    host: Host;
    onclose: () => void;
    onsaved: (host: Host) => void;
    ondelete?: (id: string) => void;
  } = $props();

  let showDelete = $state(false);
  // svelte-ignore state_referenced_locally
  let draft = $state<Host>(JSON.parse(JSON.stringify(host)));
  // svelte-ignore state_referenced_locally
  let tagsText = $state((host.tags ?? []).join(', '));
  let localError = $state('');
  let busy = $state(false);
  let fileInput = $state<HTMLInputElement | null>(null);
  let importing = $state(false);

  const existing = $derived(
    (app.vault?.hosts ?? []).some(h => h.id === host.id && !h.deleted),
  );

  function passwordValue(): string {
    return draft.auth.type === 'password' || draft.auth.type === 'keyboard_interactive'
      ? draft.auth.password
      : '';
  }

  function setPassword(value: string) {
    if (draft.auth.type === 'password' || draft.auth.type === 'keyboard_interactive') {
      draft.auth.password = value;
    }
  }

  function setAuthType(type: string) {
    if (type === 'key') {
      const first = identities()[0];
      draft.auth = { type: 'key', identity_id: first?.id ?? '', passphrase: null };
    } else if (type === 'agent') {
      draft.auth = { type: 'agent' };
    } else if (type === 'keyboard_interactive') {
      draft.auth = { type: 'keyboard_interactive', password: passwordValue() };
    } else {
      draft.auth = { type: 'password', password: passwordValue() };
    }
  }

  function keyPassphrase(): string | null {
    const value = draft.auth.type === 'key' ? draft.auth.passphrase : null;
    return value && value.trim() ? value : null;
  }

  async function importKeyFile(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    importing = true;
    localError = '';
    try {
      const identity: Identity = await api.sshImportKey(
        file.name.replace(/\.[^.]+$/, ''),
        await file.text(),
        keyPassphrase(),
      );
      await addIdentity(identity);
      draft.auth = { type: 'key', identity_id: identity.id, passphrase: null };
      notify(`${identity.label} imported`);
    } catch (error) {
      fail(error);
    } finally {
      importing = false;
    }
  }

  async function generateKey() {
    busy = true;
    try {
      const identity: Identity = await api.sshKeygen(
        'ed25519',
        `key-${new Date().toISOString().slice(0, 10)}`,
      );
      await addIdentity(identity);
      draft.auth = { type: 'key', identity_id: identity.id, passphrase: null };
      notify('Ed25519 key created');
    } catch (error) {
      fail(error);
    } finally {
      busy = false;
    }
  }

  function save(event: SubmitEvent) {
    event.preventDefault();
    localError = '';

    if (!draft.address.trim()) {
      localError = 'Host address is required.';
      return;
    }
    if (draft.auth.type === 'key' && !draft.auth.identity_id) {
      localError = 'Select an identity for key authentication.';
      return;
    }

    draft.tags = tagsText
      .split(',')
      .map(tag => tag.trim())
      .filter(Boolean);
    onsaved(draft);
  }
</script>

<form class="page" onsubmit={save}>
  <header class="page-head">
    <div class="titles">
      <h1>{existing ? 'Edit host' : 'New host'}</h1>
      <p class="sub">Connection details are stored in your JSON vault.</p>
    </div>
    <span class="spacer"></span>
    {#if existing && ondelete}
      <button class="btn sm danger" type="button" onclick={() => (showDelete = true)}>
        <Icon name="trash" size={14} />
        Delete
      </button>
    {/if}
    <button class="btn ghost" type="button" onclick={onclose}>Cancel</button>
    <button class="btn primary" type="submit">Save host</button>
  </header>

  <div class="scroll">
    <div class="column">
      <section class="section">
        <h2 class="section-title">Connection</h2>
        <div class="group">
          <label class="row">
            <span class="row-label">Label</span>
            <input autocapitalize="off" autocorrect="off" spellcheck="false" id="label" bind:value={draft.label} placeholder="Tokyo web server" />
          </label>
          <div class="row">
            <label class="row-label" for="address">Host or IP</label>
            <div class="pair">
              <input autocapitalize="off" autocorrect="off" spellcheck="false" id="address" bind:value={draft.address} placeholder="203.0.113.10" />
              <input class="port" autocapitalize="off" autocorrect="off" spellcheck="false" id="port" type="number" min="1" max="65535" bind:value={draft.port} aria-label="Port" />
            </div>
          </div>
          <label class="row">
            <span class="row-label">Username</span>
            <input autocapitalize="off" autocorrect="off" spellcheck="false" id="username" bind:value={draft.username} placeholder="root" />
          </label>
          <label class="row">
            <span class="row-label">Tags</span>
            <input autocapitalize="off" autocorrect="off" spellcheck="false" id="tags" bind:value={tagsText} placeholder="production, tokyo" />
          </label>
          <label class="row check">
            <input type="checkbox" bind:checked={draft.favorite} />
            <span class="check-text">Mark as favorite</span>
          </label>
        </div>
      </section>

      <section class="section">
        <h2 class="section-title">Authentication</h2>
        <div class="group">
          <div class="row">
            <label class="row-label" for="auth">Method</label>
            <select id="auth" value={draft.auth.type} onchange={e => setAuthType(e.currentTarget.value)}>
              <option value="password">Password</option>
              <option value="key">SSH key</option>
              <option value="keyboard_interactive">Keyboard-interactive</option>
              <option value="agent">SSH agent</option>
            </select>
          </div>

          {#if draft.auth.type === 'password' || draft.auth.type === 'keyboard_interactive'}
            <div class="row">
              <label class="row-label" for="password">Password</label>
              <input autocapitalize="off" autocorrect="off" spellcheck="false"
                id="password"
                type="password"
                value={passwordValue()}
                oninput={e => setPassword(e.currentTarget.value)}
                placeholder="Stored encrypted"
              />
            </div>
          {/if}

          {#if draft.auth.type === 'key'}
            <div class="row">
              <label class="row-label" for="identity">Identity</label>
              <select
                id="identity"
                value={draft.auth.type === 'key' ? draft.auth.identity_id : ''}
                onchange={e => {
                  if (draft.auth.type === 'key') draft.auth.identity_id = e.currentTarget.value;
                }}
              >
                {#if identities().length === 0}
                  <option value="">No keys yet — import or create one</option>
                {/if}
                {#each identities() as identity (identity.id)}
                  <option value={identity.id}>
                    {identity.label}
                    {identity.kind ? `· ${identity.kind}` : ''}
                  </option>
                {/each}
              </select>
            </div>

            <div class="row">
              <span class="row-label">Keys</span>
              <div class="row-actions">
                <button
                  class="btn sm"
                  type="button"
                  disabled={busy || importing}
                  title="Import a PEM or OpenSSH private key"
                  onclick={() => fileInput?.click()}
                >
                  <Icon name="key" size={14} />
                  {importing ? 'Reading key…' : 'Import .pem'}
                </button>
                <button class="btn sm ghost" type="button" disabled={busy} onclick={generateKey}>
                  Create Ed25519
                </button>
              </div>
            </div>

            <div class="row top">
              <label class="row-label" for="keyPass">Passphrase</label>
              <div class="stack">
                <input autocapitalize="off" autocorrect="off" spellcheck="false"
                  id="keyPass"
                  type="password"
                  value={draft.auth.type === 'key' ? draft.auth.passphrase ?? '' : ''}
                  oninput={e => {
                    if (draft.auth.type === 'key') draft.auth.passphrase = e.currentTarget.value;
                  }}
                  placeholder="Only if the file is encrypted"
                />
                <p class="hint">
                  Stored sealed in the vault, so the key works on every device without retyping it.
                </p>
              </div>
            </div>

            <input
              bind:this={fileInput}
              class="file"
              type="file"
              accept=".pem,.key,.ppk,.txt,.openssh"
              onchange={importKeyFile}
            />
          {/if}

          <div class="row top">
            <span class="row-label">Host key</span>
            {#if draft.host_key_fingerprint}
              <div class="stack">
                <code class="fp">{draft.host_key_fingerprint}</code>
                <div class="row-actions">
                  <button
                    class="btn sm ghost"
                    type="button"
                    onclick={() => (draft.host_key_fingerprint = null)}
                  >
                    Forget fingerprint
                  </button>
                </div>
              </div>
            {:else}
              <p class="note">
                Empty means trust on first use — the fingerprint is pinned automatically on the
                first connection.
              </p>
            {/if}
          </div>
        </div>
      </section>

      <section class="section">
        <h2 class="section-title">Terminal and network</h2>
        <div class="group">
          <div class="row">
            <label class="row-label" for="fontSize">Font size</label>
            <input class="narrow" autocapitalize="off" autocorrect="off" spellcheck="false" id="fontSize" type="number" min="9" max="24" bind:value={draft.terminal.font_size} />
          </div>
          <div class="row">
            <label class="row-label" for="scrollback">Scrollback lines</label>
            <input class="narrow" autocapitalize="off" autocorrect="off" spellcheck="false"
              id="scrollback"
              type="number"
              min="1000"
              step="1000"
              bind:value={draft.terminal.scrollback}
            />
          </div>
          <div class="row">
            <label class="row-label" for="timeout">Connect timeout</label>
            <input class="narrow" autocapitalize="off" autocorrect="off" spellcheck="false" id="timeout" type="number" min="5" max="120" bind:value={draft.connect_timeout_secs} />
          </div>
          <div class="row">
            <label class="row-label" for="keepalive">Keepalive</label>
            <input class="narrow" autocapitalize="off" autocorrect="off" spellcheck="false" id="keepalive" type="number" min="0" max="600" bind:value={draft.keepalive_secs} />
          </div>
          <label class="row check">
            <input type="checkbox" bind:checked={draft.compression} />
            <span class="check-text">Compression</span>
          </label>
          <label class="row check">
            <input type="checkbox" bind:checked={draft.agent_forwarding} />
            <span class="check-text">Agent forwarding</span>
          </label>
          <div class="row top">
            <label class="row-label" for="notes">Notes</label>
            <textarea autocapitalize="off" {...{ autocorrect: 'off' }} spellcheck="false" id="notes" rows="3" bind:value={draft.notes} placeholder="Optional"></textarea>
          </div>
        </div>
      </section>
    </div>
  </div>

  {#if localError}
    <p class="error">{localError}</p>
  {/if}
</form>

{#if showDelete}
  <ConfirmDialog
    title="Delete this host?"
    confirmLabel="Delete host"
    oncancel={() => (showDelete = false)}
    onconfirm={() => {
      showDelete = false;
      ondelete?.(draft.id);
    }}
  >
    <p>
      <strong>“{draft.label || draft.address}”</strong> will be removed from your vault —
      on this device and everywhere it syncs. Open tabs for this host will be closed and
      their scrollback erased.
    </p>
    <p class="muted tiny">Sealed secrets are destroyed with the record and cannot be recovered.</p>
  </ConfirmDialog>
{/if}

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
    gap: 10px;
    padding: 20px 28px 14px;
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
    padding: 4px 28px 48px;
  }
  .column {
    width: 100%;
    max-width: 680px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }

  .section-title {
    margin: 0 0 8px 2px;
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--text-secondary);
    user-select: none;
  }

  .group {
    border-radius: 12px;
    background: var(--bg-elevated);
    overflow: hidden;
  }

  .row {
    display: grid;
    grid-template-columns: 156px minmax(0, 1fr);
    align-items: center;
    gap: 18px;
    min-height: 42px;
    padding: 7px 14px;
    margin: 0;
  }
  .row + .row {
    border-top: 1px solid var(--line);
  }
  .row.top {
    align-items: start;
    padding-top: 12px;
    padding-bottom: 12px;
  }

  .row-label {
    margin: 0;
    font-size: 13px;
    font-weight: 400;
    letter-spacing: -0.01em;
    color: var(--text-secondary);
    min-width: 0;
  }

  .row input:not([type='checkbox']),
  .row select,
  .row textarea {
    width: 100%;
    min-width: 0;
    height: 34px;
    padding: 0 9px;
    font-size: 13.5px;
    border-radius: 8px;
    border: 1px solid var(--line);
    background-color: var(--bg);
  }
  .row textarea {
    height: auto;
    min-height: 78px;
    padding: 8px 10px;
    line-height: 1.55;
  }
  .row input:not([type='checkbox']):focus,
  .row select:focus,
  .row textarea:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-dim);
  }

  .row .narrow {
    width: 132px;
    justify-self: end;
  }
  .row input.narrow {
    width: 132px;
    flex: 0 0 auto;
  }

  .pair {
    display: flex;
    gap: 8px;
    min-width: 0;
  }
  .pair > input {
    width: auto;
    flex: 1 1 0;
    min-width: 0;
  }
  .pair > input.port {
    width: 92px;
    flex: 0 0 92px;
    text-align: center;
  }

  .row.check {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
    min-height: 44px;
  }
  .check-text {
    font-size: 13px;
    font-weight: 400;
    color: var(--text);
  }

  .row-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .stack {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .hint {
    margin: 0;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-tertiary);
  }
  .note {
    margin: 0;
    align-self: center;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-tertiary);
  }

  .fp {
    display: block;
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.5;
    word-break: break-all;
    color: var(--text-secondary);
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 8px 10px;
  }

  .file {
    display: none;
  }

  .error {
    margin: 0;
    padding: 11px 32px;
    font-size: 13px;
    color: var(--bad);
    border-top: 1px solid var(--line);
    background: var(--bg);
  }

  @media (max-width: 720px) {
    .page-head {
      padding: 18px 20px 14px;
    }
    .scroll {
      padding: 4px 20px 44px;
    }
    .row {
      grid-template-columns: 1fr;
      gap: 6px;
      align-items: start;
      padding: 12px 16px;
    }
    .row + .row {
      border-top: 1px solid var(--line);
    }
    .row.check {
      display: flex;
    }
    .row .narrow {
      justify-self: start;
    }
  }
</style>
