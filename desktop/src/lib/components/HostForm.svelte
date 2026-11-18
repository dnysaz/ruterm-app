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

<form class="hostform-page" onsubmit={save}>
  <header class="page-head">
    <div class="titles">
      <h1>{existing ? 'Edit host' : 'New host'}</h1>
      <p class="muted sub">Connection details are stored in your JSON vault.</p>
    </div>
    <span class="spacer"></span>
    {#if existing && ondelete}
      <button
        class="btn sm danger"
        type="button"
        onclick={() => (showDelete = true)}
      >
        <Icon name="trash" size={14} />
        Delete
      </button>
    {/if}
    <button class="btn sm ghost" type="button" onclick={onclose}>Cancel</button>
    <button class="btn sm primary" type="submit">Save host</button>
  </header>

  <div class="scroll">
    <div class="column">
    <section class="card">
      <h3>Connection</h3>
      <div class="field">
        <label for="label">Label</label>
        <input autocapitalize="off" autocorrect="off" spellcheck="false" id="label" bind:value={draft.label} placeholder="Tokyo web server" />
      </div>
      <div class="grid-2">
        <div class="field">
          <label for="address">Host or IP</label>
          <input autocapitalize="off" autocorrect="off" spellcheck="false" id="address" bind:value={draft.address} placeholder="203.0.113.10" />
        </div>
        <div class="field">
          <label for="port">Port</label>
          <input autocapitalize="off" autocorrect="off" spellcheck="false" id="port" type="number" min="1" max="65535" bind:value={draft.port} />
        </div>
      </div>
      <div class="field">
        <label for="username">Username</label>
        <input autocapitalize="off" autocorrect="off" spellcheck="false" id="username" bind:value={draft.username} placeholder="root" />
      </div>
      <div class="field">
        <label for="tags">Tags</label>
        <input autocapitalize="off" autocorrect="off" spellcheck="false" id="tags" bind:value={tagsText} placeholder="production, tokyo" />
      </div>
      <label class="check">
        <input type="checkbox" bind:checked={draft.favorite} />
        Mark as favorite
      </label>
    </section>

    <section class="card">
      <h3>Authentication</h3>
      <div class="field">
        <label for="auth">Method</label>
        <select id="auth" value={draft.auth.type} onchange={e => setAuthType(e.currentTarget.value)}>
          <option value="password">Password</option>
          <option value="key">SSH key</option>
          <option value="keyboard_interactive">Keyboard-interactive</option>
          <option value="agent">SSH agent</option>
        </select>
      </div>

      {#if draft.auth.type === 'password' || draft.auth.type === 'keyboard_interactive'}
        <div class="field">
          <label for="password">Password</label>
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
        <div class="field">
          <label for="identity">Identity</label>
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

        <div class="key-actions">
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
        <input
          bind:this={fileInput}
          class="file"
          type="file"
          accept=".pem,.key,.ppk,.txt,.openssh"
          onchange={importKeyFile}
        />
        <div class="field">
          <label for="keyPass">Key passphrase (optional)</label>
          <input autocapitalize="off" autocorrect="off" spellcheck="false"
            id="keyPass"
            type="password"
            value={draft.auth.type === 'key' ? draft.auth.passphrase ?? '' : ''}
            oninput={e => {
              if (draft.auth.type === 'key') draft.auth.passphrase = e.currentTarget.value;
            }}
            placeholder="Only if the file is encrypted"
          />
          <p class="muted tiny hint">
            Stored sealed in the vault, so the key works on every device without
            retyping it.
          </p>
        </div>
      {/if}

      <div class="field fingerprint">
        <span class="flabel">Host key</span>
        {#if draft.host_key_fingerprint}
          <div class="mono fp">{draft.host_key_fingerprint}</div>
          <button
            class="btn sm ghost"
            type="button"
            onclick={() => (draft.host_key_fingerprint = null)}
          >
            Forget fingerprint
          </button>
        {:else}
          <p class="muted note">
            Empty means trust on first use. The fingerprint is pinned automatically on the
            first connection.
          </p>
        {/if}
      </div>
    </section>

    <section class="card">
      <h3>Terminal and network</h3>
      <div class="grid-2">
        <div class="field">
          <label for="fontSize">Font size</label>
          <input autocapitalize="off" autocorrect="off" spellcheck="false" id="fontSize" type="number" min="9" max="24" bind:value={draft.terminal.font_size} />
        </div>
        <div class="field">
          <label for="scrollback">Scrollback lines</label>
          <input autocapitalize="off" autocorrect="off" spellcheck="false"
            id="scrollback"
            type="number"
            min="1000"
            step="1000"
            bind:value={draft.terminal.scrollback}
          />
        </div>
      </div>
      <div class="grid-2">
        <div class="field">
          <label for="timeout">Connect timeout (s)</label>
          <input autocapitalize="off" autocorrect="off" spellcheck="false" id="timeout" type="number" min="5" max="120" bind:value={draft.connect_timeout_secs} />
        </div>
        <div class="field">
          <label for="keepalive">Keepalive (s)</label>
          <input autocapitalize="off" autocorrect="off" spellcheck="false" id="keepalive" type="number" min="0" max="600" bind:value={draft.keepalive_secs} />
        </div>
      </div>
      <label class="check">
        <input type="checkbox" bind:checked={draft.compression} />
        Compression
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={draft.agent_forwarding} />
        Agent forwarding
      </label>
      <div class="field notes">
        <label for="notes">Notes</label>
        <textarea autocapitalize="off" {...{ autocorrect: 'off' }} spellcheck="false" id="notes" bind:value={draft.notes} placeholder="Optional"></textarea>
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
  .hostform-page {
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
    gap: 10px;
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
    max-width: 42rem;
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

  .check {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 8px 0 10px;
    font-size: 15.5px;
    font-weight: 400;
    color: var(--text);
  }
  .check input {
    flex: 0 0 auto;
  }

  .flabel {
    display: block;
    font-size: 14px;
    font-weight: 500;
    color: var(--muted);
    margin-bottom: 7px;
  }
  .fp {
    font-size: 13.5px;
    word-break: break-all;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 10px 12px;
    margin-bottom: 8px;
  }
.key-actions {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }
  .file {
    display: none;
  }
  .hint {
    margin: 6px 0 0;
    line-height: 1.5;
  }
  .notes {
    margin-top: 14px;
  }

  .error {
    margin: 0;
    padding: 12px 26px;
    font-size: 14.5px;
    color: var(--bad);
    border-top: 1px solid rgba(255, 114, 124, 0.25);
    background: rgba(255, 114, 124, 0.06);
  }
</style>
