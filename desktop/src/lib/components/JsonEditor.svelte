<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorText, type JsonKind } from '$lib/api';
  import { app, applyVaultPayload, notify } from '$lib/state.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import Icon from './Icon.svelte';

  let {
    kind: initial = 'vault',
    onclose,
  }: { kind?: JsonKind; onclose?: () => void } = $props();

  const FILES: { kind: JsonKind; label: string; hint: string }[] = [
    {
      kind: 'vault',
      label: 'vault.json',
      hint: 'Hosts, keys and preferences — passwords and private keys stay sealed.',
    },
    {
      kind: 'settings',
      label: 'settings.json',
      hint: 'Client id, bridge URL and device flags for this install.',
    },
  ];

  // Set from `initial` on mount (see `load`), so a retargeted tab shows the
  // right file without reading the prop on every render.
  let kind = $state<JsonKind>('vault');
  let path = $state('');
  let text = $state('');
  let saved = $state('');
  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let savedNote = $state('');
  let pendingKind = $state<JsonKind | null>(null);

  const dirty = $derived(text !== saved);
  const file = $derived(FILES.find(option => option.kind === kind) ?? FILES[0]);

  async function load(next: JsonKind) {
    kind = next;
    loading = true;
    error = '';
    savedNote = '';
    try {
      const doc = await api.jsonRead(next);
      path = doc.path;
      text = doc.text;
      saved = doc.text;
    } catch (e) {
      path = '';
      text = '';
      saved = '';
      error = errorText(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    // Opening the page clears any stuck text selection from behind it.
    window.getSelection()?.removeAllRanges();
    void load(initial);
  });

  function requestKind(next: JsonKind) {
    if (next === kind) return;
    if (dirty) {
      pendingKind = next;
      return;
    }
    void load(next);
  }

  async function save() {
    if (busy || loading) return;
    busy = true;
    error = '';
    savedNote = '';
    try {
      const payload = await api.jsonWrite(kind, text);
      // A vault edit while unlocked returns the freshly decrypted vault, so the
      // rest of the app sees the new hosts without a re-unlock.
      if (payload) applyVaultPayload(payload);
      saved = text;
      if (!payload && kind === 'vault') {
        savedNote = 'Saved — applies next time the vault unlocks';
      } else {
        savedNote = 'Saved';
      }
      notify(`${file.label} saved`);
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  function format() {
    try {
      text = `${JSON.stringify(JSON.parse(text), null, 2)}\n`;
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }

  async function download() {
    if (busy) return;
    busy = true;
    error = '';
    try {
      const dest = await api.jsonDownload(kind);
      notify(`Saved a copy to ${dest}`);
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
      event.preventDefault();
      void save();
      return;
    }
    if (event.key !== 'Tab') return;
    // Keep Tab inside the editor as an indent instead of leaving the page.
    event.preventDefault();
    const target = event.currentTarget as HTMLTextAreaElement;
    const start = target.selectionStart;
    const end = target.selectionEnd;
    text = `${text.slice(0, start)}  ${text.slice(end)}`;
    void Promise.resolve().then(() => {
      target.selectionStart = start + 2;
      target.selectionEnd = start + 2;
    });
  }
</script>

<div class="json-page">
  <header class="page-head">
    <div class="titles">
      <h1>JSON editor</h1>
      <p class="muted sub">{file.hint}</p>
    </div>
    <span class="spacer"></span>
    {#if onclose}
      <button class="icon-btn" title="Close" aria-label="Close JSON editor" onclick={onclose}>
        <Icon name="close" size={16} />
      </button>
    {/if}
  </header>

  <div class="toolbar">
    <div class="file-tabs" role="tablist" aria-label="JSON files">
      {#each FILES as option (option.kind)}
        <button
          class="file-tab"
          class:on={kind === option.kind}
          type="button"
          role="tab"
          aria-selected={kind === option.kind}
          onclick={() => requestKind(option.kind)}
        >
          <Icon name="file-text" size={13} />
          {option.label}
          {#if option.kind === kind && dirty}
            <span class="dot" title="Unsaved changes"></span>
          {/if}
        </button>
      {/each}
    </div>
    <span class="path mono" title={path}>{path || '—'}</span>
    <span class="spacer"></span>
    <button class="btn sm" type="button" disabled={loading || busy} onclick={format}>
      Format
    </button>
    <button
      class="btn sm"
      type="button"
      disabled={loading || busy || !dirty}
      onclick={() => void load(kind)}
    >
      Revert
    </button>
    <button class="btn sm" type="button" disabled={loading || busy} onclick={() => void download()}>
      <Icon name="download" size={13} />
      Download
    </button>
    <button
      class="btn sm primary"
      type="button"
      disabled={loading || busy || !dirty}
      onclick={() => void save()}
    >
      Save
    </button>
  </div>

  {#if kind === 'vault' && app.status && !app.status.unlocked}
    <p class="banner info">
      The vault is locked, so edits are saved to disk only and apply after the next unlock.
    </p>
  {:else if kind === 'vault'}
    <p class="banner info">
      Secrets appear as <span class="mono">enc:v1:…</span> values — leave them exactly as they are.
      Every save keeps a <span class="mono">vault.json.bak</span> copy and is rejected if the file
      could not be opened again.
    </p>
  {/if}

  {#if error}
    <p class="banner bad">{error}</p>
  {:else if savedNote && !dirty}
    <p class="banner good">{savedNote} · {file.label}</p>
  {/if}

  <div class="editor">
    {#if loading}
      <div class="loading muted">Loading {file.label}…</div>
    {:else}
      <textarea
        class="code"
        spellcheck="false"
        autocomplete="off"
        autocapitalize="off"
        wrap="off"
        aria-label={`Contents of ${file.label}`}
        bind:value={text}
        onkeydown={onKeydown}
      ></textarea>
    {/if}
  </div>
</div>

{#if pendingKind}
  <ConfirmDialog
    title="Discard unsaved changes?"
    confirmLabel="Discard"
    onconfirm={() => {
      const next = pendingKind;
      pendingKind = null;
      if (next) void load(next);
    }}
    oncancel={() => (pendingKind = null)}
  >
    {file.label} has edits you have not saved yet.
  </ConfirmDialog>
{/if}

<style>
  .json-page {
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

  .toolbar {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 26px;
    border-bottom: 1px solid var(--line);
  }
  .file-tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 3px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
  }
  .file-tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--muted);
    font-family: var(--mono);
    font-size: 13px;
    cursor: pointer;
  }
  .file-tab:hover {
    color: var(--text);
    background: var(--surface-hover);
  }
  .file-tab.on {
    background: var(--hover-strong);
    color: var(--text);
  }
  .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--accent);
  }
  .path {
    max-width: 34ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12.5px;
    color: var(--dim);
    user-select: none;
  }

  .banner {
    flex: 0 0 auto;
    margin: 0;
    padding: 8px 26px;
    border-bottom: 1px solid var(--line);
    font-size: 13px;
    line-height: 1.5;
  }
  .banner.info {
    color: var(--muted);
  }
  .banner.bad {
    color: var(--bad);
    background: rgba(238, 139, 139, 0.07);
  }
  .banner.good {
    color: var(--dim);
  }

  .editor {
    flex: 1 1 auto;
    display: flex;
    min-height: 0;
    padding: 14px 26px 22px;
  }
  .loading {
    margin: auto;
    font-size: 14px;
  }
  .code {
    flex: 1 1 auto;
    width: 100%;
    min-height: 0;
    height: 100%;
    margin: 0;
    padding: 14px 16px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--surface);
    color: var(--text);
    font-family: var(--mono);
    font-size: 13.5px;
    line-height: 1.6;
    tab-size: 2;
    white-space: pre;
    overflow: auto;
    resize: none;
  }
  .code:focus {
    outline: none;
    border-color: var(--line-strong);
    background: var(--surface-focus);
  }
</style>
