<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorText, getSftpToken } from '#lib/api';
  import { imageMimeForPath } from '#lib/images';
  import { languageForPath } from '#lib/languages';
  import Icon from './Icon.svelte';
  import CodeEditor from './CodeEditor.svelte';

  let { sftpId, path }: { sftpId: string; path: string } = $props();

  type Phase = 'loading' | 'ready' | 'image' | 'binary' | 'error';

  let phase = $state<Phase>('loading');
  let error = $state('');
  let content = $state('');
  let size = $state(0);
  let editing = $state(false);
  let draft = $state('');
  let dirty = $state(false);
  let saving = $state(false);
  let saveError = $state('');
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  /** `data:` URL for the image preview. */
  let imageSrc = $state<string | null>(null);

  const language = $derived(languageForPath(path));
  const bytesLabel = $derived.by(() => formatBytes(size));

  function formatBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    const units = ['KB', 'MB', 'GB'];
    let value = n / 1024;
    let i = 0;
    while (value >= 1024 && i < units.length - 1) {
      value /= 1024;
      i += 1;
    }
    return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[i]}`;
  }

  function baseName(p: string): string {
    const trimmed = p.replace(/\/+$/, '');
    return trimmed.slice(trimmed.lastIndexOf('/') + 1) || p;
  }

  function showToast(message: string) {
    toast = message;
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), 3200);
  }

  async function load() {
    phase = 'loading';
    error = '';
    saveError = '';
    const mime = imageMimeForPath(path);
    if (mime) {
      // Images are previewed, not edited: one call, then a data: URL.
      try {
        const image = await api.sftpReadImage(sftpId, getSftpToken(sftpId), path);
        imageSrc = `data:${mime};base64,${image.data}`;
        size = image.size;
        phase = 'image';
      } catch (e) {
        error = errorText(e);
        phase = 'error';
      }
      return;
    }
    try {
      const payload = await api.sftpReadFile(sftpId, getSftpToken(sftpId), path);
      size = payload.size;
      if (payload.content === null) {
        content = '';
        phase = 'binary';
      } else {
        content = payload.content;
        phase = 'ready';
        editing = false;
        dirty = false;
      }
    } catch (e) {
      error = errorText(e);
      phase = 'error';
    }
  }

  function startEdit() {
    draft = content;
    dirty = false;
    saveError = '';
    editing = true;
  }

  function handleChange(next: string) {
    if (!editing) return;
    draft = next;
    dirty = next !== content;
  }

  function cancelEdit() {
    draft = content;
    dirty = false;
    saveError = '';
    editing = false;
  }

  async function save() {
    if (saving) return;
    saving = true;
    saveError = '';
    try {
      await api.sftpWriteFile(sftpId, getSftpToken(sftpId), path, draft);
      content = draft;
      size = new TextEncoder().encode(draft).length;
      dirty = false;
      editing = false;
      showToast('Saved to the server');
    } catch (e) {
      saveError = errorText(e);
    } finally {
      saving = false;
    }
  }

  async function downloadInstead() {
    try {
      const localToken = await api.sftpPickDownload(baseName(path));
      await api.sftpDownload(sftpId, getSftpToken(sftpId), path, localToken);
      showToast('Downloaded');
    } catch (e) {
      showToast(errorText(e));
    }
  }

  onMount(() => {
    void load();
  });
</script>

<svelte:window
  onkeydown={event => {
    const meta = event.metaKey || event.ctrlKey;
    if (meta && (event.key === 's' || event.key === 'S') && editing) {
      event.preventDefault();
      void save();
    }
    if (event.key === 'Escape' && editing) {
      event.preventDefault();
      cancelEdit();
    }
  }}
/>

<div class="fileview">
  <header class="bar">
    <span class="file-icon" aria-hidden="true">
      <Icon name="file-text" size={17} />
    </span>
    <span class="crumb mono" title={path}>{path}</span>
    <span class="flex-spacer"></span>
    {#if phase === 'ready'}
      {#if editing}
        <button class="btn sm" type="button" onclick={cancelEdit} disabled={saving}>Cancel</button>
        <button class="btn sm primary" type="button" onclick={save} disabled={saving} title="Save (⌘S)">
          {#if saving}
            <span class="spinner xs" aria-hidden="true"></span>
          {:else}
            <Icon name="check" size={15} />
          {/if}
          Save
        </button>
      {:else}
        <button class="btn sm" type="button" onclick={downloadInstead} title="Download a copy">
          <Icon name="download" size={15} />
          Download
        </button>
        <button class="btn sm primary" type="button" onclick={startEdit} title="Edit this file">
          <Icon name="edit" size={14} />
          Edit
        </button>
      {/if}
    {:else if phase === 'image' || phase === 'binary' || phase === 'error'}
      <button class="btn sm" type="button" onclick={downloadInstead}>
        <Icon name="download" size={15} />
        Download
      </button>
    {/if}
  </header>

  {#if phase === 'loading'}
    <div class="state">
      <div class="spinner" aria-hidden="true"></div>
      <p class="state-title">Loading file…</p>
      <p class="state-sub mono">{path}</p>
    </div>
  {:else if phase === 'error'}
    <div class="state">
      <p class="state-title">Couldn’t open this file</p>
      <p class="state-sub">{error}</p>
      <button class="btn primary" type="button" onclick={load}>Retry</button>
    </div>
  {:else if phase === 'image' && imageSrc}
    <div class="image-wrap">
      <img class="preview" src={imageSrc} alt={baseName(path)} draggable="false" />
    </div>
  {:else if phase === 'binary'}
    <div class="state">
      <Icon name="file" size={34} stroke={1.3} />
      <p class="state-title">Binary file</p>
      <p class="state-sub">This file isn’t an image or UTF-8 text, so it can’t be shown here. Download it to open it locally.</p>
    </div>
  {:else}
    <div class="editor-wrap">
      {#if saveError}
        <p class="save-error" role="alert">{saveError}</p>
      {/if}
      <CodeEditor
        value={editing ? draft : content}
        {language}
        readonly={!editing}
        {path}
        onchange={handleChange}
        onsave={() => {
          if (editing) void save();
        }}
        onescape={cancelEdit}
      />
    </div>
  {/if}

  <footer class="statusbar">
    <span class="mono path">{path}</span>
    <span class="flex-spacer"></span>
    {#if editing}
      <span class="mode" class:dirty>
        {#if dirty}
          <span class="dot" aria-hidden="true"></span> Unsaved changes
        {:else}
          Editing
        {/if}
      </span>
    {:else if phase === 'image'}
      <span class="mode">Image</span>
    {:else if phase === 'ready'}
      <span class="mode">Read-only</span>
    {/if}
    <span class="count">{bytesLabel}</span>
  </footer>

  {#if toast}
    <div class="toast" role="status">{toast}</div>
  {/if}
</div>

<style>
  .fileview {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--panel);
    color: var(--text);
    font-family: var(--sans);
  }

  .flex-spacer {
    flex: 1 1 auto;
  }
  .mono {
    font-family: var(--mono);
  }

  /* ---------------- toolbar ---------------- */
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 46px;
    padding: 0 12px;
    border-bottom: 1px solid var(--line);
    background: var(--bg-elevated);
    flex: 0 0 auto;
  }
  .file-icon {
    display: inline-flex;
    color: var(--text-tertiary);
    flex: 0 0 auto;
  }
  .crumb {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-secondary);
  }

  /* ---------------- image preview ---------------- */
  .image-wrap {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: grid;
    place-items: center;
    padding: 16px;
    background-color: var(--bg);
    background-image: linear-gradient(45deg, var(--panel-hover) 25%, transparent 25%),
      linear-gradient(-45deg, var(--panel-hover) 25%, transparent 25%),
      linear-gradient(45deg, transparent 75%, var(--panel-hover) 75%),
      linear-gradient(-45deg, transparent 75%, var(--panel-hover) 75%);
    background-size: 16px 16px;
    background-position: 0 0, 0 8px, 8px -8px, -8px 0;
  }
  .preview {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    user-select: none;
    -webkit-user-drag: none;
  }

  /* ---------------- editor ---------------- */
  .editor-wrap {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .save-error {
    margin: 0;
    padding: 6px 16px;
    font-size: 12px;
    color: var(--bad);
    background: rgba(255, 69, 58, 0.08);
    border-bottom: 1px solid var(--line);
  }
  /* ---------------- states ---------------- */
  .state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 40px;
    text-align: center;
    color: var(--text-secondary);
  }
  .state-title {
    margin: 4px 0 0;
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .state-sub {
    margin: 0;
    max-width: 440px;
    font-size: 13px;
    line-height: 1.5;
    word-break: break-word;
  }
  .spinner {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 2px solid var(--line);
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
  }
  .spinner.xs {
    width: 12px;
    height: 12px;
    border-width: 1.5px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
    }
  }

  /* ---------------- status bar ---------------- */
  .statusbar {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 12px;
    height: 30px;
    padding: 0 12px;
    border-top: 1px solid var(--line);
    background: var(--bg-elevated);
    font-size: 11px;
    color: var(--text-tertiary);
    white-space: nowrap;
    overflow: hidden;
  }
  .statusbar .path {
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 10.5px;
  }
  .mode {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-secondary);
  }
  .mode.dirty {
    color: var(--accent);
    font-weight: 600;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  .count {
    color: var(--text-tertiary);
  }

  .toast {
    position: absolute;
    left: 50%;
    bottom: 40px;
    transform: translateX(-50%);
    padding: 8px 14px;
    border-radius: 8px;
    background: var(--bg-elevated);
    border: 1px solid var(--line-hover);
    color: var(--text);
    font-size: 12.5px;
    z-index: 5;
  }
</style>
